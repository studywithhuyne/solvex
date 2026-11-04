//! Poseidon hashing over the BN254 scalar field.
//!
//! # Parameter provenance (read this before trusting the hash)
//!
//! The permutation shape is the one recommended by the Poseidon paper for a ~254-bit
//! prime field with `alpha = 5`: state width 3 (rate 2, capacity 1), 8 full rounds and
//! 57 partial rounds.
//!
//! The *constants are NOT the output of the reference Grain-LFSR generator* and are
//! **not** the constants used by circomlib/iden3/zcash or any other published instance:
//!
//! * round constants: `PoseidonParameters::random_ark` (arkworks `ark-sponge` 0.3) fed by
//!   `rand::rngs::StdRng::from_seed([42; 32])`;
//! * MDS matrix: next 9 `Fr::rand` outputs of the same RNG with `1` added to the diagonal.
//!   This is a pseudo-random matrix, not a Cauchy construction. Its MDS property is
//!   checked for these concrete values in the unit tests (all entries, all 2x2 minors and
//!   the determinant are non-zero) but no security-margin analysis was performed.
//!
//! These are deterministic *public* constants (no secrecy requirement), so the setup is
//! reproducible, but the provenance is "seeded RNG", which is weaker than a verifiable
//! reference generator. `StdRng`'s algorithm is not guaranteed stable across `rand`
//! releases, therefore the known-answer tests below pin the outputs, the first round
//! constant and the MDS entries; if a dependency bump changes them the tests fail
//! instead of silently forking every root.
//!
//! Replacing the parameters (e.g. with Grain-generated constants) changes every hash and
//! therefore REQUIRES a new `PROTOCOL_VERSION`.
//!
//! # Hash functions
//!
//! * v1 (legacy, `poseidon_leaf_hash` / `poseidon_internal_hash`): kept only for the
//!   legacy membership SNARK and for vector pinning; no domain separation.
//! * v2 (`leaf_hash_v2`, `node_hash_v2`, `empty_hash_v2`): every preimage starts with a
//!   domain tag (ASCII packed into one field element). Leaf and empty hashes absorb the
//!   asset and snapshot context; nodes bind the context transitively through children.
//!
//! ```text
//! leaf  = P(TAG_LEAF,  asset, snapshot, user_id, balance_units)
//! node  = P(TAG_NODE,  left_hash, right_hash, left_sum_units, right_sum_units)
//! empty = P(TAG_EMPTY, asset, snapshot)
//! ```

use ark_bn254::Fr;
use ark_ff::{BigInteger, One, PrimeField, UniformRand, Zero};
use ark_sponge::{
    poseidon::{PoseidonParameters, PoseidonSponge},
    CryptographicSponge,
};
use rand::{rngs::StdRng, SeedableRng};
use rust_decimal::Decimal;

use crate::amount::{decimal_to_units, AmountError, MAX_SUM_UNITS, MAX_UNITS};
use crate::parse::is_canonical_field_bytes;
use crate::protocol::{pack_ascii_to_field, validate_user_id, TreeContext};
use crate::tree::HashBytes;

const FULL_ROUNDS: u64 = 8;
const PARTIAL_ROUNDS: u64 = 57;
const POSEIDON_ALPHA: u64 = 5;
const STATE_WIDTH: usize = 3;
const PARAM_SEED: [u8; 32] = [42u8; 32];

/// Identifier of the parameter set; part of the format documentation, bump with the format.
pub const POSEIDON_PARAMS_ID: &str = "bn254-t3-rf8-rp57-alpha5-stdrng-seed42";

const TAG_LEAF: &[u8] = b"cexzkp/mst/v2/leaf";
const TAG_NODE: &[u8] = b"cexzkp/mst/v2/node";
const TAG_EMPTY: &[u8] = b"cexzkp/mst/v2/empty";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoseidonError {
    NegativeBalance(Decimal),
    BalanceOverflow(Decimal),
    InvalidAmount(AmountError),
    AmountOutOfRange { units: u128, max: u128 },
    NonCanonicalHash,
    InvalidUserId(u64),
}

