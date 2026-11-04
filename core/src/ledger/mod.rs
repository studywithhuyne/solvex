use std::collections::HashMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::engine::{Order, Side, Trade};
use crate::money::{self, MoneyError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BalanceState {
    pub free: Decimal,
    pub locked: Decimal,
}

impl BalanceState {
    const ZERO: Self = Self {
        free: Decimal::ZERO,
        locked: Decimal::ZERO,
    };
}

#[derive(Debug, Clone)]
pub struct UserBalanceSnapshot {
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}

/// Live reservation backing one resting order (locked funds for its remaining quantity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub side: Side,
    pub user_id: u64,
    pub base_asset: String,
    pub quote_asset: String,
    pub limit_price: Decimal,
    pub remaining: Decimal,
}

/// Everything needed to create a reservation; stored in transaction records so replay
/// never has to recompute it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationSpec {
    pub order_id: u64,
    pub user_id: u64,
    pub side: Side,
    pub base_asset: String,
    pub quote_asset: String,
    #[serde(with = "crate::money::dec_str")]
    pub limit_price: Decimal,
    #[serde(with = "crate::money::dec_str")]
    pub amount: Decimal,
}

/// Signed change to one account. Deltas are decided once at prepare time and are only
/// applied (never recomputed) by commit and replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceDelta {
    pub user_id: u64,
    pub asset: String,
    #[serde(with = "crate::money::dec_str")]
    pub free: Decimal,
    #[serde(with = "crate::money::dec_str")]
    pub locked: Decimal,
}

/// Quantity taken out of one reservation (fill or release).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationConsume {
    pub order_id: u64,
    #[serde(with = "crate::money::dec_str")]
    pub quantity: Decimal,
}

/// Atomic unit of ledger change: balance deltas, reservation consumption and creation.
///
/// Application order inside `apply`: `adds` first, then `consumes`, then deltas, so one set
/// may create a reservation and consume it again (an order that fills immediately).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    pub deltas: Vec<BalanceDelta>,
    /// Quantities removed from reservations' remaining; a reservation is removed at zero.
    pub consumes: Vec<ReservationConsume>,
    pub adds: Vec<ReservationSpec>,
}

impl ChangeSet {
    pub fn extend(&mut self, other: ChangeSet) {
        self.deltas.extend(other.deltas);
        self.consumes.extend(other.consumes);
        self.adds.extend(other.adds);
    }
}

/// Fee charged on one settlement leg, in the received asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeCharge {
    pub asset: String,
    pub amount: Decimal,
}

const EXCHANGE_REVENUE_ACCOUNT: &str = "EXCHANGE_REVENUE";
const EXCHANGE_REVENUE_USER_ID: u64 = 0;

