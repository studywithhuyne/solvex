// core/src/api/orders.rs
// REST handlers for order placement and cancellation.
//
// Routes (registered in router.rs):
//   POST   /api/orders       — place a new limit order
//   DELETE /api/orders/:id   — cancel a resting order
//
// Auth:
//   Both endpoints require `x-user-id` header (UserId extractor, API-02).
//
// Persistence contract (from db/worker.rs):
//   1. Send OrderPlaced for the taker BEFORE any TradeFilled that references it.
//   2. Maker orders already exist in orders_log from their own OrderPlaced events.
//
// WebSocket broadcast contract:
//   After every book mutation (place or cancel), fire:
//     - WsEvent::TradeExecuted  for each generated fill (place_order only).
//     - WsEvent::OrderbookUpdate with a fresh depth snapshot.
//   Broadcast is synchronous (no .await) and fire-and-forget.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::Instant;

use crate::{
    api::{
        auth::UserId,
        state::AppState,
        ws::{WsEvent, WsPriceLevel},
    },
    db::worker::PersistenceEvent,
    engine::{Order, Side},
    ledger::LedgerError,
    money::{self, MarketPolicy},
};

#[derive(Debug, sqlx::FromRow)]
struct OpenOrderLookupRow {
    user_id: i64,
    market_symbol: String,
}

#[derive(Debug, sqlx::FromRow)]
struct MarketRow {
    symbol: String,
    base_asset: String,
    quote_asset: String,
    is_active: bool,
}

/// Maximum allowed deviation from reference price (in basis points).
/// 2000 bps = 20%.
const MAX_PRICE_DEVIATION_BPS: i64 = 2_000;
/// Default maker/taker fee rates used by live settlement.
/// 0.001 = 0.10% (maker), 0.002 = 0.20% (taker).
const MAKER_FEE_RATE_MILLIS: i64 = 1;
const TAKER_FEE_RATE_MILLIS: i64 = 2;

// ─────────────────────────────────────────────────────────────────────────────
// Request / Response types
// ─────────────────────────────────────────────────────────────────────────────

/// JSON body for POST /api/orders.
/// price and amount are accepted as decimal strings to preserve full precision
/// (avoids f64 round-trip loss — critical for financial data).
#[derive(Deserialize)]
pub struct PlaceOrderRequest {
    /// "buy" or "sell" (case-insensitive)
    pub side: String,
    /// Limit price, e.g. "100.50"
    pub price: String,
    /// Order quantity, e.g. "0.5"
    pub amount: String,
    pub market_symbol: Option<String>,
    pub base_asset: Option<String>,
    pub quote_asset: Option<String>,
}

#[derive(Serialize)]
pub struct UpdatedBalanceDto {
    pub asset: String,
    pub available: String,
    pub locked: String,
}

