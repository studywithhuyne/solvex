//! Strict, panic-free parsing of untrusted textual inputs (hash hex, user ids).
//!
//! All parsers operate on bytes and reject non-ASCII input up front, so multi-byte UTF-8
//! characters can never land on a slice boundary. No input is trimmed or normalised.

use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};

use crate::protocol::validate_user_id;
use crate::tree::{HashBytes, HASH_BYTES};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    HashLength(usize),
    NonAscii,
    InvalidHexDigit,
    NonCanonicalField,
    InvalidUserId(&'static str),
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::HashLength(len) => write!(f, "hash must be exactly 64 hex chars, got {len}"),
            Self::NonAscii => write!(f, "input must be ASCII"),
            Self::InvalidHexDigit => write!(f, "invalid hex digit"),
            Self::NonCanonicalField => write!(f, "hash is not a canonical BN254 field element"),
            Self::InvalidUserId(reason) => write!(f, "invalid user_id: {reason}"),
        }
    }
}

impl std::error::Error for ParseError {}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Returns true if `bytes` (little-endian) encode an integer below the BN254 scalar modulus.
pub fn is_canonical_field_bytes(bytes: &HashBytes) -> bool {
    let reduced = Fr::from_le_bytes_mod_order(bytes).into_repr().to_bytes_le();
    reduced.len() == HASH_BYTES && reduced.as_slice() == bytes.as_slice()
}

/// Parses exactly 64 ASCII hex characters into a canonical field-element hash.
pub fn parse_hash_hex(value: &str) -> Result<HashBytes, ParseError> {
    if !value.is_ascii() {
        return Err(ParseError::NonAscii);
    }
    let bytes = value.as_bytes();
    if bytes.len() != HASH_BYTES * 2 {
        return Err(ParseError::HashLength(bytes.len()));
    }

    let mut out = [0u8; HASH_BYTES];
    for (slot, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
        let hi = hex_value(pair[0]).ok_or(ParseError::InvalidHexDigit)?;
        let lo = hex_value(pair[1]).ok_or(ParseError::InvalidHexDigit)?;
        *slot = (hi << 4) | lo;
    }

    if !is_canonical_field_bytes(&out) {
        return Err(ParseError::NonCanonicalField);
    }
    Ok(out)
}

/// Lowercase hex encoding of a hash.
pub fn hash_to_hex(hash: &HashBytes) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(HASH_BYTES * 2);
    for byte in hash {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// Parses a canonical decimal user id: ASCII digits, no sign/whitespace/leading zeros,
/// value in `1..=i64::MAX`.
pub fn parse_user_id_str(value: &str) -> Result<u64, ParseError> {
    if value.is_empty() {
        return Err(ParseError::InvalidUserId("empty"));
    }
    if value.len() > 19 {
        return Err(ParseError::InvalidUserId("too long"));
    }
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::InvalidUserId("only ASCII digits are allowed"));
    }
    if value.len() > 1 && value.starts_with('0') {
        return Err(ParseError::InvalidUserId("leading zeros"));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| ParseError::InvalidUserId("overflow"))?;
    validate_user_id(parsed).map_err(|_| ParseError::InvalidUserId("out of range 1..=i64::MAX"))
}

/// Validates a numeric user id (JSON number) against the same domain as the string form.
pub fn parse_user_id_u64(value: u64) -> Result<u64, ParseError> {
    validate_user_id(value).map_err(|_| {
        if value == 0 {
            ParseError::InvalidUserId("zero is reserved")
        } else {
            ParseError::InvalidUserId("out of range 1..=i64::MAX")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MAX_USER_ID;

    #[test]
    fn hex_roundtrip() {
        let mut hash = [0u8; 32];
        hash[0] = 0xab;
        hash[31] = 0x01;
        assert_eq!(parse_hash_hex(&hash_to_hex(&hash)), Ok(hash));
        assert_eq!(parse_hash_hex(&hash_to_hex(&hash).to_uppercase()), Ok(hash));
    }

    #[test]
    fn rejects_malformed_hex() {
        let good = "11".repeat(32);
        assert!(parse_hash_hex(&good).is_ok());
        assert!(parse_hash_hex(&format!(" {good}")).is_err());
        assert!(parse_hash_hex(&format!("{good} ")).is_err());
        assert!(parse_hash_hex(&"11".repeat(31)).is_err());
        assert!(parse_hash_hex(&"11".repeat(33)).is_err());
        assert!(parse_hash_hex(&format!("0x{}", "11".repeat(31))).is_err());
        assert!(parse_hash_hex(&"zz".repeat(32)).is_err());
        assert!(parse_hash_hex(&"+1".repeat(32)).is_err());
    }

    #[test]
    fn unicode_input_never_panics() {
        // 64 bytes long but with a multi-byte char straddling a 2-byte slice boundary.
        let tricky = format!("1é{}", "1".repeat(61));
        assert_eq!(tricky.len(), 64);
        assert_eq!(parse_hash_hex(&tricky), Err(ParseError::NonAscii));
        assert!(parse_hash_hex("é").is_err());
        assert!(parse_hash_hex("😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀").is_err());
        assert!(parse_user_id_str("１２").is_err());
    }

    #[test]
    fn rejects_non_canonical_field_element() {
        // 0xff..ff is far above the BN254 modulus.
        assert_eq!(
            parse_hash_hex(&"ff".repeat(32)),
            Err(ParseError::NonCanonicalField)
        );
    }

    #[test]
    fn user_id_strictness() {
        assert_eq!(parse_user_id_str("1"), Ok(1));
        assert_eq!(parse_user_id_str("9223372036854775807"), Ok(MAX_USER_ID));
        for bad in [
            "",
            "0",
            "00",
            "01",
            "+1",
            "-1",
            " 1",
            "1 ",
            "1.0",
            "9223372036854775808",
            "18446744073709551616",
            "99999999999999999999",
        ] {
            assert!(parse_user_id_str(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(parse_user_id_u64(0).is_err());
        assert!(parse_user_id_u64(MAX_USER_ID + 1).is_err());
        assert!(parse_user_id_u64(5).is_ok());
    }
}
