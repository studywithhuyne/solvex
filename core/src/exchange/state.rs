// core/src/exchange/state.rs
// State owned by the single writer, with prepare / commit / replay (PLAN-08, PLAN-09).
//
// Design
// ──────
// * `prepare(&self, ..)` is read-only: it plans every fill against the book (`plan_match`),
//   builds ONE `ChangeSet` for the whole command (reserve, all fills, fees, refunds) and
//   dry-runs it on the ledger. Any failure returns an error and no state has been touched,
//   so there is no "late fill failed" half-applied order.
// * `commit(&mut self, ..)` and `apply_record` are the only mutation path. They apply the
//   decided deltas and book operations of a `TransactionRecord`; nothing is recomputed, so
//   live commits and journal replay run the very same code.
// * Everything is synchronous and I/O-free. A journal (PLAN-12) sits between prepare and
//   commit; the writer does not hold any lock across it.
//
// Invariants (checked by `check_invariants`, used by tests):
//   - balances are non-negative; locked == funds required by live reservations;
//   - every resting order has a reservation with the same remaining quantity;
//   - per asset, users + fee accounts == net external inflow.

use std::collections::{BTreeMap, HashMap, HashSet};

use rust_decimal::Decimal;
use thiserror::Error;

use crate::engine::{Engine, EngineError, Order, Side};
use crate::exchange::command::{Actor, Command, CommandEnvelope, MarketConfig};
use crate::exchange::record::{
    BookOp, CapitalDelta, CommittedTransaction, FeeRecord, FillRecord, OrderRecord, Outcome,
    PreparedTransaction, PriceRecord, TransactionRecord, RECORD_VERSION,
};
use crate::ledger::{
    BalanceDelta, ChangeSet, InMemoryLedger, LedgerError, Reservation, ReservationSpec,
};
use crate::money::{self, MoneyError};

