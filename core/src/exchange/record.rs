// core/src/exchange/record.rs
// Versioned transaction record (PLAN-08).
//
// A record is the *decided* result of one command: sequence, timestamp, every balance delta,
// reservation change, fill and fee. Commit and replay only apply the record; they never
// recompute fees, prices or time. Owners of maker and taker orders are stored in the record
// itself, so persistence does not depend on a mutable ownership index.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::engine::{Order, Side};
use crate::exchange::command::{Actor, MarketConfig};
use crate::ledger::ChangeSet;
use crate::money::dec_str;

/// Serialization version of `TransactionRecord`. Bump on any incompatible change.
pub const RECORD_VERSION: u32 = 1;

/// Self-contained snapshot of an order inside a record (exact decimal strings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderRecord {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol: String,
    pub side: Side,
    #[serde(with = "dec_str")]
    pub price: Decimal,
    #[serde(with = "dec_str")]
    pub amount: Decimal,
    #[serde(with = "dec_str")]
    pub remaining: Decimal,
}

impl OrderRecord {
    pub fn from_order(order: &Order) -> Self {
        Self {
            order_id: order.id,
            user_id: order.user_id,
            symbol: order.symbol.clone(),
            side: order.side,
            price: order.price,
            amount: order.amount,
            remaining: order.remaining,
        }
    }

    pub fn to_order(&self) -> Order {
        Order {
            id: self.order_id,
            user_id: self.user_id,
            symbol: self.symbol.clone(),
            side: self.side,
            price: self.price,
            amount: self.amount,
            remaining: self.remaining,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeRecord {
    pub asset: String,
    #[serde(with = "dec_str")]
    pub amount: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FillRecord {
    pub symbol: String,
    pub maker_order_id: u64,
    pub maker_user_id: u64,
    pub taker_order_id: u64,
    pub taker_user_id: u64,
    pub taker_side: Side,
    #[serde(with = "dec_str")]
    pub price: Decimal,
    #[serde(with = "dec_str")]
    pub amount: Decimal,
    pub maker_fee: FeeRecord,
    pub taker_fee: FeeRecord,
}

/// Replayable order book mutation (no matching happens at apply time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum BookOp {
    /// Consume `quantity` from the front resting order `maker_order_id`.
    Fill {
        symbol: String,
        maker_order_id: u64,
        #[serde(with = "dec_str")]
        quantity: Decimal,
    },
    /// Add a resting order (with its remaining quantity).
    Rest { order: OrderRecord },
    /// Remove a resting order (cancel).
    Remove { symbol: String, order_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceRecord {
    pub symbol: String,
    #[serde(with = "dec_str")]
    pub price: Decimal,
}

/// Business result of the command, used by ACK, projection and WebSocket publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Outcome {
    OrderPlaced {
        order: OrderRecord,
        fills: Vec<FillRecord>,
        rested: bool,
    },
    OrderCancelled {
        order_id: u64,
        user_id: u64,
        symbol: String,
        released_asset: String,
        #[serde(with = "dec_str")]
        released_amount: Decimal,
    },
    Deposited {
        asset: String,
        #[serde(with = "dec_str")]
        amount: Decimal,
    },
    Withdrawn {
        asset: String,
        #[serde(with = "dec_str")]
        amount: Decimal,
    },
    Transferred {
        from_asset: String,
        to_asset: String,
        #[serde(with = "dec_str")]
        amount: Decimal,
    },
    CapitalAdjusted {
        asset: String,
        #[serde(with = "dec_str")]
        delta: Decimal,
    },
    MarketConfigured {
        symbol: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapitalDelta {
    pub asset: String,
    #[serde(with = "dec_str")]
    pub delta: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionRecord {
    pub version: u32,
    pub seq: u64,
    pub timestamp_ms: i64,
    pub request_key: String,
    pub payload_digest: String,
    pub actor: Actor,
    pub outcome: Outcome,
    pub ledger: ChangeSet,
    pub book: Vec<BookOp>,
    /// Value of the order id counter after this transaction.
    pub next_order_id: u64,
    pub last_trade_prices: Vec<PriceRecord>,
    pub capital: Option<CapitalDelta>,
    pub market: Option<MarketConfig>,
}

/// Lifecycle of one transaction: prepared (no state change) -> committed (state applied)
/// -> published (ACK/broadcast released). Only `Committed` implies durability once a journal
/// sits between prepare and commit (PLAN-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionPhase {
    Prepared,
    Committed,
    Published,
}

/// Decided but not yet applied. Holding it does not change any state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTransaction {
    pub record: TransactionRecord,
}

impl PreparedTransaction {
    pub fn phase(&self) -> TransactionPhase {
        TransactionPhase::Prepared
    }
}

/// Applied to the in-memory state; not yet visible to clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedTransaction {
    pub record: TransactionRecord,
}

impl CommittedTransaction {
    pub fn phase(&self) -> TransactionPhase {
        TransactionPhase::Committed
    }

    /// Marks the transaction as published once ACK/broadcast have been released.
    pub fn publish(self) -> PublishedTransaction {
        PublishedTransaction {
            record: self.record,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedTransaction {
    pub record: TransactionRecord,
}

impl PublishedTransaction {
    pub fn phase(&self) -> TransactionPhase {
        TransactionPhase::Published
    }
}
