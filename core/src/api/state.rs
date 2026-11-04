// core/src/api/state.rs
// Shared application state injected into every Axum handler via `.with_state()`.
// All fields are Clone + Send + Sync; cheap to clone because each is Arc-backed.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use metrics_exporter_prometheus::PrometheusHandle;
use parking_lot::{Mutex, RwLock};
use rust_decimal::Decimal;
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};

use crate::db::worker::PersistenceEvent;
use crate::engine::Engine;
use crate::ledger::{InMemoryLedger, LedgerError};

use super::ws::{WsEvent, BROADCAST_CAPACITY};

const EXCHANGE_BASE_CAPITAL_USDT: u64 = 500_000_000;

// ─────────────────────────────────────────────────────────────────────────────
// AppState
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    /// The multi-symbol matching engine — sync, guarded by a parking_lot RwLock.
    /// Write lock for match/cancel; read lock for depth snapshots.
    pub engine: Arc<RwLock<Engine>>,

    /// Monotonically increasing counter for generating unique u64 order IDs.
    /// Shared across handler clones via Arc; fetch_add is lock-free.
    pub next_order_id: Arc<AtomicU64>,

    /// sqlx connection pool for async DB reads (balances, order history).
    pub db: PgPool,

    /// Channel sender to the async persistence worker.
    /// Handlers send OrderPlaced / TradeFilled / OrderCancelled without blocking.
    pub events: mpsc::Sender<PersistenceEvent>,

    /// Maps order_id → (user_id, symbol) for ownership checks and routing on cancel.
    /// Populated when an order is placed; entries are retained until server restart.
    pub order_users: Arc<Mutex<HashMap<u64, (u64, String)>>>,

    /// In-memory wallet ledger (free/locked balances + order reservations).
    pub ledger: Arc<Mutex<InMemoryLedger>>,

    /// Internal exchange totals, kept for future admin views only.
    /// Not exposed in user wallet APIs.
    pub exchange_funds: Arc<Mutex<ExchangeFunds>>,

    /// Last executed trade price per symbol. Used as a stable anchor for
    /// order price-band checks to reduce quote-spam market skew.
    pub last_trade_price: Arc<Mutex<HashMap<String, Decimal>>>,

    /// Broadcast sender for the WebSocket event bus.
    /// Each WebSocket connection clones a Receiver via `subscribe()`.
    /// `send` is synchronous and non-blocking; ignored if no active receivers.
    pub broadcast: broadcast::Sender<WsEvent>,

    /// Prometheus exporter handle used by GET /metrics.
    pub metrics: PrometheusHandle,

    /// Background simulator runtime state (testing mode).
    /// The worker loop runs independently from browser sessions.
    pub simulator: Arc<Mutex<SimulatorState>>,
    /// Development-only runtime mode and server-owned role configuration.
    pub policy: super::policy::RuntimePolicy,
}

impl AppState {
    pub async fn new(
        db: PgPool,
        events: mpsc::Sender<PersistenceEvent>,
        metrics: PrometheusHandle,
        policy: super::policy::RuntimePolicy,
    ) -> Result<Self, sqlx::Error> {
        let (broadcast_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let ledger = bootstrap_ledger(&db).await.map_err(|e| match e {
            BootstrapLedgerError::Db(err) => err,
            BootstrapLedgerError::InvalidSnapshot => {
                sqlx::Error::Protocol("invalid balances snapshot for in-memory ledger".to_string())
            }
        })?;
        let total_user_usdt = load_total_user_usdt(&db).await?;
        let exchange_funds =
            ExchangeFunds::new(Decimal::from(EXCHANGE_BASE_CAPITAL_USDT), total_user_usdt);

        Ok(Self {
            engine: Arc::new(RwLock::new(Engine::new())),
            next_order_id: Arc::new(AtomicU64::new(1)),
            db,
            events,
            order_users: Arc::new(Mutex::new(HashMap::new())),
            ledger: Arc::new(Mutex::new(ledger)),
            exchange_funds: Arc::new(Mutex::new(exchange_funds)),
            last_trade_price: Arc::new(Mutex::new(HashMap::new())),
            broadcast: broadcast_tx,
            metrics,
            simulator: Arc::new(Mutex::new(SimulatorState::default())),
            policy,
        })
    }

    /// Atomically allocate the next order ID (monotonically increasing).
    #[inline]
    pub fn alloc_order_id(&self) -> u64 {
        self.next_order_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Like `alloc_order_id`, but rejects IDs outside the SQL BIGINT domain (PLAN-06).
    pub fn try_alloc_order_id(&self) -> Result<u64, crate::money::MoneyError> {
        let id = self.alloc_order_id();
        crate::money::id_to_sql(id)?;
        Ok(id)
    }

    /// Register an order → (user, symbol) mapping when a new order is submitted.
    #[inline]
    pub fn register_order_user(&self, order_id: u64, user_id: u64, symbol: String) {
        self.order_users.lock().insert(order_id, (user_id, symbol));
    }

    /// Look up the owner and symbol of an order; returns `None` if the order is unknown.
    #[inline]
    pub fn get_order_user(&self, order_id: u64) -> Option<(u64, String)> {
        self.order_users.lock().get(&order_id).cloned()
    }

    /// Remove order owner mapping. Used to clean up pre-registered IDs when
    /// placement fails validation/matching.
    #[inline]
    pub fn unregister_order_user(&self, order_id: u64) {
        self.order_users.lock().remove(&order_id);
    }

    #[inline]
    pub fn set_last_trade_price(&self, symbol: String, price: Decimal) {
        self.last_trade_price.lock().insert(symbol, price);
    }

    #[inline]
    pub fn get_last_trade_price(&self, symbol: &str) -> Option<Decimal> {
        self.last_trade_price.lock().get(symbol).copied()
    }

    #[inline]
    pub fn adjust_exchange_user_usdt(&self, delta: Decimal) {
        self.exchange_funds.lock().apply_user_delta(delta);
    }

    #[inline]
    pub fn adjust_exchange_capital_usdt(&self, delta: Decimal) -> Result<(), &'static str> {
        self.exchange_funds.lock().apply_capital_delta(delta)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulatorProfile {
    Normal,
    Fast,
    Turbo,
    Hyper,
}

impl SimulatorProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            SimulatorProfile::Normal => "normal",
            SimulatorProfile::Fast => "fast",
            SimulatorProfile::Turbo => "turbo",
            SimulatorProfile::Hyper => "hyper",
        }
    }