impl core::fmt::Display for PoseidonError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NegativeBalance(value) => write!(f, "balance must be non-negative, got {value}"),
            Self::BalanceOverflow(value) => write!(f, "balance conversion overflow for {value}"),
            Self::InvalidAmount(err) => write!(f, "invalid amount: {err}"),
            Self::AmountOutOfRange { units, max } => {
                write!(f, "amount {units} units exceeds maximum {max} units")
            }
            Self::NonCanonicalHash => write!(f, "hash bytes are not a canonical field element"),
            Self::InvalidUserId(id) => write!(f, "invalid user_id {id}"),
        }
    }
}

impl std::error::Error for PoseidonError {}

fn map_amount_error(value: &Decimal, err: AmountError) -> PoseidonError {
    match err {
        AmountError::Negative(_) => PoseidonError::NegativeBalance(*value),
        AmountError::Overflow | AmountError::ExceedsMax { .. } => {
            PoseidonError::BalanceOverflow(*value)
        }
        other => PoseidonError::InvalidAmount(other),
    }
}

// ---------------------------------------------------------------------------------
// Legacy (format 1) hashes
// ---------------------------------------------------------------------------------

/// LEGACY v1 leaf hash `P(user_id, balance)`. Not domain separated; do not use for new
/// trees. Amounts are converted exactly (no silent rescale/rounding).
pub fn poseidon_leaf_hash(user_id: u64, balance: &Decimal) -> Result<HashBytes, PoseidonError> {
    let mut sponge = PoseidonSponge::<Fr>::new(poseidon_parameters());

    sponge.absorb(&Fr::from(user_id));
    sponge.absorb(&decimal_to_field(balance)?);

    Ok(field_to_hash_bytes(sponge.squeeze_field_elements(1)[0]))
}

/// LEGACY v1 node hash `P(left_hash, right_hash, left_balance, right_balance)`.
pub fn poseidon_internal_hash(
    left_hash: &HashBytes,
    right_hash: &HashBytes,
    left_balance: &Decimal,
    right_balance: &Decimal,
) -> Result<HashBytes, PoseidonError> {
    let mut sponge = PoseidonSponge::<Fr>::new(poseidon_parameters());

    sponge.absorb(&hash_to_field(left_hash));
    sponge.absorb(&hash_to_field(right_hash));
    sponge.absorb(&decimal_to_field(left_balance)?);
    sponge.absorb(&decimal_to_field(right_balance)?);

    Ok(field_to_hash_bytes(sponge.squeeze_field_elements(1)[0]))
}

// ---------------------------------------------------------------------------------
// Format 2 hashes
// ---------------------------------------------------------------------------------

pub(crate) fn tag_leaf_field() -> Fr {
    pack_ascii_to_field(TAG_LEAF)
}

pub(crate) fn tag_node_field() -> Fr {
    pack_ascii_to_field(TAG_NODE)
}

pub(crate) fn tag_empty_field() -> Fr {
    pack_ascii_to_field(TAG_EMPTY)
}

fn squeeze_one(inputs: &[Fr]) -> HashBytes {
    let mut sponge = PoseidonSponge::<Fr>::new(poseidon_parameters());
    for input in inputs {
        sponge.absorb(input);
    }
    field_to_hash_bytes(sponge.squeeze_field_elements(1)[0])
}

/// Converts hash bytes to a field element, rejecting non-canonical encodings.
pub fn hash_to_field_canonical(hash: &HashBytes) -> Result<Fr, PoseidonError> {
    if !is_canonical_field_bytes(hash) {
        return Err(PoseidonError::NonCanonicalHash);
    }
    Ok(hash_to_field(hash))
}

/// v2 leaf hash. `balance_units` must be `<= MAX_UNITS`, `user_id` in `1..=i64::MAX`.
pub fn leaf_hash_v2(
    context: &TreeContext,
    user_id: u64,
    balance_units: u128,
) -> Result<HashBytes, PoseidonError> {
    validate_user_id(user_id).map_err(|_| PoseidonError::InvalidUserId(user_id))?;
    if balance_units > MAX_UNITS {
        return Err(PoseidonError::AmountOutOfRange {
            units: balance_units,
            max: MAX_UNITS,
        });
    }
    Ok(squeeze_one(&[
        tag_leaf_field(),
        context.asset_field(),
        context.snapshot_field(),
        Fr::from(user_id),
        Fr::from(balance_units),
    ]))
}

