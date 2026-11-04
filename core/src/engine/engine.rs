// core/src/engine/engine.rs
// Multi-symbol routing layer over individual OrderBook instances.
//
// Design
// ──────
// `Engine` owns a `HashMap<String, OrderBook>` keyed by symbol (e.g. "BTC_USDT").
// Lookup is O(1) average-case — satisfying the spec requirement of O(1) routing.
// A new OrderBook is created on-demand the first time an order for a symbol arrives.
//
// Concurrency note:
//   The whole Engine is wrapped in an `Arc<RwLock<Engine>>` at the AppState level.
//   The sync methods here hold no locks themselves — all locking responsibility
//   lives in the Axum handler layer.

use std::collections::HashMap;

use rust_decimal::Decimal;

use super::order_book::DepthSnapshot;
use super::{EngineError, Order, OrderBook, PlannedFill, Trade};

/// Top-level engine: routes each order to the correct per-symbol OrderBook
/// via a `HashMap` for O(1) average lookup.
pub struct Engine {
    books: HashMap<String, OrderBook>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            books: HashMap::new(),
        }
    }

    /// Match `order` against the book for `order.symbol`.
    /// The book is created lazily on the first order for a new symbol.
    pub fn match_order(&mut self, order: Order) -> Result<Vec<Trade>, EngineError> {
        let book = self.books.entry(order.symbol.clone()).or_default();
        book.match_order(order)
    }

    /// Cancel a resting order in the `symbol` book by its `order_id`.
    /// Returns `OrderNotFound` if the symbol has no matching book or the
    /// order is not in the book.
    pub fn cancel_order(&mut self, symbol: &str, order_id: u64) -> Result<Order, EngineError> {
        self.books
            .get_mut(symbol)
            .ok_or(EngineError::OrderNotFound(order_id))?
            .cancel_order(order_id)
    }

    /// Read-only plan of the fills `match_order` would produce (no book mutation).
    pub fn plan_match(&self, order: &Order) -> Result<Vec<PlannedFill>, EngineError> {
        match self.books.get(&order.symbol) {
            Some(book) => book.plan_match(order),
            None => OrderBook::new().plan_match(order),
        }
    }

    /// Look up a resting order.
    pub fn get_order(&self, symbol: &str, order_id: u64) -> Option<&Order> {
        self.books.get(symbol)?.get_order(order_id)
    }

    /// Dry-run validation of replayable fills for one symbol.
    pub fn validate_fills(
        &self,
        symbol: &str,
        fills: &[(u64, Decimal)],
    ) -> Result<(), EngineError> {
        match self.books.get(symbol) {
            Some(book) => book.validate_fills(fills),
            None => match fills.first() {
                Some((order_id, _)) => Err(EngineError::InvalidFill(*order_id)),
                None => Ok(()),
            },
        }
    }

    /// Replay primitive: consume `quantity` of a resting maker order.
    pub fn apply_fill(
        &mut self,
        symbol: &str,
        maker_order_id: u64,
        quantity: Decimal,
    ) -> Result<(), EngineError> {
        self.books
            .get_mut(symbol)
            .ok_or(EngineError::InvalidFill(maker_order_id))?
            .apply_fill(maker_order_id, quantity)
    }

    /// Replay primitive: place a (possibly partially filled) order on the book without matching.
    pub fn add_resting(&mut self, order: Order) -> Result<(), EngineError> {
        self.books
            .entry(order.symbol.clone())
            .or_default()
            .add_order(order)
    }

    /// Symbols in deterministic (sorted) order.
    pub fn sorted_symbols(&self) -> Vec<&str> {
        let mut symbols = self.symbols();
        symbols.sort_unstable();
        symbols
    }

    /// Resting orders of one symbol in price-time order.
    pub fn open_orders(&self, symbol: &str) -> Vec<Order> {
        self.books
            .get(symbol)
            .map(OrderBook::open_orders)
            .unwrap_or_default()
    }

    /// Return the top-`limit` depth levels for `symbol`.
    /// Returns empty vecs if no book exists for the symbol yet.
    pub fn depth_snapshot(&self, symbol: &str, limit: usize) -> DepthSnapshot {
        self.books
            .get(symbol)
            .map(|b| b.depth_snapshot(limit))
            .unwrap_or_else(|| (vec![], vec![]))
    }

    /// All symbols for which at least one order has been placed.
    pub fn symbols(&self) -> Vec<&str> {
        self.books.keys().map(String::as_str).collect()
    }

    /// All currently resting orders for a user across all symbols.
    pub fn open_orders_by_user(&self, user_id: u64) -> Vec<Order> {
        let mut out = Vec::new();

        for book in self.books.values() {
            out.extend(
                book.open_orders()
                    .into_iter()
                    .filter(|order| order.user_id == user_id),
            );
        }

        out.sort_by_key(|order| std::cmp::Reverse(order.id));
        out
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}
