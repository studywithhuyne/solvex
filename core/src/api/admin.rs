use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use super::state::AppState;

// --- Dashboard & Metrics ---

#[derive(Serialize)]
pub struct AdminMetrics {
    pub volume_24h_usdt: String,
    pub total_users: i64,
    pub active_orders: i64,
}

pub async fn admin_metrics_handler(
    State(state): State<AppState>,
) -> Result<Json<AdminMetrics>, (StatusCode, String)> {
    // Queries without macro to avoid compile-time DB dependency
    let total_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let active_orders: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM orders_log WHERE status IN ('open', 'partial')")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);

    let volume_usdt: Option<Decimal> = sqlx::query_scalar(
        "SELECT SUM(price * amount) FROM trades_log WHERE executed_at > NOW() - INTERVAL '1 day'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(None);

    Ok(Json(AdminMetrics {
        volume_24h_usdt: volume_usdt.unwrap_or(Decimal::ZERO).to_string(),
        total_users,
        active_orders,
    }))
}

#[derive(Serialize)]
pub struct TreasuryMetrics {
    pub exchange_capital: String,
    pub exchange_revenue: String,
    pub total_user_liabilities: String,
    pub total_exchange_funds: String,
    pub solvency_ratio: String,
}

pub async fn admin_treasury_handler(
    State(state): State<AppState>,
) -> Result<Json<TreasuryMetrics>, (StatusCode, String)> {
    let base_capital = state.exchange_funds.lock().base_capital_usdt;
    let exchange_revenue = state.ledger.lock().exchange_revenue_by_asset("USDT");

    let liab: Option<Decimal> = sqlx::query_scalar(
        "SELECT SUM(available + locked) FROM balances WHERE asset_symbol = 'USDT'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(Some(Decimal::ZERO));

    let total_liabilities = liab.unwrap_or(Decimal::ZERO);
    let total_exchange_funds = base_capital + exchange_revenue;

    let solvency_ratio = if total_liabilities > Decimal::ZERO {
        (total_exchange_funds / total_liabilities).to_string()
    } else {
        "infinity".to_string()
    };

    Ok(Json(TreasuryMetrics {
        exchange_capital: base_capital.to_string(),
        exchange_revenue: exchange_revenue.to_string(),
        total_exchange_funds: total_exchange_funds.to_string(),
        total_user_liabilities: total_liabilities.to_string(),
        solvency_ratio,
    }))
}

// --- Asset & Market Management ---

#[derive(Serialize)]
pub struct AdminAssetDto {
    pub symbol: String,
    pub name: String,
    pub decimals: i32,
    pub is_active: bool,
}

#[derive(serde::Deserialize)]
pub struct AddAssetReq {
    pub symbol: String,
    pub name: String,
}

pub async fn get_assets_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminAssetDto>>, (StatusCode, String)> {
    let rows = sqlx::query_as::<_, (String, String, i16, bool)>(
        "SELECT symbol, name, decimals, is_active FROM assets ORDER BY symbol ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let assets = rows
        .into_iter()
        .map(|(symbol, name, decimals, is_active)| AdminAssetDto {
            symbol,
            name,
            decimals: decimals as i32,
            is_active,
        })
        .collect();

    Ok(Json(assets))
}

pub async fn add_asset_handler(
    State(state): State<AppState>,
    Json(req): Json<AddAssetReq>,
) -> Result<Json<()>, (StatusCode, String)> {
    let symbol = req.symbol.to_uppercase();
    let name = req.name.clone();
    let market_symbol = format!("{}_USDT", symbol);

    // Insert into assets table
    sqlx::query(
        "INSERT INTO assets (symbol, name, decimals, is_active) VALUES ($1, $2, 8, true) ON CONFLICT (symbol) DO NOTHING"
    )
    .bind(&symbol)
    .bind(&name)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Create a default market against USDT if needed
    if symbol != "USDT" {
        sqlx::query(
            "INSERT INTO markets (symbol, base_asset, quote_asset) VALUES ($1, $2, 'USDT') ON CONFLICT (symbol) DO NOTHING"
        )
        .bind(&market_symbol)
        .bind(&symbol)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // Ensure all existing users have a zero balance row for this newly added asset.
        sqlx::query(
            "INSERT INTO balances (user_id, asset_symbol, available, locked)
             SELECT id, $1, 0, 0 FROM users
             ON CONFLICT (user_id, asset_symbol) DO NOTHING",
        )
        .bind(&symbol)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok(Json(()))
}

#[derive(Deserialize)]
pub struct MarketHaltReq {
    pub symbol: String,
}

pub async fn halt_market_handler(
    State(state): State<AppState>,
    Json(req): Json<MarketHaltReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let result = sqlx::query("UPDATE markets SET is_active = FALSE WHERE symbol = $1")
        .bind(&req.symbol)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Market {} not found", req.symbol),
        ));
    }

    Ok(Json(serde_json::json!({
        "status": "success",
        "message": format!("Market {} has been halted.", req.symbol)
    })))
}

// --- User Management ---

#[derive(Serialize)]
pub struct UserListDto {
    pub user_id: i64,
    pub username: String,
    pub is_suspended: bool,
}

pub async fn admin_users_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<UserListDto>>, (StatusCode, String)> {
    let rows = sqlx::query_as::<_, (i64, String, bool)>(
        "SELECT id, username, is_suspended FROM users ORDER BY id ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let users = rows
        .into_iter()
        .map(|(id, username, is_suspended)| UserListDto {
            user_id: id,
            username,
            is_suspended,
        })
        .collect();

    Ok(Json(users))
}

pub async fn suspend_user_handler(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let result = sqlx::query("UPDATE users SET is_suspended = TRUE WHERE id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, format!("User {} not found", user_id)));
    }

    Ok(Json(serde_json::json!({
        "status": "success",
        "message": format!("User {} has been suspended.", user_id)
    })))
}

// --- ZKP Audit Operations ---

pub async fn trigger_zkp_snapshot_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let now = chrono::Utc::now();
    let snapshot_id = format!("simulated_{}", uuid::Uuid::new_v4());

    // This endpoint records demo activity; it does not capture a ledger or build a root.
    let count = sqlx::query!("SELECT COUNT(*) AS \"count!\" FROM users")
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .count;

    let root_hash = "simulated:no-cryptographic-root";
    let count = i32::try_from(count).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "user count exceeds storage range".to_string(),
        )
    })?;

    sqlx::query!(
        "INSERT INTO zkp_snapshots (snapshot_id, root_hash, users_included) VALUES ($1, $2, $3)",
        snapshot_id,
        root_hash,
        count
    )
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "status": "simulated",
        "snapshot_id": snapshot_id,
        "verification_status": "unverified",
        "trusted": false,
        "created_at": now,
        "message": "Demo record only: no immutable ledger snapshot or cryptographic root was produced."
    })))
}

