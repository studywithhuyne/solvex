use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::api::{auth::UserId, state::AppState};

use zkp::snark::{create_membership_snark, MembershipProofInput, SnarkProofPackage};
use zkp::tree::{build_poseidon_merkle_sum_tree, BalanceSnapshot};

#[derive(Debug, Clone)]
struct CachedSnarkEntry {
    leaf_balance: String,
    root_hash: String,
    package: SnarkProofPackage,
}

type SnarkCacheMap = HashMap<(u64, String), CachedSnarkEntry>;

fn snark_cache() -> &'static RwLock<SnarkCacheMap> {
    static CACHE: OnceLock<RwLock<SnarkCacheMap>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

#[derive(Debug, Deserialize)]
pub struct ZkpProofQuery {
    pub asset: Option<String>,
    pub cold_wallet_assets: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ZkpProofResponse {
    pub protocol_version: u32,
    pub statement: &'static str,
    pub verification_status: &'static str,
    pub user_id: String,
    pub asset: String,
    pub snapshot_size: usize,
    pub leaf_index: usize,
    pub leaf_balance: String,
    pub root_hash: String,
    pub public_inputs: ZkpPublicInputsDto,
    pub snark: ZkpSnarkDto,
    pub solvency: Option<ZkpSolvencyDto>,
}

#[derive(Debug, Serialize)]
pub struct ZkpPublicInputsDto {
    pub expected_root_hash: String,
    pub expected_user_id: String,
}

#[derive(Debug, Serialize)]
pub struct ZkpSnarkDto {
    pub scheme: String,
    pub proof: String,
    pub public_inputs: String,
    pub verified: bool,
}

#[derive(Debug, Serialize)]
pub struct ZkpSolvencyDto {
    pub liabilities_leq_assets: bool,
    pub total_liabilities: String,
    pub compared_assets: String,
    pub assets_source: String,
    pub computed_at: String,
}

#[derive(Debug, sqlx::FromRow)]
struct SolvencyRow {
    user_id: i64,
    balance: Decimal,
}

pub async fn proof_handler(
    State(state): State<AppState>,
    UserId(user_id): UserId,
    Query(query): Query<ZkpProofQuery>,
) -> Result<Json<ZkpProofResponse>, (StatusCode, Json<serde_json::Value>)> {
    let asset = query
        .asset
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("USDT")
        .to_ascii_uppercase();

    let rows: Vec<SolvencyRow> = sqlx::query_as(
        "SELECT user_id, (available + locked) AS balance
         FROM balances
         WHERE asset_symbol = $1
         ORDER BY user_id ASC",
    )
    .bind(&asset)
    .fetch_all(&state.db)
    .await
    .map_err(internal_error)?;

    if rows.is_empty() {
        return Err(not_found("no balances found for selected asset"));
    }

    let mut snapshots = Vec::with_capacity(rows.len());
    let mut user_leaf_index: Option<usize> = None;

    for (index, row) in rows.iter().enumerate() {
        if row.user_id <= 0 {
            return Err(internal_error_msg("invalid user_id in balances snapshot"));
        }

        let snapshot_user_id = row.user_id as u64;
        if snapshot_user_id == user_id {
            user_leaf_index = Some(index);
        }

        snapshots.push(BalanceSnapshot {
            user_id: snapshot_user_id,
            balance: row.balance,
        });
    }

    let user_leaf_index =
        user_leaf_index.ok_or_else(|| not_found("user balance not found for selected asset"))?;

    let tree = build_poseidon_merkle_sum_tree(&snapshots)
        .map_err(|e| internal_error_msg(&format!("failed to build solvency tree: {e}")))?;

    let proof = tree
        .generate_proof(user_leaf_index)
        .map_err(|e| internal_error_msg(&format!("failed to generate merkle path: {e}")))?;

    let leaf_balance_text = proof.leaf.balance.to_string();
    let root_hash_text = hash_to_hex(&proof.root.hash);

    let snark_package = get_or_create_snark_package(
        user_id,
        &asset,
        &leaf_balance_text,
        &root_hash_text,
        proof.leaf.balance,
    )?;

    let solvency =
        resolve_effective_assets_optional(&state, &asset, query.cold_wallet_assets.as_deref())?
            .map(|resolved| ZkpSolvencyDto {
                liabilities_leq_assets: proof.root.balance <= resolved.assets,
                total_liabilities: proof.root.balance.to_string(),
                compared_assets: resolved.assets.to_string(),
                assets_source: resolved.source,
                computed_at: chrono::Utc::now().to_rfc3339(),
            });

    Ok(Json(ZkpProofResponse {
        protocol_version: 1,
        statement: "leaf_commitment",
        verification_status: "unverified",
        user_id: user_id.to_string(),
        asset,
        snapshot_size: tree.original_leaf_count(),
        leaf_index: proof.leaf_index,
        leaf_balance: leaf_balance_text,
        root_hash: root_hash_text.clone(),
        public_inputs: ZkpPublicInputsDto {
            expected_root_hash: root_hash_text,
            expected_user_id: user_id.to_string(),
        },
        snark: ZkpSnarkDto {
            scheme: snark_package.scheme,
            proof: snark_package.proof_b64,
            public_inputs: snark_package.public_inputs_b64,
            verified: snark_package.verified,
        },
        solvency,
    }))
}

fn get_or_create_snark_package(
    user_id: u64,
    asset: &str,
    leaf_balance: &str,
    root_hash: &str,
    leaf_balance_decimal: Decimal,
) -> Result<SnarkProofPackage, (StatusCode, Json<serde_json::Value>)> {
    let key = (user_id, asset.to_string());

    if let Some(entry) = snark_cache().read().get(&key) {
        if entry.leaf_balance == leaf_balance && entry.root_hash == root_hash {
            return Ok(entry.package.clone());
        }
    }

    let package = create_membership_snark(MembershipProofInput {
        user_id,
        leaf_balance: leaf_balance_decimal,
    })
    .map_err(|e| internal_error_msg(&format!("failed to create zk-SNARK proof: {e}")))?;

    snark_cache().write().insert(
        key,
        CachedSnarkEntry {
            leaf_balance: leaf_balance.to_string(),
            root_hash: root_hash.to_string(),
            package: package.clone(),
        },
    );

    Ok(package)
}

struct ResolvedAssets {
    assets: Decimal,
    source: String,
}

fn resolve_effective_assets_optional(
    state: &AppState,
    asset: &str,
    query_value: Option<&str>,
) -> Result<Option<ResolvedAssets>, (StatusCode, Json<serde_json::Value>)> {
    if let Some(raw) = query_value.map(str::trim).filter(|s| !s.is_empty()) {
        return parse_decimal(raw, "invalid query param cold_wallet_assets").map(|value| {
            Some(ResolvedAssets {
                assets: value,
                source: "query:cold_wallet_assets".to_string(),
            })
        });
    }

    let env_key = format!("COLD_WALLET_ASSETS_{}", asset);
    match std::env::var(&env_key) {
        Ok(raw) => parse_decimal(raw.trim(), &format!("invalid env {env_key}")).map(|value| {
            Some(ResolvedAssets {
                assets: value,
                source: format!("env:{env_key}"),
            })
        }),
        Err(_) => {
            if asset == "USDT" {
                let base_capital = state.exchange_funds.lock().base_capital_usdt;
                let exchange_revenue = state.ledger.lock().exchange_revenue_by_asset("USDT");
                return Ok(Some(ResolvedAssets {
                    assets: base_capital + exchange_revenue,
                    source: "live:treasury_base_capital_plus_revenue".to_string(),
                }));
            }
            Ok(None)
        }
    }
}

fn resolve_effective_assets(
    state: &AppState,
    asset: &str,
    query_value: Option<&str>,
) -> Result<ResolvedAssets, (StatusCode, Json<serde_json::Value>)> {
    if let Some(resolved) = resolve_effective_assets_optional(state, asset, query_value)? {
        return Ok(resolved);
    }

    let env_key = format!("COLD_WALLET_ASSETS_{}", asset);
    Err((
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": format!(
                "missing solvency assets value for {asset}; pass ?cold_wallet_assets=... or set env {}",
                env_key
            )
        })),
    ))
}

