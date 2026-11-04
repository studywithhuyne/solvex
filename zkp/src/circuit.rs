//! R1CS primitives for the Merkle Sum Tree node relation (format version 2).
//!
//! Statement: `parent_hash == node_hash_v2(left_hash, right_hash, left_sum, right_sum)`
//! and `parent_sum == left_sum + right_sum` with every sum range-checked.
//!
//! Soundness notes (PLAN-07):
//! * Each balance is allocated once as 128 boolean bits; the `FpVar` used in the hash and
//!   in the sum is *derived* from those bits, so bit and field representations cannot
//!   diverge.
//! * Sums are range checked to `SUM_BITS` bits (`MAX_SUM_UNITS < 2^72`).
//! * The ripple-carry adder forces the carry-in to 0 and the carry-out to 0.
//! * Hashes and the parent sum are public inputs (instance variables), in the order
//!   `[left_hash, right_hash, parent_hash, parent_sum]`.
//! * Hash bytes must be canonical field elements; non-canonical encodings make
//!   constraint generation fail instead of being reduced modulo `r`.
//!
//! This is only the single-node relation; the full tree/path circuit and its trusted
//! setup are out of scope (PLAN-20/21).

use ark_bn254::Fr;
use ark_r1cs_std::{
    bits::{boolean::Boolean, uint128::UInt128},
    fields::fp::FpVar,
    prelude::{AllocVar, EqGadget},
};
use ark_relations::ns;
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};
use ark_sponge::{constraints::CryptographicSpongeVar, poseidon::constraints::PoseidonSpongeVar};
use rust_decimal::Decimal;

use crate::amount::{decimal_to_sum_units, AmountError};
use crate::poseidon::{hash_to_field_canonical, poseidon_parameters, tag_node_field};
use crate::tree::HashBytes;

/// Number of low bits a sum may occupy: `MAX_SUM_UNITS` (4.096e21) < 2^72.
pub const SUM_BITS: usize = 72;
const U128_BITS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitInputError {
    NegativeBalance(Decimal),
    BalanceConversionOverflow(Decimal),
    InvalidAmount(AmountError),
}

impl core::fmt::Display for CircuitInputError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NegativeBalance(value) => write!(f, "balance must be non-negative, got {value}"),
            Self::BalanceConversionOverflow(value) => {
                write!(f, "balance conversion overflow for {value}")
            }
            Self::InvalidAmount(err) => write!(f, "invalid amount: {err}"),
        }
    }
}

impl std::error::Error for CircuitInputError {}

#[derive(Debug, Clone)]
pub struct MerkleNodeRelationCircuit {
    pub left_hash: HashBytes,
    pub right_hash: HashBytes,
    pub parent_hash: HashBytes,
    pub left_balance_scaled: u128,
    pub right_balance_scaled: u128,
    pub parent_balance_scaled: u128,
}

impl MerkleNodeRelationCircuit {
    pub fn from_scaled(
        left_hash: HashBytes,
        right_hash: HashBytes,
        parent_hash: HashBytes,
        left_balance_scaled: u128,
        right_balance_scaled: u128,
        parent_balance_scaled: u128,
    ) -> Self {
        Self {
            left_hash,
            right_hash,
            parent_hash,
            left_balance_scaled,
            right_balance_scaled,
            parent_balance_scaled,
        }
    }

    /// Converts exact decimals (scale <= 8, no rounding) to units.
    pub fn from_decimals(
        left_hash: HashBytes,
        right_hash: HashBytes,
        parent_hash: HashBytes,
        left_balance: Decimal,
        right_balance: Decimal,
        parent_balance: Decimal,
    ) -> Result<Self, CircuitInputError> {
        Ok(Self {
            left_hash,
            right_hash,
            parent_hash,
            left_balance_scaled: decimal_to_scaled_u128(left_balance)?,
            right_balance_scaled: decimal_to_scaled_u128(right_balance)?,
            parent_balance_scaled: decimal_to_scaled_u128(parent_balance)?,
        })
    }
}