/// Synthetic per-asset account that receives fees (counted in conservation checks).
pub fn revenue_account_asset(asset: &str) -> String {
    format!("{}:{}", EXCHANGE_REVENUE_ACCOUNT, asset)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LedgerError {
    #[error("insufficient free balance: user_id={user_id}, asset={asset}, required={required}, available={available}")]
    InsufficientFreeBalance {
        user_id: u64,
        asset: String,
        required: Decimal,
        available: Decimal,
    },

    #[error("insufficient locked balance: user_id={user_id}, asset={asset}, required={required}, available={available}")]
    InsufficientLockedBalance {
        user_id: u64,
        asset: String,
        required: Decimal,
        available: Decimal,
    },

    #[error("order reservation not found: order_id={0}")]
    ReservationNotFound(u64),

    #[error("reservation already exists: order_id={0}")]
    ReservationExists(u64),

    #[error("reservation underflow: order_id={order_id}, remaining={remaining}, fill={fill}")]
    ReservationUnderflow {
        order_id: u64,
        remaining: Decimal,
        fill: Decimal,
    },

    #[error("invalid user id in balances snapshot")]
    InvalidUserId,

    #[error("invalid balance in snapshot: user_id={user_id}, asset={asset}")]
    InvalidBalance { user_id: u64, asset: String },

    #[error("settlement failed: {0}")]
    SettlementFailed(String),

    #[error(transparent)]
    Money(#[from] MoneyError),
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryLedger {
    balances: HashMap<(u64, String), BalanceState>,
    reservations: HashMap<u64, Reservation>,
}

/// Result of staging a `ChangeSet` without touching the ledger.
struct Staged {
    balances: HashMap<(u64, String), BalanceState>,
    /// `Some` = new/updated reservation, `None` = removed.
    reservations: HashMap<u64, Option<Reservation>>,
}

impl InMemoryLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rows(rows: &[(i64, String, Decimal, Decimal)]) -> Result<Self, LedgerError> {
        let mut ledger = Self::new();

        for (user_id_i64, asset, free, locked) in rows {
            let user_id =
                money::id_from_sql(*user_id_i64).map_err(|_| LedgerError::InvalidUserId)?;
            // A persisted balance must already satisfy the amount policy (scale <= 8, >= 0).
            if money::validate_non_negative(free).is_err()
                || money::validate_non_negative(locked).is_err()
            {
                return Err(LedgerError::InvalidBalance {
                    user_id,
                    asset: asset.clone(),
                });
            }
            ledger.balances.insert(
                (user_id, asset.clone()),
                BalanceState {
                    free: *free,
                    locked: *locked,
                },
            );
        }

        Ok(ledger)
    }

    // ── Pure change-set builders (no state access) ───────────────────────────

    /// Change set that reserves funds for a new resting order.
    ///
    /// Buy locks `price * amount` quote (exactly representable, checked); sell locks `amount` base.
    pub fn reserve_change(spec: &ReservationSpec) -> Result<ChangeSet, LedgerError> {
        let (asset, lock_amount) = match spec.side {
            Side::Buy => (
                spec.quote_asset.clone(),
                money::checked_notional(&spec.limit_price, &spec.amount)?,
            ),
            Side::Sell => {
                money::validate_positive(&spec.amount)?;
                (spec.base_asset.clone(), spec.amount)
            }
        };
        money::validate_positive(&spec.limit_price)?;

        Ok(ChangeSet {
            deltas: vec![BalanceDelta {
                user_id: spec.user_id,
                asset,
                free: -lock_amount,
                locked: lock_amount,
            }],
            consumes: Vec::new(),
            adds: vec![spec.clone()],
        })
    }

    /// Change set that releases whatever a reservation still locks.
    pub fn release_change(
        order_id: u64,
        reservation: &Reservation,
    ) -> Result<ChangeSet, LedgerError> {
        let (asset, amount) = match reservation.side {
            Side::Buy => (
                reservation.quote_asset.clone(),
                money::checked_notional(&reservation.limit_price, &reservation.remaining)?,
            ),
            Side::Sell => (reservation.base_asset.clone(), reservation.remaining),
        };
        Ok(ChangeSet {
            deltas: vec![BalanceDelta {
                user_id: reservation.user_id,
                asset,
                free: amount,
                locked: -amount,
            }],
            consumes: vec![ReservationConsume {
                order_id,
                quantity: reservation.remaining,
            }],
            adds: Vec::new(),
        })
    }

    /// Change set for one settlement leg (maker or taker) of a fill.
    ///
    /// Fees are charged on the received asset and rounded toward zero once (`money::fee_amount`);
    /// buyers are refunded the price improvement `limit * qty - exec * qty`.
    pub fn leg_change(
        order_id: u64,
        reservation: &Reservation,
        fill_qty: &Decimal,
        exec_price: &Decimal,
        fee_rate: &Decimal,
    ) -> Result<(ChangeSet, FeeCharge), LedgerError> {
        money::validate_positive(fill_qty)?;
        money::validate_positive(exec_price)?;
        money::validate_fee_rate(fee_rate)?;

        let mut deltas = Vec::with_capacity(3);
        let fee = match reservation.side {
            Side::Buy => {
                let reserved_quote = money::checked_notional(&reservation.limit_price, fill_qty)?;
                let spent_quote = money::checked_notional(exec_price, fill_qty)?;
                let refund = money::checked_sub(reserved_quote, spent_quote)?;
                if refund.is_sign_negative() {
                    return Err(LedgerError::SettlementFailed(format!(
                        "execution price {exec_price} exceeds limit price {} of order {order_id}",
                        reservation.limit_price
                    )));
                }

                deltas.push(BalanceDelta {
                    user_id: reservation.user_id,
                    asset: reservation.quote_asset.clone(),
                    free: refund,
                    locked: -reserved_quote,
                });

                let fee_amount = money::fee_amount(fill_qty, fee_rate)?;
                deltas.push(BalanceDelta {
                    user_id: reservation.user_id,
                    asset: reservation.base_asset.clone(),
                    free: money::checked_sub(*fill_qty, fee_amount)?,
                    locked: Decimal::ZERO,
                });
                FeeCharge {
                    asset: reservation.base_asset.clone(),
                    amount: fee_amount,
                }
            }
            Side::Sell => {
                let proceeds = money::checked_notional(exec_price, fill_qty)?;
                deltas.push(BalanceDelta {
                    user_id: reservation.user_id,
                    asset: reservation.base_asset.clone(),
                    free: Decimal::ZERO,
                    locked: -*fill_qty,
                });

                let fee_amount = money::fee_amount(&proceeds, fee_rate)?;
                deltas.push(BalanceDelta {
                    user_id: reservation.user_id,
                    asset: reservation.quote_asset.clone(),
                    free: money::checked_sub(proceeds, fee_amount)?,
                    locked: Decimal::ZERO,
                });
                FeeCharge {
                    asset: reservation.quote_asset.clone(),
                    amount: fee_amount,
                }
            }
        };

        if fee.amount > Decimal::ZERO {
            deltas.push(BalanceDelta {
                user_id: EXCHANGE_REVENUE_USER_ID,
                asset: revenue_account_asset(&fee.asset),
                free: fee.amount,
                locked: Decimal::ZERO,
            });
        }

        Ok((
            ChangeSet {
                deltas,
                consumes: vec![ReservationConsume {
                    order_id,
                    quantity: *fill_qty,
                }],
                adds: Vec::new(),
            },
            fee,
        ))
    }

    // ── Staging / atomic apply ───────────────────────────────────────────────

    fn balance_or_zero(
        &self,
        staged: &HashMap<(u64, String), BalanceState>,
        key: &(u64, String),
    ) -> BalanceState {
        staged
            .get(key)
            .or_else(|| self.balances.get(key))
            .copied()
            .unwrap_or(BalanceState::ZERO)
    }

    /// Computes the result of `change` without mutating the ledger. Any failure leaves the
    /// ledger untouched; only the final non-negative state of each touched account is checked.
    fn stage(&self, change: &ChangeSet) -> Result<Staged, LedgerError> {
        let mut reservations: HashMap<u64, Option<Reservation>> = HashMap::new();

        for spec in &change.adds {
            let exists = match reservations.get(&spec.order_id) {
                Some(entry) => entry.is_some(),
                None => self.reservations.contains_key(&spec.order_id),
            };
            if exists {
                return Err(LedgerError::ReservationExists(spec.order_id));
            }
            reservations.insert(
                spec.order_id,
                Some(Reservation {
                    side: spec.side,
                    user_id: spec.user_id,
                    base_asset: spec.base_asset.clone(),
                    quote_asset: spec.quote_asset.clone(),
                    limit_price: spec.limit_price,
                    remaining: spec.amount,
                }),
            );
        }

        for consume in &change.consumes {
            let (order_id, quantity) = (&consume.order_id, &consume.quantity);
            let current = match reservations.get(order_id) {
                Some(Some(reservation)) => reservation.clone(),
                Some(None) => return Err(LedgerError::ReservationNotFound(*order_id)),
                None => self
                    .reservations
                    .get(order_id)
                    .cloned()
                    .ok_or(LedgerError::ReservationNotFound(*order_id))?,
            };
            if *quantity > current.remaining {
                return Err(LedgerError::ReservationUnderflow {
                    order_id: *order_id,
                    remaining: current.remaining,
                    fill: *quantity,
                });
            }
            let mut next = current;
            next.remaining = money::checked_sub(next.remaining, *quantity)?;
            if next.remaining.is_zero() {
                reservations.insert(*order_id, None);
            } else {
                reservations.insert(*order_id, Some(next));
            }
        }

        let mut balances: HashMap<(u64, String), BalanceState> = HashMap::new();
        for delta in &change.deltas {
            let key = (delta.user_id, delta.asset.clone());
            let current = self.balance_or_zero(&balances, &key);
            let next = BalanceState {
                free: money::checked_add(current.free, delta.free)?,
                locked: money::checked_add(current.locked, delta.locked)?,
            };
            balances.insert(key, next);
        }

        for ((user_id, asset), state) in &balances {
            let before = self
                .balances
                .get(&(*user_id, asset.clone()))
                .copied()
                .unwrap_or(BalanceState::ZERO);
            if state.free.is_sign_negative() {
                return Err(LedgerError::InsufficientFreeBalance {
                    user_id: *user_id,
                    asset: asset.clone(),
                    required: before.free.checked_sub(state.free).unwrap_or(Decimal::MAX),
                    available: before.free,
                });
            }
            if state.locked.is_sign_negative() {
                return Err(LedgerError::InsufficientLockedBalance {
                    user_id: *user_id,
                    asset: asset.clone(),
                    required: before
                        .locked
                        .checked_sub(state.locked)
                        .unwrap_or(Decimal::MAX),
                    available: before.locked,
                });
            }
        }

        Ok(Staged {
            balances,
            reservations,
        })
    }

    /// Dry run: verifies that `change` can be applied, without mutating anything.
    pub fn validate(&self, change: &ChangeSet) -> Result<(), LedgerError> {
        self.stage(change).map(|_| ())
    }

    /// Atomically applies `change`: either everything is applied or nothing is.
    pub fn apply(&mut self, change: &ChangeSet) -> Result<(), LedgerError> {
        let staged = self.stage(change)?;
        for (key, state) in staged.balances {
            self.balances.insert(key, state);
        }
        for (order_id, reservation) in staged.reservations {
            match reservation {
                Some(reservation) => {
                    self.reservations.insert(order_id, reservation);
                }
                None => {
                    self.reservations.remove(&order_id);
                }
            }
        }
        Ok(())
    }

    // ── Read access ──────────────────────────────────────────────────────────

    pub fn balance(&self, user_id: u64, asset: &str) -> BalanceState {
        self.balances
            .get(&(user_id, asset.to_string()))
            .copied()
            .unwrap_or(BalanceState::ZERO)
    }

    pub fn reservation(&self, order_id: u64) -> Option<&Reservation> {
        self.reservations.get(&order_id)
    }

    /// Sum of `free + locked` per asset over every account, including fee accounts.
    pub fn totals_by_asset(&self) -> std::collections::BTreeMap<String, Decimal> {
        let mut totals = std::collections::BTreeMap::new();
        for ((_, asset), state) in &self.balances {
            let key = asset
                .strip_prefix(&format!("{EXCHANGE_REVENUE_ACCOUNT}:"))
                .unwrap_or(asset)
                .to_string();
            let entry = totals.entry(key).or_insert(Decimal::ZERO);
            let account = state.free.checked_add(state.locked).unwrap_or(Decimal::MAX);
            *entry = entry.checked_add(account).unwrap_or(Decimal::MAX);
        }
        totals
    }

    /// Locked funds required by live reservations, per `(user, asset)`.
    pub fn reserved_by_account(&self) -> std::collections::BTreeMap<(u64, String), Decimal> {
        let mut out = std::collections::BTreeMap::new();
        for reservation in self.reservations.values() {
            let (asset, amount) = match reservation.side {
                Side::Buy => (
                    reservation.quote_asset.clone(),
                    reservation
                        .limit_price
                        .checked_mul(reservation.remaining)
                        .unwrap_or(Decimal::MAX),
                ),
                Side::Sell => (reservation.base_asset.clone(), reservation.remaining),
            };
            let entry = out
                .entry((reservation.user_id, asset))
                .or_insert(Decimal::ZERO);
            *entry = entry.checked_add(amount).unwrap_or(Decimal::MAX);
        }
        out
    }

    /// Every account in deterministic order (used for state fingerprints and snapshots).
    pub fn balances_sorted(&self) -> Vec<((u64, String), BalanceState)> {
        let mut out: Vec<_> = self.balances.iter().map(|(k, v)| (k.clone(), *v)).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Every live reservation in deterministic order.
    pub fn reservations_sorted(&self) -> Vec<(u64, Reservation)> {
        let mut out: Vec<_> = self
            .reservations
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        out.sort_by_key(|(order_id, _)| *order_id);
        out
    }

    pub fn balances_for_user(&self, user_id: u64) -> Vec<UserBalanceSnapshot> {
        let mut out: Vec<UserBalanceSnapshot> = self
            .balances
            .iter()
            .filter_map(|((uid, asset), state)| {
                if *uid != user_id {
                    return None;
                }
                Some(UserBalanceSnapshot {
                    asset: asset.clone(),
                    free: state.free,
                    locked: state.locked,
                })
            })
            .collect();

        out.sort_by(|a, b| a.asset.cmp(&b.asset));
        out
    }

    pub fn exchange_revenue_by_asset(&self, asset: &str) -> Decimal {
        self.balance(EXCHANGE_REVENUE_USER_ID, &revenue_account_asset(asset))
            .free
    }

    // ── Compatibility entry points (all go through `ChangeSet`) ──────────────

    pub fn reserve_for_new_order(
        &mut self,
        order: &Order,
        base_asset: &str,
        quote_asset: &str,
    ) -> Result<(), LedgerError> {
        let spec = ReservationSpec {
            order_id: order.id,
            user_id: order.user_id,
            side: order.side,
            base_asset: base_asset.to_string(),
            quote_asset: quote_asset.to_string(),
            limit_price: order.price,
            amount: order.amount,
        };
        let change = Self::reserve_change(&spec)?;
        self.apply(&change)
    }

    pub fn cancel_reservation(&mut self, order_id: u64) -> Result<(), LedgerError> {
        let reservation = self
            .reservations
            .get(&order_id)
            .ok_or(LedgerError::ReservationNotFound(order_id))?;
        let change = Self::release_change(order_id, reservation)?;
        self.apply(&change)
    }

    pub fn apply_trade_fill(&mut self, trade: &Trade) -> Result<(), LedgerError> {
        self.settle_trade(trade, Decimal::ZERO, Decimal::ZERO)
            .map_err(LedgerError::SettlementFailed)
    }

    /// Settle a trade using maker-taker fees.
    ///
    /// Fees are charged from the received asset:
    /// - buyer pays in base asset received
    /// - seller pays in quote asset received
    ///
    /// Settlement is atomic: no state is committed unless the entire operation succeeds.
    pub fn settle_trade(
        &mut self,
        trade: &Trade,
        maker_fee_rate: Decimal,
        taker_fee_rate: Decimal,
    ) -> Result<(), String> {
        if maker_fee_rate.is_sign_negative() || taker_fee_rate.is_sign_negative() {
            return Err("fee rates must be non-negative".to_string());
        }
        if maker_fee_rate > Decimal::ONE || taker_fee_rate > Decimal::ONE {
            return Err("fee rates must be <= 1".to_string());
        }

        let change = self
            .trade_change(trade, &maker_fee_rate, &taker_fee_rate)
            .map_err(|e| e.to_string())?;
        self.apply(&change).map_err(|e| e.to_string())
    }

    fn trade_change(
        &self,
        trade: &Trade,
        maker_fee_rate: &Decimal,
        taker_fee_rate: &Decimal,
    ) -> Result<ChangeSet, LedgerError> {
        let maker = self
            .reservations
            .get(&trade.maker_order_id)
            .ok_or(LedgerError::ReservationNotFound(trade.maker_order_id))?;
        let taker = self
            .reservations
            .get(&trade.taker_order_id)
            .ok_or(LedgerError::ReservationNotFound(trade.taker_order_id))?;
        let (mut change, _) = Self::leg_change(
            trade.maker_order_id,
            maker,
            &trade.amount,
            &trade.price,
            maker_fee_rate,
        )?;
        let (taker_change, _) = Self::leg_change(
            trade.taker_order_id,
            taker,
            &trade.amount,
            &trade.price,
            taker_fee_rate,
        )?;
        change.extend(taker_change);
        Ok(change)
    }

    pub fn deposit(
        &mut self,
        user_id: u64,
        asset: &str,
        amount: Decimal,
    ) -> Result<Decimal, LedgerError> {
        money::validate_positive(&amount)?;
        self.apply(&ChangeSet {
            deltas: vec![BalanceDelta {
                user_id,
                asset: asset.to_string(),
                free: amount,
                locked: Decimal::ZERO,
            }],
            ..ChangeSet::default()
        })?;
        Ok(self.balance(user_id, asset).free)
    }

    pub fn withdraw(
        &mut self,
        user_id: u64,
        asset: &str,
        amount: Decimal,
    ) -> Result<Decimal, LedgerError> {
        money::validate_positive(&amount)?;
        self.apply(&ChangeSet {
            deltas: vec![BalanceDelta {
                user_id,
                asset: asset.to_string(),
                free: -amount,
                locked: Decimal::ZERO,
            }],
            ..ChangeSet::default()
        })
        .map_err(|e| match e {
            LedgerError::InsufficientFreeBalance { .. } => LedgerError::InsufficientFreeBalance {
                user_id,
                asset: asset.to_string(),
                required: amount,
                available: self.balance(user_id, asset).free,
            },
            other => other,
        })?;
        Ok(self.balance(user_id, asset).free)
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn reserve_and_cancel_buy_moves_between_free_and_locked() {
        let rows = vec![(1_i64, "USDT".to_string(), dec!(1000), dec!(0))];
        let mut ledger = InMemoryLedger::from_rows(&rows).unwrap();
        let order = Order::new(10, 1, "BTC_USDT", Side::Buy, dec!(100), dec!(2));

        ledger.reserve_for_new_order(&order, "BTC", "USDT").unwrap();
        let usdt_after_reserve = ledger
            .balances_for_user(1)
            .into_iter()
            .find(|b| b.asset == "USDT")
            .unwrap();
        assert_eq!(usdt_after_reserve.free, dec!(800));
        assert_eq!(usdt_after_reserve.locked, dec!(200));

        ledger.cancel_reservation(10).unwrap();
        let usdt_after_cancel = ledger
            .balances_for_user(1)
            .into_iter()
            .find(|b| b.asset == "USDT")
            .unwrap();
        assert_eq!(usdt_after_cancel.free, dec!(1000));
        assert_eq!(usdt_after_cancel.locked, dec!(0));
    }

    #[test]
    fn fill_buy_trade_refunds_price_improvement() {
        let rows = vec![
            (1_i64, "USDT".to_string(), dec!(1000), dec!(0)),
            (1_i64, "BTC".to_string(), dec!(0), dec!(0)),
            (2_i64, "BTC".to_string(), dec!(2), dec!(0)),
            (2_i64, "USDT".to_string(), dec!(0), dec!(0)),
        ];
        let mut ledger = InMemoryLedger::from_rows(&rows).unwrap();

        let taker = Order::new(100, 1, "BTC_USDT", Side::Buy, dec!(100), dec!(1));
        let maker = Order::new(99, 2, "BTC_USDT", Side::Sell, dec!(95), dec!(1));

        ledger.reserve_for_new_order(&maker, "BTC", "USDT").unwrap();
        ledger.reserve_for_new_order(&taker, "BTC", "USDT").unwrap();

        let trade = Trade {
            maker_order_id: 99,
            taker_order_id: 100,
            symbol: "BTC_USDT".to_string(),
            price: dec!(95),
            amount: dec!(1),
        };

        ledger.apply_trade_fill(&trade).unwrap();

        let buyer_usdt = ledger
            .balances_for_user(1)
            .into_iter()
            .find(|b| b.asset == "USDT")
            .unwrap();
        let buyer_btc = ledger
            .balances_for_user(1)
            .into_iter()
            .find(|b| b.asset == "BTC")
            .unwrap();

        assert_eq!(buyer_usdt.locked, dec!(0));
        assert_eq!(buyer_usdt.free, dec!(905));
        assert_eq!(buyer_btc.free, dec!(1));
    }

    #[test]
    fn withdraw_decreases_free_balance() {
        let rows = vec![(1_i64, "USDT".to_string(), dec!(1000), dec!(0))];
        let mut ledger = InMemoryLedger::from_rows(&rows).unwrap();

        let new_available = ledger.withdraw(1, "USDT", dec!(250)).unwrap();
        assert_eq!(new_available, dec!(750));

        let usdt = ledger
            .balances_for_user(1)
            .into_iter()
            .find(|b| b.asset == "USDT")
            .unwrap();

        assert_eq!(usdt.free, dec!(750));
        assert_eq!(usdt.locked, dec!(0));
    }

    #[test]
    fn withdraw_rejects_when_amount_exceeds_free_balance() {
        let rows = vec![(1_i64, "USDT".to_string(), dec!(100), dec!(0))];
        let mut ledger = InMemoryLedger::from_rows(&rows).unwrap();

        let err = ledger.withdraw(1, "USDT", dec!(150)).unwrap_err();
        assert!(matches!(
            err,
            LedgerError::InsufficientFreeBalance {
                user_id: 1,
                ref asset,
                required,
                available,
            } if asset == "USDT" && required == dec!(150) && available == dec!(100)
        ));
    }

    #[test]
    fn settle_trade_charges_maker_taker_fees_on_received_assets() {
        let rows = vec![
            (1_i64, "USDT".to_string(), dec!(1000), dec!(0)),
            (1_i64, "BTC".to_string(), dec!(0), dec!(0)),
            (2_i64, "BTC".to_string(), dec!(2), dec!(0)),
            (2_i64, "USDT".to_string(), dec!(0), dec!(0)),
        ];
        let mut ledger = InMemoryLedger::from_rows(&rows).unwrap();

        // maker sells, taker buys
        let taker = Order::new(100, 1, "BTC_USDT", Side::Buy, dec!(105), dec!(1));
        let maker = Order::new(99, 2, "BTC_USDT", Side::Sell, dec!(100), dec!(1));

        ledger.reserve_for_new_order(&maker, "BTC", "USDT").unwrap();
        ledger.reserve_for_new_order(&taker, "BTC", "USDT").unwrap();

        let trade = Trade {
            maker_order_id: 99,
            taker_order_id: 100,
            symbol: "BTC_USDT".to_string(),
            price: dec!(100),
            amount: dec!(1),
        };

        ledger
            .settle_trade(&trade, dec!(0.001), dec!(0.002))
            .unwrap();

        let buyer_usdt = ledger
            .balances_for_user(1)
            .into_iter()
            .find(|b| b.asset == "USDT")
            .unwrap();
        let buyer_btc = ledger
            .balances_for_user(1)
            .into_iter()
            .find(|b| b.asset == "BTC")
            .unwrap();
        let seller_usdt = ledger
            .balances_for_user(2)
            .into_iter()
            .find(|b| b.asset == "USDT")
            .unwrap();

        // buyer reserved 105 USDT, spent 100, refunded 5; receives 1 BTC minus taker fee 0.002 BTC
        assert_eq!(buyer_usdt.free, dec!(900));
        assert_eq!(buyer_usdt.locked, dec!(0));
        assert_eq!(buyer_btc.free, dec!(0.998));

        // seller receives 100 USDT minus maker fee 0.1 USDT
        assert_eq!(seller_usdt.free, dec!(99.9));

        let revenue_usdt = ledger
            .balances
            .get(&(
                EXCHANGE_REVENUE_USER_ID,
                "EXCHANGE_REVENUE:USDT".to_string(),
            ))
            .copied()
            .unwrap();
        let revenue_btc = ledger
            .balances
            .get(&(EXCHANGE_REVENUE_USER_ID, "EXCHANGE_REVENUE:BTC".to_string()))
            .copied()
            .unwrap();

        assert_eq!(revenue_usdt.free, dec!(0.1));
        assert_eq!(revenue_btc.free, dec!(0.002));
    }

    #[test]
    fn settle_trade_is_atomic_when_validation_fails() {
        let rows = vec![
            (1_i64, "USDT".to_string(), dec!(1000), dec!(0)),
            (1_i64, "BTC".to_string(), dec!(0), dec!(0)),
            (2_i64, "BTC".to_string(), dec!(2), dec!(0)),
            (2_i64, "USDT".to_string(), dec!(0), dec!(0)),
        ];
        let mut ledger = InMemoryLedger::from_rows(&rows).unwrap();

        let taker = Order::new(100, 1, "BTC_USDT", Side::Buy, dec!(100), dec!(1));
        let maker = Order::new(99, 2, "BTC_USDT", Side::Sell, dec!(100), dec!(1));

        ledger.reserve_for_new_order(&maker, "BTC", "USDT").unwrap();
        ledger.reserve_for_new_order(&taker, "BTC", "USDT").unwrap();

        let balances_before = ledger.balances.clone();
        let reservations_before = ledger.reservations.clone();

        let trade = Trade {
            maker_order_id: 99,
            taker_order_id: 100,
            symbol: "BTC_USDT".to_string(),
            price: dec!(100),
            amount: dec!(1),
        };

        let err = ledger
            .settle_trade(&trade, dec!(0.01), dec!(1.2))
            .unwrap_err();
        assert!(err.contains("fee rates must be <= 1"));
        assert_eq!(ledger.balances, balances_before);
        assert_eq!(ledger.reservations, reservations_before);
    }
}

#[cfg(test)]
mod money_policy_tests {
    use rust_decimal_macros::dec;

    use super::*;

    fn funded() -> InMemoryLedger {
        InMemoryLedger::from_rows(&[
            (1, "USDT".to_string(), dec!(1000), dec!(0)),
            (1, "BTC".to_string(), dec!(0), dec!(0)),
            (2, "BTC".to_string(), dec!(2), dec!(0)),
            (2, "USDT".to_string(), dec!(0), dec!(0)),
        ])
        .unwrap()
    }

    #[test]
    fn invalid_amounts_are_rejected_before_any_state_change() {
        let mut ledger = funded();
        let before = ledger.balances_sorted();
        for bad in [dec!(-1), dec!(0), dec!(0.000000001), dec!(10000000000.00000001)] {
            assert!(matches!(ledger.deposit(1, "USDT", bad), Err(LedgerError::Money(_))));
            assert!(matches!(ledger.withdraw(1, "USDT", bad), Err(LedgerError::Money(_))));
        }
        assert_eq!(ledger.balances_sorted(), before);
    }

    #[test]
    fn snapshot_rows_must_follow_the_money_policy() {
        let negative = InMemoryLedger::from_rows(&[(1, "USDT".to_string(), dec!(-1), dec!(0))]);
        assert!(matches!(negative, Err(LedgerError::InvalidBalance { .. })));
        let precise = InMemoryLedger::from_rows(&[(1, "USDT".to_string(), dec!(0.000000001), dec!(0))]);
        assert!(matches!(precise, Err(LedgerError::InvalidBalance { .. })));
        let bad_id = InMemoryLedger::from_rows(&[(0, "USDT".to_string(), dec!(1), dec!(0))]);
        assert_eq!(bad_id.unwrap_err(), LedgerError::InvalidUserId);
    }

    #[test]
    fn rejected_fee_dust_stays_with_the_receiver_and_assets_are_conserved() {
        let mut ledger = funded();
        let before = ledger.totals_by_asset();

        // Fee on 0.00000009 BTC at 0.2% is 1.8e-10, which rounds toward zero: no dust is lost.
        let maker = Order::new(1, 2, "BTC_USDT", Side::Sell, dec!(1), dec!(0.00000009));
        let taker = Order::new(2, 1, "BTC_USDT", Side::Buy, dec!(1), dec!(0.00000009));
        ledger.reserve_for_new_order(&maker, "BTC", "USDT").unwrap();
        ledger.reserve_for_new_order(&taker, "BTC", "USDT").unwrap();
        let trade = Trade {
            maker_order_id: 1,
            taker_order_id: 2,
            symbol: "BTC_USDT".to_string(),
            price: dec!(1),
            amount: dec!(0.00000009),
        };
        ledger.settle_trade(&trade, dec!(0.001), dec!(0.002)).unwrap();

        assert_eq!(ledger.exchange_revenue_by_asset("BTC"), dec!(0));
        assert_eq!(ledger.balance(1, "BTC").free, dec!(0.00000009));
        assert_eq!(ledger.totals_by_asset(), before);
    }

    #[test]
    fn notional_that_is_not_representable_is_rejected_on_reserve() {
        let mut ledger = funded();
        let order = Order::new(5, 1, "BTC_USDT", Side::Buy, dec!(0.5), dec!(0.00000001));
        let before = ledger.balances_sorted();
        assert!(matches!(
            ledger.reserve_for_new_order(&order, "BTC", "USDT"),
            Err(LedgerError::Money(MoneyError::NotionalNotRepresentable { .. }))
        ));
        assert_eq!(ledger.balances_sorted(), before);
        assert!(ledger.reservation(5).is_none());
    }

    #[test]
    fn change_set_apply_is_all_or_nothing() {
        let mut ledger = funded();
        let before = ledger.balances_sorted();
        let change = ChangeSet {
            deltas: vec![
                BalanceDelta { user_id: 1, asset: "USDT".into(), free: dec!(-10), locked: dec!(0) },
                BalanceDelta { user_id: 2, asset: "USDT".into(), free: dec!(-1), locked: dec!(0) },
            ],
            ..ChangeSet::default()
        };
        assert!(matches!(
            ledger.apply(&change),
            Err(LedgerError::InsufficientFreeBalance { user_id: 2, .. })
        ));
        assert_eq!(ledger.balances_sorted(), before);
    }
}
