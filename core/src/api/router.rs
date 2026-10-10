// core/src/api/router.rs
// Assembles the Axum Router and registers all routes.
//
// Routes are added incrementally across API tasks:
//   API-02  middleware: x-user-id auth
//   API-03  POST   /api/orders
//           DELETE /api/orders/:id
//   API-04  GET    /api/orderbook
//           GET    /api/balances
//   API-05  GET    /ws  (WebSocket upgrade)

use axum::{
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde_json::{json, Value};

use super::{admin, auth, data, metrics, orders, simulator, state::AppState, wallet, ws, zkp};

// ─────────────────────────────────────────────────────────────────────────────
// Public factory
// ─────────────────────────────────────────────────────────────────────────────

/// Build the application Router with AppState injected.
/// Call once at startup and pass the result to `axum::serve`.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics::metrics_handler))
        // Pillar 1: username/password auth + identity bootstrap
        .route("/api/auth/register", post(auth::register_handler))
        .route("/api/auth/login", post(auth::login_handler))
        .route("/api/auth/me", get(auth::me_handler))
        .route("/api/auth/users", get(auth::users_handler))
        .route(
            "/api/auth/display-name",
            put(auth::update_display_name_handler),
        )
        // API-03: order management
        .route("/api/orders", post(orders::place_order))
        .route("/api/orders/open", get(data::open_orders_handler))
        .route("/api/orders/:id", delete(orders::cancel_order))
        // API-04: market data and user balances
        .route("/api/orderbook", get(data::orderbook_handler))
        .route("/api/assets", get(data::assets_handler))
        .route("/api/market/tickers/live", get(data::live_tickers_handler))
        .route("/api/price/average", get(data::average_price_handler))
        .route("/api/balances", get(data::balances_handler))
        .route("/api/balances/:asset", get(data::balance_asset_handler))
        // Wallet: deposit and personal trade history
        .route("/api/deposit", post(wallet::deposit_handler))
        .route("/api/withdraw", post(wallet::withdraw_handler))
        .route("/api/transfer", post(wallet::transfer_handler))
        .route("/api/trades/recent", get(data::recent_trades_handler))
        .route("/api/trades/user", get(wallet::user_trades_handler))
        // Testing simulator runtime controls (backend background loop)
        .route(
            "/api/simulator/status",
            get(simulator::simulator_status_handler),
        )
        .route(
            "/api/simulator/start",
            post(simulator::simulator_start_handler),
        )
        .route(
            "/api/simulator/stop",
            post(simulator::simulator_stop_handler),
        )
        .route(
            "/api/simulator/reset",
            post(simulator::simulator_reset_handler),
        )
        .route(
            "/api/simulator/profile",
            put(simulator::simulator_profile_handler),
        )
        // OHLCV candlestick data for the chart
        .route("/api/candles", get(data::candles_handler))
        // ZKP-05: solvency proof package for authenticated user
        .route("/api/zkp/proof", get(zkp::proof_handler))
        // ZKP: exchange-facing solvency check (no user-specific proof)
        .route("/api/zkp/solvency", get(zkp::solvency_handler))
        // API-05: real-time WebSocket feed
        .route("/ws", get(ws::ws_handler))
        // Admin Panel Endpoints
        .route("/api/admin/metrics", get(admin::admin_metrics_handler))
        .route("/api/admin/treasury", get(admin::admin_treasury_handler))
        .route(
            "/api/admin/treasury/deposit",
            post(admin::admin_treasury_deposit_handler),
        )
        .route(
            "/api/admin/treasury/withdraw",
            post(admin::admin_treasury_withdraw_handler),
        )
        .route("/api/admin/assets", get(admin::get_assets_handler))
        .route("/api/admin/assets", post(admin::add_asset_handler))
        .route("/api/admin/markets/halt", post(admin::halt_market_handler))
        .route("/api/admin/users", get(admin::admin_users_handler))
        .route(
            "/api/admin/users/:id/suspend",
            put(admin::suspend_user_handler),
        )
        .route(
            "/api/admin/zkp/snapshot",
            post(admin::trigger_zkp_snapshot_handler),
        )
        .route("/api/admin/zkp/history", get(admin::zkp_history_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::policy::enforce_policy,
        ))
        .with_state(state)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /health — liveness probe used by Docker healthcheck and load balancers.
async fn health_handler() -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}
