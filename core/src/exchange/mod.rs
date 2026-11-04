// core/src/exchange/mod.rs
// Single-writer exchange core (PLAN-08, PLAN-09).
//
// `command`  — actor/request key/payload digest and the set of state-changing commands.
// `record`   — versioned transaction record carrying every decided delta, fill and fee.
// `state`    — state owner: read-only `prepare`, atomic `commit`, replay via `apply_record`.
// `writer`   — bounded queue, commit sink seam and the synchronous writer loop.
//
// The HTTP handlers still use the legacy lock-per-structure path; routing every write
// through this module is PLAN-10.

pub mod command;
pub mod record;
pub mod state;
pub mod writer;

pub use command::{Actor, Command, CommandEnvelope, CommandError, MarketConfig, RequestKey};
pub use record::{
    BookOp, CommittedTransaction, FeeRecord, FillRecord, OrderRecord, Outcome, PreparedTransaction,
    PublishedTransaction, TransactionPhase, TransactionRecord, RECORD_VERSION,
};
pub use state::{ExchangeError, ExchangeState};
pub use writer::{command_queue, CommandSender, CommitSink, ExchangeWriter, NoopSink};

#[cfg(test)]
mod tests;