    /// Parse a profile from a string in a case-insensitive way.
    /// Also accepts short aliases like `n`, `f`, `t`, `h`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "normal" | "n" => Some(SimulatorProfile::Normal),
            "fast" | "f" => Some(SimulatorProfile::Fast),
            "turbo" | "t" => Some(SimulatorProfile::Turbo),
            "hyper" | "h" => Some(SimulatorProfile::Hyper),
            _ => None,
        }
    }
}

impl std::fmt::Display for SimulatorProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SimulatorProfile {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SimulatorProfile::parse(s).ok_or("invalid simulator profile")
    }
}

#[derive(Debug, Clone)]
pub struct SimulatorPairStats {
    pub orders: u64,
    pub fills: u64,
}

#[derive(Debug, Clone)]
pub struct SimulatorState {
    pub running: bool,
    pub profile: SimulatorProfile,
    pub ticks: u64,
    pub total_orders: u64,
    pub total_fills: u64,
    pub pair_stats: HashMap<String, SimulatorPairStats>,
}

impl SimulatorState {
    pub fn reset_counters(&mut self) {
        self.ticks = 0;
        self.total_orders = 0;
        self.total_fills = 0;
        for stats in self.pair_stats.values_mut() {
            stats.orders = 0;
            stats.fills = 0;
        }
    }
}

impl Default for SimulatorState {
    fn default() -> Self {
        let pair_stats = HashMap::from([
            (
                "BTC_USDT".to_string(),
                SimulatorPairStats {
                    orders: 0,
                    fills: 0,
                },
            ),
            (
                "ETH_USDT".to_string(),
                SimulatorPairStats {
                    orders: 0,
                    fills: 0,
                },
            ),
            (
                "SOL_USDT".to_string(),
                SimulatorPairStats {
                    orders: 0,
                    fills: 0,
                },
            ),
            (
                "BNB_USDT".to_string(),
                SimulatorPairStats {
                    orders: 0,
                    fills: 0,
                },
            ),
        ]);

        Self {
            // Synthetic writes require an explicit admin action in demo mode.
            running: false,
            profile: SimulatorProfile::Turbo,
            ticks: 0,
            total_orders: 0,
            total_fills: 0,
            pair_stats,
        }
    }
}

#[derive(Debug)]
pub struct ExchangeFunds {
    pub base_capital_usdt: Decimal,
    pub total_user_usdt: Decimal,
    pub total_exchange_usdt: Decimal,
}

impl ExchangeFunds {
    fn new(base_capital_usdt: Decimal, total_user_usdt: Decimal) -> Self {
        Self {
            base_capital_usdt,
            total_user_usdt,
            total_exchange_usdt: base_capital_usdt + total_user_usdt,
        }
    }

    fn apply_user_delta(&mut self, delta: Decimal) {
        self.total_user_usdt += delta;
        self.total_exchange_usdt = self.base_capital_usdt + self.total_user_usdt;
    }

    fn apply_capital_delta(&mut self, delta: Decimal) -> Result<(), &'static str> {
        let next_capital = self.base_capital_usdt + delta;
        if next_capital < Decimal::ZERO {
            return Err("exchange capital cannot be negative");
        }

        self.base_capital_usdt = next_capital;
        self.total_exchange_usdt = self.base_capital_usdt + self.total_user_usdt;
        Ok(())
    }
}

#[derive(Debug)]
enum BootstrapLedgerError {
    Db(sqlx::Error),
    InvalidSnapshot,
}

async fn bootstrap_ledger(db: &PgPool) -> Result<InMemoryLedger, BootstrapLedgerError> {
    let rows: Vec<(i64, String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT user_id, asset_symbol, available, locked
         FROM balances",
    )
    .fetch_all(db)
    .await
    .map_err(BootstrapLedgerError::Db)?;

    InMemoryLedger::from_rows(&rows).map_err(|e| match e {
        LedgerError::InvalidUserId => BootstrapLedgerError::InvalidSnapshot,
        _ => BootstrapLedgerError::InvalidSnapshot,
    })
}

async fn load_total_user_usdt(db: &PgPool) -> Result<Decimal, sqlx::Error> {
    let row: (Decimal,) = sqlx::query_as(
        "SELECT COALESCE(SUM(available + locked), 0)::numeric
         FROM balances
         WHERE asset_symbol = 'USDT'",
    )
    .fetch_one(db)
    .await?;

    Ok(row.0)
}
