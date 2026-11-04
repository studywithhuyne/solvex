//! Merkle Sum Tree (format version 2, see `crate::protocol`).
//!
//! Algorithm outline:
//! 1. Validate input: count <= `MAX_LEAVES`, `user_id` in `1..=i64::MAX`, balances
//!    non-negative exact units <= `MAX_UNITS`, no duplicate `user_id`.
//! 2. Sort leaves canonically (strictly ascending `user_id`).
//! 3. Pad with zero-balance empty leaves up to `next_power_of_two(max(N, 1))`.
//! 4. Hash bottom-up; every parent sum is a checked sum of its children.
//!
//! Padding never duplicates a funded leaf, so `10 + 5 + 7` yields a root sum of `22`.

use rust_decimal::Decimal;

use crate::amount::{decimal_to_sum_units, decimal_to_units, AmountError};
use crate::poseidon::{empty_hash_v2, leaf_hash_v2, node_hash_v2, PoseidonError};
use crate::protocol::{
    validate_user_id, TreeContext, CUSTOM_FORMAT_VERSION, MAX_LEAVES, PROTOCOL_VERSION,
};

pub const HASH_BYTES: usize = 32;
pub type HashBytes = [u8; HASH_BYTES];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleNode {
    pub hash: HashBytes,
    pub balance: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceSnapshot {
    pub user_id: u64,
    pub balance: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbBalanceSnapshot {
    pub user_id: i64,
    pub available: Decimal,
    pub locked: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleProofStep {
    pub sibling_hash: HashBytes,
    pub sibling_balance: Decimal,
    pub sibling_is_left: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleProof {
    /// Canonical leaf position inside the tree.
    pub leaf_index: usize,
    pub leaf: MerkleNode,
    pub path: Vec<MerkleProofStep>,
    pub root: MerkleNode,
    /// Format version of the tree that produced this proof (`PROTOCOL_VERSION`).
    pub format_version: u32,
    pub context: TreeContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleSumTree {
    levels: Vec<Vec<MerkleNode>>,
    root: MerkleNode,
    original_leaf_count: usize,
    format_version: u32,
    context: TreeContext,
    /// Canonical (sorted) user ids of the real leaves; empty for raw-leaf trees.
    leaf_user_ids: Vec<u64>,
    /// `input_to_canonical[i]` = canonical position of the `i`-th input snapshot.
    input_to_canonical: Option<Vec<usize>>,
}

impl MerkleSumTree {
    pub fn root(&self) -> &MerkleNode {
        &self.root
    }

    pub fn levels(&self) -> &[Vec<MerkleNode>] {
        &self.levels
    }

    /// Number of real (non padding) leaves.
    pub fn original_leaf_count(&self) -> usize {
        self.original_leaf_count
    }

    /// Capacity: number of leaves including zero padding (a power of two).
    pub fn padded_leaf_count(&self) -> usize {
        self.levels.first().map_or(0, Vec::len)
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    pub fn context(&self) -> &TreeContext {
        &self.context
    }

    /// Canonical position of `user_id`, if the tree was built from snapshots.
    pub fn leaf_index_of_user(&self, user_id: u64) -> Option<usize> {
        self.leaf_user_ids.binary_search(&user_id).ok()
    }

    /// Generates a proof for the leaf at `leaf_index`.
    ///
    /// For trees built from snapshots, `leaf_index` is the index into the *input* slice
    /// (callers do not need to know the canonical order); the returned
    /// `MerkleProof::leaf_index` is the canonical position. For raw-leaf trees the index
    /// is the leaf position.
    pub fn generate_proof(&self, leaf_index: usize) -> Result<MerkleProof, TreeError> {
        if leaf_index >= self.original_leaf_count {
            return Err(TreeError::InvalidLeafIndex {
                index: leaf_index,
                leaf_count: self.original_leaf_count,
            });
        }
        let canonical = match &self.input_to_canonical {
            Some(map) => *map.get(leaf_index).ok_or(TreeError::CorruptTree)?,
            None => leaf_index,
        };
        self.proof_at_canonical(canonical)
    }

    /// Generates a proof for `user_id` (snapshot-built trees only).
    pub fn generate_proof_for_user(&self, user_id: u64) -> Result<MerkleProof, TreeError> {
        let index = self
            .leaf_index_of_user(user_id)
            .ok_or(TreeError::UserNotFound(user_id))?;
        self.proof_at_canonical(index)
    }

    fn proof_at_canonical(&self, canonical: usize) -> Result<MerkleProof, TreeError> {
        let mut index = canonical;
        let mut path = Vec::with_capacity(self.levels.len().saturating_sub(1));

        for level_nodes in self.levels.iter().take(self.levels.len().saturating_sub(1)) {
            let is_right = index % 2 == 1;
            let sibling_index = if is_right { index - 1 } else { index + 1 };
            let sibling = level_nodes
                .get(sibling_index)
                .cloned()
                .ok_or(TreeError::CorruptTree)?;

            path.push(MerkleProofStep {
                sibling_hash: sibling.hash,
                sibling_balance: sibling.balance,
                sibling_is_left: is_right,
            });

            index /= 2;
        }

        let leaf = self
            .levels
            .first()
            .and_then(|level| level.get(canonical))
            .cloned()
            .ok_or(TreeError::CorruptTree)?;

        Ok(MerkleProof {
            leaf_index: canonical,
            leaf,
            path,
            root: self.root.clone(),
            format_version: self.format_version,
            context: self.context.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    /// Leaf-level builders need at least one input (tree builders accept N = 0).
    EmptySnapshotInput,
    InvalidUserId(i64),
    InvalidUserIdValue(u64),
    InvalidLeafIndex {
        index: usize,
        leaf_count: usize,
    },
    NegativeBalance {
        user_id: u64,
        balance: Decimal,
    },
    BalanceOverflow {
        user_id: u64,
    },
    ParentBalanceOverflow,
    HashingError(PoseidonError),
    DuplicateLeaf {
        user_id: u64,
    },
    TooManyLeaves {
        count: usize,
        max: usize,
    },
    InvalidAmount {
        user_id: Option<u64>,
        error: AmountError,
    },
    /// A leaf carrying money uses the reserved empty-padding hash.
    PaddingWithBalance,
    NonCanonicalHash,
    UserNotFound(u64),
    CorruptTree,
}

impl core::fmt::Display for TreeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptySnapshotInput => write!(f, "snapshot input must not be empty"),
            Self::InvalidUserId(user_id) => {
                write!(f, "invalid db user_id (must be >= 0): {user_id}")
            }
            Self::InvalidUserIdValue(user_id) => {
                write!(f, "user_id {user_id} outside 1..=i64::MAX")
            }
            Self::InvalidLeafIndex { index, leaf_count } => {
                write!(f, "invalid leaf index {index}, total leaves: {leaf_count}")
            }
            Self::NegativeBalance { user_id, balance } => {
                write!(f, "negative balance for user {user_id}: {balance}")
            }
            Self::BalanceOverflow { user_id } => {
                write!(
                    f,
                    "balance overflow while aggregating snapshot for user {user_id}"
                )
            }
            Self::ParentBalanceOverflow => write!(f, "balance overflow while building parent node"),
            Self::HashingError(err) => write!(f, "poseidon hashing failed: {err}"),
            Self::DuplicateLeaf { user_id } => write!(f, "duplicate leaf for user {user_id}"),
            Self::TooManyLeaves { count, max } => {
                write!(f, "too many leaves: {count} (maximum {max})")
            }
            Self::InvalidAmount { user_id, error } => match user_id {
                Some(id) => write!(f, "invalid balance for user {id}: {error}"),
                None => write!(f, "invalid balance: {error}"),
            },
            Self::PaddingWithBalance => {
                write!(
                    f,
                    "leaf with the reserved padding hash must have zero balance"
                )
            }
            Self::NonCanonicalHash => write!(f, "leaf hash is not a canonical field element"),
            Self::UserNotFound(id) => write!(f, "user {id} is not a leaf of this tree"),
            Self::CorruptTree => write!(f, "internal tree structure is inconsistent"),
        }
    }
}

impl std::error::Error for TreeError {}

impl From<PoseidonError> for TreeError {
    fn from(value: PoseidonError) -> Self {
        Self::HashingError(value)
    }
}

// ---------------------------------------------------------------------------------
// Snapshot validation and canonical ordering
// ---------------------------------------------------------------------------------

fn validate_balance(snapshot: &BalanceSnapshot) -> Result<u128, TreeError> {
    decimal_to_units(&snapshot.balance).map_err(|error| match error {
        AmountError::Negative(balance) => TreeError::NegativeBalance {
            user_id: snapshot.user_id,
            balance,
        },
        other => TreeError::InvalidAmount {
            user_id: Some(snapshot.user_id),
            error: other,
        },
    })
}

/// Validates snapshots and returns them in canonical order (strictly ascending
/// `user_id`) together with `input_to_canonical` (input index -> canonical position).
///
/// Rejects: more than `MAX_LEAVES` rows, `user_id` outside `1..=i64::MAX`, negative /
/// over-precision / over-limit balances and duplicate `user_id`s.
pub fn canonicalize_snapshots(
    snapshots: &[BalanceSnapshot],
) -> Result<(Vec<BalanceSnapshot>, Vec<usize>), TreeError> {
    if snapshots.len() > MAX_LEAVES {
        return Err(TreeError::TooManyLeaves {
            count: snapshots.len(),
            max: MAX_LEAVES,
        });
    }

    for snapshot in snapshots {
        validate_user_id(snapshot.user_id)
            .map_err(|_| TreeError::InvalidUserIdValue(snapshot.user_id))?;
        validate_balance(snapshot)?;
    }

    let mut order: Vec<usize> = (0..snapshots.len()).collect();
    order.sort_by_key(|&i| snapshots[i].user_id);

    let mut input_to_canonical = vec![0usize; snapshots.len()];
    let mut canonical = Vec::with_capacity(snapshots.len());
    for (position, &input_index) in order.iter().enumerate() {
        let snapshot = &snapshots[input_index];
        if let Some(previous) = canonical.last() {
            let previous: &BalanceSnapshot = previous;
            if previous.user_id == snapshot.user_id {
                return Err(TreeError::DuplicateLeaf {
                    user_id: snapshot.user_id,
                });
            }
        }
        input_to_canonical[input_index] = position;
        canonical.push(snapshot.clone());
    }

    Ok((canonical, input_to_canonical))
}

/// Returns true if snapshots are already in canonical order and valid.
pub fn is_canonical_order(snapshots: &[BalanceSnapshot]) -> bool {
    snapshots.windows(2).all(|w| w[0].user_id < w[1].user_id)
}

// ---------------------------------------------------------------------------------
// Leaf builders
// ---------------------------------------------------------------------------------

/// Generic leaf builder with a caller supplied hasher (input order preserved).
/// Rejects empty input, negative balances and duplicate `user_id`s.
pub fn build_leaf_nodes<F>(
    snapshots: &[BalanceSnapshot],
    mut leaf_hasher: F,
) -> Result<Vec<MerkleNode>, TreeError>
where
    F: FnMut(u64, &Decimal) -> HashBytes,
{
    if snapshots.is_empty() {
        return Err(TreeError::EmptySnapshotInput);
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut leaves = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        if snapshot.balance.mantissa() < 0 {
            return Err(TreeError::NegativeBalance {
                user_id: snapshot.user_id,
                balance: snapshot.balance,
            });
        }
        if !seen.insert(snapshot.user_id) {
            return Err(TreeError::DuplicateLeaf {
                user_id: snapshot.user_id,
            });
        }

        leaves.push(MerkleNode {
            hash: leaf_hasher(snapshot.user_id, &snapshot.balance),
            balance: snapshot.balance,
        });
    }

    Ok(leaves)
}

pub fn build_leaf_nodes_from_db_snapshots<F>(
    db_snapshots: &[DbBalanceSnapshot],
    leaf_hasher: F,
) -> Result<Vec<MerkleNode>, TreeError>
where
    F: FnMut(u64, &Decimal) -> HashBytes,
{
    let normalized = normalize_db_snapshots(db_snapshots)?;
    build_leaf_nodes(&normalized, leaf_hasher)
}

/// v2 leaves in canonical order, unscoped context.
pub fn build_poseidon_leaf_nodes(
    snapshots: &[BalanceSnapshot],
) -> Result<Vec<MerkleNode>, TreeError> {
    build_poseidon_leaf_nodes_with_context(snapshots, &TreeContext::unscoped())
}

/// v2 leaves in canonical order (strictly ascending `user_id`), bound to `context`.
pub fn build_poseidon_leaf_nodes_with_context(
    snapshots: &[BalanceSnapshot],
    context: &TreeContext,
) -> Result<Vec<MerkleNode>, TreeError> {
    if snapshots.is_empty() {
        return Err(TreeError::EmptySnapshotInput);
    }
    let (canonical, _) = canonicalize_snapshots(snapshots)?;
    leaves_from_canonical(&canonical, context)
}

fn leaves_from_canonical(
    canonical: &[BalanceSnapshot],
    context: &TreeContext,
) -> Result<Vec<MerkleNode>, TreeError> {
    let mut leaves = Vec::with_capacity(canonical.len());
    for snapshot in canonical {
        let units = validate_balance(snapshot)?;
        let hash = leaf_hash_v2(context, snapshot.user_id, units)?;
        leaves.push(MerkleNode {
            hash,
            balance: snapshot.balance,
        });
    }
    Ok(leaves)
}

pub fn build_poseidon_leaf_nodes_from_db_snapshots(
    db_snapshots: &[DbBalanceSnapshot],
) -> Result<Vec<MerkleNode>, TreeError> {
    let normalized = normalize_db_snapshots(db_snapshots)?;
    build_poseidon_leaf_nodes(&normalized)
}

// ---------------------------------------------------------------------------------
// Tree builders
// ---------------------------------------------------------------------------------

/// Builds the levels with zero padding up to the canonical capacity.
fn build_levels<F>(
    leaves: &[MerkleNode],
    padding_hash: HashBytes,
    mut internal_hasher: F,
) -> Result<(Vec<Vec<MerkleNode>>, MerkleNode), TreeError>
where
    F: FnMut(&MerkleNode, &MerkleNode) -> Result<HashBytes, TreeError>,
{
    if leaves.len() > MAX_LEAVES {
        return Err(TreeError::TooManyLeaves {
            count: leaves.len(),
            max: MAX_LEAVES,
        });
    }

    let capacity = leaves.len().max(1).next_power_of_two();
    let padding = MerkleNode {
        hash: padding_hash,
        balance: Decimal::ZERO,
    };

    let mut current_level = Vec::with_capacity(capacity);
    current_level.extend_from_slice(leaves);
    current_level.resize(capacity, padding);

    let mut levels: Vec<Vec<MerkleNode>> = vec![current_level.clone()];

    while current_level.len() > 1 {
        let mut next_level = Vec::with_capacity(current_level.len() / 2);

        for pair in current_level.chunks_exact(2) {
            let left = &pair[0];
            let right = &pair[1];

            let parent_balance = left
                .balance
                .checked_add(right.balance)
                .ok_or(TreeError::ParentBalanceOverflow)?;
            let parent_hash = internal_hasher(left, right)?;

            next_level.push(MerkleNode {
                hash: parent_hash,
                balance: parent_balance,
            });
        }

        levels.push(next_level.clone());
        current_level = next_level;
    }

    let root = current_level
        .into_iter()
        .next()
        .ok_or(TreeError::CorruptTree)?;
    Ok((levels, root))
}

/// Generic tree builder with caller supplied hasher. Padding leaves are zero-balance
/// nodes with an all-zero hash. `leaves` may be empty (empty tree, sum 0).
///
/// The resulting tree is tagged `CUSTOM_FORMAT_VERSION`; it is not a protocol tree.
pub fn build_merkle_sum_tree_from_leaves<F>(
    leaves: &[MerkleNode],
    internal_hasher: F,
) -> Result<MerkleSumTree, TreeError>
where
    F: FnMut(&MerkleNode, &MerkleNode) -> Result<HashBytes, TreeError>,
{
    let (levels, root) = build_levels(leaves, [0u8; HASH_BYTES], internal_hasher)?;
    Ok(MerkleSumTree {
        levels,
        root,
        original_leaf_count: leaves.len(),
        format_version: CUSTOM_FORMAT_VERSION,
        context: TreeContext::unscoped(),
        leaf_user_ids: Vec::new(),
        input_to_canonical: None,
    })
}

fn node_hasher_v2(left: &MerkleNode, right: &MerkleNode) -> Result<HashBytes, TreeError> {
    let left_units =
        decimal_to_sum_units(&left.balance).map_err(|error| TreeError::InvalidAmount {
            user_id: None,
            error,
        })?;
    let right_units =
        decimal_to_sum_units(&right.balance).map_err(|error| TreeError::InvalidAmount {
            user_id: None,
            error,
        })?;
    node_hash_v2(&left.hash, &right.hash, left_units, right_units).map_err(TreeError::from)
}

/// v2 tree from prebuilt leaves, unscoped context.
pub fn build_poseidon_merkle_sum_tree_from_leaves(
    leaves: &[MerkleNode],
) -> Result<MerkleSumTree, TreeError> {
    build_poseidon_merkle_sum_tree_from_leaves_with_context(leaves, &TreeContext::unscoped())
}

/// v2 tree from prebuilt leaves (position = canonical position). Leaves must be
/// canonical field elements with balance `<= MAX_UNITS`; a leaf using the reserved
/// padding hash must carry zero balance.
pub fn build_poseidon_merkle_sum_tree_from_leaves_with_context(
    leaves: &[MerkleNode],
    context: &TreeContext,
) -> Result<MerkleSumTree, TreeError> {
    let padding_hash = empty_hash_v2(context);

    for leaf in leaves {
        let units = decimal_to_units(&leaf.balance).map_err(|error| TreeError::InvalidAmount {
            user_id: None,
            error,
        })?;
        if !crate::parse::is_canonical_field_bytes(&leaf.hash) {
            return Err(TreeError::NonCanonicalHash);
        }
        if leaf.hash == padding_hash && units != 0 {
            return Err(TreeError::PaddingWithBalance);
        }
    }

    let (levels, root) = build_levels(leaves, padding_hash, node_hasher_v2)?;
    Ok(MerkleSumTree {
        levels,
        root,
        original_leaf_count: leaves.len(),
        format_version: PROTOCOL_VERSION,
        context: context.clone(),
        leaf_user_ids: Vec::new(),
        input_to_canonical: None,
    })
}

/// v2 tree from snapshots, unscoped context (compatibility entry point).
/// `N = 0` yields the empty tree.
pub fn build_poseidon_merkle_sum_tree(
    snapshots: &[BalanceSnapshot],
) -> Result<MerkleSumTree, TreeError> {
    build_poseidon_merkle_sum_tree_with_context(snapshots, &TreeContext::unscoped())
}

/// v2 tree from snapshots bound to `context` (asset + snapshot id).
///
/// Snapshots may be given in any order; leaves are sorted canonically and
/// `generate_proof(i)` refers to the i-th input snapshot.
pub fn build_poseidon_merkle_sum_tree_with_context(
    snapshots: &[BalanceSnapshot],
    context: &TreeContext,
) -> Result<MerkleSumTree, TreeError> {
    let (canonical, input_to_canonical) = canonicalize_snapshots(snapshots)?;
    let leaves = leaves_from_canonical(&canonical, context)?;

    let mut tree = build_poseidon_merkle_sum_tree_from_leaves_with_context(&leaves, context)?;
    tree.leaf_user_ids = canonical.iter().map(|s| s.user_id).collect();
    tree.input_to_canonical = Some(input_to_canonical);
    Ok(tree)
}

pub fn build_poseidon_merkle_sum_tree_from_db_snapshots(
    db_snapshots: &[DbBalanceSnapshot],
) -> Result<MerkleSumTree, TreeError> {
    let normalized = normalize_db_snapshots(db_snapshots)?;
    build_poseidon_merkle_sum_tree(&normalized)
}

fn normalize_db_snapshots(
    db_snapshots: &[DbBalanceSnapshot],
) -> Result<Vec<BalanceSnapshot>, TreeError> {
    if db_snapshots.is_empty() {
        return Err(TreeError::EmptySnapshotInput);
    }

    let mut normalized = Vec::with_capacity(db_snapshots.len());
    for row in db_snapshots {
        if row.user_id < 0 {
            return Err(TreeError::InvalidUserId(row.user_id));
        }

        let user_id =
            u64::try_from(row.user_id).map_err(|_| TreeError::InvalidUserId(row.user_id))?;
        let balance = row
            .available
            .checked_add(row.locked)
            .ok_or(TreeError::BalanceOverflow { user_id })?;

        normalized.push(BalanceSnapshot { user_id, balance });
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rust_decimal_macros::dec;

    fn deterministic_hash(user_id: u64, balance: &Decimal) -> [u8; 32] {
        let mut hash = [0u8; 32];
        hash[0] = (user_id & 0xFF) as u8;
        hash[1] = (balance.scale() & 0xFF) as u8;
        hash
    }

    fn deterministic_internal_hash(
        left: &MerkleNode,
        right: &MerkleNode,
    ) -> Result<[u8; 32], TreeError> {
        let mut hash = [0u8; 32];
        hash[0] = left.hash[0].wrapping_add(right.hash[0]);
        hash[1] = ((left.balance.mantissa() + right.balance.mantissa()) & 0xFF) as u8;
        Ok(hash)
    }

    fn snap(user_id: u64, balance: Decimal) -> BalanceSnapshot {
        BalanceSnapshot { user_id, balance }
    }

    fn snaps(balances: &[u32]) -> Vec<BalanceSnapshot> {
        balances
            .iter()
            .enumerate()
            .map(|(i, b)| snap(i as u64 + 1, Decimal::from(*b)))
            .collect()
    }

    #[test]
    fn build_leaf_nodes_successfully_from_snapshots() {
        let snapshots = vec![snap(1, dec!(12.5)), snap(2, dec!(7.25))];

        let leaves =
            build_leaf_nodes(&snapshots, deterministic_hash).expect("leaf build must succeed");
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].balance, dec!(12.5));
        assert_eq!(leaves[0].hash[0], 1);
        assert_eq!(leaves[1].balance, dec!(7.25));
        assert_eq!(leaves[1].hash[0], 2);
    }

    #[test]
    fn build_leaf_nodes_from_db_snapshots_aggregates_balances() {
        let db_snapshots = vec![
            DbBalanceSnapshot {
                user_id: 42,
                available: dec!(10.50),
                locked: dec!(1.25),
            },
            DbBalanceSnapshot {
                user_id: 7,
                available: dec!(3),
                locked: dec!(0),
            },
        ];

        let leaves = build_leaf_nodes_from_db_snapshots(&db_snapshots, deterministic_hash)
            .expect("leaf build must succeed");

        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].balance, dec!(11.75));
        assert_eq!(leaves[0].hash[0], 42);
        assert_eq!(leaves[1].balance, dec!(3));
        assert_eq!(leaves[1].hash[0], 7);
    }

    #[test]
    fn build_leaf_nodes_rejects_negative_balance() {
        let err = build_leaf_nodes(&[snap(9, dec!(-0.1))], deterministic_hash)
            .expect_err("negative balance must fail");
        assert_eq!(
            err,
            TreeError::NegativeBalance {
                user_id: 9,
                balance: dec!(-0.1)
            }
        );
    }

    #[test]
    fn build_leaf_nodes_rejects_empty_input() {
        let err = build_leaf_nodes(&[], deterministic_hash).expect_err("empty input must fail");
        assert_eq!(err, TreeError::EmptySnapshotInput);
    }

    #[test]
    fn build_leaf_nodes_rejects_duplicate_user() {
        let err = build_leaf_nodes(&[snap(1, dec!(1)), snap(1, dec!(2))], deterministic_hash)
            .expect_err("duplicate must fail");
        assert_eq!(err, TreeError::DuplicateLeaf { user_id: 1 });
    }

    #[test]
    fn build_leaf_nodes_from_db_snapshots_rejects_negative_user_id() {
        let db_snapshots = vec![DbBalanceSnapshot {
            user_id: -1,
            available: dec!(1),
            locked: dec!(2),
        }];

        let err = build_leaf_nodes_from_db_snapshots(&db_snapshots, deterministic_hash)
            .expect_err("invalid user id must fail");
        assert_eq!(err, TreeError::InvalidUserId(-1));
    }

    #[test]
    fn build_poseidon_leaf_nodes_successfully() {
        let snapshots = vec![snap(11, dec!(100.01)), snap(12, dec!(99.99))];

        let leaves =
            build_poseidon_leaf_nodes(&snapshots).expect("poseidon leaf build must succeed");
        assert_eq!(leaves.len(), 2);
        assert_ne!(leaves[0].hash, leaves[1].hash);
    }

    #[test]
    fn build_merkle_sum_tree_bottom_up_with_zero_padding() {
        let leaves = vec![
            MerkleNode {
                hash: [1u8; 32],
                balance: dec!(10),
            },
            MerkleNode {
                hash: [2u8; 32],
                balance: dec!(5),
            },
            MerkleNode {
                hash: [3u8; 32],
                balance: dec!(7),
            },
        ];

        let tree = build_merkle_sum_tree_from_leaves(&leaves, deterministic_internal_hash)
            .expect("tree build must succeed");

        assert_eq!(tree.original_leaf_count(), 3);
        assert_eq!(tree.padded_leaf_count(), 4);
        assert_eq!(tree.levels().len(), 3);
        assert_ne!(tree.levels()[0][2], tree.levels()[0][3]);
        assert_eq!(tree.levels()[0][3].balance, dec!(0));
        assert_eq!(tree.root().balance, dec!(22));
    }

    /// Acceptance regression for the legacy defect pinned by
    /// `known_failure_odd_tree_duplicates_funded_leaf` in core/tests/baseline.rs.
    #[test]
    fn odd_tree_does_not_duplicate_funded_leaf_10_5_7_is_22() {
        let tree = build_poseidon_merkle_sum_tree(&snaps(&[10, 5, 7])).expect("tree");
        assert_eq!(tree.root().balance, dec!(22));
        assert_ne!(tree.root().balance, dec!(29));
        assert_eq!(tree.padded_leaf_count(), 4);
        let padding = &tree.levels()[0][3];
        assert_eq!(padding.balance, dec!(0));
        assert_ne!(tree.levels()[0][2], *padding);
    }

    #[test]
    fn root_sum_is_correct_for_small_n() {
        for n in [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 9] {
            let balances: Vec<u32> = (0..n).map(|i| (i as u32 + 1) * 3).collect();
            let expected: u32 = balances.iter().sum();
            let tree = build_poseidon_merkle_sum_tree(&snaps(&balances)).expect("tree");
            assert_eq!(tree.root().balance, Decimal::from(expected), "N = {n}");
            assert_eq!(tree.original_leaf_count(), n);
            assert_eq!(tree.padded_leaf_count(), n.max(1).next_power_of_two());
        }
    }

    #[test]
    fn empty_and_single_leaf_conventions() {
        let ctx = TreeContext::new("USDT", 1).expect("ctx");

        let empty = build_poseidon_merkle_sum_tree_with_context(&[], &ctx).expect("empty tree");
        assert_eq!(empty.root().balance, dec!(0));
        assert_eq!(empty.root().hash, empty_hash_v2(&ctx));
        assert_eq!(empty.padded_leaf_count(), 1);
        assert!(empty.generate_proof(0).is_err());

        let single =
            build_poseidon_merkle_sum_tree_with_context(&[snap(5, dec!(3))], &ctx).expect("single");
        assert_eq!(single.root().balance, dec!(3));
        assert_eq!(single.padded_leaf_count(), 1);
        let proof = single.generate_proof(0).expect("proof");
        assert!(proof.path.is_empty());
        assert_eq!(proof.leaf, proof.root);
        assert_eq!(proof.format_version, PROTOCOL_VERSION);
        assert_ne!(single.root().hash, empty.root().hash);
    }

    #[test]
    fn rejects_duplicate_user_ids() {
        let err =
            build_poseidon_merkle_sum_tree(&[snap(1, dec!(1)), snap(2, dec!(1)), snap(1, dec!(4))])
                .expect_err("duplicate");
        assert_eq!(err, TreeError::DuplicateLeaf { user_id: 1 });
    }

    #[test]
    fn rejects_invalid_user_ids_and_amounts() {
        assert_eq!(
            build_poseidon_merkle_sum_tree(&[snap(0, dec!(1))]).expect_err("zero id"),
            TreeError::InvalidUserIdValue(0)
        );
        assert!(build_poseidon_merkle_sum_tree(&[snap(u64::MAX, dec!(1))]).is_err());
        assert!(matches!(
            build_poseidon_merkle_sum_tree(&[snap(1, dec!(-1))]),
            Err(TreeError::NegativeBalance { .. })
        ));
        assert!(matches!(
            build_poseidon_merkle_sum_tree(&[snap(1, dec!(0.000000001))]),
            Err(TreeError::InvalidAmount { .. })
        ));
        assert!(matches!(
            build_poseidon_merkle_sum_tree(&[snap(1, dec!(10000000001))]),
            Err(TreeError::InvalidAmount { .. })
        ));
    }

    #[test]
    fn rejects_too_many_leaves() {
        let many: Vec<_> = (1..=(MAX_LEAVES as u64 + 1))
            .map(|i| snap(i, dec!(0)))
            .collect();
        assert_eq!(
            build_poseidon_merkle_sum_tree(&many).expect_err("too many"),
            TreeError::TooManyLeaves {
                count: MAX_LEAVES + 1,
                max: MAX_LEAVES
            }
        );
    }

    #[test]
    fn padding_leaf_carrying_money_is_rejected() {
        let ctx = TreeContext::unscoped();
        let forged = MerkleNode {
            hash: empty_hash_v2(&ctx),
            balance: dec!(1),
        };
        assert_eq!(
            build_poseidon_merkle_sum_tree_from_leaves(&[forged]).expect_err("padding with money"),
            TreeError::PaddingWithBalance
        );
        let zero_ok = MerkleNode {
            hash: empty_hash_v2(&ctx),
            balance: dec!(0),
        };
        assert!(build_poseidon_merkle_sum_tree_from_leaves(&[zero_ok]).is_ok());
    }

    #[test]
    fn non_canonical_leaf_hash_is_rejected() {
        let leaf = MerkleNode {
            hash: [0xff; 32],
            balance: dec!(1),
        };
        assert_eq!(
            build_poseidon_merkle_sum_tree_from_leaves(&[leaf]).expect_err("non canonical"),
            TreeError::NonCanonicalHash
        );
    }

    #[test]
    fn input_order_does_not_change_root_and_index_follows_input() {
        let ordered = vec![snap(1, dec!(10)), snap(2, dec!(5)), snap(3, dec!(7))];
        let shuffled = vec![snap(3, dec!(7)), snap(1, dec!(10)), snap(2, dec!(5))];
        let a = build_poseidon_merkle_sum_tree(&ordered).expect("tree");
        let b = build_poseidon_merkle_sum_tree(&shuffled).expect("tree");
        assert_eq!(a.root(), b.root());

        // Input index 0 of `shuffled` is user 3, canonical position 2.
        let proof = b.generate_proof(0).expect("proof");
        assert_eq!(proof.leaf_index, 2);
        assert_eq!(proof.leaf.balance, dec!(7));
        assert_eq!(b.generate_proof_for_user(3).expect("proof"), proof);
        assert_eq!(
            b.generate_proof_for_user(99),
            Err(TreeError::UserNotFound(99))
        );
    }

    #[test]
    fn context_changes_root() {
        let data = snaps(&[1, 2, 3]);
        let usdt = build_poseidon_merkle_sum_tree_with_context(
            &data,
            &TreeContext::new("USDT", 1).expect("ctx"),
        )
        .expect("tree");
        let btc = build_poseidon_merkle_sum_tree_with_context(
            &data,
            &TreeContext::new("BTC", 1).expect("ctx"),
        )
        .expect("tree");
        let later = build_poseidon_merkle_sum_tree_with_context(
            &data,
            &TreeContext::new("USDT", 2).expect("ctx"),
        )
        .expect("tree");
        assert_ne!(usdt.root().hash, btc.root().hash);
        assert_ne!(usdt.root().hash, later.root().hash);
        assert_eq!(usdt.root().balance, btc.root().balance);
    }

    #[test]
    fn canonical_order_helper() {
        assert!(is_canonical_order(&[snap(1, dec!(0)), snap(2, dec!(0))]));
        assert!(!is_canonical_order(&[snap(2, dec!(0)), snap(1, dec!(0))]));
        assert!(!is_canonical_order(&[snap(1, dec!(0)), snap(1, dec!(0))]));
    }

    #[test]
    fn generate_merkle_proof_returns_expected_path_depth() {
        let snapshots = vec![snap(1, dec!(4)), snap(2, dec!(6)), snap(3, dec!(10))];

        let tree =
            build_poseidon_merkle_sum_tree(&snapshots).expect("poseidon tree build must succeed");
        let proof = tree
            .generate_proof(1)
            .expect("proof generation must succeed");

        assert_eq!(proof.leaf_index, 1);
        assert_eq!(proof.path.len(), tree.levels().len() - 1);
        assert_eq!(proof.root, tree.root().clone());
    }

    #[test]
    fn generate_merkle_proof_rejects_out_of_range_leaf_index() {
        let tree = build_poseidon_merkle_sum_tree(&[snap(1, dec!(1)), snap(2, dec!(2))])
            .expect("poseidon tree build must succeed");
        let err = tree
            .generate_proof(2)
            .expect_err("out-of-range index must fail");

        assert_eq!(
            err,
            TreeError::InvalidLeafIndex {
                index: 2,
                leaf_count: 2
            }
        );
    }

    #[test]
    fn padding_leaf_has_no_proof() {
        let tree = build_poseidon_merkle_sum_tree(&snaps(&[1, 2, 3])).expect("tree");
        assert!(tree.generate_proof(3).is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn random_trees_have_exact_root_sum(
            balances in proptest::collection::vec(0u64..=1_000_000_000_000u64, 0..40),
            seed in any::<u64>(),
        ) {
            let mut data: Vec<BalanceSnapshot> = balances
                .iter()
                .enumerate()
                .map(|(i, units)| BalanceSnapshot {
                    user_id: (i as u64 + 1) * 7 + 1,
                    balance: Decimal::new(*units as i64, 8),
                })
                .collect();
            // Deterministic pseudo-shuffle driven by the seed.
            let len = data.len();
            if len > 1 {
                for i in 0..len {
                    let j = ((seed.wrapping_mul(6364136223846793005).wrapping_add(i as u64)) % len as u64) as usize;
                    data.swap(i, j);
                }
            }
            let expected: u128 = balances.iter().map(|b| u128::from(*b)).sum();

            let tree = build_poseidon_merkle_sum_tree(&data).expect("tree");
            let actual = decimal_to_sum_units(&tree.root().balance).expect("units");
            prop_assert_eq!(actual, expected);

            for (index, snapshot) in data.iter().enumerate() {
                let proof = tree.generate_proof(index).expect("proof");
                prop_assert_eq!(proof.leaf.balance, snapshot.balance);
                prop_assert_eq!(proof.path.len(), tree.levels().len() - 1);
            }
        }
    }
}