impl ConstraintSynthesizer<Fr> for MerkleNodeRelationCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let left_hash =
            hash_to_field_canonical(&self.left_hash).map_err(|_| SynthesisError::Unsatisfiable)?;
        let right_hash =
            hash_to_field_canonical(&self.right_hash).map_err(|_| SynthesisError::Unsatisfiable)?;
        let parent_hash = hash_to_field_canonical(&self.parent_hash)
            .map_err(|_| SynthesisError::Unsatisfiable)?;

        // Public inputs: [left_hash, right_hash, parent_hash, parent_sum].
        let left_hash_var = FpVar::<Fr>::new_input(ns!(cs, "left_hash"), || Ok(left_hash))?;
        let right_hash_var = FpVar::<Fr>::new_input(ns!(cs, "right_hash"), || Ok(right_hash))?;
        let parent_hash_var = FpVar::<Fr>::new_input(ns!(cs, "parent_hash"), || Ok(parent_hash))?;
        let parent_sum_var = FpVar::<Fr>::new_input(ns!(cs, "parent_sum"), || {
            Ok(Fr::from(self.parent_balance_scaled))
        })?;

        // Balances exist once, as bits.
        let left_bits = alloc_u128_bits(&cs, self.left_balance_scaled)?;
        let right_bits = alloc_u128_bits(&cs, self.right_balance_scaled)?;
        let parent_bits = alloc_u128_bits(&cs, self.parent_balance_scaled)?;

        // Range: every sum fits in SUM_BITS bits.
        enforce_bits_fit(&left_bits, SUM_BITS)?;
        enforce_bits_fit(&right_bits, SUM_BITS)?;
        enforce_bits_fit(&parent_bits, SUM_BITS)?;

        // Field values are derived from the bits (no independent field witness).
        let left_fp = bits_to_fp(&left_bits)?;
        let right_fp = bits_to_fp(&right_bits)?;
        let parent_fp = bits_to_fp(&parent_bits)?;

        // The public parent sum is the one the bits encode.
        enforce_fp_matches_bits(&parent_sum_var, &parent_bits)?;
        // Sum constraint in field form (exact: all operands < 2^72).
        (left_fp.clone() + right_fp.clone()).enforce_equal(&parent_fp)?;

        // Poseidon hash: parent == P(TAG_NODE, left_hash, right_hash, left_sum, right_sum).
        let tag_var = FpVar::<Fr>::new_constant(cs.clone(), tag_node_field())?;
        let mut sponge = PoseidonSpongeVar::<Fr>::new(cs.clone(), poseidon_parameters());
        sponge.absorb(&tag_var)?;
        sponge.absorb(&left_hash_var)?;
        sponge.absorb(&right_hash_var)?;
        sponge.absorb(&left_fp)?;
        sponge.absorb(&right_fp)?;
        let expected_parent_hash = sponge.squeeze_field_elements(1)?[0].clone();
        expected_parent_hash.enforce_equal(&parent_hash_var)?;

        // Bitwise adder with carry-in == 0 and carry-out == 0 (no u128 overflow).
        let carries = compute_carry_bits(self.left_balance_scaled, self.right_balance_scaled);
        enforce_u128_addition_no_overflow(&cs, &left_bits, &right_bits, &parent_bits, &carries)
    }
}

fn alloc_u128_bits(
    cs: &ConstraintSystemRef<Fr>,
    value: u128,
) -> Result<Vec<Boolean<Fr>>, SynthesisError> {
    let var = UInt128::<Fr>::new_witness(cs.clone(), || Ok(value))?;
    Ok(var.to_bits_le())
}

/// Forces every bit at index `>= width` to be 0 (range check `value < 2^width`).
pub(crate) fn enforce_bits_fit(bits: &[Boolean<Fr>], width: usize) -> Result<(), SynthesisError> {
    for bit in bits.iter().skip(width) {
        bit.enforce_equal(&Boolean::constant(false))?;
    }
    Ok(())
}

/// Linear-combination of little-endian bits as a field element. Exact for <= 252 bits.
pub(crate) fn bits_to_fp(bits: &[Boolean<Fr>]) -> Result<FpVar<Fr>, SynthesisError> {
    if bits.len() > 252 {
        return Err(SynthesisError::Unsatisfiable);
    }
    Boolean::le_bits_to_fp_var(bits)
}

