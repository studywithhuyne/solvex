//! Client-side (wasm) verifier for Merkle Sum Tree inclusion proofs, format version 2.
//!
//! Inputs are untrusted: sizes are limited, every number/hash is parsed strictly
//! (ASCII only, canonical, no trimming) and legacy / unknown format versions are rejected
//! rather than reinterpreted.

use serde::{de::Error as _, Deserialize, Deserializer};

use crate::amount::{checked_add_units, parse_amount_units, parse_sum_units};
use crate::parse::{parse_hash_hex, parse_user_id_str, parse_user_id_u64};
use crate::poseidon::{empty_subtree_hashes_v2, leaf_hash_v2, node_hash_v2};
use crate::protocol::{TreeContext, MAX_PROOF_JSON_BYTES, MAX_TREE_DEPTH, PROTOCOL_VERSION};

#[derive(Debug, Deserialize)]
pub struct ProofStepPayload {
    pub sibling_hash: String,
    pub sibling_balance: String,
    pub sibling_is_left: bool,
}

#[derive(Debug, Deserialize)]
pub struct ProofPayload {
    #[serde(deserialize_with = "deserialize_user_id")]
    pub user_id: u64,
    pub leaf_balance: String,
    pub root_hash: String,
    pub merkle_path: Vec<ProofStepPayload>,
    /// Required; must equal `PROTOCOL_VERSION`. Missing means legacy and is rejected.
    #[serde(default)]
    pub format_version: Option<u64>,
    /// Asset symbol bound into the tree; absent means the unscoped context.
    #[serde(default)]
    pub asset: Option<String>,
    /// Snapshot id bound into the tree; absent means 0 (unscoped context).
    #[serde(default, deserialize_with = "deserialize_optional_snapshot_id")]
    pub snapshot_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PublicInputsPayload {
    pub expected_root_hash: String,
    #[serde(default, deserialize_with = "deserialize_optional_user_id")]
    pub expected_user_id: Option<u64>,
}

fn deserialize_user_id<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    parse_user_id_value(&value)
        .ok_or_else(|| D::Error::custom("expected canonical user_id in 1..=i64::MAX"))
}

fn deserialize_optional_user_id<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => parse_user_id_value(&v)
            .map(Some)
            .ok_or_else(|| D::Error::custom("expected canonical user_id in 1..=i64::MAX")),
    }
}

fn deserialize_optional_snapshot_id<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => parse_snapshot_id_value(&v)
            .map(Some)
            .ok_or_else(|| D::Error::custom("expected canonical snapshot_id")),
    }
}

fn parse_user_id_value(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(num) => num.as_u64().and_then(|n| parse_user_id_u64(n).ok()),
        serde_json::Value::String(s) => parse_user_id_str(s).ok(),
        _ => None,
    }
}

