// core/src/api/wallet.rs
// Wallet-related handlers: mock deposit and personal trade history.
//
// Routes (registered in router.rs):
//   POST /api/deposit      — add funds to a user's balance (requires x-user-id)
//   POST /api/withdraw     — withdraw funds from a user's balance (requires x-user-id)
//   POST /api/transfer     — transfer funds between assets for authenticated user
//   GET  /api/trades/user  — personal trade history for the authenticated user

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::api::{auth::UserId, state::AppState};
use crate::ledger::LedgerError;

// ─────────────────────────────────────────────────────────────────────────────
// Request / Response types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DepositRequest {
    pub asset:  Option<String>,
    pub amount: String,
}

#[derive(Serialize)]
pub struct DepositResponse {
    pub asset:         String,
    pub deposited:     String,
    pub new_available: String,
}

#[derive(Deserialize)]
pub struct WithdrawRequest {
    pub asset:  Option<String>,
    pub amount: String,
}

#[derive(Serialize)]
pub struct WithdrawResponse {
    pub asset:         String,
    pub withdrawn:     String,
    pub new_available: String,
}

#[derive(Deserialize)]
pub struct TransferRequest {
    pub from_asset: Option<String>,
    pub to_asset:   Option<String>,
    pub asset:      Option<String>,
    pub from_wallet: Option<String>,
    pub to_wallet:   Option<String>,
    pub amount:     String,
}

#[derive(Serialize)]
pub struct TransferResponse {
    pub from_asset:         Option<String>,
    pub to_asset:           Option<String>,
    pub asset:              String,
    pub from_wallet:        Option<String>,
    pub to_wallet:          Option<String>,
    pub transferred:        String,
    pub new_from_available: Option<String>,
    pub new_to_available:   Option<String>,
}

#[derive(Serialize)]
pub struct UserTradeDto {
    pub id:             String,
    pub maker_order_id: i64,
    pub taker_order_id: i64,
    pub side:           String,
    pub price:          String,
    pub amount:         String,
    pub base_asset:     String,
    pub quote_asset:    String,
    pub executed_at:    String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

type ApiError = (StatusCode, Json<serde_json::Value>);

type TradeRow = (
    uuid::Uuid,
    i64,
    i64,
    i64,
    i64,
    Decimal,
    Decimal,
    String,
    chrono::DateTime<chrono::Utc>,
);

fn db_err(e: sqlx::Error) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": format!("database error: {e}") })),
    )
}

fn bad_request(msg: &str) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": msg })),
    )
}

/// POST /api/deposit — inject test funds into the authenticated user's balance.
pub async fn deposit_handler(
    State(state): State<AppState>,
    UserId(user_id): UserId,
    Json(body): Json<DepositRequest>,
) -> Result<Json<DepositResponse>, ApiError> {
    let asset = normalized_transfer_asset(body.asset.as_deref())?;
    ensure_asset_exists(&state, &asset).await?;
    ensure_user_balance_row(&state, user_id, &asset).await?;

    let amount: Decimal = body
        .amount
        .parse()
        .map_err(|_| bad_request("amount must be a valid decimal string"))?;

    // PLAN-06: positive, scale <= 8 and within range, rejected before any DB/ledger write.
    crate::money::validate_positive(&amount).map_err(|e| bad_request(&e.to_string()))?;

    let row: Option<(Decimal,)> = sqlx::query_as(
        "UPDATE balances
         SET available = available + $1, updated_at = now()
         WHERE user_id = $2 AND asset_symbol = $3
         RETURNING available",
    )
    .bind(amount)
    .bind(user_id as i64)
    .bind(&asset)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    match row {
        Some((_db_available,)) => {
            let new_available = state
                .ledger
                .lock()
                .deposit(user_id, &asset, amount)
                .map_err(internal_ledger_error)?;
            if asset == "USDT" {
                state.adjust_exchange_user_usdt(amount);
            }

            Ok(Json(DepositResponse {
                asset,
                deposited:     amount.to_string(),
                new_available: new_available.to_string(),
            }))
        }
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "user/asset combination not found" })),
        )),
    }
}