#[derive(Serialize)]
pub struct ZkSnapshotDto {
    pub snapshot_kind: &'static str,
    pub verification_status: &'static str,
    pub trusted: bool,
    pub snapshot_id: String,
    pub root_hash: String,
    pub users_observed: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn zkp_history_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<ZkSnapshotDto>>, (StatusCode, String)> {
    let rows = sqlx::query!(
        "SELECT snapshot_id, root_hash, users_included, created_at FROM zkp_snapshots ORDER BY created_at DESC LIMIT 20"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let dtos = rows
        .into_iter()
        .map(|row| ZkSnapshotDto {
            snapshot_kind: if row.root_hash == "simulated:no-cryptographic-root" {
                "simulated"
            } else {
                "legacy_unverified"
            },
            verification_status: "unverified",
            trusted: false,
            snapshot_id: row.snapshot_id,
            root_hash: row.root_hash,
            users_observed: row.users_included,
            created_at: row.created_at,
        })
        .collect();

    Ok(Json(dtos))
}

#[derive(Deserialize)]
pub struct TreasuryAdjustRequest {
    pub amount: String,
}

#[derive(Serialize)]
pub struct TreasuryAdjustResponse {
    pub ok: bool,
    pub action: String,
    pub exchange_capital: String,
    pub total_exchange_funds: String,
}

pub async fn admin_treasury_deposit_handler(
    State(state): State<AppState>,
    Json(payload): Json<TreasuryAdjustRequest>,
) -> Result<Json<TreasuryAdjustResponse>, (StatusCode, String)> {
    let amount = Decimal::from_str(payload.amount.trim()).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "amount must be a valid decimal".to_string(),
        )
    })?;
    if amount <= Decimal::ZERO {
        return Err((StatusCode::BAD_REQUEST, "amount must be > 0".to_string()));
    }

    state
        .adjust_exchange_capital_usdt(amount)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let funds = state.exchange_funds.lock();
    Ok(Json(TreasuryAdjustResponse {
        ok: true,
        action: "deposit".to_string(),
        exchange_capital: funds.base_capital_usdt.to_string(),
        total_exchange_funds: funds.total_exchange_usdt.to_string(),
    }))
}

pub async fn admin_treasury_withdraw_handler(
    State(state): State<AppState>,
    Json(payload): Json<TreasuryAdjustRequest>,
) -> Result<Json<TreasuryAdjustResponse>, (StatusCode, String)> {
    let amount = Decimal::from_str(payload.amount.trim()).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "amount must be a valid decimal".to_string(),
        )
    })?;
    if amount <= Decimal::ZERO {
        return Err((StatusCode::BAD_REQUEST, "amount must be > 0".to_string()));
    }

    state
        .adjust_exchange_capital_usdt(-amount)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let funds = state.exchange_funds.lock();
    Ok(Json(TreasuryAdjustResponse {
        ok: true,
        action: "withdraw".to_string(),
        exchange_capital: funds.base_capital_usdt.to_string(),
        total_exchange_funds: funds.total_exchange_usdt.to_string(),
    }))
}