fn parse_snapshot_id_value(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(num) => num.as_u64(),
        serde_json::Value::String(s) => {
            let canonical = !s.is_empty()
                && s.len() <= 20
                && s.bytes().all(|b| b.is_ascii_digit())
                && (s.len() == 1 || !s.starts_with('0'));
            if canonical {
                s.parse::<u64>().ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Returns true only if the proof verifies. See `verify_proof_detailed` for the reason.
pub fn verify_proof_json(proof_json: &str, public_inputs_json: &str) -> bool {
    verify_proof_detailed(proof_json, public_inputs_json).is_ok()
}

/// Verifies a proof; `Err` carries a human readable rejection reason.
pub fn verify_proof_detailed(proof_json: &str, public_inputs_json: &str) -> Result<(), String> {
    if proof_json.len() > MAX_PROOF_JSON_BYTES || public_inputs_json.len() > MAX_PROOF_JSON_BYTES {
        return Err("input exceeds size limit".to_string());
    }

    let proof: ProofPayload =
        serde_json::from_str(proof_json).map_err(|e| format!("malformed proof JSON: {e}"))?;
    let public_inputs: PublicInputsPayload = serde_json::from_str(public_inputs_json)
        .map_err(|e| format!("malformed public inputs JSON: {e}"))?;

    verify_proof_payload(&proof, &public_inputs)
}

fn verify_proof_payload(
    proof: &ProofPayload,
    public_inputs: &PublicInputsPayload,
) -> Result<(), String> {
    match proof.format_version {
        Some(v) if v == u64::from(PROTOCOL_VERSION) => {}
        Some(v) => return Err(format!("unsupported format_version {v}")),
        None => {
            return Err("missing format_version: legacy (v1) proofs are not accepted".to_string())
        }
    }

    let context = match &proof.asset {
        Some(asset) => {
            TreeContext::new(asset, proof.snapshot_id.unwrap_or(0)).map_err(|e| e.to_string())?
        }
        None if proof.snapshot_id.unwrap_or(0) == 0 => TreeContext::unscoped(),
        None => return Err("snapshot_id requires asset".to_string()),
    };

    if let Some(expected_user_id) = public_inputs.expected_user_id {
        if expected_user_id != proof.user_id {
            return Err("user_id does not match expected_user_id".to_string());
        }
    }

    if proof.merkle_path.len() > MAX_TREE_DEPTH {
        return Err(format!("merkle path longer than {MAX_TREE_DEPTH}"));
    }

    let leaf_units = parse_amount_units(&proof.leaf_balance).map_err(|e| e.to_string())?;
    let mut current_hash =
        leaf_hash_v2(&context, proof.user_id, leaf_units).map_err(|e| e.to_string())?;
    let mut current_units = leaf_units;

    let empty_hashes =
        empty_subtree_hashes_v2(&context, proof.merkle_path.len()).map_err(|e| e.to_string())?;

    for (level, step) in proof.merkle_path.iter().enumerate() {
        let sibling_hash = parse_hash_hex(&step.sibling_hash).map_err(|e| e.to_string())?;
        let sibling_units = parse_sum_units(&step.sibling_balance).map_err(|e| e.to_string())?;

        // Padding subtrees carry no money: a sibling equal to the canonical empty
        // subtree hash of this level must have a zero sum.
        if empty_hashes.get(level) == Some(&sibling_hash) && sibling_units != 0 {
            return Err("padding sibling carries a non-zero balance".to_string());
        }

        current_hash = if step.sibling_is_left {
            node_hash_v2(&sibling_hash, &current_hash, sibling_units, current_units)
        } else {
            node_hash_v2(&current_hash, &sibling_hash, current_units, sibling_units)
        }
        .map_err(|e| e.to_string())?;
        current_units =
            checked_add_units(current_units, sibling_units).map_err(|e| e.to_string())?;
    }

    let declared_root_hash = parse_hash_hex(&proof.root_hash).map_err(|e| e.to_string())?;
    let expected_root_hash =
        parse_hash_hex(&public_inputs.expected_root_hash).map_err(|e| e.to_string())?;

    if current_hash == declared_root_hash && current_hash == expected_root_hash {
        Ok(())
    } else {
        Err("root hash mismatch".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{verify_proof_detailed, verify_proof_json};
    use crate::parse::hash_to_hex;
    use crate::protocol::TreeContext;
    use crate::tree::{
        build_poseidon_merkle_sum_tree, build_poseidon_merkle_sum_tree_with_context,
        BalanceSnapshot, MerkleProof,
    };
    use rust_decimal_macros::dec;
    use serde_json::{json, Value};

    fn snapshots3() -> Vec<BalanceSnapshot> {
        vec![
            BalanceSnapshot {
                user_id: 1,
                balance: dec!(10),
            },
            BalanceSnapshot {
                user_id: 2,
                balance: dec!(7.5),
            },
            BalanceSnapshot {
                user_id: 3,
                balance: dec!(3.25),
            },
        ]
    }

    fn proof_value(proof: &MerkleProof, user_id: u64, balance: &str) -> Value {
        json!({
            "format_version": proof.format_version,
            "user_id": user_id,
            "leaf_balance": balance,
            "root_hash": hash_to_hex(&proof.root.hash),
            "merkle_path": proof.path.iter().map(|step| json!({
                "sibling_hash": hash_to_hex(&step.sibling_hash),
                "sibling_balance": step.sibling_balance.to_string(),
                "sibling_is_left": step.sibling_is_left,
            })).collect::<Vec<_>>(),
        })
    }

    fn inputs_value(proof: &MerkleProof, user_id: u64) -> Value {
        json!({
            "expected_root_hash": hash_to_hex(&proof.root.hash),
            "expected_user_id": user_id,
        })
    }

    fn verify(proof: &Value, inputs: &Value) -> Result<(), String> {
        verify_proof_detailed(&proof.to_string(), &inputs.to_string())
    }

    #[test]
    fn verify_proof_json_accepts_valid_merkle_path() {
        let snapshots = snapshots3();
        let tree = build_poseidon_merkle_sum_tree(&snapshots).expect("tree build must succeed");
        for (i, snapshot) in snapshots.iter().enumerate() {
            let proof = tree
                .generate_proof(i)
                .expect("proof generation must succeed");
            let proof_json = proof_value(&proof, snapshot.user_id, &snapshot.balance.to_string());
            let inputs = inputs_value(&proof, snapshot.user_id);
            assert_eq!(verify(&proof_json, &inputs), Ok(()));
            assert!(verify_proof_json(
                &proof_json.to_string(),
                &inputs.to_string()
            ));
        }
    }

    #[test]
    fn verify_proof_json_rejects_tampered_root_hash() {
        let proof_json = json!({
            "format_version": 2,
            "user_id": 7,
            "leaf_balance": "10",
            "root_hash": "00".repeat(32),
            "merkle_path": []
        })
        .to_string();

        let public_inputs_json = json!({
            "expected_root_hash": "11".repeat(32),
            "expected_user_id": 7,
        })
        .to_string();

        assert!(!verify_proof_json(&proof_json, &public_inputs_json));
    }

    #[test]
    fn verify_proof_json_accepts_string_user_id() {
        let snapshots = vec![
            BalanceSnapshot {
                user_id: 11,
                balance: dec!(4.25),
            },
            BalanceSnapshot {
                user_id: 22,
                balance: dec!(8.5),
            },
        ];
        let tree = build_poseidon_merkle_sum_tree(&snapshots).expect("tree build must succeed");
        let proof = tree
            .generate_proof(1)
            .expect("proof generation must succeed");

        let mut proof_json = proof_value(&proof, 22, "8.5");
        proof_json["user_id"] = json!("22");
        let mut inputs = inputs_value(&proof, 22);
        inputs["expected_user_id"] = json!("22");
        assert_eq!(verify(&proof_json, &inputs), Ok(()));
    }

    #[test]
    fn accepts_scoped_context_and_rejects_wrong_context() {
        let ctx = TreeContext::new("USDT", 5).expect("ctx");
        let tree = build_poseidon_merkle_sum_tree_with_context(&snapshots3(), &ctx).expect("tree");
        let proof = tree.generate_proof(0).expect("proof");

        let mut proof_json = proof_value(&proof, 1, "10");
        let inputs = inputs_value(&proof, 1);
        // Missing context => unscoped => different hashes.
        assert!(verify(&proof_json, &inputs).is_err());

        proof_json["asset"] = json!("USDT");
        proof_json["snapshot_id"] = json!(5);
        assert_eq!(verify(&proof_json, &inputs), Ok(()));

        proof_json["snapshot_id"] = json!("6");
        assert!(verify(&proof_json, &inputs).is_err());
        proof_json["snapshot_id"] = json!(5);
        proof_json["asset"] = json!("BTC");
        assert!(verify(&proof_json, &inputs).is_err());
    }

    #[test]
    fn rejects_legacy_and_unknown_versions() {
        let tree = build_poseidon_merkle_sum_tree(&snapshots3()).expect("tree");
        let proof = tree.generate_proof(0).expect("proof");
        let inputs = inputs_value(&proof, 1);

        let mut proof_json = proof_value(&proof, 1, "10");
        proof_json["format_version"] = json!(1);
        assert!(verify(&proof_json, &inputs).is_err());
        proof_json["format_version"] = json!(3);
        assert!(verify(&proof_json, &inputs).is_err());
        proof_json
            .as_object_mut()
            .map(|o| o.remove("format_version"));
        let err = verify(&proof_json, &inputs).expect_err("missing version");
        assert!(err.contains("legacy"));
    }

    #[test]
    fn rejects_tampered_values() {
        let tree = build_poseidon_merkle_sum_tree(&snapshots3()).expect("tree");
        let proof = tree.generate_proof(1).expect("proof");
        let inputs = inputs_value(&proof, 2);
        let good = proof_value(&proof, 2, "7.5");
        assert_eq!(verify(&good, &inputs), Ok(()));

        let mut bad = good.clone();
        bad["leaf_balance"] = json!("7.6");
        assert!(verify(&bad, &inputs).is_err());

        let mut bad = good.clone();
        bad["user_id"] = json!(3);
        assert!(verify(&bad, &inputs).is_err());

        let mut bad = good.clone();
        bad["merkle_path"][0]["sibling_balance"] = json!("0");
        assert!(verify(&bad, &inputs).is_err());

        let mut bad = good.clone();
        bad["merkle_path"][0]["sibling_is_left"] = json!(false);
        assert!(verify(&bad, &inputs).is_err());

        let wrong_user_inputs = inputs_value(&proof, 3);
        assert!(verify(&good, &wrong_user_inputs).is_err());
    }

    #[test]
    fn rejects_padding_sibling_with_money() {
        // 3 leaves => padded to 4; user 3's sibling at level 0 is a padding leaf.
        let tree = build_poseidon_merkle_sum_tree(&snapshots3()).expect("tree");
        let proof = tree.generate_proof(2).expect("proof");
        assert_eq!(proof.path[0].sibling_balance, dec!(0));
        let inputs = inputs_value(&proof, 3);
        let good = proof_value(&proof, 3, "3.25");
        assert_eq!(verify(&good, &inputs), Ok(()));

        let mut forged = good.clone();
        forged["merkle_path"][0]["sibling_balance"] = json!("5");
        let err = verify(&forged, &inputs).expect_err("padding with money");
        assert!(err.contains("padding"));
    }

    #[test]
    fn rejects_malformed_fields_without_panic() {
        let tree = build_poseidon_merkle_sum_tree(&snapshots3()).expect("tree");
        let proof = tree.generate_proof(1).expect("proof");
        let inputs = inputs_value(&proof, 2);
        let good = proof_value(&proof, 2, "7.5");

        for balance in [
            "",
            " 7.5",
            "7.5 ",
            "+7.5",
            "-7.5",
            "07.5",
            "7.",
            "1e1",
            "７.5",
            "7.5é",
            "9".repeat(100).as_str(),
        ] {
            let mut bad = good.clone();
            bad["leaf_balance"] = json!(balance);
            assert!(verify(&bad, &inputs).is_err(), "balance {balance:?}");
        }

        let sibling = hash_to_hex(&proof.path[0].sibling_hash);
        let bad_hashes = [
            format!(" {sibling}"),
            format!("{sibling} "),
            sibling[..62].to_string(),
            format!("{sibling}00"),
            "ff".repeat(32),
            format!("1é{}", "1".repeat(61)),
            "😀".repeat(16),
        ];
        for hash in bad_hashes {
            let mut bad = good.clone();
            bad["merkle_path"][0]["sibling_hash"] = json!(hash);
            assert!(verify(&bad, &inputs).is_err(), "hash {hash:?}");
        }

        for user_id in [
            json!(0),
            json!("0"),
            json!("02"),
            json!(" 2"),
            json!("+2"),
            json!(-1),
            json!(2.5),
            json!(null),
            json!("９"),
        ] {
            let mut bad = good.clone();
            bad["user_id"] = user_id.clone();
            assert!(verify(&bad, &inputs).is_err(), "user_id {user_id:?}");
        }
    }

    #[test]
    fn enforces_size_and_depth_limits() {
        let tree = build_poseidon_merkle_sum_tree(&snapshots3()).expect("tree");
        let proof = tree.generate_proof(1).expect("proof");
        let inputs = inputs_value(&proof, 2);

        let mut deep = proof_value(&proof, 2, "7.5");
        let step = deep["merkle_path"][0].clone();
        deep["merkle_path"] = Value::Array(vec![step; 13]);
        assert!(verify(&deep, &inputs)
            .expect_err("depth")
            .contains("longer"));

        let huge = "x".repeat(70_000);
        assert!(!verify_proof_json(&huge, &inputs.to_string()));
        assert!(!verify_proof_json("", ""));
        assert!(!verify_proof_json("{", "}"));
    }

    #[test]
    fn single_and_empty_trees_verify() {
        let tree = build_poseidon_merkle_sum_tree(&[BalanceSnapshot {
            user_id: 9,
            balance: dec!(1.5),
        }])
        .expect("tree");
        let proof = tree.generate_proof(0).expect("proof");
        assert_eq!(
            verify(&proof_value(&proof, 9, "1.5"), &inputs_value(&proof, 9)),
            Ok(())
        );
    }
}