#[derive(Serialize)]
pub struct PlaceOrderResponse {
    pub order_id: u64,
    /// Number of trades generated immediately (0 = order rested on the book).
    pub trades_count: usize,
    /// How much of the order was matched immediately (0 if fully resting).
    pub matched_amount: String,
    /// Updated balances for the relevant assets after this order was processed.
    /// Clients can use this to refresh the UI without an extra GET /api/balances.
    pub updated_balances: Vec<UpdatedBalanceDto>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// POST /api/orders — submit a new limit order.
///
/// Flow:
///   1. Parse and validate (side, price, amount, assets).
///   2. Allocate order ID; register order → user mapping for trade attribution.
///   3. Acquire engine write lock → match_order → Vec<Trade>, then release lock.
///   4. Fire OrderPlaced event (must arrive before TradeFilled in the channel).
///   5. Fire TradeFilled event per generated trade.
///   6. Broadcast TradeExecuted + OrderbookUpdate via WebSocket channel.
///   7. Return 201 Created with { order_id, trades_count }.
pub async fn place_order(
    State(state): State<AppState>,
    UserId(user_id): UserId,
    Json(req): Json<PlaceOrderRequest>,
) -> Result<(StatusCode, Json<PlaceOrderResponse>), (StatusCode, Json<serde_json::Value>)> {
    // --- Parse side ---
    let side = match req.side.to_lowercase().as_str() {
        "buy" => Side::Buy,
        "sell" => Side::Sell,
        _ => return Err(bad_request("side must be 'buy' or 'sell'")),
    };

    // --- Parse price and amount as Decimal (no f64 intermediate) ---
    let price = Decimal::from_str(&req.price)
        .map_err(|_| bad_request("price must be a valid decimal number"))?;
    let amount = Decimal::from_str(&req.amount)
        .map_err(|_| bad_request("amount must be a valid decimal number"))?;

    // --- Resolve/validate market from DB ---
    let market = resolve_market(&state, &req).await?;
    let symbol = market.symbol.clone();
    let base_asset = market.base_asset;
    let quote_asset = market.quote_asset;

    // --- PLAN-06: reject precision/range/tick/lot/notional problems before any mutation ---
    MarketPolicy::standard()
        .validate_order(&price, &amount)
        .map_err(|e| bad_request(&e.to_string()))?;

    // --- Allocate ID and build Order ---
    let order_id = state
        .try_alloc_order_id()
        .map_err(|e| bad_request(&e.to_string()))?;
    let side_label = match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    let order = Order::new(order_id, user_id, &symbol, side, price, amount);

    if let Some(reference) = reference_price_for_symbol(&state, &symbol) {
        validate_price_band(price, reference)?;
    }

    {
        let mut ledger = state.ledger.lock();
        ledger
            .reserve_for_new_order(&order, &base_asset, &quote_asset)
            .map_err(ledger_bad_request)?;
    }

    // Register owner + symbol before matching so cancel and TradeFilled lookup always succeed.
    state.register_order_user(order_id, user_id, symbol.clone());

    // --- Match (sync, engine write-locked, no async I/O inside) ---
    let start = Instant::now();
    let trades = {
        let mut engine = state.engine.write();
        match engine.match_order(order.clone()) {
            Ok(trades) => trades,
            Err(e) => {
                // The order was pre-registered for ownership lookup; remove it
                // if matching rejected the incoming order.
                if let Err(ledger_err) = state.ledger.lock().cancel_reservation(order_id) {
                    tracing::error!(
                        ?ledger_err,
                        order_id,
                        "Failed to rollback ledger reservation after rejected match"
                    );
                }
                state.unregister_order_user(order_id);
                return Err(bad_request(&e.to_string()));
            }
        }
    };
    {
        let mut ledger = state.ledger.lock();
        let maker_fee_rate = Decimal::new(MAKER_FEE_RATE_MILLIS, 3);
        let taker_fee_rate = Decimal::new(TAKER_FEE_RATE_MILLIS, 3);
        for trade in &trades {
            ledger
                .settle_trade(trade, maker_fee_rate, taker_fee_rate)
                .map_err(|e| internal_ledger_error(LedgerError::SettlementFailed(e)))?;
        }
    }

    let match_latency_us = start.elapsed().as_secs_f64() * 1_000_000.0;
    metrics::histogram!(
        "cex_order_match_latency_us",
        "symbol" => symbol.clone(),
        "side" => side_label
    )
    .record(match_latency_us);
    metrics::counter!(
        "cex_orders_total",
        "symbol" => symbol.clone(),
        "side" => side_label
    )
    .increment(1);
    // Engine lock released here — async persistence and broadcasts happen below.

    // --- Persist: OrderPlaced MUST arrive before TradeFilled ---
    let _ = state
        .events
        .send(PersistenceEvent::OrderPlaced {
            order: order.clone(),
            market_symbol: symbol.clone(),
        })
        .await;

    let trades_count = trades.len();
    if trades_count > 0 {
        metrics::counter!(
            "cex_trades_total",
            "symbol" => symbol.clone()
        )
        .increment(trades_count as u64);
    }
    for trade in &trades {
        // Persist (clone Trade because PersistenceEvent takes ownership).
        let maker_user_id = state
            .get_order_user(trade.maker_order_id)
            .map(|(uid, _)| uid)
            .unwrap_or(0);
        let _ = state
            .events
            .send(PersistenceEvent::TradeFilled {
                trade: trade.clone(),
                maker_user_id,
                taker_user_id: user_id,
                taker_side: side,
                market_symbol: symbol.clone(),
            })
            .await;

        // Broadcast individual fill to WebSocket clients (synchronous, non-blocking).
        let _ = state.broadcast.send(WsEvent::RecentTrade {
            symbol: symbol.clone(),
            price: trade.price.to_string(),
            amount: trade.amount.to_string(),
        });

        state.set_last_trade_price(symbol.clone(), trade.price);
    }

    // Broadcast a fresh depth snapshot so clients see the updated book.
    broadcast_orderbook_snapshot(&state, &symbol);
    let active_symbols = {
        let engine = state.engine.read();
        engine.symbols().len() as f64
    };
    metrics::gauge!("cex_active_symbols").set(active_symbols);

    // Compute matched_amount: sum of all fill quantities.
    let matched_amount: Decimal = trades.iter().map(|t| t.amount).sum();

    // Read fresh balances for base + quote assets directly from the in-memory
    // ledger (already updated synchronously above via apply_trade_fill).
    // This lets the client update the UI in a single round-trip.
    let updated_balances = {
        let ledger = state.ledger.lock();
        let snapshots = ledger.balances_for_user(user_id);
        snapshots
            .into_iter()
            .filter(|b| b.asset == base_asset || b.asset == quote_asset)
            .map(|b| UpdatedBalanceDto {
                asset: b.asset,
                available: b.free.to_string(),
                locked: b.locked.to_string(),
            })
            .collect()
    };

    Ok((
        StatusCode::CREATED,
        Json(PlaceOrderResponse {
            order_id,
            trades_count,
            matched_amount: matched_amount.to_string(),
            updated_balances,
        }),
    ))
}

/// DELETE /api/orders/:id — cancel a resting order.
///
/// Flow:
///   1. Check ownership via order_users map (O(1), no engine lock needed).
///   2. Acquire engine write lock → cancel_order, then release lock.
///   3. Fire OrderCancelled event.
///   4. Broadcast updated orderbook snapshot.
///   5. Return 204 No Content.
pub async fn cancel_order(
    State(state): State<AppState>,
    UserId(user_id): UserId,
    Path(order_id): Path<u64>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    // Order IDs must stay inside the SQL BIGINT domain before any `as i64` binding.
    money::id_to_sql(order_id).map_err(|_| not_found("order not found"))?;

    // --- Ownership + symbol resolution ---
    // Fast path: in-memory map (hot path while process is alive)
    // Fallback: DB lookup (covers stale rows after restart).
    let (owner_id, symbol) = if let Some((owner_id, symbol)) = state.get_order_user(order_id) {
        (owner_id, symbol)
    } else {
        let row: OpenOrderLookupRow = sqlx::query_as(
            "SELECT user_id, market_symbol
             FROM orders_log
             WHERE order_id = $1 AND status::text IN ('open', 'partial')",
        )
        .bind(order_id as i64)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("database error: {e}") })),
            )
        })?
        .ok_or_else(|| not_found("order not found"))?;

        if row.user_id <= 0 {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "invalid order owner in database" })),
            ));
        }

        (row.user_id as u64, row.market_symbol)
    };

    if owner_id != user_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "forbidden: order belongs to a different user" })),
        ));
    }

    // --- Cancel in engine if currently loaded (sync, write-locked) ---
    // If not found in memory (e.g. after restart), continue and persist cancellation to DB.
    let mut cancelled_in_memory = false;
    {
        let mut engine = state.engine.write();
        if engine.cancel_order(&symbol, order_id).is_ok() {
            cancelled_in_memory = true;
        }
    }

    if cancelled_in_memory {
        state
            .ledger
            .lock()
            .cancel_reservation(order_id)
            .map_err(internal_ledger_error)?;
        state.unregister_order_user(order_id);
    }

    // --- Persist async ---
    let _ = state
        .events
        .send(PersistenceEvent::OrderCancelled { order_id })
        .await;
    metrics::counter!(
        "cex_order_cancellations_total",
        "symbol" => symbol.clone()
    )
    .increment(1);

    // --- Broadcast updated orderbook snapshot ---
    broadcast_orderbook_snapshot(&state, &symbol);
    let active_symbols = {
        let engine = state.engine.read();
        engine.symbols().len() as f64
    };
    metrics::gauge!("cex_active_symbols").set(active_symbols);

    Ok(StatusCode::NO_CONTENT)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot the engine depth for `symbol` and broadcast an OrderbookUpdate event.