/// POST /api/withdraw — withdraw supported assets from the authenticated user's available balance.
pub async fn withdraw_handler(
    State(state): State<AppState>,
    UserId(user_id): UserId,
    Json(body): Json<WithdrawRequest>,
) -> Result<Json<WithdrawResponse>, ApiError> {
    let asset = normalized_transfer_asset(body.asset.as_deref())?;
    ensure_asset_exists(&state, &asset).await?;
    ensure_user_balance_row(&state, user_id, &asset).await?;

    let amount: Decimal = body
        .amount
        .parse()
        .map_err(|_| bad_request("amount must be a valid decimal string"))?;

    // PLAN-06: positive, scale <= 8 and within range, rejected before any DB/ledger write.
    crate::money::validate_positive(&amount).map_err(|e| bad_request(&e.to_string()))?;

    let row: Option<(Decimal,)> = sqlx::query_as(
        "UPDATE balances
         SET available = available - $1, updated_at = now()
         WHERE user_id = $2 AND asset_symbol = $3 AND available >= $1
         RETURNING available",
    )
    .bind(amount)
    .bind(user_id as i64)
    .bind(&asset)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    match row {
        Some((_db_available,)) => {
            let new_available = state
                .ledger
                .lock()
                .withdraw(user_id, &asset, amount)
                .map_err(internal_ledger_error)?;
            if asset == "USDT" {
                state.adjust_exchange_user_usdt(-amount);
            }

            Ok(Json(WithdrawResponse {
                asset,
                withdrawn:     amount.to_string(),
                new_available: new_available.to_string(),
            }))
        }
        None => {
            let exists: Option<(i64,)> = sqlx::query_as(
                "SELECT user_id
                 FROM balances
                 WHERE user_id = $1 AND asset_symbol = $2",
            )
            .bind(user_id as i64)
            .bind(&asset)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

            if exists.is_some() {
                Err(bad_request("insufficient available balance"))
            } else {
                Err((
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "user/asset combination not found" })),
                ))
            }
        }
    }
}

/// POST /api/transfer — transfer available balance between two assets.
///
/// This is a mock internal transfer in dev/test mode with 1:1 amount semantics.
pub async fn transfer_handler(
    State(state): State<AppState>,
    UserId(user_id): UserId,
    Json(body): Json<TransferRequest>,
) -> Result<Json<TransferResponse>, ApiError> {
    let amount: Decimal = body
        .amount
        .parse()
        .map_err(|_| bad_request("amount must be a valid decimal string"))?;

    // PLAN-06: positive, scale <= 8 and within range, rejected before any DB/ledger write.
    crate::money::validate_positive(&amount).map_err(|e| bad_request(&e.to_string()))?;

    if let Some(asset_raw) = body.asset.as_deref() {
        let asset = normalized_required_asset(asset_raw)?;
        ensure_asset_exists(&state, &asset).await?;
        ensure_user_balance_row(&state, user_id, &asset).await?;

        let from_wallet = body
            .from_wallet
            .as_deref()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let to_wallet = body
            .to_wallet
            .as_deref()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        if from_wallet.is_none() || to_wallet.is_none() {
            return Err(bad_request("from_wallet and to_wallet are required for wallet transfer"));
        }

        if from_wallet == to_wallet {
            return Err(bad_request("from_wallet and to_wallet must be different"));
        }

        let row: Option<(Decimal,)> = sqlx::query_as(
            "SELECT available
             FROM balances
             WHERE user_id = $1 AND asset_symbol = $2",
        )
        .bind(user_id as i64)
        .bind(&asset)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?;

        let (available_now,) = row.ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "user/asset combination not found" })),
            )
        })?;

        if available_now < amount {
            return Err(bad_request("insufficient available balance"));
        }

        return Ok(Json(TransferResponse {
            from_asset: None,
            to_asset: None,
            asset,
            from_wallet,
            to_wallet,
            transferred: amount.to_string(),
            new_from_available: Some(available_now.to_string()),
            new_to_available: Some(available_now.to_string()),
        }));
    }

    let from_asset = normalized_required_asset(
        body.from_asset
            .as_deref()
            .ok_or_else(|| bad_request("from_asset is required"))?,
    )?;
    let to_asset = normalized_required_asset(
        body.to_asset
            .as_deref()
            .ok_or_else(|| bad_request("to_asset is required"))?,
    )?;

    if from_asset == to_asset {
        return Err(bad_request("from_asset and to_asset must be different"));
    }

    ensure_asset_exists(&state, &from_asset).await?;
    ensure_asset_exists(&state, &to_asset).await?;
    ensure_user_balance_row(&state, user_id, &from_asset).await?;
    ensure_user_balance_row(&state, user_id, &to_asset).await?;

    let mut tx = state.db.begin().await.map_err(db_err)?;

    let from_row: Option<(Decimal,)> = sqlx::query_as(
        "UPDATE balances
         SET available = available - $1, updated_at = now()
         WHERE user_id = $2 AND asset_symbol = $3 AND available >= $1
         RETURNING available",
    )
    .bind(amount)
    .bind(user_id as i64)
    .bind(&from_asset)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;

    let (from_available_after,) = match from_row {
        Some(row) => row,
        None => {
            tx.rollback().await.map_err(db_err)?;
            let exists: Option<(i64,)> = sqlx::query_as(
                "SELECT user_id
                 FROM balances
                 WHERE user_id = $1 AND asset_symbol = $2",
            )
            .bind(user_id as i64)
            .bind(&from_asset)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

            if exists.is_some() {
                return Err(bad_request("insufficient available balance"));
            }

            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "user/asset combination not found" })),
            ));
        }
    };

    let (to_available_after,): (Decimal,) = sqlx::query_as(
        "UPDATE balances
         SET available = available + $1, updated_at = now()
         WHERE user_id = $2 AND asset_symbol = $3
         RETURNING available",
    )
    .bind(amount)
    .bind(user_id as i64)
    .bind(&to_asset)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;

    tx.commit().await.map_err(db_err)?;

    {
        let mut ledger = state.ledger.lock();
        ledger
            .withdraw(user_id, &from_asset, amount)
            .map_err(internal_ledger_error)?;
        ledger
            .deposit(user_id, &to_asset, amount)
            .map_err(internal_ledger_error)?;
    }

    if from_asset == "USDT" {
        state.adjust_exchange_user_usdt(-amount);
    }
    if to_asset == "USDT" {
        state.adjust_exchange_user_usdt(amount);
    }

    let response_asset = from_asset.clone();

    Ok(Json(TransferResponse {
        from_asset: Some(from_asset),
        to_asset: Some(to_asset),
        asset: response_asset,
        from_wallet: None,
        to_wallet: None,
        transferred: amount.to_string(),
        new_from_available: Some(from_available_after.to_string()),
        new_to_available: Some(to_available_after.to_string()),
    }))
}

