//! Exact integer-unit amounts (docs/DECISIONS.md §2 money contract).
//!
//! * Scale 8: `1.00000001` is `100_000_001` units.
//! * Units are `u128`; a single balance/price/quantity is at most `MAX_UNITS` (10^18).
//! * Sums (tree internal nodes) are at most `MAX_SUM_UNITS` (`MAX_UNITS * MAX_LEAVES`).
//! * Conversions never round or rescale silently: negative, over-precision and overflow
//!   are rejected. Trailing fractional zeros beyond scale 8 are accepted because the
//!   value is represented exactly.

use rust_decimal::Decimal;

use crate::protocol::MAX_LEAVES;

pub const AMOUNT_SCALE: u32 = 8;
pub const UNITS_PER_WHOLE: u128 = 100_000_000;
/// Upper bound for a single balance/price/quantity: 10^18 units.
pub const MAX_UNITS: u128 = 1_000_000_000_000_000_000;
/// Upper bound for a tree sum: `MAX_UNITS * MAX_LEAVES`.
pub const MAX_SUM_UNITS: u128 = MAX_UNITS * (MAX_LEAVES as u128);

/// Maximum accepted length of a textual amount.
pub const MAX_AMOUNT_TEXT_LEN: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmountError {
    Negative(Decimal),
    OverPrecision(Decimal),
    Overflow,
    ExceedsMax { units: u128, max: u128 },
    Zero,
    Malformed(&'static str),
}

impl core::fmt::Display for AmountError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Negative(v) => write!(f, "amount must be non-negative, got {v}"),
            Self::OverPrecision(v) => {
                write!(f, "amount {v} has more than {AMOUNT_SCALE} decimal places")
            }
            Self::Overflow => write!(f, "amount overflows u128 units"),
            Self::ExceedsMax { units, max } => {
                write!(f, "amount {units} units exceeds maximum {max} units")
            }
            Self::Zero => write!(f, "amount must be greater than zero"),
            Self::Malformed(reason) => write!(f, "malformed amount: {reason}"),
        }
    }
}

impl std::error::Error for AmountError {}

fn pow10(exp: u32) -> Result<u128, AmountError> {
    10u128.checked_pow(exp).ok_or(AmountError::Overflow)
}

fn bounded(units: u128, max: u128) -> Result<u128, AmountError> {
    if units > max {
        return Err(AmountError::ExceedsMax { units, max });
    }
    Ok(units)
}

/// Exact conversion without any bound other than `u128`.
fn decimal_to_units_raw(value: &Decimal) -> Result<u128, AmountError> {
    let mantissa = value.mantissa();
    if mantissa < 0 {
        return Err(AmountError::Negative(*value));
    }
    let magnitude = u128::try_from(mantissa).map_err(|_| AmountError::Overflow)?;
    let scale = value.scale();

    if scale <= AMOUNT_SCALE {
        magnitude
            .checked_mul(pow10(AMOUNT_SCALE - scale)?)
            .ok_or(AmountError::Overflow)
    } else {
        let divisor = pow10(scale - AMOUNT_SCALE)?;
        if magnitude % divisor != 0 {
            return Err(AmountError::OverPrecision(*value));
        }
        Ok(magnitude / divisor)
    }
}

/// Converts a single balance (zero allowed) to exact units, `<= MAX_UNITS`.
pub fn decimal_to_units(value: &Decimal) -> Result<u128, AmountError> {
    bounded(decimal_to_units_raw(value)?, MAX_UNITS)
}

/// Converts a trade amount/price (strictly positive) to exact units, `<= MAX_UNITS`.
pub fn decimal_to_positive_units(value: &Decimal) -> Result<u128, AmountError> {
    let units = decimal_to_units(value)?;
    if units == 0 {
        return Err(AmountError::Zero);
    }
    Ok(units)
}

/// Converts an aggregate (tree sum) to exact units, `<= MAX_SUM_UNITS`.
pub fn decimal_to_sum_units(value: &Decimal) -> Result<u128, AmountError> {
    bounded(decimal_to_units_raw(value)?, MAX_SUM_UNITS)
}

/// Exact units back to a scale-8 `Decimal`.
pub fn units_to_decimal(units: u128) -> Result<Decimal, AmountError> {
    let mantissa = i128::try_from(units).map_err(|_| AmountError::Overflow)?;
    Decimal::try_from_i128_with_scale(mantissa, AMOUNT_SCALE).map_err(|_| AmountError::Overflow)
}

/// Checked sum of units, bounded by `MAX_SUM_UNITS`.
pub fn checked_add_units(a: u128, b: u128) -> Result<u128, AmountError> {
    bounded(
        a.checked_add(b).ok_or(AmountError::Overflow)?,
        MAX_SUM_UNITS,
    )
}