/// Links an independently allocated field variable to a bit decomposition.
pub(crate) fn enforce_fp_matches_bits(
    fp: &FpVar<Fr>,
    bits: &[Boolean<Fr>],
) -> Result<(), SynthesisError> {
    fp.enforce_equal(&bits_to_fp(bits)?)
}

/// Ripple-carry adder over 128 bits: `left + right == parent` with `carry_in == 0` and
/// `carry_out == 0`. `carries[i]` is the carry into bit `i`; `carries[128]` is the final
/// carry. Carries are witnesses; passing hostile values must make the system
/// unsatisfiable.
pub(crate) fn enforce_u128_addition_no_overflow(
    cs: &ConstraintSystemRef<Fr>,
    left_bits: &[Boolean<Fr>],
    right_bits: &[Boolean<Fr>],
    parent_bits: &[Boolean<Fr>],
    carry_values: &[bool; U128_BITS + 1],
) -> Result<(), SynthesisError> {
    if left_bits.len() != U128_BITS
        || right_bits.len() != U128_BITS
        || parent_bits.len() != U128_BITS
    {
        return Err(SynthesisError::Unsatisfiable);
    }

    let mut carries = Vec::with_capacity(U128_BITS + 1);
    for carry_value in carry_values.iter() {
        carries.push(Boolean::<Fr>::new_witness(cs.clone(), || Ok(*carry_value))?);
    }

    // First carry (carry-in) must be zero.
    carries[0].enforce_equal(&Boolean::constant(false))?;

    for (((left_bit, right_bit), parent_bit), carry_pair) in left_bits
        .iter()
        .zip(right_bits.iter())
        .zip(parent_bits.iter())
        .zip(carries.windows(2))
    {
        let li = bits_to_fp(core::slice::from_ref(left_bit))?;
        let ri = bits_to_fp(core::slice::from_ref(right_bit))?;
        let ci = bits_to_fp(core::slice::from_ref(&carry_pair[0]))?;
        let pi = bits_to_fp(core::slice::from_ref(parent_bit))?;
        let co = bits_to_fp(core::slice::from_ref(&carry_pair[1]))?;

        (li + ri + ci).enforce_equal(&(pi + co.clone() + co))?;
    }

    // Final carry must be zero to guarantee no u128 overflow.
    carries[U128_BITS].enforce_equal(&Boolean::constant(false))
}

pub(crate) fn compute_carry_bits(left: u128, right: u128) -> [bool; U128_BITS + 1] {
    let mut carries = [false; U128_BITS + 1];
    let mut carry = false;
    for (i, carry_slot) in carries.iter_mut().take(U128_BITS).enumerate() {
        *carry_slot = carry;
        let a = ((left >> i) & 1) == 1;
        let b = ((right >> i) & 1) == 1;
        carry = (a & b) | (a & carry) | (b & carry);
    }
    carries[U128_BITS] = carry;
    carries
}

