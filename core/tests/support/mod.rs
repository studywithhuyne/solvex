use std::collections::BTreeMap;

use matching_engine::{engine::Order, ledger::InMemoryLedger};
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub balances: BTreeMap<(u64, String), (Decimal, Decimal)>,
    pub fees: BTreeMap<String, Decimal>,
    pub orders: Vec<Order>,
}

pub fn observe(ledger: &InMemoryLedger, orders: Vec<Order>) -> Observation {
    let mut balances = BTreeMap::new();
    for user in 1..=3 {
        for balance in ledger.balances_for_user(user) {
            balances.insert((user, balance.asset), (balance.free, balance.locked));
        }
    }
    Observation {
        balances,
        fees: ["BTC", "USDT", "ETH"]
            .into_iter()
            .map(|asset| (asset.to_owned(), ledger.exchange_revenue_by_asset(asset)))
            .collect(),
        orders,
    }
}

pub fn totals(state: &Observation) -> BTreeMap<String, Decimal> {
    let mut sums = state.fees.clone();
    for ((_, asset), (free, locked)) in &state.balances {
        *sums.entry(asset.clone()).or_default() += *free + *locked;
    }
    sums
}

/// Independent accounting oracle: derive reservations from resting orders.
pub fn check_accounting(
    state: &Observation,
    expected: &BTreeMap<String, Decimal>,
) -> Result<(), String> {
    let mut locks: BTreeMap<(u64, String), Decimal> = BTreeMap::new();
    for order in &state.orders {
        if order.remaining <= Decimal::ZERO || order.remaining > order.amount {
            return Err("invalid remaining quantity".into());
        }
        let (base, quote) = order.symbol.split_once('_').ok_or("invalid market")?;
        let (asset, amount) = match order.side {
            matching_engine::engine::Side::Buy => (quote, order.remaining * order.price),
            matching_engine::engine::Side::Sell => (base, order.remaining),
        };
        *locks.entry((order.user_id, asset.into())).or_default() += amount;
    }
    for (key, (free, locked)) in &state.balances {
        if *free < Decimal::ZERO || *locked < Decimal::ZERO {
            return Err("negative balance".into());
        }
        if *locked != locks.remove(key).unwrap_or_default() {
            return Err("reservation mismatch".into());
        }
    }
    if !locks.is_empty() || state.fees.values().any(|fee| *fee < Decimal::ZERO) {
        return Err("missing account or negative fees".into());
    }
    if totals(state) != *expected {
        return Err("asset conservation mismatch".into());
    }
    Ok(())
}

/// Order volume is separate from financial conservation.
pub fn check_volume(submitted: Decimal, filled: Decimal, resting: Decimal) -> bool {
    submitted == filled + resting && filled >= Decimal::ZERO && resting >= Decimal::ZERO
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultBoundary {
    SqlStatement,
    JournalWrite,
    JournalSync,
    SnapshotPublish,
    BeforeAck,
}

/// Scripted injector for future storage adapters; not a runtime crash simulation.
pub struct FaultScript {
    pub stop_at: FaultBoundary,
}

impl FaultScript {
    pub fn visit(&self, boundary: FaultBoundary) -> Result<(), FaultBoundary> {
        if self.stop_at == boundary {
            Err(boundary)
        } else {
            Ok(())
        }
    }
}