/// Strict canonical textual amount -> units (not bounded yet).
///
/// Grammar: `(0|[1-9][0-9]*)(\.[0-9]+)?`, ASCII only, no sign, no exponent, no
/// whitespace; digits after the 8th fractional place must all be `0`.
fn parse_units_raw(text: &str) -> Result<u128, AmountError> {
    if text.is_empty() {
        return Err(AmountError::Malformed("empty"));
    }
    if text.len() > MAX_AMOUNT_TEXT_LEN {
        return Err(AmountError::Malformed("too long"));
    }
    if !text.is_ascii() {
        return Err(AmountError::Malformed("non-ASCII input"));
    }

    let (int_part, frac_part) = match text.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (text, None),
    };

    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AmountError::Malformed("integer part must be ASCII digits"));
    }
    if int_part.len() > 1 && int_part.starts_with('0') {
        return Err(AmountError::Malformed("leading zeros"));
    }

    let mut units: u128 = 0;
    for byte in int_part.bytes() {
        units = units
            .checked_mul(10)
            .and_then(|v| v.checked_add(u128::from(byte - b'0')))
            .ok_or(AmountError::Overflow)?;
    }
    units = units
        .checked_mul(UNITS_PER_WHOLE)
        .ok_or(AmountError::Overflow)?;

    if let Some(frac) = frac_part {
        if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AmountError::Malformed("fraction must be ASCII digits"));
        }
        let mut frac_units: u128 = 0;
        for (i, byte) in frac.bytes().enumerate() {
            let digit = u128::from(byte - b'0');
            if i < AMOUNT_SCALE as usize {
                frac_units = frac_units * 10 + digit;
            } else if digit != 0 {
                return Err(AmountError::Malformed("more than 8 decimal places"));
            }
        }
        let missing = AMOUNT_SCALE.saturating_sub(frac.len() as u32);
        frac_units = frac_units
            .checked_mul(pow10(missing)?)
            .ok_or(AmountError::Overflow)?;
        units = units.checked_add(frac_units).ok_or(AmountError::Overflow)?;
    }

    Ok(units)
}

/// Parses a canonical amount string to units, `<= MAX_UNITS`.
pub fn parse_amount_units(text: &str) -> Result<u128, AmountError> {
    bounded(parse_units_raw(text)?, MAX_UNITS)
}

/// Parses a canonical aggregate string to units, `<= MAX_SUM_UNITS`.
pub fn parse_sum_units(text: &str) -> Result<u128, AmountError> {
    bounded(parse_units_raw(text)?, MAX_SUM_UNITS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn exact_conversion() {
        assert_eq!(decimal_to_units(&dec!(0)), Ok(0));
        assert_eq!(decimal_to_units(&dec!(1)), Ok(100_000_000));
        assert_eq!(decimal_to_units(&dec!(1.00000001)), Ok(100_000_001));
        assert_eq!(decimal_to_units(&dec!(1.500000000)), Ok(150_000_000));
        assert_eq!(decimal_to_units(&dec!(10000000000)), Ok(MAX_UNITS));
    }

    #[test]
    fn rejects_negative_overprecision_overflow() {
        assert!(matches!(
            decimal_to_units(&dec!(-1)),
            Err(AmountError::Negative(_))
        ));
        assert!(matches!(
            decimal_to_units(&dec!(0.000000001)),
            Err(AmountError::OverPrecision(_))
        ));
        assert!(matches!(
            decimal_to_units(&dec!(10000000000.00000001)),
            Err(AmountError::ExceedsMax { .. })
        ));
        assert!(matches!(
            decimal_to_units(&Decimal::MAX),
            Err(AmountError::ExceedsMax { .. }) | Err(AmountError::Overflow)
        ));
    }

    #[test]
    fn positive_rejects_zero() {
        assert_eq!(decimal_to_positive_units(&dec!(0)), Err(AmountError::Zero));
        assert_eq!(decimal_to_positive_units(&dec!(0.00000001)), Ok(1));
    }

    #[test]
    fn sum_units_allow_larger_than_single_balance() {
        let big = dec!(20000000000);
        assert!(decimal_to_units(&big).is_err());
        assert_eq!(decimal_to_sum_units(&big), Ok(2_000_000_000_000_000_000));
    }

    #[test]
    fn units_roundtrip() {
        for units in [0u128, 1, 99_999_999, 100_000_000, MAX_UNITS, MAX_SUM_UNITS] {
            let dec = units_to_decimal(units).expect("convertible");
            assert_eq!(decimal_to_sum_units(&dec), Ok(units));
        }
        assert_eq!(units_to_decimal(u128::MAX), Err(AmountError::Overflow));
    }

    #[test]
    fn checked_add_bounds() {
        assert_eq!(checked_add_units(1, 2), Ok(3));
        assert!(checked_add_units(MAX_SUM_UNITS, 1).is_err());
        assert_eq!(checked_add_units(u128::MAX, 1), Err(AmountError::Overflow));
    }

    #[test]
    fn strict_text_parsing() {
        assert_eq!(parse_amount_units("0"), Ok(0));
        assert_eq!(parse_amount_units("7.5"), Ok(750_000_000));
        assert_eq!(parse_amount_units("10.50000000"), Ok(1_050_000_000));
        assert_eq!(parse_amount_units("1.000000000"), Ok(100_000_000));
        for bad in [
            "",
            " 1",
            "1 ",
            "+1",
            "-1",
            "01",
            "1.",
            ".5",
            "1e3",
            "1,5",
            "1.000000001",
            "１",
            "1.5٣",
            "0x10",
            "1_0",
            "10000000000.00000001",
        ] {
            assert!(parse_amount_units(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(parse_amount_units(&"9".repeat(60)).is_err());
        assert!(parse_amount_units(&format!("1{}", "0".repeat(40))).is_err());
    }
}
