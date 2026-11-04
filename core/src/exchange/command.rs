// core/src/exchange/command.rs
// Command contract of the single writer (PLAN-08).
//
// Every operation that changes value state (orders, balances, market configuration)
// is expressed as a `Command`. The envelope carries the declared actor, a caller supplied
// request key and a digest of the canonical payload, so a later retry with the same key can be
// matched against the stored outcome (PLAN-13) and a key reused with another payload can be
// reported as a conflict.
//
// The actor is a *declared* identity taken from `x-user-id`; it is not authenticated.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::engine::Side;
use crate::money::{self, dec_str, MarketPolicy, MoneyError};

pub const MAX_REQUEST_KEY_LEN: usize = 128;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandError {
    #[error("request key must be 1..={MAX_REQUEST_KEY_LEN} printable ASCII characters")]
    InvalidRequestKey,

    #[error(transparent)]
    Money(#[from] MoneyError),
}

/// Declared (unauthenticated) caller identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Actor {
    pub user_id: u64,
}

impl Actor {
    /// `user_id` must be positive and fit the SQL BIGINT domain.
    pub fn declared(user_id: u64) -> Result<Self, MoneyError> {
        money::id_to_sql(user_id)?;
        Ok(Self { user_id })
    }
}

/// Caller supplied idempotency key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestKey(String);

impl RequestKey {
    pub fn new(value: &str) -> Result<Self, CommandError> {
        if value.is_empty()
            || value.len() > MAX_REQUEST_KEY_LEN
            || !value.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(CommandError::InvalidRequestKey);
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Market definition owned by the writer (tick/lot, fees and halt flag).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketConfig {
    pub symbol: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub active: bool,
    pub policy: MarketPolicy,
}

impl MarketConfig {
    pub fn new(base_asset: &str, quote_asset: &str, policy: MarketPolicy) -> Self {
        Self {
            symbol: format!("{base_asset}_{quote_asset}"),
            base_asset: base_asset.to_string(),
            quote_asset: quote_asset.to_string(),
            active: true,
            policy,
        }
    }
}

/// Every state-changing business operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    PlaceOrder {
        symbol: String,
        side: Side,
        #[serde(with = "dec_str")]
        price: Decimal,
        #[serde(with = "dec_str")]
        amount: Decimal,
    },
    CancelOrder {
        order_id: u64,
    },
    Deposit {
        asset: String,
        #[serde(with = "dec_str")]
        amount: Decimal,
    },
    Withdraw {
        asset: String,
        #[serde(with = "dec_str")]
        amount: Decimal,
    },
    /// Mock 1:1 internal conversion between two assets of the same user.
    Transfer {
        from_asset: String,
        to_asset: String,
        #[serde(with = "dec_str")]
        amount: Decimal,
    },
    /// Signed change of the exchange capital tracked for one asset (must stay >= 0).
    AdjustCapital {
        asset: String,
        #[serde(with = "dec_str")]
        delta: Decimal,
    },
    /// Create or replace a market definition (including halting it).
    ConfigureMarket(MarketConfig),
}

impl Command {
    /// Copy with every decimal normalized, so `1.0` and `1` yield the same digest.
    fn canonical(&self) -> Command {
        match self.clone() {
            Command::PlaceOrder {
                symbol,
                side,
                price,
                amount,
            } => Command::PlaceOrder {
                symbol,
                side,
                price: price.normalize(),
                amount: amount.normalize(),
            },
            Command::Deposit { asset, amount } => Command::Deposit {
                asset,
                amount: amount.normalize(),
            },
            Command::Withdraw { asset, amount } => Command::Withdraw {
                asset,
                amount: amount.normalize(),
            },
            Command::Transfer {
                from_asset,
                to_asset,
                amount,
            } => Command::Transfer {
                from_asset,
                to_asset,
                amount: amount.normalize(),
            },
            Command::AdjustCapital { asset, delta } => Command::AdjustCapital {
                asset,
                delta: delta.normalize(),
            },
            Command::ConfigureMarket(mut config) => {
                config.policy.tick = config.policy.tick.normalize();
                config.policy.lot = config.policy.lot.normalize();
                config.policy.maker_fee_rate = config.policy.maker_fee_rate.normalize();
                config.policy.taker_fee_rate = config.policy.taker_fee_rate.normalize();
                Command::ConfigureMarket(config)
            }
            other @ Command::CancelOrder { .. } => other,
        }
    }

    /// blake3 digest (hex) of the canonical JSON of this command.
    ///
    /// JSON field order follows the struct definitions, and decimals are exact strings, so the
    /// digest is stable across processes.
    pub fn payload_digest(&self) -> String {
        // Serialization of these plain data types cannot fail; fall back to the debug form
        // rather than panicking so the digest is still deterministic.
        let bytes = serde_json::to_vec(&self.canonical())
            .unwrap_or_else(|_| format!("{:?}", self.canonical()).into_bytes());
        blake3::hash(&bytes).to_hex().to_string()
    }
}

/// A command with the metadata required for idempotency and audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEnvelope {
    pub actor: Actor,
    pub request_key: RequestKey,
    pub command: Command,
}

impl CommandEnvelope {
    pub fn new(actor: Actor, request_key: RequestKey, command: Command) -> Self {
        Self {
            actor,
            request_key,
            command,
        }
    }

    pub fn payload_digest(&self) -> String {
        self.command.payload_digest()
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn request_key_rules() {
        assert!(RequestKey::new("order-1").is_ok());
        assert!(RequestKey::new("").is_err());
        assert!(RequestKey::new("has space").is_err());
        assert!(RequestKey::new("m\u{e9}").is_err());
        assert!(RequestKey::new(&"k".repeat(MAX_REQUEST_KEY_LEN + 1)).is_err());
        assert!(RequestKey::new(&"k".repeat(MAX_REQUEST_KEY_LEN)).is_ok());
    }

    #[test]
    fn actor_must_be_in_sql_domain() {
        assert!(Actor::declared(1).is_ok());
        assert!(Actor::declared(0).is_err());
        assert!(Actor::declared(u64::MAX).is_err());
    }

    #[test]
    fn digest_ignores_decimal_formatting_but_not_content() {
        let a = Command::Deposit {
            asset: "USDT".into(),
            amount: dec!(1.0),
        };
        let b = Command::Deposit {
            asset: "USDT".into(),
            amount: dec!(1.000),
        };
        let c = Command::Deposit {
            asset: "USDT".into(),
            amount: dec!(1.1),
        };
        let d = Command::Withdraw {
            asset: "USDT".into(),
            amount: dec!(1),
        };
        assert_eq!(a.payload_digest(), b.payload_digest());
        assert_ne!(a.payload_digest(), c.payload_digest());
        assert_ne!(a.payload_digest(), d.payload_digest());
    }

    #[test]
    fn command_json_round_trip_is_exact() {
        let command = Command::PlaceOrder {
            symbol: "BTC_USDT".into(),
            side: Side::Buy,
            price: dec!(67123.45000001),
            amount: dec!(0.00000001),
        };
        let json = serde_json::to_string(&command).unwrap();
        assert!(json.contains("\"67123.45000001\""));
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), command);
    }
}