/// Acquires a read lock (not write), so this never contends with matching.
/// Returns immediately if no WebSocket clients are connected.
fn broadcast_orderbook_snapshot(state: &AppState, symbol: &str) {
    let (raw_bids, raw_asks) = {
        let engine = state.engine.read();
        engine.depth_snapshot(symbol, 50)
    };

    let to_level = |(price, amount): (Decimal, Decimal)| WsPriceLevel {
        price: price.to_string(),
        amount: amount.to_string(),
    };

    // Err(SendError) is returned only when there are no active receivers;
    // this is normal during startup or when no clients are connected.
    let _ = state.broadcast.send(WsEvent::OrderbookUpdate {
        symbol: symbol.to_owned(),
        bids: raw_bids.into_iter().map(to_level).collect(),
        asks: raw_asks.into_iter().map(to_level).collect(),
    });
}

#[inline]
fn bad_request(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
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

fn reference_price_for_symbol(state: &AppState, symbol: &str) -> Option<Decimal> {
    if let Some(last_trade) = state.get_last_trade_price(symbol) {
        return Some(last_trade);
    }

    let (bids, asks) = {
        let engine = state.engine.read();
        engine.depth_snapshot(symbol, 1)
    };

    match (bids.first().map(|(p, _)| *p), asks.first().map(|(p, _)| *p)) {
        (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::from(2_u64)),
        (Some(bid), None) => Some(bid),
        (None, Some(ask)) => Some(ask),
        (None, None) => None,
    }
}

async fn resolve_market(
    state: &AppState,
    req: &PlaceOrderRequest,
) -> Result<MarketRow, (StatusCode, Json<serde_json::Value>)> {
    if let Some(symbol_raw) = req.market_symbol.as_deref() {
        let symbol = symbol_raw.trim().to_ascii_uppercase();
        if symbol.is_empty() {
            return Err(bad_request("market_symbol must not be empty"));
        }
        return fetch_market_by_symbol(state, &symbol).await;
    }

    let base_asset = req
        .base_asset
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_uppercase();
    let quote_asset = req
        .quote_asset
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_uppercase();

    if base_asset.is_empty() || quote_asset.is_empty() {
        return Err(bad_request(
            "provide market_symbol or both base_asset and quote_asset",
        ));
    }

    let symbol = format!("{}_{}", base_asset, quote_asset);
    fetch_market_by_symbol(state, &symbol).await
}

async fn fetch_market_by_symbol(
    state: &AppState,
    symbol: &str,
) -> Result<MarketRow, (StatusCode, Json<serde_json::Value>)> {
    let market = sqlx::query_as::<_, MarketRow>(
        "SELECT symbol, base_asset, quote_asset, is_active
         FROM markets
         WHERE symbol = $1",
    )
    .bind(symbol)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("database error: {e}") })),
        )
    })?
    .ok_or_else(|| not_found("market not found"))?;

    if !market.is_active {
        return Err(bad_request("market is halted"));
    }

    Ok(market)
}

fn validate_price_band(
    incoming_price: Decimal,
    reference_price: Decimal,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if reference_price <= Decimal::ZERO {
        return Ok(());
    }

    let bps = Decimal::from(MAX_PRICE_DEVIATION_BPS) / Decimal::from(10_000_u64);
    let lower = reference_price * (Decimal::ONE - bps);
    let upper = reference_price * (Decimal::ONE + bps);

    if incoming_price < lower || incoming_price > upper {
        return Err(bad_request(&format!(
            "price out of allowed band: incoming={incoming_price}, reference={reference_price}, allowed=[{lower}, {upper}]"
        )));
    }

    Ok(())
}

#[inline]
fn ledger_bad_request(err: LedgerError) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": err.to_string() })),
    )
}

#[inline]
fn internal_ledger_error(err: LedgerError) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": format!("ledger error: {err}") })),
    )
}
