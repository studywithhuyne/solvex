// Engine module: in-memory order book matching logic (BTreeMap-based, sync, no I/O)

#[allow(clippy::module_inception)]
mod engine;
mod error;
mod order_book;
mod types;

pub use engine::Engine;
pub use error::EngineError;
pub use order_book::{DepthLevel, DepthSnapshot, OrderBook, PlannedFill};
pub use types::{Order, Side, Trade};
