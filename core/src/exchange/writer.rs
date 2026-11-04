// core/src/exchange/writer.rs
// Bounded command queue and the single writer loop (PLAN-08).
//
// Flow of one command:   queue -> prepare (read-only) -> sink.append -> commit -> reply
//
// * The queue is bounded: when it is full `submit` fails immediately with `QueueFull`,
//   before the command is accepted and before any state could change.
// * `CommitSink` is the seam where the durable journal (PLAN-12) plugs in. A sink failure
//   happens before `commit`, so the in-memory state never gets ahead of what was persisted.
// * The writer is synchronous and owns the state exclusively; no lock is held across the
//   sink call. Time comes from an injected clock so tests stay deterministic.

use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

use crate::exchange::command::CommandEnvelope;
use crate::exchange::record::{CommittedTransaction, TransactionRecord};
use crate::exchange::state::{ExchangeError, ExchangeState};

/// Receives each prepared record before it is applied. The journal implements this.
pub trait CommitSink {
    fn append(&mut self, record: &TransactionRecord) -> Result<(), String>;
}

/// Sink that accepts everything (state only; no durability).
#[derive(Debug, Default)]
pub struct NoopSink;

impl CommitSink for NoopSink {
    fn append(&mut self, _record: &TransactionRecord) -> Result<(), String> {
        Ok(())
    }
}

pub type CommandResult = Result<CommittedTransaction, ExchangeError>;

struct Submission {
    envelope: CommandEnvelope,
    reply: SyncSender<CommandResult>,
}

/// Cloneable handle used by API handlers and the simulator.
#[derive(Clone)]
pub struct CommandSender {
    tx: SyncSender<Submission>,
}

impl CommandSender {
    /// Enqueues a command. Fails fast with `QueueFull` / `QueueClosed`; on success the returned
    /// receiver yields exactly one result once the writer has processed the command.
    pub fn submit(
        &self,
        envelope: CommandEnvelope,
    ) -> Result<Receiver<CommandResult>, ExchangeError> {
        let (reply, result) = mpsc::sync_channel(1);
        match self.tx.try_send(Submission { envelope, reply }) {
            Ok(()) => Ok(result),
            Err(TrySendError::Full(_)) => Err(ExchangeError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(ExchangeError::QueueClosed),
        }
    }
}

pub struct ExchangeWriter<S: CommitSink> {
    state: ExchangeState,
    rx: Receiver<Submission>,
    sink: S,
    clock: Box<dyn FnMut() -> i64 + Send>,
}

/// Creates a bounded queue (`capacity >= 1`) and its writer.
pub fn command_queue<S: CommitSink>(
    state: ExchangeState,
    sink: S,
    capacity: usize,
    clock: Box<dyn FnMut() -> i64 + Send>,
) -> (CommandSender, ExchangeWriter<S>) {
    let (tx, rx) = mpsc::sync_channel(capacity.max(1));
    (
        CommandSender { tx },
        ExchangeWriter {
            state,
            rx,
            sink,
            clock,
        },
    )
}

impl<S: CommitSink> ExchangeWriter<S> {
    pub fn state(&self) -> &ExchangeState {
        &self.state
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Processes one command synchronously (also used directly by tests and replay tooling).
    pub fn execute(&mut self, envelope: &CommandEnvelope) -> CommandResult {
        let timestamp_ms = (self.clock)();
        let prepared = self.state.prepare(envelope, timestamp_ms)?;
        self.sink
            .append(&prepared.record)
            .map_err(ExchangeError::SinkFailed)?;
        self.state.commit(prepared)
    }

    /// Handles at most one queued command. Returns `false` when the queue is empty or closed.
    pub fn process_one(&mut self) -> bool {
        match self.rx.try_recv() {
            Ok(submission) => {
                let result = self.execute(&submission.envelope);
                // The caller may have stopped waiting; the command is still decided.
                let _ = submission.reply.send(result);
                true
            }
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => false,
        }
    }

    /// Processes everything currently queued; returns the number of commands handled.
    pub fn drain(&mut self) -> usize {
        let mut handled = 0;
        while self.process_one() {
            handled += 1;
        }
        handled
    }
}
