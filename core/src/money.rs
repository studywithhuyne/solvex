// core/src/money.rs
// Single money policy for RAM ledger, API validation and SQL boundary (PLAN-06).
//
// Contract (docs/DECISIONS.md section 2):
// - Scale 8 for every asset in policy version 1. Values are `Decimal` in RAM but must be
//   exactly representable as integer units (`zkp::amount`), the same units used by hashes.
// - Nothing is ever rescaled or rounded to "fix" caller input. Invalid input is rejected
//   before any state mutation.
// - Fees are rounded toward zero to the asset precision exactly once; the remainder stays
//   with the receiving user, so `net + fee == gross`.
// - IDs are positive and must not exceed `i64::MAX` (SQL BIGINT boundary).

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zkp::amount::{decimal_to_positive_units, decimal_to_units, AmountError};

pub use zkp::amount::{AMOUNT_SCALE, MAX_UNITS};

/// Version of the amount/rounding policy. Bump on any rule change.
pub const MONEY_POLICY_VERSION: u32 = 1;

/// Largest ID that fits the PostgreSQL BIGINT boundary.
pub const MAX_ID: u64 = i64::MAX as u64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MoneyError {
    #[error(transparent)]
    Amount(#[from] AmountError),

    #[error("value {value} is not a multiple of step {step}")]
    NotMultipleOfStep { value: Decimal, step: Decimal },

    #[error("notional of price {price} x quantity {quantity} is not exactly representable in quote units")]
    NotionalNotRepresentable { price: Decimal, quantity: Decimal },

    #[error("arithmetic overflow")]
    Overflow,

    #[error("fee rate must be within [0, 1], got {0}")]
    InvalidFeeRate(Decimal),

    #[error("id {0} is outside the supported range 1..=i64::MAX")]
    InvalidId(u64),

    #[error("invalid market policy: {0}")]
    InvalidPolicy(&'static str),
}

/// Smallest representable step for scale 8.
pub fn min_unit() -> Decimal {
    Decimal::new(1, AMOUNT_SCALE)
}

/// Validates a strictly positive amount/price (no rescale, scale <= 8, <= 10^18 units).
pub fn validate_positive(value: &Decimal) -> Result<(), MoneyError> {
    decimal_to_positive_units(value)?;
    Ok(())
}

/// Validates a non-negative balance (zero allowed).
pub fn validate_non_negative(value: &Decimal) -> Result<(), MoneyError> {
    decimal_to_units(value)?;
    Ok(())
}

/// Exact units of a non-negative value.
pub fn to_units(value: &Decimal) -> Result<u128, MoneyError> {
    Ok(decimal_to_units(value)?)
}

/// Exact units of a strictly positive value.
pub fn to_positive_units(value: &Decimal) -> Result<u128, MoneyError> {
    Ok(decimal_to_positive_units(value)?)
}

/// Checked `a + b` that stays a valid scale-8 value.
pub fn checked_add(a: Decimal, b: Decimal) -> Result<Decimal, MoneyError> {
    a.checked_add(b).ok_or(MoneyError::Overflow)
}

/// Checked `a - b` (may be negative; callers decide whether that is allowed).
pub fn checked_sub(a: Decimal, b: Decimal) -> Result<Decimal, MoneyError> {
    a.checked_sub(b).ok_or(MoneyError::Overflow)
}

/// Quote notional `price * quantity` with checked arithmetic.
///
/// The result must be exactly representable in quote units (scale <= 8 and within the
/// per-value maximum); otherwise the input would create or destroy value and is rejected.
pub fn checked_notional(price: &Decimal, quantity: &Decimal) -> Result<Decimal, MoneyError> {
    let product = price.checked_mul(*quantity).ok_or(MoneyError::Overflow)?;
    if decimal_to_units(&product).is_err() {
        return Err(MoneyError::NotionalNotRepresentable {
            price: *price,
            quantity: *quantity,
        });
    }
    Ok(product)
}

/// Fee in the received asset: `gross * rate`, rounded toward zero to scale 8.
///
/// This is the only place fee rounding happens. The caller credits `gross - fee` to the
/// receiver, so dust remains with the receiver and nothing disappears.
pub fn fee_amount(gross: &Decimal, rate: &Decimal) -> Result<Decimal, MoneyError> {
    validate_fee_rate(rate)?;
    let raw = gross.checked_mul(*rate).ok_or(MoneyError::Overflow)?;
    let fee = raw.round_dp_with_strategy(AMOUNT_SCALE, RoundingStrategy::ToZero);
    // Rounding may leave trailing zeros beyond scale 8 only if they are zero; normalize
    // the representation without changing the value.
    Ok(fee.normalize())
}

pub fn validate_fee_rate(rate: &Decimal) -> Result<(), MoneyError> {
    if rate.is_sign_negative() || *rate > Decimal::ONE {
        return Err(MoneyError::InvalidFeeRate(*rate));
    }
    Ok(())
}

/// Validates a caller/order/sequence ID and converts to the SQL BIGINT domain.
pub fn id_to_sql(id: u64) -> Result<i64, MoneyError> {
    if id == 0 || id > MAX_ID {
        return Err(MoneyError::InvalidId(id));
    }
    i64::try_from(id).map_err(|_| MoneyError::InvalidId(id))
}

/// Converts a SQL BIGINT back into the u64 domain (positive only).
pub fn id_from_sql(id: i64) -> Result<u64, MoneyError> {
    if id <= 0 {
        return Err(MoneyError::InvalidId(id as u64));
    }
    u64::try_from(id).map_err(|_| MoneyError::InvalidId(id as u64))
}

/// Per-market tick/lot and fee configuration (versioned by `MONEY_POLICY_VERSION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketPolicy {
    #[serde(with = "dec_str")]
    pub tick: Decimal,
    #[serde(with = "dec_str")]
    pub lot: Decimal,
    #[serde(with = "dec_str")]
    pub maker_fee_rate: Decimal,
    #[serde(with = "dec_str")]
    pub taker_fee_rate: Decimal,
}

impl MarketPolicy {
    /// Default tick/lot `0.00000001`, maker 0.10%, taker 0.20%.
    pub fn standard() -> Self {
        Self {
            tick: min_unit(),
            lot: min_unit(),
            maker_fee_rate: Decimal::new(1, 3),
            taker_fee_rate: Decimal::new(2, 3),
        }
    }

    pub fn new(
        tick: Decimal,
        lot: Decimal,
        maker_fee_rate: Decimal,
        taker_fee_rate: Decimal,
    ) -> Result<Self, MoneyError> {
        let policy = Self {
            tick,
            lot,
            maker_fee_rate,
            taker_fee_rate,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), MoneyError> {
        to_positive_units(&self.tick)
            .map_err(|_| MoneyError::InvalidPolicy("tick must be a positive scale-8 value"))?;
        to_positive_units(&self.lot)
            .map_err(|_| MoneyError::InvalidPolicy("lot must be a positive scale-8 value"))?;
        validate_fee_rate(&self.maker_fee_rate)?;
        validate_fee_rate(&self.taker_fee_rate)?;
        Ok(())
    }

    /// Rejects a limit order before any mutation: positive, scale <= 8, tick/lot multiples
    /// and an exactly representable quote notional.
    pub fn validate_order(&self, price: &Decimal, amount: &Decimal) -> Result<(), MoneyError> {
        let price_units = to_positive_units(price)?;
        let amount_units = to_positive_units(amount)?;
        let tick_units = to_positive_units(&self.tick)?;
        let lot_units = to_positive_units(&self.lot)?;

        if price_units % tick_units != 0 {
            return Err(MoneyError::NotMultipleOfStep {
                value: *price,
                step: self.tick,
            });
        }
        if amount_units % lot_units != 0 {
            return Err(MoneyError::NotMultipleOfStep {
                value: *amount,
                step: self.lot,
            });
        }
        checked_notional(price, amount)?;
        Ok(())
    }
}

impl Default for MarketPolicy {
    fn default() -> Self {
        Self::standard()
    }
}

/// Exact decimal text for serialized records (the crate enables `serde-float`, which would
/// otherwise round-trip through floating point).
pub mod dec_str {
    use std::str::FromStr;

    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
        let text = String::deserialize(deserializer)?;
        Decimal::from_str(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn rejects_negative_zero_and_over_precision_without_rescaling() {
        assert!(validate_positive(&dec!(-1)).is_err());
        assert!(validate_positive(&dec!(0)).is_err());
        assert!(validate_positive(&dec!(0.000000001)).is_err());
        assert!(validate_positive(&dec!(1.00000001)).is_ok());
        // Trailing zeros beyond scale 8 are exact and therefore accepted.
        assert!(validate_positive(&dec!(1.500000000)).is_ok());
        assert!(validate_non_negative(&dec!(0)).is_ok());
        assert!(validate_non_negative(&dec!(-0.00000001)).is_err());
    }

    #[test]
    fn rejects_values_above_max_units() {
        // 10^10 whole units == 10^18 units: the maximum.
        assert!(validate_positive(&dec!(10000000000)).is_ok());
        assert!(validate_positive(&dec!(10000000000.00000001)).is_err());
    }

    #[test]
    fn notional_overflow_and_non_representable_are_rejected() {
        assert_eq!(checked_notional(&dec!(100), &dec!(0.5)).unwrap(), dec!(50));
        assert!(matches!(
            checked_notional(&Decimal::MAX, &dec!(2)),
            Err(MoneyError::Overflow)
        ));
        // 0.5 * 0.00000001 = 0.000000005 cannot be represented in quote units.
        assert!(matches!(
            checked_notional(&dec!(0.5), &dec!(0.00000001)),
            Err(MoneyError::NotionalNotRepresentable { .. })
        ));
        // Above the per-value cap although the multiplication itself does not overflow.
        assert!(checked_notional(&dec!(10000000000), &dec!(10)).is_err());
    }

    #[test]
    fn fee_rounds_toward_zero_once_and_net_plus_fee_equals_gross() {
        let gross = dec!(0.00000999);
        let fee = fee_amount(&gross, &dec!(0.001)).unwrap();
        // 0.00000999 * 0.001 = 0.00000000999 -> 0 at scale 8.
        assert_eq!(fee, Decimal::ZERO);
        let gross = dec!(12.34567891);
        let fee = fee_amount(&gross, &dec!(0.002)).unwrap();
        assert_eq!(fee, dec!(0.02469135));
        assert_eq!((gross - fee) + fee, gross);
        assert!(fee_amount(&gross, &dec!(1.0000001)).is_err());
        assert!(fee_amount(&gross, &dec!(-0.1)).is_err());
    }

    #[test]
    fn ids_must_fit_sql_bigint() {
        assert_eq!(id_to_sql(1).unwrap(), 1);
        assert_eq!(id_to_sql(MAX_ID).unwrap(), i64::MAX);
        assert!(id_to_sql(0).is_err());
        assert!(id_to_sql(MAX_ID + 1).is_err());
        assert!(id_to_sql(u64::MAX).is_err());
        assert_eq!(id_from_sql(7).unwrap(), 7);
        assert!(id_from_sql(0).is_err());
        assert!(id_from_sql(-5).is_err());
    }

    #[test]
    fn market_policy_enforces_tick_lot_and_notional() {
        let policy = MarketPolicy::new(dec!(0.01), dec!(0.001), dec!(0.001), dec!(0.002)).unwrap();
        assert!(policy.validate_order(&dec!(100.01), &dec!(0.5)).is_ok());
        assert!(matches!(
            policy.validate_order(&dec!(100.005), &dec!(0.5)),
            Err(MoneyError::NotMultipleOfStep { .. })
        ));
        assert!(matches!(
            policy.validate_order(&dec!(100.01), &dec!(0.0005)),
            Err(MoneyError::NotMultipleOfStep { .. })
        ));
        assert!(MarketPolicy::new(dec!(0), dec!(1), dec!(0), dec!(0)).is_err());
        assert!(MarketPolicy::new(dec!(1), dec!(1), dec!(2), dec!(0)).is_err());
        assert!(MarketPolicy::standard()
            .validate_order(&dec!(0.5), &dec!(0.00000001))
            .is_err());
    }

    #[test]
    fn dec_str_round_trips_exactly() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Wrapper(#[serde(with = "super::dec_str")] Decimal);
        let value = Wrapper(dec!(0.12345678));
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, "\"0.12345678\"");
        assert_eq!(serde_json::from_str::<Wrapper>(&json).unwrap(), value);
    }
}
