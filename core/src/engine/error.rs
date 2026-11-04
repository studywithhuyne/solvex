use rust_decimal::Decimal;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("order id {0} already exists in the book")]
    DuplicateOrderId(u64),

    #[error("order id {0} not found")]
    OrderNotFound(u64),

    #[error("price must be positive, got {0}")]
    InvalidPrice(Decimal),

    #[error("amount must be positive, got {0}")]
    InvalidAmount(Decimal),

    #[error("fill for order {0} is invalid (unknown order or quantity out of range)")]
    InvalidFill(u64),

    #[error("fill for order {0} violates price-time priority")]
    FifoViolation(u64),

    #[error("order book index is inconsistent")]
    CorruptBook,
}