#[inline]
fn internal_ledger_error(err: LedgerError) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": format!("ledger error: {err}") })),
    )
}

/// GET /api/trades/user — personal trade history (as maker or taker).
pub async fn user_trades_handler(
    State(state): State<AppState>,
    UserId(user_id): UserId,
) -> Result<Json<Vec<UserTradeDto>>, ApiError> {
    let uid = user_id as i64;

    let rows: Vec<TradeRow> =
        sqlx::query_as(
            "SELECT id, maker_order_id, taker_order_id, maker_user_id, taker_user_id,
                    price, amount, market_symbol, executed_at
             FROM trades_log
             WHERE maker_user_id = $1 OR taker_user_id = $1
             ORDER BY executed_at DESC
             LIMIT 100",
        )
        .bind(uid)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?;

    let dtos = rows
        .into_iter()
        .map(|(id, maker_oid, taker_oid, maker_uid, _taker_uid, price, amount, symbol, exec_at)| {
            let (base, quote) = split_symbol_assets(&symbol);
            let side = if maker_uid == uid { "maker" } else { "taker" };
            UserTradeDto {
                id:             id.to_string(),
                maker_order_id: maker_oid,
                taker_order_id: taker_oid,
                side:           side.to_string(),
                price:          price.to_string(),
                amount:         amount.to_string(),
                base_asset:     base,
                quote_asset:    quote,
                executed_at:    exec_at.to_rfc3339(),
            }
        })
        .collect();

    Ok(Json(dtos))
}

fn split_symbol_assets(symbol: &str) -> (String, String) {
    match symbol.split_once('_') {
        Some((base, quote)) => (base.to_string(), quote.to_string()),
        None => (symbol.to_string(), String::new()),
    }
}

fn normalized_transfer_asset(asset: Option<&str>) -> Result<String, ApiError> {
    let normalized = match asset {
        Some(value) => value.trim().to_ascii_uppercase(),
        None => "USDT".to_string(),
    };

    validate_asset_symbol(&normalized)?;
    Ok(normalized)
}

fn normalized_required_asset(asset: &str) -> Result<String, ApiError> {
    let normalized = asset.trim().to_ascii_uppercase();
    validate_asset_symbol(&normalized)?;
    Ok(normalized)
}

fn validate_asset_symbol(asset: &str) -> Result<(), ApiError> {
    if asset.len() < 2 || asset.len() > 16 {
        return Err(bad_request("asset length must be between 2 and 16 characters"));
    }

    if !asset.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(bad_request("asset may only contain [A-Z0-9_]"));
    }

    Ok(())
}

async fn ensure_asset_exists(state: &AppState, asset: &str) -> Result<(), ApiError> {
    let exists: Option<(String,)> = sqlx::query_as(
        "SELECT symbol
         FROM assets
         WHERE symbol = $1",
    )
    .bind(asset)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    if exists.is_none() {
        return Err(bad_request("asset is not supported"));
    }

    Ok(())
}

async fn ensure_user_balance_row(state: &AppState, user_id: u64, asset: &str) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO balances (user_id, asset_symbol, available, locked)
         VALUES ($1, $2, 0, 0)
         ON CONFLICT (user_id, asset_symbol) DO NOTHING",
    )
    .bind(user_id as i64)
    .bind(asset)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(())
}
