//! Merkle Sum Tree protocol constants, limits and tree context (PLAN-07).
//!
//! # Format versions
//!
//! * `LEGACY_PROTOCOL_VERSION` (1): the original construction. Leaf hash is
//!   `Poseidon(user_id, balance)`, node hash is
//!   `Poseidon(left_hash, right_hash, left_sum, right_sum)`, odd levels were padded by
//!   duplicating the last node (which double counted funded leaves), no domain
//!   separation, no asset/snapshot context. Roots and proofs of this format are
//!   **not** valid in format 2 and must never be relabelled as such.
//! * `PROTOCOL_VERSION` (2): zero-balance padding to a power-of-two capacity,
//!   domain-separated leaf/node/empty hashes, asset + snapshot context bound into every
//!   leaf and empty-padding hash, canonical (strictly ascending `user_id`) leaf order.
//!
//! Any change to hash inputs, Poseidon parameters, padding or ordering MUST bump
//! `PROTOCOL_VERSION`.
//!
//! # Conventions
//!
//! * Capacity is `next_power_of_two(max(N, 1))` and at most `MAX_LEAVES`.
//! * `N = 0`: capacity 1, the root is the empty-padding hash with sum 0 (depth 0).
//! * `N = 1`: capacity 1, the root *is* the leaf (depth 0, empty proof path).
//! * Canonical order: strictly ascending `user_id`; a `(user_id, asset)` pair may occur
//!   at most once (the asset is fixed by the tree context, so `user_id` is unique per
//!   tree). `user_id` is in `1..=i64::MAX` (0 is reserved, > `i64::MAX` cannot cross the
//!   SQL boundary).

use ark_bn254::Fr;
use ark_ff::PrimeField;

/// Current Merkle Sum Tree format version.
pub const PROTOCOL_VERSION: u32 = 2;
/// Legacy format (duplicate padding, no domain separation). Not accepted by the verifier.
pub const LEGACY_PROTOCOL_VERSION: u32 = 1;
/// Version tag for trees built with a caller supplied (non Poseidon) hasher.
pub const CUSTOM_FORMAT_VERSION: u32 = 0;

/// Maximum number of real leaves per tree (research capacity from docs/DECISIONS.md §4).
pub const MAX_LEAVES: usize = 4096;
/// `log2(MAX_LEAVES)`: longest valid Merkle path.
pub const MAX_TREE_DEPTH: usize = 12;
/// Maximum asset symbol length (fits one BN254 field element when packed as bytes).
pub const MAX_ASSET_LEN: usize = 31;
/// Largest accepted `user_id` (`i64::MAX`, the SQL boundary).
pub const MAX_USER_ID: u64 = i64::MAX as u64;
/// Maximum size of a JSON document accepted by the verifier.
pub const MAX_PROOF_JSON_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidAsset(String),
    InvalidUserId(u64),
}

impl core::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidAsset(reason) => write!(f, "invalid asset symbol: {reason}"),
            Self::InvalidUserId(id) => {
                write!(f, "invalid user_id {id}: must be in 1..={MAX_USER_ID}")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Validates a user id against the protocol domain (`1..=i64::MAX`).
pub fn validate_user_id(user_id: u64) -> Result<u64, ProtocolError> {
    if user_id == 0 || user_id > MAX_USER_ID {
        return Err(ProtocolError::InvalidUserId(user_id));
    }
    Ok(user_id)
}

/// Asset/snapshot context bound into every leaf and empty-padding hash.
///
/// `TreeContext::unscoped()` (empty asset, snapshot 0) exists for callers that have not
/// been adapted yet; production snapshots should use `TreeContext::new`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeContext {
    asset: String,
    snapshot_id: u64,
}

impl TreeContext {
    pub fn unscoped() -> Self {
        Self::default()
    }

    /// `asset` must be 1..=31 chars of `A-Z`, `0-9`, `_`, `-`.
    pub fn new(asset: &str, snapshot_id: u64) -> Result<Self, ProtocolError> {
        if asset.is_empty() {
            return Err(ProtocolError::InvalidAsset("empty".to_string()));
        }
        if asset.len() > MAX_ASSET_LEN {
            return Err(ProtocolError::InvalidAsset(format!(
                "longer than {MAX_ASSET_LEN} bytes"
            )));
        }
        if !asset
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err(ProtocolError::InvalidAsset(
                "only A-Z, 0-9, '_' and '-' are allowed".to_string(),
            ));
        }
        Ok(Self {
            asset: asset.to_string(),
            snapshot_id,
        })
    }

    pub fn asset(&self) -> &str {
        &self.asset
    }

    pub fn snapshot_id(&self) -> u64 {
        self.snapshot_id
    }

    /// Injective packing of the (validated, <= 31 byte) asset into one field element.
    pub(crate) fn asset_field(&self) -> Fr {
        pack_ascii_to_field(self.asset.as_bytes())
    }

    pub(crate) fn snapshot_field(&self) -> Fr {
        Fr::from(self.snapshot_id)
    }
}

/// Packs at most 31 bytes little-endian into a field element (always < modulus).
pub(crate) fn pack_ascii_to_field(bytes: &[u8]) -> Fr {
    let len = core::cmp::min(bytes.len(), MAX_ASSET_LEN);
    Fr::from_le_bytes_mod_order(&bytes[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_domain() {
        assert!(validate_user_id(0).is_err());
        assert!(validate_user_id(1).is_ok());
        assert!(validate_user_id(MAX_USER_ID).is_ok());
        assert!(validate_user_id(MAX_USER_ID + 1).is_err());
    }

    #[test]
    fn context_validation() {
        assert!(TreeContext::new("USDT", 1).is_ok());
        assert!(TreeContext::new("", 1).is_err());
        assert!(TreeContext::new("usdt", 1).is_err());
        assert!(TreeContext::new("US DT", 1).is_err());
        assert!(TreeContext::new("ÜSDT", 1).is_err());
        assert!(TreeContext::new(&"A".repeat(32), 1).is_err());
        assert!(TreeContext::new(&"A".repeat(31), 1).is_ok());
    }

    #[test]
    fn asset_packing_is_injective_for_distinct_assets() {
        let a = TreeContext::new("BTC", 1).map(|c| c.asset_field());
        let b = TreeContext::new("BTD", 1).map(|c| c.asset_field());
        assert_ne!(a, b);
    }
}