fn parse_decimal(
    value: &str,
    err_prefix: &str,
) -> Result<Decimal, (StatusCode, Json<serde_json::Value>)> {
    let decimal = value.parse::<Decimal>().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{err_prefix}: {value}") })),
        )
    })?;

    if decimal.is_sign_negative() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("{err_prefix}: value must be non-negative") }),
            ),
        ));
    }

    Ok(decimal)
}

// ─────────────────────────────────────────────────────────────────────────────
// Exchange-facing solvency check (no user-specific proof needed)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ZkpSolvencyResponse {
    pub protocol_version: u32,
    pub statement: &'static str,
    pub evidence_status: &'static str,
    pub asset: String,
    pub snapshot_size: usize,
    pub root_hash: String,
    pub total_liabilities: String,
    pub compared_assets: String,
    pub assets_source: String,
    pub liabilities_leq_assets: bool,
    pub computed_at: String,
}

pub async fn solvency_handler(
    State(state): State<AppState>,
    Query(query): Query<ZkpProofQuery>,
) -> Result<Json<ZkpSolvencyResponse>, (StatusCode, Json<serde_json::Value>)> {
    let asset = query
        .asset
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("USDT")
        .to_ascii_uppercase();

    let rows: Vec<SolvencyRow> = sqlx::query_as(
        "SELECT user_id, (available + locked) AS balance
         FROM balances
         WHERE asset_symbol = $1
         ORDER BY user_id ASC",
    )
    .bind(&asset)
    .fetch_all(&state.db)
    .await
    .map_err(internal_error)?;

    if rows.is_empty() {
        return Err(not_found("no balances found for selected asset"));
    }

    let mut snapshots = Vec::with_capacity(rows.len());
    for row in &rows {
        if row.user_id <= 0 {
            return Err(internal_error_msg("invalid user_id in balances snapshot"));
        }
        snapshots.push(BalanceSnapshot {
            user_id: row.user_id as u64,
            balance: row.balance,
        });
    }

    let tree = build_poseidon_merkle_sum_tree(&snapshots)
        .map_err(|e| internal_error_msg(&format!("failed to build solvency tree: {e}")))?;

    let root = tree.root();
    let resolved_assets =
        resolve_effective_assets(&state, &asset, query.cold_wallet_assets.as_deref())?;
    let total_liabilities = root.balance;
    let liabilities_leq_assets = total_liabilities <= resolved_assets.assets;

    let verified_at = chrono::Utc::now().to_rfc3339();

    Ok(Json(ZkpSolvencyResponse {
        protocol_version: 1,
        statement: "simulated_comparison",
        evidence_status: "unverified",
        asset,
        snapshot_size: tree.original_leaf_count(),
        root_hash: hash_to_hex(&root.hash),
        total_liabilities: total_liabilities.to_string(),
        compared_assets: resolved_assets.assets.to_string(),
        assets_source: resolved_assets.source,
        liabilities_leq_assets,
        computed_at: verified_at,
    }))
}

fn hash_to_hex(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[inline]
fn internal_error(err: sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": format!("database error: {err}") })),
    )
}

#[inline]
fn internal_error_msg(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": msg })),
    )
}

#[inline]
fn not_found(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": msg })),
    )
}