/// Maximum allowed deviation from the reference price in basis points (20%).
pub const MAX_PRICE_DEVIATION_BPS: i64 = 2_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExchangeError {
    #[error(transparent)]
    Money(#[from] MoneyError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Engine(#[from] EngineError),

    #[error("market {0} not found")]
    MarketNotFound(String),
    #[error("market {0} is halted")]
    MarketHalted(String),
    #[error("order {0} not found")]
    UnknownOrder(u64),
    #[error("order {order_id} belongs to another user")]
    Forbidden { order_id: u64 },
    #[error("price {incoming} outside allowed band [{lower}, {upper}] around {reference}")]
    PriceOutOfBand {
        incoming: Decimal,
        reference: Decimal,
        lower: Decimal,
        upper: Decimal,
    },
    #[error("invalid command: {0}")]
    InvalidCommand(&'static str),
    #[error("order id space exhausted")]
    IdExhausted,

    #[error("unsupported record version {0}")]
    UnsupportedVersion(u32),
    #[error("sequence mismatch: expected {expected}, got {found}")]
    SequenceMismatch { expected: u64, found: u64 },
    #[error("record is inconsistent with state: {0}")]
    Inconsistent(String),
    #[error("state is poisoned after a failed apply; restart from journal required")]
    Poisoned,

    #[error("command queue is full")]
    QueueFull,
    #[error("command queue is closed")]
    QueueClosed,
    #[error("commit sink failed: {0}")]
    SinkFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderMeta {
    user_id: u64,
    symbol: String,
}

/// Everything the writer owns: book, ledger, ownership index, counters, market policy and
/// reference prices.
pub struct ExchangeState {
    engine: Engine,
    ledger: InMemoryLedger,
    markets: BTreeMap<String, MarketConfig>,
    /// Open-order ownership index (derived; records carry owners themselves).
    orders: HashMap<u64, OrderMeta>,
    last_trade_price: BTreeMap<String, Decimal>,
    capital: BTreeMap<String, Decimal>,
    /// Net external inflow per asset (deposits - withdrawals); transfers net to zero.
    external_flow: BTreeMap<String, Decimal>,
    next_order_id: u64,
    next_seq: u64,
    poisoned: bool,
}

impl Default for ExchangeState {
    fn default() -> Self {
        Self::new()
    }
}

impl ExchangeState {
    pub fn new() -> Self {
        Self::with_ledger(InMemoryLedger::new())
    }

    /// Starts from an existing ledger (for example rows loaded from PostgreSQL); the current
    /// totals become the baseline of the conservation check.
    pub fn with_ledger(ledger: InMemoryLedger) -> Self {
        let external_flow = ledger.totals_by_asset();
        Self {
            engine: Engine::new(),
            ledger,
            markets: BTreeMap::new(),
            orders: HashMap::new(),
            last_trade_price: BTreeMap::new(),
            capital: BTreeMap::new(),
            external_flow,
            next_order_id: 1,
            next_seq: 1,
            poisoned: false,
        }
    }

    // ── Read access ──────────────────────────────────────────────────────────

    /// Read-state version: sequence of the last committed transaction (0 = none).
    pub fn version(&self) -> u64 {
        self.next_seq - 1
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn next_order_id(&self) -> u64 {
        self.next_order_id
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn ledger(&self) -> &InMemoryLedger {
        &self.ledger
    }

    pub fn market(&self, symbol: &str) -> Option<&MarketConfig> {
        self.markets.get(symbol)
    }

    pub fn last_trade_price(&self, symbol: &str) -> Option<Decimal> {
        self.last_trade_price.get(symbol).copied()
    }

    pub fn capital(&self, asset: &str) -> Decimal {
        self.capital.get(asset).copied().unwrap_or(Decimal::ZERO)
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Owner of an open order, from the derived index.
    pub fn order_owner(&self, order_id: u64) -> Option<u64> {
        self.orders.get(&order_id).map(|meta| meta.user_id)
    }

    /// Deterministic digest of the complete state; two states with equal fingerprints are
    /// equal for every purpose of the exchange (used to compare live state with replay).
    pub fn fingerprint(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        let mut put = |text: String| {
            hasher.update(text.as_bytes());
            hasher.update(b"\n");
        };

        put(format!(
            "seq={} next_order_id={}",
            self.next_seq, self.next_order_id
        ));
        for ((user_id, asset), state) in self.ledger.balances_sorted() {
            put(format!(
                "bal {user_id} {asset} {} {}",
                state.free.normalize(),
                state.locked.normalize()
            ));
        }
        for (order_id, r) in self.ledger.reservations_sorted() {
            put(format!(
                "res {order_id} {:?} {} {} {} {} {}",
                r.side,
                r.user_id,
                r.base_asset,
                r.quote_asset,
                r.limit_price.normalize(),
                r.remaining.normalize()
            ));
        }
        for symbol in self.engine.sorted_symbols() {
            for o in self.engine.open_orders(symbol) {
                put(format!(
                    "ord {} {} {symbol} {:?} {} {} {}",
                    o.id,
                    o.user_id,
                    o.side,
                    o.price.normalize(),
                    o.amount.normalize(),
                    o.remaining.normalize()
                ));
            }
        }
        for (symbol, config) in &self.markets {
            put(format!(
                "mkt {symbol} {} {} {} {} {} {} {}",
                config.base_asset,
                config.quote_asset,
                config.active,
                config.policy.tick.normalize(),
                config.policy.lot.normalize(),
                config.policy.maker_fee_rate.normalize(),
                config.policy.taker_fee_rate.normalize()
            ));
        }
        for (symbol, price) in &self.last_trade_price {
            put(format!("px {symbol} {}", price.normalize()));
        }
        for (asset, amount) in &self.capital {
            put(format!("cap {asset} {}", amount.normalize()));
        }
        for (asset, amount) in &self.external_flow {
            put(format!("flow {asset} {}", amount.normalize()));
        }
        hasher.finalize().to_hex().to_string()
    }

    /// Verifies the accounting invariants of docs/DECISIONS.md section 5.
    pub fn check_invariants(&self) -> Result<(), String> {
        // Reservations vs locked funds.
        let reserved = self.ledger.reserved_by_account();
        for ((user_id, asset), state) in self.ledger.balances_sorted() {
            if state.free.is_sign_negative() || state.locked.is_sign_negative() {
                return Err(format!("negative balance for user {user_id} asset {asset}"));
            }
            let expected = reserved
                .get(&(user_id, asset.clone()))
                .copied()
                .unwrap_or(Decimal::ZERO);
            if state.locked != expected {
                return Err(format!(
                    "locked mismatch for user {user_id} asset {asset}: locked={} reserved={expected}",
                    state.locked
                ));
            }
        }
        for ((user_id, asset), amount) in &reserved {
            if self.ledger.balance(*user_id, asset).locked != *amount {
                return Err(format!(
                    "reservation without locked funds for user {user_id} asset {asset}"
                ));
            }
        }

        // Book vs reservations.
        let mut resting: HashSet<u64> = HashSet::new();
        for symbol in self.engine.sorted_symbols() {
            for order in self.engine.open_orders(symbol) {
                if order.remaining <= Decimal::ZERO || order.remaining > order.amount {
                    return Err(format!("order {} has invalid remaining quantity", order.id));
                }
                let reservation = self
                    .ledger
                    .reservation(order.id)
                    .ok_or_else(|| format!("order {} has no reservation", order.id))?;
                if reservation.remaining != order.remaining || reservation.user_id != order.user_id
                {
                    return Err(format!("order {} disagrees with its reservation", order.id));
                }
                resting.insert(order.id);
            }
        }
        for (order_id, _) in self.ledger.reservations_sorted() {
            if !resting.contains(&order_id) {
                return Err(format!("reservation {order_id} has no resting order"));
            }
        }

        // Conservation per asset.
        let totals = self.ledger.totals_by_asset();
        let assets: HashSet<&String> = totals.keys().chain(self.external_flow.keys()).collect();
        for asset in assets {
            let have = totals.get(asset).copied().unwrap_or(Decimal::ZERO);
            let want = self
                .external_flow
                .get(asset)
                .copied()
                .unwrap_or(Decimal::ZERO);
            if have != want {
                return Err(format!(
                    "asset {asset} not conserved: ledger={have} expected={want}"
                ));
            }
        }
        Ok(())
    }

    // ── Prepare (read-only) ──────────────────────────────────────────────────

    /// Decides the complete outcome of `envelope` without changing any state.
    pub fn prepare(
        &self,
        envelope: &CommandEnvelope,
        timestamp_ms: i64,
    ) -> Result<PreparedTransaction, ExchangeError> {
        if self.poisoned {
            return Err(ExchangeError::Poisoned);
        }

        let actor = envelope.actor;
        let mut record = TransactionRecord {
            version: RECORD_VERSION,
            seq: self.next_seq,
            timestamp_ms,
            request_key: envelope.request_key.as_str().to_string(),
            payload_digest: envelope.payload_digest(),
            actor,
            outcome: Outcome::MarketConfigured {
                symbol: String::new(),
            },
            ledger: ChangeSet::default(),
            book: Vec::new(),
            next_order_id: self.next_order_id,
            last_trade_prices: Vec::new(),
            capital: None,
            market: None,
        };

        match &envelope.command {
            Command::PlaceOrder {
                symbol,
                side,
                price,
                amount,
            } => self.prepare_place(&mut record, actor, symbol, *side, *price, *amount)?,
            Command::CancelOrder { order_id } => {
                self.prepare_cancel(&mut record, actor, *order_id)?
            }
            Command::Deposit { asset, amount } => {
                money::validate_positive(amount)?;
                record
                    .ledger
                    .deltas
                    .push(free_delta(actor.user_id, asset, *amount));
                record.outcome = Outcome::Deposited {
                    asset: asset.clone(),
                    amount: *amount,
                };
            }
            Command::Withdraw { asset, amount } => {
                money::validate_positive(amount)?;
                record
                    .ledger
                    .deltas
                    .push(free_delta(actor.user_id, asset, -*amount));
                record.outcome = Outcome::Withdrawn {
                    asset: asset.clone(),
                    amount: *amount,
                };
            }
            Command::Transfer {
                from_asset,
                to_asset,
                amount,
            } => {
                money::validate_positive(amount)?;
                if from_asset == to_asset {
                    return Err(ExchangeError::InvalidCommand("transfer assets must differ"));
                }
                record
                    .ledger
                    .deltas
                    .push(free_delta(actor.user_id, from_asset, -*amount));
                record
                    .ledger
                    .deltas
                    .push(free_delta(actor.user_id, to_asset, *amount));
                record.outcome = Outcome::Transferred {
                    from_asset: from_asset.clone(),
                    to_asset: to_asset.clone(),
                    amount: *amount,
                };
            }
            Command::AdjustCapital { asset, delta } => {
                if delta.is_zero() {
                    return Err(ExchangeError::InvalidCommand(
                        "capital delta must not be zero",
                    ));
                }
                money::validate_non_negative(&delta.abs())?;
                record.capital = Some(CapitalDelta {
                    asset: asset.clone(),
                    delta: *delta,
                });
                record.outcome = Outcome::CapitalAdjusted {
                    asset: asset.clone(),
                    delta: *delta,
                };
            }
            Command::ConfigureMarket(config) => {
                validate_market_config(config)?;
                record.market = Some(config.clone());
                record.outcome = Outcome::MarketConfigured {
                    symbol: config.symbol.clone(),
                };
            }
        }

        // The very same validation commit/replay run: if it passes here, commit cannot fail
        // as long as the state is unchanged (guarded by the sequence number).
        self.validate_record(&record)?;
        Ok(PreparedTransaction { record })
    }

    fn prepare_place(
        &self,
        record: &mut TransactionRecord,
        actor: Actor,
        symbol: &str,
        side: Side,
        price: Decimal,
        amount: Decimal,
    ) -> Result<(), ExchangeError> {
        let market = self
            .markets
            .get(symbol)
            .ok_or_else(|| ExchangeError::MarketNotFound(symbol.to_string()))?;
        if !market.active {
            return Err(ExchangeError::MarketHalted(symbol.to_string()));
        }
        market.policy.validate_order(&price, &amount)?;
        if let Some(reference) = self.reference_price(symbol) {
            validate_price_band(price, reference)?;
        }

        let order_id = self.next_order_id;
        money::id_to_sql(order_id).map_err(|_| ExchangeError::IdExhausted)?;
        let order = Order::new(order_id, actor.user_id, symbol, side, price, amount);

        let spec = ReservationSpec {
            order_id,
            user_id: actor.user_id,
            side,
            base_asset: market.base_asset.clone(),
            quote_asset: market.quote_asset.clone(),
            limit_price: price,
            amount,
        };
        let mut change = InMemoryLedger::reserve_change(&spec)?;
        let taker_reservation = Reservation {
            side,
            user_id: actor.user_id,
            base_asset: market.base_asset.clone(),
            quote_asset: market.quote_asset.clone(),
            limit_price: price,
            remaining: amount,
        };

        let planned = self.engine.plan_match(&order)?;
        let mut fills = Vec::with_capacity(planned.len());
        let mut book = Vec::with_capacity(planned.len() + 1);
        let mut remaining = amount;

        for fill in &planned {
            let trade = &fill.trade;
            let maker_reservation = self
                .ledger
                .reservation(trade.maker_order_id)
                .ok_or(LedgerError::ReservationNotFound(trade.maker_order_id))?;

            let (maker_change, maker_fee) = InMemoryLedger::leg_change(
                trade.maker_order_id,
                maker_reservation,
                &trade.amount,
                &trade.price,
                &market.policy.maker_fee_rate,
            )?;
            let (taker_change, taker_fee) = InMemoryLedger::leg_change(
                order_id,
                &taker_reservation,
                &trade.amount,
                &trade.price,
                &market.policy.taker_fee_rate,
            )?;
            change.extend(maker_change);
            change.extend(taker_change);

            remaining = money::checked_sub(remaining, trade.amount)?;
            book.push(BookOp::Fill {
                symbol: symbol.to_string(),
                maker_order_id: trade.maker_order_id,
                quantity: trade.amount,
            });
            fills.push(FillRecord {
                symbol: symbol.to_string(),
                maker_order_id: trade.maker_order_id,
                maker_user_id: fill.maker_user_id,
                taker_order_id: order_id,
                taker_user_id: actor.user_id,
                taker_side: side,
                price: trade.price,
                amount: trade.amount,
                maker_fee: FeeRecord {
                    asset: maker_fee.asset,
                    amount: maker_fee.amount,
                },
                taker_fee: FeeRecord {
                    asset: taker_fee.asset,
                    amount: taker_fee.amount,
                },
            });
        }

        // Dry-run the whole command: insufficient funds or any late settlement problem
        // rejects the order before anything is applied.
        self.ledger.validate(&change)?;

        let mut resting = order.clone();
        resting.remaining = remaining;
        let rested = !remaining.is_zero();
        if rested {
            book.push(BookOp::Rest {
                order: OrderRecord::from_order(&resting),
            });
        }

        if let Some(last) = fills.last() {
            record.last_trade_prices.push(PriceRecord {
                symbol: symbol.to_string(),
                price: last.price,
            });
        }

        record.next_order_id = order_id.checked_add(1).ok_or(ExchangeError::IdExhausted)?;
        record.ledger = change;
        record.book = book;
        record.outcome = Outcome::OrderPlaced {
            order: OrderRecord::from_order(&order),
            fills,
            rested,
        };
        Ok(())
    }

    fn prepare_cancel(
        &self,
        record: &mut TransactionRecord,
        actor: Actor,
        order_id: u64,
    ) -> Result<(), ExchangeError> {
        let meta = self
            .orders
            .get(&order_id)
            .ok_or(ExchangeError::UnknownOrder(order_id))?;
        if meta.user_id != actor.user_id {
            return Err(ExchangeError::Forbidden { order_id });
        }
        self.engine
            .get_order(&meta.symbol, order_id)
            .ok_or(ExchangeError::UnknownOrder(order_id))?;
        let reservation = self
            .ledger
            .reservation(order_id)
            .ok_or(LedgerError::ReservationNotFound(order_id))?;

        let change = InMemoryLedger::release_change(order_id, reservation)?;
        let (released_asset, released_amount) = change
            .deltas
            .first()
            .map(|d| (d.asset.clone(), d.free))
            .ok_or(ExchangeError::InvalidCommand("empty release"))?;

        record.ledger = change;
        record.book = vec![BookOp::Remove {
            symbol: meta.symbol.clone(),
            order_id,
        }];
        record.outcome = Outcome::OrderCancelled {
            order_id,
            user_id: meta.user_id,
            symbol: meta.symbol.clone(),
            released_asset,
            released_amount,
        };
        Ok(())
    }

    /// Reference for the price band: last trade, else mid / best quote of the book.
    pub fn reference_price(&self, symbol: &str) -> Option<Decimal> {
        if let Some(last) = self.last_trade_price.get(symbol) {
            return Some(*last);
        }
        let (bids, asks) = self.engine.depth_snapshot(symbol, 1);
        match (bids.first().map(|(p, _)| *p), asks.first().map(|(p, _)| *p)) {
            (Some(bid), Some(ask)) => bid.checked_add(ask).map(|sum| sum / Decimal::from(2_u64)),
            (Some(bid), None) => Some(bid),
            (None, Some(ask)) => Some(ask),
            (None, None) => None,
        }
    }

    // ── Validation shared by prepare, commit and replay ─────────────────────

    fn validate_record(&self, record: &TransactionRecord) -> Result<(), ExchangeError> {
        if record.version != RECORD_VERSION {
            return Err(ExchangeError::UnsupportedVersion(record.version));
        }
        if record.seq != self.next_seq {
            return Err(ExchangeError::SequenceMismatch {
                expected: self.next_seq,
                found: record.seq,
            });
        }
        if record.next_order_id < self.next_order_id {
            return Err(ExchangeError::Inconsistent(
                "order id counter moved backwards".into(),
            ));
        }

        self.ledger.validate(&record.ledger)?;

        // Fills grouped per symbol are validated against the book (existence, quantity, FIFO).
        let mut fills: BTreeMap<&str, Vec<(u64, Decimal)>> = BTreeMap::new();
        let mut new_orders: HashSet<u64> = HashSet::new();
        for op in &record.book {
            match op {
                BookOp::Fill {
                    symbol,
                    maker_order_id,
                    quantity,
                } => {
                    fills
                        .entry(symbol.as_str())
                        .or_default()
                        .push((*maker_order_id, *quantity));
                }
                BookOp::Rest { order } => {
                    if order.price <= Decimal::ZERO
                        || order.amount <= Decimal::ZERO
                        || order.remaining <= Decimal::ZERO
                        || order.remaining > order.amount
                    {
                        return Err(ExchangeError::Inconsistent("invalid resting order".into()));
                    }
                    if self
                        .engine
                        .get_order(&order.symbol, order.order_id)
                        .is_some()
                        || !new_orders.insert(order.order_id)
                    {
                        return Err(EngineError::DuplicateOrderId(order.order_id).into());
                    }
                }
                BookOp::Remove { symbol, order_id } => {
                    if self.engine.get_order(symbol, *order_id).is_none() {
                        return Err(ExchangeError::UnknownOrder(*order_id));
                    }
                }
            }
        }
        for (symbol, list) in &fills {
            self.engine.validate_fills(symbol, list)?;
        }

        if let Some(capital) = &record.capital {
            let next = money::checked_add(self.capital(&capital.asset), capital.delta)?;
            if next.is_sign_negative() {
                return Err(ExchangeError::InvalidCommand(
                    "exchange capital cannot become negative",
                ));
            }
        }
        if let Some(config) = &record.market {
            validate_market_config(config)?;
        }
        Ok(())
    }

    // ── Commit / replay (the only mutation path) ─────────────────────────────

    /// Applies a prepared transaction. Fails (without mutation) when the state moved since
    /// `prepare`, which the sequence number detects.
    pub fn commit(
        &mut self,
        prepared: PreparedTransaction,
    ) -> Result<CommittedTransaction, ExchangeError> {
        self.apply_record(&prepared.record)?;
        Ok(CommittedTransaction {
            record: prepared.record,
        })
    }

    /// Applies one record: used by `commit` and by journal replay.
    ///
    /// Validation runs first and leaves the state untouched on failure. If a failure still
    /// happens after mutation started (a bug or a corrupted record that passed validation),
    /// the state is marked poisoned and refuses further work.
    pub fn apply_record(&mut self, record: &TransactionRecord) -> Result<(), ExchangeError> {
        if self.poisoned {
            return Err(ExchangeError::Poisoned);
        }
        self.validate_record(record)?;

        if let Err(error) = self.ledger.apply(&record.ledger) {
            self.poisoned = true;
            return Err(ExchangeError::Inconsistent(format!(
                "ledger apply failed: {error}"
            )));
        }

        for op in &record.book {
            let result = match op {
                BookOp::Fill {
                    symbol,
                    maker_order_id,
                    quantity,
                } => {
                    let applied = self.engine.apply_fill(symbol, *maker_order_id, *quantity);
                    if applied.is_ok() && self.engine.get_order(symbol, *maker_order_id).is_none() {
                        self.orders.remove(maker_order_id);
                    }
                    applied
                }
                BookOp::Rest { order } => {
                    let applied = self.engine.add_resting(order.to_order());
                    if applied.is_ok() {
                        self.orders.insert(
                            order.order_id,
                            OrderMeta {
                                user_id: order.user_id,
                                symbol: order.symbol.clone(),
                            },
                        );
                    }
                    applied
                }
                BookOp::Remove { symbol, order_id } => {
                    let applied = self.engine.cancel_order(symbol, *order_id).map(|_| ());
                    if applied.is_ok() {
                        self.orders.remove(order_id);
                    }
                    applied
                }
            };
            if let Err(error) = result {
                self.poisoned = true;
                return Err(ExchangeError::Inconsistent(format!(
                    "book apply failed: {error}"
                )));
            }
        }

        for price in &record.last_trade_prices {
            self.last_trade_price
                .insert(price.symbol.clone(), price.price);
        }
        if let Some(capital) = &record.capital {
            // Overflow was ruled out by `validate_record`.
            let next = self
                .capital(&capital.asset)
                .checked_add(capital.delta)
                .unwrap_or(Decimal::MAX);
            self.capital.insert(capital.asset.clone(), next);
        }
        if let Some(config) = &record.market {
            self.markets.insert(config.symbol.clone(), config.clone());
        }
        self.track_external_flow(&record.outcome);

        self.next_order_id = record.next_order_id;
        self.next_seq = record.seq + 1;
        Ok(())
    }

    fn track_external_flow(&mut self, outcome: &Outcome) {
        match outcome {
            Outcome::Deposited { asset, amount } => self.add_flow(asset, *amount),
            Outcome::Withdrawn { asset, amount } => self.add_flow(asset, -*amount),
            // Transfers move value between two assets of one user and are modelled as an
            // external outflow of one asset and inflow of the other (mock 1:1 conversion).
            Outcome::Transferred {
                from_asset,
                to_asset,
                amount,
            } => {
                self.add_flow(from_asset, -*amount);
                self.add_flow(to_asset, *amount);
            }
            _ => {}
        }
    }

    fn add_flow(&mut self, asset: &str, delta: Decimal) {
        let entry = self
            .external_flow
            .entry(asset.to_string())
            .or_insert(Decimal::ZERO);
        // Amounts are bounded by the money policy; saturate instead of panicking and let
        // `check_invariants` report any resulting mismatch.
        *entry = entry.checked_add(delta).unwrap_or(Decimal::MAX);
    }
}

fn free_delta(user_id: u64, asset: &str, free: Decimal) -> BalanceDelta {
    BalanceDelta {
        user_id,
        asset: asset.to_string(),
        free,
        locked: Decimal::ZERO,
    }
}

fn validate_market_config(config: &MarketConfig) -> Result<(), ExchangeError> {
    config.policy.validate()?;
    let expected = format!("{}_{}", config.base_asset, config.quote_asset);
    if config.base_asset.is_empty() || config.quote_asset.is_empty() || config.symbol != expected {
        return Err(ExchangeError::InvalidCommand(
            "market symbol must be BASE_QUOTE",
        ));
    }
    Ok(())
}

fn validate_price_band(incoming: Decimal, reference: Decimal) -> Result<(), ExchangeError> {
    if reference <= Decimal::ZERO {
        return Ok(());
    }
    let bps = Decimal::from(MAX_PRICE_DEVIATION_BPS) / Decimal::from(10_000_u64);
    let lower = reference * (Decimal::ONE - bps);
    let upper = reference * (Decimal::ONE + bps);
    if incoming < lower || incoming > upper {
        return Err(ExchangeError::PriceOutOfBand {
            incoming,
            reference,
            lower,
            upper,
        });
    }
    Ok(())
}