/// v2 node hash. Sums must be `<= MAX_SUM_UNITS`; child hashes must be canonical.
pub fn node_hash_v2(
    left_hash: &HashBytes,
    right_hash: &HashBytes,
    left_sum_units: u128,
    right_sum_units: u128,
) -> Result<HashBytes, PoseidonError> {
    for units in [left_sum_units, right_sum_units] {
        if units > MAX_SUM_UNITS {
            return Err(PoseidonError::AmountOutOfRange {
                units,
                max: MAX_SUM_UNITS,
            });
        }
    }
    Ok(squeeze_one(&[
        tag_node_field(),
        hash_to_field_canonical(left_hash)?,
        hash_to_field_canonical(right_hash)?,
        Fr::from(left_sum_units),
        Fr::from(right_sum_units),
    ]))
}

/// v2 hash of an empty padding leaf (balance is implicitly 0).
pub fn empty_hash_v2(context: &TreeContext) -> HashBytes {
    squeeze_one(&[
        tag_empty_field(),
        context.asset_field(),
        context.snapshot_field(),
    ])
}

/// Hashes of all-empty subtrees: index `k` is the root of a zero-sum subtree with `2^k`
/// padding leaves (`k = 0` is the empty leaf). Returns `levels` entries.
pub fn empty_subtree_hashes_v2(
    context: &TreeContext,
    levels: usize,
) -> Result<Vec<HashBytes>, PoseidonError> {
    let mut out = Vec::with_capacity(levels);
    let mut current = empty_hash_v2(context);
    for index in 0..levels {
        if index > 0 {
            current = node_hash_v2(&current, &current, 0, 0)?;
        }
        out.push(current);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------------

pub(crate) fn poseidon_parameters() -> &'static PoseidonParameters<Fr> {
    static PARAMS: std::sync::OnceLock<PoseidonParameters<Fr>> = std::sync::OnceLock::new();
    PARAMS.get_or_init(build_poseidon_parameters)
}

fn derive_mds_and_ark() -> (Vec<Vec<Fr>>, Vec<Vec<Fr>>) {
    let mut rng = StdRng::from_seed(PARAM_SEED);
    let total_rounds = (FULL_ROUNDS + PARTIAL_ROUNDS) as u32;

    let mds = build_deterministic_mds(&mut rng);
    let ark = PoseidonParameters::<Fr>::random_ark(total_rounds, &mut rng);
    (mds, ark)
}

fn build_poseidon_parameters() -> PoseidonParameters<Fr> {
    let (mds, ark) = derive_mds_and_ark();

    PoseidonParameters::new(
        FULL_ROUNDS as u32,
        PARTIAL_ROUNDS as u32,
        POSEIDON_ALPHA,
        mds,
        ark,
    )
}

fn build_deterministic_mds(rng: &mut StdRng) -> Vec<Vec<Fr>> {
    let mut mds = vec![vec![Fr::zero(); STATE_WIDTH]; STATE_WIDTH];

    for (row_idx, row) in mds.iter_mut().enumerate() {
        for value in row.iter_mut() {
            *value = Fr::rand(rng);
        }
        row[row_idx] += Fr::one();
    }

    mds
}

fn decimal_to_field(value: &Decimal) -> Result<Fr, PoseidonError> {
    let units = decimal_to_units(value).map_err(|e| map_amount_error(value, e))?;
    Ok(Fr::from(units))
}

fn hash_to_field(hash: &HashBytes) -> Fr {
    Fr::from_le_bytes_mod_order(hash)
}

pub(crate) fn field_to_hash_bytes(field: Fr) -> HashBytes {
    let bytes = field.into_repr().to_bytes_le();

    let mut out = [0u8; 32];
    let copy_len = core::cmp::min(out.len(), bytes.len());
    out[..copy_len].copy_from_slice(&bytes[..copy_len]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::hash_to_hex;
    use rust_decimal_macros::dec;

    #[test]
    fn poseidon_leaf_hash_is_deterministic() {
        let h1 = poseidon_leaf_hash(7, &dec!(12.34000000)).expect("hash must succeed");
        let h2 = poseidon_leaf_hash(7, &dec!(12.34000000)).expect("hash must succeed");
        assert_eq!(h1, h2);
    }

    #[test]
    fn poseidon_leaf_hash_changes_with_input() {
        let h1 = poseidon_leaf_hash(7, &dec!(12.34)).expect("hash must succeed");
        let h2 = poseidon_leaf_hash(8, &dec!(12.34)).expect("hash must succeed");
        let h3 = poseidon_leaf_hash(7, &dec!(12.35)).expect("hash must succeed");

        assert_ne!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn poseidon_internal_hash_uses_all_inputs() {
        let l1 = poseidon_leaf_hash(1, &dec!(10)).expect("hash must succeed");
        let r1 = poseidon_leaf_hash(2, &dec!(5)).expect("hash must succeed");

        let base =
            poseidon_internal_hash(&l1, &r1, &dec!(10), &dec!(5)).expect("hash must succeed");
        let changed_balance =
            poseidon_internal_hash(&l1, &r1, &dec!(11), &dec!(5)).expect("hash must succeed");
        let changed_child =
            poseidon_internal_hash(&r1, &l1, &dec!(10), &dec!(5)).expect("hash must succeed");

        assert_ne!(base, changed_balance);
        assert_ne!(base, changed_child);
    }

    #[test]
    fn poseidon_rejects_negative_balance() {
        let err = poseidon_leaf_hash(1, &dec!(-1.0)).expect_err("negative balance must fail");
        assert_eq!(err, PoseidonError::NegativeBalance(dec!(-1.0)));
    }

    #[test]
    fn legacy_hash_rejects_over_precision_instead_of_rounding() {
        let err = poseidon_leaf_hash(1, &dec!(0.000000001)).expect_err("must not round");
        assert!(matches!(
            err,
            PoseidonError::InvalidAmount(AmountError::OverPrecision(_))
        ));
    }

    #[test]
    fn same_value_different_scale_hashes_equal() {
        assert_eq!(
            poseidon_leaf_hash(3, &dec!(1.5)).expect("hash"),
            poseidon_leaf_hash(3, &dec!(1.50000000)).expect("hash")
        );
    }

    #[test]
    fn v2_domain_separation() {
        let ctx = TreeContext::new("USDT", 9).expect("ctx");
        let leaf = leaf_hash_v2(&ctx, 1, 0).expect("leaf");
        let empty = empty_hash_v2(&ctx);
        let node = node_hash_v2(&empty, &empty, 0, 0).expect("node");
        assert_ne!(leaf, empty);
        assert_ne!(leaf, node);
        assert_ne!(empty, node);
        let legacy = poseidon_leaf_hash(1, &dec!(0)).expect("legacy");
        assert_ne!(leaf, legacy);
    }

    #[test]
    fn v2_context_is_bound() {
        let a = TreeContext::new("USDT", 9).expect("ctx");
        let b = TreeContext::new("BTC", 9).expect("ctx");
        let c = TreeContext::new("USDT", 10).expect("ctx");
        assert_ne!(leaf_hash_v2(&a, 1, 5), leaf_hash_v2(&b, 1, 5));
        assert_ne!(leaf_hash_v2(&a, 1, 5), leaf_hash_v2(&c, 1, 5));
        assert_ne!(empty_hash_v2(&a), empty_hash_v2(&b));
        assert_ne!(empty_hash_v2(&a), empty_hash_v2(&c));
    }

    #[test]
    fn v2_input_validation() {
        let ctx = TreeContext::unscoped();
        assert!(leaf_hash_v2(&ctx, 0, 1).is_err());
        assert!(leaf_hash_v2(&ctx, 1, MAX_UNITS + 1).is_err());
        assert!(node_hash_v2(&[0xff; 32], &[0; 32], 0, 0).is_err());
        assert!(node_hash_v2(&[0; 32], &[0; 32], MAX_SUM_UNITS + 1, 0).is_err());
    }

    #[test]
    fn empty_subtree_hashes_chain() {
        let ctx = TreeContext::unscoped();
        let hashes = empty_subtree_hashes_v2(&ctx, 3).expect("hashes");
        assert_eq!(hashes[0], empty_hash_v2(&ctx));
        assert_eq!(
            hashes[1],
            node_hash_v2(&hashes[0], &hashes[0], 0, 0).expect("node")
        );
        assert_eq!(hashes.len(), 3);
    }

    // ---- Parameter properties ------------------------------------------------------

    fn det3(m: &[Vec<Fr>]) -> Fr {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    #[test]
    fn mds_matrix_is_mds_for_current_parameters() {
        let (mds, ark) = derive_mds_and_ark();
        assert_eq!(ark.len(), (FULL_ROUNDS + PARTIAL_ROUNDS) as usize);
        assert!(ark.iter().all(|row| row.len() == STATE_WIDTH));

        // Every square submatrix of an MDS matrix is invertible.
        for row in &mds {
            for value in row {
                assert!(!value.is_zero());
            }
        }
        for r0 in 0..3 {
            for r1 in (r0 + 1)..3 {
                for c0 in 0..3 {
                    for c1 in (c0 + 1)..3 {
                        let minor = mds[r0][c0] * mds[r1][c1] - mds[r0][c1] * mds[r1][c0];
                        assert!(!minor.is_zero(), "singular 2x2 minor");
                    }
                }
            }
        }
        assert!(!det3(&mds).is_zero());
    }

    #[test]
    fn parameter_fingerprint_is_pinned() {
        let (mds, ark) = derive_mds_and_ark();
        assert_eq!(hash_to_hex(&field_to_hash_bytes(mds[0][0])), KAT_MDS_00);
        assert_eq!(hash_to_hex(&field_to_hash_bytes(mds[2][2])), KAT_MDS_22);
        assert_eq!(hash_to_hex(&field_to_hash_bytes(ark[0][0])), KAT_ARK_00);
        assert_eq!(hash_to_hex(&field_to_hash_bytes(ark[64][2])), KAT_ARK_LAST);
    }

    // ---- Known-answer tests (hex of little-endian field bytes) ---------------------
    //
    // These vectors were produced by THIS implementation and pin its current behaviour
    // (regression / dependency-drift guard). They are not independent reference vectors
    // from a third-party Poseidon implementation, because the constants are not a
    // published parameter set (see the module docs).

    const KAT_MDS_00: &str = "35740ec548afc23bd480ea8086045b248f341b54e2b3f11356b051b603ffb517";
    const KAT_MDS_22: &str = "d158ae4757346a3306804fec2bedfbde0b4e597cbe38f57774aa646740844118";
    const KAT_ARK_00: &str = "beea92b88c80d33afaeb0d257666fccb3d14432b7502a0d44226528b98060404";
    const KAT_ARK_LAST: &str = "5dc8d137ac49f14ffc6e32c4445e7e709ffe1d7beca444a35c9df197bcb6d218";
    const KAT_V1_LEAF: &str = "7c00c988306ff945bb2dfebde5e0ead26bf622859ea5486a508878492bdeaa11";
    const KAT_V1_NODE: &str = "44ce8e0928a86437aa3da39b4c9720bfaf7951a360779bb3edd710c5feffcb2f";
    const KAT_V2_LEAF: &str = "0ceb4d332ccabc93f4e5c6eff2daeb269999d97354414c3752cf322129cfe61a";
    const KAT_V2_NODE: &str = "93dfd0682ad25436adda075346d11638e6e70130d806e9c92e1ddfed921b5d0a";
    const KAT_V2_EMPTY: &str = "f65ba976b1272ebc7218f1aae1b5c45ed5a35f08c1911a6a96a5506050672801";

    #[test]
    fn known_answer_vectors() {
        let v1_leaf = poseidon_leaf_hash(1, &dec!(10)).expect("v1 leaf");
        let v1_sibling = poseidon_leaf_hash(2, &dec!(5)).expect("v1 leaf");
        let v1_node =
            poseidon_internal_hash(&v1_leaf, &v1_sibling, &dec!(10), &dec!(5)).expect("v1 node");
        assert_eq!(hash_to_hex(&v1_leaf), KAT_V1_LEAF);
        assert_eq!(hash_to_hex(&v1_node), KAT_V1_NODE);

        let ctx = TreeContext::new("USDT", 7).expect("ctx");
        let leaf_a = leaf_hash_v2(&ctx, 1, 1_000_000_000).expect("v2 leaf");
        let leaf_b = leaf_hash_v2(&ctx, 2, 500_000_000).expect("v2 leaf");
        let node = node_hash_v2(&leaf_a, &leaf_b, 1_000_000_000, 500_000_000).expect("v2 node");
        assert_eq!(hash_to_hex(&leaf_a), KAT_V2_LEAF);
        assert_eq!(hash_to_hex(&node), KAT_V2_NODE);
        assert_eq!(hash_to_hex(&empty_hash_v2(&ctx)), KAT_V2_EMPTY);
    }
}