fn decimal_to_scaled_u128(value: Decimal) -> Result<u128, CircuitInputError> {
    decimal_to_sum_units(&value).map_err(|err| match err {
        AmountError::Negative(v) => CircuitInputError::NegativeBalance(v),
        AmountError::Overflow | AmountError::ExceedsMax { .. } => {
            CircuitInputError::BalanceConversionOverflow(value)
        }
        other => CircuitInputError::InvalidAmount(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poseidon::node_hash_v2;
    use ark_relations::r1cs::ConstraintSystem;
    use rust_decimal_macros::dec;

    fn satisfied(circuit: MerkleNodeRelationCircuit) -> bool {
        let cs = ConstraintSystem::new_ref();
        circuit
            .generate_constraints(cs.clone())
            .expect("constraint generation must succeed");
        cs.is_satisfied().expect("cs evaluation must succeed")
    }

    fn valid_circuit(left: u128, right: u128) -> MerkleNodeRelationCircuit {
        let left_hash = [11u8; 32];
        let right_hash = [22u8; 32];
        let parent_hash =
            node_hash_v2(&left_hash, &right_hash, left, right).expect("hash must succeed");
        MerkleNodeRelationCircuit::from_scaled(
            left_hash,
            right_hash,
            parent_hash,
            left,
            right,
            left + right,
        )
    }

    #[test]
    fn circuit_accepts_valid_merkle_parent_relation() {
        let left_hash = [11u8; 32];
        let right_hash = [22u8; 32];
        let parent_hash = node_hash_v2(&left_hash, &right_hash, 1_050_000_000, 225_000_000)
            .expect("hash must succeed");

        let circuit = MerkleNodeRelationCircuit::from_decimals(
            left_hash,
            right_hash,
            parent_hash,
            dec!(10.50000000),
            dec!(2.25000000),
            dec!(12.75000000),
        )
        .expect("circuit input must be valid");

        assert!(satisfied(circuit));
    }

    #[test]
    fn circuit_accepts_boundary_sums() {
        assert!(satisfied(valid_circuit(0, 0)));
        assert!(satisfied(valid_circuit(1, 0)));
        let max_sum = crate::amount::MAX_SUM_UNITS;
        assert!(satisfied(valid_circuit(max_sum / 2, max_sum - max_sum / 2)));
        // Carries rippling across many bits.
        assert!(satisfied(valid_circuit((1u128 << 71) - 1, 1)));
    }

    #[test]
    fn circuit_matches_native_tree_hash() {
        use crate::tree::{build_poseidon_merkle_sum_tree, BalanceSnapshot};
        let tree = build_poseidon_merkle_sum_tree(&[
            BalanceSnapshot {
                user_id: 1,
                balance: dec!(10),
            },
            BalanceSnapshot {
                user_id: 2,
                balance: dec!(5),
            },
        ])
        .expect("tree");
        let levels = tree.levels();
        let (l, r, p) = (&levels[0][0], &levels[0][1], &levels[1][0]);
        let circuit = MerkleNodeRelationCircuit::from_decimals(
            l.hash, r.hash, p.hash, l.balance, r.balance, p.balance,
        )
        .expect("input");
        assert!(satisfied(circuit));
    }

    #[test]
    fn circuit_rejects_invalid_parent_hash() {
        let circuit = MerkleNodeRelationCircuit::from_decimals(
            [11u8; 32],
            [22u8; 32],
            [9u8; 32],
            dec!(10.5),
            dec!(2.25),
            dec!(12.75),
        )
        .expect("circuit input must be valid");
        assert!(!satisfied(circuit));
    }

    #[test]
    fn circuit_rejects_wrong_sum() {
        let mut circuit = valid_circuit(5, 6);
        circuit.parent_balance_scaled = 12;
        assert!(!satisfied(circuit));
    }

    #[test]
    fn circuit_rejects_overflowing_u128_addition() {
        let circuit = MerkleNodeRelationCircuit::from_scaled(
            [1u8; 32],
            [2u8; 32],
            [3u8; 32],
            u128::MAX,
            1,
            0,
        );
        assert!(!satisfied(circuit));
    }

    #[test]
    fn circuit_rejects_out_of_range_sums() {
        // 2^100 + 0 = 2^100 is arithmetically fine, but outside the 72-bit range
        // (the native hasher refuses such sums, so the hash is arbitrary here; the
        // range gadget is isolated in `range_gadget_rejects_high_bits`).
        let out_of_range = |l: u128, r: u128| {
            MerkleNodeRelationCircuit::from_scaled([1u8; 32], [2u8; 32], [3u8; 32], l, r, l + r)
        };
        let big = 1u128 << 100;
        assert!(!satisfied(out_of_range(big, 0)));
        assert!(!satisfied(out_of_range(0, big)));
        assert!(!satisfied(out_of_range(1u128 << 72, 0)));
    }

    #[test]
    fn circuit_generation_fails_for_non_canonical_hash() {
        let mut circuit = valid_circuit(1, 2);
        circuit.left_hash = [0xff; 32];
        let cs = ConstraintSystem::new_ref();
        assert!(circuit.generate_constraints(cs).is_err());
    }

    #[test]
    fn from_decimals_rejects_inexact_or_negative() {
        let h = [1u8; 32];
        assert!(matches!(
            MerkleNodeRelationCircuit::from_decimals(h, h, h, dec!(-1), dec!(0), dec!(0)),
            Err(CircuitInputError::NegativeBalance(_))
        ));
        assert!(matches!(
            MerkleNodeRelationCircuit::from_decimals(h, h, h, dec!(0.000000001), dec!(0), dec!(0)),
            Err(CircuitInputError::InvalidAmount(
                AmountError::OverPrecision(_)
            ))
        ));
    }

    // ---- Gadget-level adversarial tests (hostile witnesses) ----

    fn alloc_bits(cs: &ConstraintSystemRef<Fr>, value: u128) -> Vec<Boolean<Fr>> {
        alloc_u128_bits(cs, value).expect("allocation must succeed")
    }

    fn adder_satisfied(left: u128, right: u128, parent: u128, carries: &[bool; 129]) -> bool {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let l = alloc_bits(&cs, left);
        let r = alloc_bits(&cs, right);
        let p = alloc_bits(&cs, parent);
        enforce_u128_addition_no_overflow(&cs, &l, &r, &p, carries)
            .expect("synthesis must succeed");
        cs.is_satisfied().expect("cs evaluation must succeed")
    }

    #[test]
    fn adder_accepts_honest_carries() {
        assert!(adder_satisfied(1, 1, 2, &compute_carry_bits(1, 1)));
        assert!(adder_satisfied(
            u128::MAX >> 1,
            1,
            (u128::MAX >> 1) + 1,
            &compute_carry_bits(u128::MAX >> 1, 1)
        ));
    }

    #[test]
    fn adder_rejects_carry_in_one() {
        // 1 + 1 + carry_in(1) = 3 satisfies every per-bit equation with carries [1,1,0,...],
        // so only the first-carry constraint rejects it.
        let mut carries = [false; 129];
        carries[0] = true;
        carries[1] = true;
        assert!(!adder_satisfied(1, 1, 3, &carries));
    }

    #[test]
    fn adder_rejects_carry_out_one() {
        // u128::MAX + 1 wraps to 0 with a final carry of 1.
        let carries = compute_carry_bits(u128::MAX, 1);
        assert!(carries[128]);
        assert!(!adder_satisfied(u128::MAX, 1, 0, &carries));
    }

    #[test]
    fn adder_rejects_inconsistent_carries_and_sum() {
        let mut carries = compute_carry_bits(3, 1);
        carries[1] = !carries[1];
        assert!(!adder_satisfied(3, 1, 4, &carries));
        assert!(!adder_satisfied(3, 1, 5, &compute_carry_bits(3, 1)));
    }

    #[test]
    fn range_gadget_rejects_high_bits() {
        let check = |value: u128, width: usize| {
            let cs = ConstraintSystem::<Fr>::new_ref();
            let bits = alloc_bits(&cs, value);
            enforce_bits_fit(&bits, width).expect("synthesis must succeed");
            cs.is_satisfied().expect("cs evaluation must succeed")
        };
        assert!(check((1u128 << 72) - 1, 72));
        assert!(!check(1u128 << 72, 72));
        assert!(!check(u128::MAX, 72));
        assert!(check(0, 0));
    }

    #[test]
    fn field_variable_must_match_bits() {
        let check = |bits_value: u128, field_value: u128| {
            let cs = ConstraintSystem::<Fr>::new_ref();
            let bits = alloc_bits(&cs, bits_value);
            let fp = FpVar::<Fr>::new_witness(ns!(cs, "fp"), || Ok(Fr::from(field_value)))
                .expect("allocation must succeed");
            enforce_fp_matches_bits(&fp, &bits).expect("synthesis must succeed");
            cs.is_satisfied().expect("cs evaluation must succeed")
        };
        assert!(check(5, 5));
        assert!(!check(5, 6));
        assert!(!check(0, 1));
        assert!(!check(u128::MAX, 0));
    }

    #[test]
    fn bits_to_fp_rejects_too_wide_input() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let bits: Vec<Boolean<Fr>> = (0..253)
            .map(|_| Boolean::new_witness(cs.clone(), || Ok(false)).expect("alloc"))
            .collect();
        assert!(bits_to_fp(&bits).is_err());
    }
}
