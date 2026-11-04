#![cfg(not(target_arch = "wasm32"))]

use ark_bn254::{Bn254, Fr};
use ark_groth16::{
    create_random_proof, generate_random_parameters, prepare_verifying_key, verify_proof,
    ProvingKey,
};
use ark_r1cs_std::{
    fields::fp::FpVar,
    prelude::{AllocVar, EqGadget},
};
use ark_relations::{
    ns,
    r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError},
};
use ark_sponge::{constraints::CryptographicSpongeVar, poseidon::constraints::PoseidonSpongeVar};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand::{rngs::StdRng, SeedableRng};
use rust_decimal::Decimal;
use std::sync::OnceLock;

use crate::amount::{decimal_to_units, AmountError};
use crate::poseidon::poseidon_parameters;

// LEGACY: this membership circuit still commits `P(user_id, balance)` (format 1 leaf,
// no domain tag/context) and uses the non-trusted seed-42 Groth16 setup. It is NOT bound
// to format-2 tree roots.

#[derive(Debug, Clone)]
pub struct MembershipProofInput {
    pub user_id: u64,
    pub leaf_balance: Decimal,
}

#[derive(Debug, Clone)]
pub struct SnarkProofPackage {
    pub scheme: String,
    pub proof_b64: String,
    pub public_inputs_b64: String,
    pub verified: bool,
}

#[derive(Debug)]
pub enum SnarkError {
    NegativeBalance,
    DecimalOverflow,
    Groth16(String),
    Serialization(String),
}

impl core::fmt::Display for SnarkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NegativeBalance => write!(f, "balance must be non-negative"),
            Self::DecimalOverflow => write!(f, "balance conversion overflow"),
            Self::Groth16(e) => write!(f, "groth16 error: {e}"),
            Self::Serialization(e) => write!(f, "serialization error: {e}"),
        }
    }
}

impl std::error::Error for SnarkError {}

#[derive(Clone)]
struct MembershipCircuit {
    leaf_commitment: Fr,
    expected_user_id: Fr,
    user_id: Fr,
    leaf_balance: Fr,
}

impl ConstraintSynthesizer<Fr> for MembershipCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let leaf_commitment_public =
            FpVar::<Fr>::new_input(ns!(cs, "leaf_commitment_public"), || {
                Ok(self.leaf_commitment)
            })?;
        let expected_user_id_public =
            FpVar::<Fr>::new_input(ns!(cs, "expected_user_id_public"), || {
                Ok(self.expected_user_id)
            })?;

        let user_id_witness =
            FpVar::<Fr>::new_witness(ns!(cs, "user_id_witness"), || Ok(self.user_id))?;
        let leaf_balance_witness =
            FpVar::<Fr>::new_witness(ns!(cs, "leaf_balance_witness"), || Ok(self.leaf_balance))?;

        user_id_witness.enforce_equal(&expected_user_id_public)?;

        let mut leaf_sponge = PoseidonSpongeVar::<Fr>::new(cs, poseidon_parameters());
        leaf_sponge.absorb(&user_id_witness)?;
        leaf_sponge.absorb(&leaf_balance_witness)?;

        let computed_leaf_commitment = leaf_sponge.squeeze_field_elements(1)?[0].clone();
        computed_leaf_commitment.enforce_equal(&leaf_commitment_public)?;

        Ok(())
    }
}

pub fn create_membership_snark(
    input: MembershipProofInput,
) -> Result<SnarkProofPackage, SnarkError> {
    let user_id_fr = Fr::from(input.user_id);
    let leaf_balance_fr = decimal_to_field(&input.leaf_balance)?;
    let leaf_commitment_fr = compute_leaf_commitment(user_id_fr, leaf_balance_fr);

    let circuit = MembershipCircuit {
        leaf_commitment: leaf_commitment_fr,
        expected_user_id: user_id_fr,
        user_id: user_id_fr,
        leaf_balance: leaf_balance_fr,
    };

    let params = membership_parameters();

    let mut rng = rand::thread_rng();
    let proof = create_random_proof(circuit, params, &mut rng)
        .map_err(|e| SnarkError::Groth16(e.to_string()))?;

    let pvk = prepare_verifying_key(&params.vk);
    let verified = verify_proof(&pvk, &proof, &[leaf_commitment_fr, user_id_fr])
        .map_err(|e| SnarkError::Groth16(e.to_string()))?;

    let mut proof_bytes = Vec::new();
    ark_serialize::CanonicalSerialize::serialize(&proof, &mut proof_bytes)
        .map_err(|e| SnarkError::Serialization(e.to_string()))?;

    let mut public_inputs_bytes = Vec::new();
    ark_serialize::CanonicalSerialize::serialize(&leaf_commitment_fr, &mut public_inputs_bytes)
        .map_err(|e| SnarkError::Serialization(e.to_string()))?;
    ark_serialize::CanonicalSerialize::serialize(&user_id_fr, &mut public_inputs_bytes)
        .map_err(|e| SnarkError::Serialization(e.to_string()))?;

    Ok(SnarkProofPackage {
        scheme: "groth16-bn254".to_string(),
        proof_b64: STANDARD.encode(proof_bytes),
        public_inputs_b64: STANDARD.encode(public_inputs_bytes),
        verified,
    })
}

fn membership_parameters() -> &'static ProvingKey<Bn254> {
    static PARAMS: OnceLock<ProvingKey<Bn254>> = OnceLock::new();

    PARAMS.get_or_init(|| {
        // Fixed-shape circuit: setup once and reuse for all requests.
        let setup_circuit = MembershipCircuit {
            leaf_commitment: Fr::from(0u64),
            expected_user_id: Fr::from(0u64),
            user_id: Fr::from(0u64),
            leaf_balance: Fr::from(0u64),
        };

        let mut rng = StdRng::seed_from_u64(42);
        generate_random_parameters::<Bn254, _, _>(setup_circuit, &mut rng)
            .unwrap_or_else(|e| panic!("failed to initialize Groth16 parameters: {e}"))
    })
}

/// Exact conversion (no silent rescale/rounding); over-precision maps to `DecimalOverflow`.
fn decimal_to_field(value: &Decimal) -> Result<Fr, SnarkError> {
    match decimal_to_units(value) {
        Ok(units) => Ok(Fr::from(units)),
        Err(AmountError::Negative(_)) => Err(SnarkError::NegativeBalance),
        Err(_) => Err(SnarkError::DecimalOverflow),
    }
}

fn compute_leaf_commitment(user_id: Fr, leaf_balance: Fr) -> Fr {
    use ark_sponge::poseidon::PoseidonSponge;
    use ark_sponge::CryptographicSponge;

    let mut sponge = PoseidonSponge::<Fr>::new(poseidon_parameters());
    sponge.absorb(&user_id);
    sponge.absorb(&leaf_balance);
    sponge.squeeze_field_elements(1)[0]
}
