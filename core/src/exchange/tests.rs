// Tests for the single writer: whole-command atomicity, replay equivalence, queue and
// durability seam behaviour.

use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::*;
use crate::engine::Side;
use crate::exchange::record::TransactionRecord;
use crate::exchange::writer::CommandResult;
use crate::ledger::LedgerError;
use crate::money::{MarketPolicy, MoneyError};

#[derive(Default)]
struct MemorySink {
    records: Vec<TransactionRecord>,
    fail_next: bool,
}

impl CommitSink for MemorySink {
    fn append(&mut self, record: &TransactionRecord) -> Result<(), String> {
        if self.fail_next {
            self.fail_next = false;
            return Err("injected sink failure".to_string());
        }
        self.records.push(record.clone());
        Ok(())
    }
}

struct Fixture {
    writer: ExchangeWriter<MemorySink>,
    keys: u64,
}

impl Fixture {
    /// Two markets sharing USDT, three funded users.
    fn new() -> Self {
        let mut clock = 1_700_000_000_000_i64;
        let (_tx, writer) = command_queue(
            ExchangeState::new(),
            MemorySink::default(),
            16,
            Box::new(move || {
                clock += 1;
                clock
            }),
        );
        let mut fx = Self { writer, keys: 0 };
        for base in ["BTC", "ETH"] {
            fx.run(
                1,
                Command::ConfigureMarket(MarketConfig::new(base, "USDT", MarketPolicy::standard())),
            )
            .unwrap();
        }
        for user in 1..=3 {
            fx.deposit(user, "USDT", dec!(100000));
            fx.deposit(user, "BTC", dec!(10));
            fx.deposit(user, "ETH", dec!(10));
        }
        fx
    }

    fn run(&mut self, user: u64, command: Command) -> CommandResult {
        self.keys += 1;
        let envelope = CommandEnvelope::new(
            Actor::declared(user).unwrap(),
            RequestKey::new(&format!("req-{}", self.keys)).unwrap(),
            command,
        );
        self.writer.execute(&envelope)
    }

    fn deposit(&mut self, user: u64, asset: &str, amount: Decimal) {
        self.run(
            user,
            Command::Deposit {
                asset: asset.into(),
                amount,
            },
        )
        .unwrap();
    }

    fn place(
        &mut self,
        user: u64,
        symbol: &str,
        side: Side,
        price: Decimal,
        amount: Decimal,
    ) -> CommandResult {
        self.run(
            user,
            Command::PlaceOrder {
                symbol: symbol.into(),
                side,
                price,
                amount,
            },
        )
    }

    fn state(&self) -> &ExchangeState {
        self.writer.state()
    }

    fn free(&self, user: u64, asset: &str) -> Decimal {
        self.state().ledger().balance(user, asset).free
    }

    fn locked(&self, user: u64, asset: &str) -> Decimal {
        self.state().ledger().balance(user, asset).locked
    }

    fn invariants(&self) {
        self.state().check_invariants().unwrap();
    }
}

fn fills_of(tx: &CommittedTransaction) -> Vec<FillRecord> {
    match &tx.record.outcome {
        Outcome::OrderPlaced { fills, .. } => fills.clone(),
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn order_id_of(tx: &CommittedTransaction) -> u64 {
    match &tx.record.outcome {
        Outcome::OrderPlaced { order, .. } => order.order_id,
        other => panic!("unexpected outcome {other:?}"),
    }
}

#[test]
fn full_match_taker_consumes_maker() {
    let mut fx = Fixture::new();
    let maker = fx
        .place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();
    let taker = fx
        .place(1, "BTC_USDT", Side::Buy, dec!(105), dec!(1))
        .unwrap();

    let fills = fills_of(&taker);
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].maker_order_id, order_id_of(&maker));
    assert_eq!(fills[0].maker_user_id, 2);
    assert_eq!(fills[0].taker_user_id, 1);
    assert_eq!(fills[0].price, dec!(100));
    assert_eq!(fills[0].maker_fee.amount, dec!(0.1));
    assert_eq!(fills[0].taker_fee.amount, dec!(0.002));

    // Buyer: paid 100 USDT (price improvement refunded), got 1 BTC minus 0.2% fee.
    assert_eq!(fx.free(1, "USDT"), dec!(99900));
    assert_eq!(fx.locked(1, "USDT"), dec!(0));
    assert_eq!(fx.free(1, "BTC"), dec!(10.998));
    // Seller: 100 USDT minus 0.1% fee.
    assert_eq!(fx.free(2, "USDT"), dec!(100099.9));
    assert_eq!(fx.free(2, "BTC"), dec!(9));
    assert!(fx.state().engine().open_orders("BTC_USDT").is_empty());
    fx.invariants();
}

#[test]
fn partial_match_keeps_remaining_reservation() {
    let mut fx = Fixture::new();
    let maker = fx
        .place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();
    let taker = fx
        .place(1, "BTC_USDT", Side::Buy, dec!(100), dec!(0.4))
        .unwrap();
    assert_eq!(fills_of(&taker)[0].amount, dec!(0.4));

    let maker_id = order_id_of(&maker);
    let open = fx.state().engine().open_orders("BTC_USDT");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].remaining, dec!(0.6));
    assert_eq!(
        fx.state().ledger().reservation(maker_id).unwrap().remaining,
        dec!(0.6)
    );
    assert_eq!(fx.locked(2, "BTC"), dec!(0.6));
    fx.invariants();
}

#[test]
fn taker_walks_the_book_in_price_time_order() {
    let mut fx = Fixture::new();
    let first_at_100 = fx
        .place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();
    let at_101 = fx
        .place(3, "BTC_USDT", Side::Sell, dec!(101), dec!(1))
        .unwrap();
    let second_at_100 = fx
        .place(3, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();

    let taker = fx
        .place(1, "BTC_USDT", Side::Buy, dec!(102), dec!(2.5))
        .unwrap();
    let fills = fills_of(&taker);
    let makers: Vec<u64> = fills.iter().map(|f| f.maker_order_id).collect();
    assert_eq!(
        makers,
        vec![
            order_id_of(&first_at_100),
            order_id_of(&second_at_100),
            order_id_of(&at_101)
        ]
    );
    let prices: Vec<Decimal> = fills.iter().map(|f| f.price).collect();
    assert_eq!(prices, vec![dec!(100), dec!(100), dec!(101)]);
    assert_eq!(fills[2].amount, dec!(0.5));
    assert_eq!(
        fx.state().engine().open_orders("BTC_USDT")[0].remaining,
        dec!(0.5)
    );
    assert_eq!(fx.state().last_trade_price("BTC_USDT"), Some(dec!(101)));
    fx.invariants();
}

#[test]
fn failure_at_the_last_fill_leaves_book_and_balances_untouched() {
    let mut fx = Fixture::new();
    // 0.45 * 0.00000002 = 9e-9 cannot be represented in quote units, but only the last
    // fill of the taker below is affected: the order as a whole must be rejected.
    fx.place(2, "BTC_USDT", Side::Sell, dec!(0.42), dec!(0.000001))
        .unwrap();
    fx.place(3, "BTC_USDT", Side::Sell, dec!(0.45), dec!(1))
        .unwrap();
    let before = fx.state().fingerprint();
    let seq_before = fx.state().version();

    let result = fx.place(1, "BTC_USDT", Side::Buy, dec!(0.5), dec!(0.00000102));
    assert!(matches!(
        result,
        Err(ExchangeError::Ledger(LedgerError::Money(
            MoneyError::NotionalNotRepresentable { .. }
        )))
    ));
    assert_eq!(fx.state().fingerprint(), before);
    assert_eq!(fx.state().version(), seq_before);
    assert_eq!(fx.state().engine().open_orders("BTC_USDT").len(), 2);
    assert_eq!(fx.locked(1, "USDT"), dec!(0));
    assert!(fx.writer.sink().records.len() as u64 == seq_before);
    fx.invariants();
}

#[test]
fn two_markets_sharing_usdt_cannot_overdraw_or_lose_reservation() {
    let mut fx = Fixture::new();
    fx.deposit(4, "USDT", dec!(1000));
    let first = fx
        .place(4, "BTC_USDT", Side::Buy, dec!(100), dec!(8))
        .unwrap();
    assert_eq!(fx.locked(4, "USDT"), dec!(800));

    let before = fx.state().fingerprint();
    let second = fx.place(4, "ETH_USDT", Side::Buy, dec!(100), dec!(8));
    assert!(matches!(
        second,
        Err(ExchangeError::Ledger(
            LedgerError::InsufficientFreeBalance { .. }
        ))
    ));
    let withdraw = fx.run(
        4,
        Command::Withdraw {
            asset: "USDT".into(),
            amount: dec!(500),
        },
    );
    assert!(matches!(
        withdraw,
        Err(ExchangeError::Ledger(
            LedgerError::InsufficientFreeBalance { .. }
        ))
    ));
    assert_eq!(fx.state().fingerprint(), before);
    assert_eq!(fx.free(4, "USDT"), dec!(200));
    assert!(fx
        .state()
        .ledger()
        .reservation(order_id_of(&first))
        .is_some());

    fx.run(
        4,
        Command::CancelOrder {
            order_id: order_id_of(&first),
        },
    )
    .unwrap();
    fx.run(
        4,
        Command::Withdraw {
            asset: "USDT".into(),
            amount: dec!(1000),
        },
    )
    .unwrap();
    assert_eq!(fx.free(4, "USDT"), dec!(0));
    fx.invariants();
}

#[test]
fn cancel_checks_owner_and_releases_funds() {
    let mut fx = Fixture::new();
    let order = fx
        .place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(2))
        .unwrap();
    let id = order_id_of(&order);

    let denied = fx.run(1, Command::CancelOrder { order_id: id });
    assert_eq!(
        denied.unwrap_err(),
        ExchangeError::Forbidden { order_id: id }
    );
    assert_eq!(fx.locked(2, "BTC"), dec!(2));

    let cancelled = fx.run(2, Command::CancelOrder { order_id: id }).unwrap();
    assert!(matches!(
        cancelled.record.outcome,
        Outcome::OrderCancelled { user_id: 2, ref released_asset, released_amount, .. }
            if released_asset == "BTC" && released_amount == dec!(2)
    ));
    assert_eq!(fx.free(2, "BTC"), dec!(10));
    assert_eq!(fx.locked(2, "BTC"), dec!(0));
    assert_eq!(
        fx.run(2, Command::CancelOrder { order_id: id })
            .unwrap_err(),
        ExchangeError::UnknownOrder(id)
    );
    fx.invariants();
}

#[test]
fn self_trade_is_not_matched() {
    let mut fx = Fixture::new();
    fx.place(1, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();
    let taker = fx
        .place(1, "BTC_USDT", Side::Buy, dec!(100), dec!(1))
        .unwrap();
    assert!(fills_of(&taker).is_empty());
    assert_eq!(fx.state().engine().open_orders("BTC_USDT").len(), 2);
    fx.invariants();
}

#[test]
fn halted_unknown_and_invalid_orders_are_rejected_before_mutation() {
    let mut fx = Fixture::new();
    let before = fx.state().fingerprint();

    assert_eq!(
        fx.place(1, "DOGE_USDT", Side::Buy, dec!(1), dec!(1))
            .unwrap_err(),
        ExchangeError::MarketNotFound("DOGE_USDT".into())
    );
    for (price, amount) in [
        (dec!(-1), dec!(1)),
        (dec!(1), dec!(0)),
        (dec!(1), dec!(0.000000001)),
    ] {
        assert!(matches!(
            fx.place(1, "BTC_USDT", Side::Buy, price, amount),
            Err(ExchangeError::Money(_))
        ));
    }
    let mut halted = MarketConfig::new("BTC", "USDT", MarketPolicy::standard());
    halted.active = false;
    let before_halt = fx.state().fingerprint();
    fx.run(1, Command::ConfigureMarket(halted)).unwrap();
    assert_ne!(fx.state().fingerprint(), before_halt);
    assert_eq!(
        fx.place(1, "BTC_USDT", Side::Buy, dec!(1), dec!(1))
            .unwrap_err(),
        ExchangeError::MarketHalted("BTC_USDT".into())
    );
    assert_ne!(before, fx.state().fingerprint());
    fx.invariants();
}

#[test]
fn replay_of_serialized_records_reproduces_the_live_state() {
    let mut fx = Fixture::new();
    fx.place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();
    fx.place(3, "BTC_USDT", Side::Sell, dec!(101), dec!(1))
        .unwrap();
    fx.place(1, "BTC_USDT", Side::Buy, dec!(101), dec!(1.5))
        .unwrap();
    let resting = fx
        .place(1, "ETH_USDT", Side::Buy, dec!(50), dec!(2))
        .unwrap();
    fx.run(
        1,
        Command::CancelOrder {
            order_id: order_id_of(&resting),
        },
    )
    .unwrap();
    fx.run(
        2,
        Command::Transfer {
            from_asset: "BTC".into(),
            to_asset: "ETH".into(),
            amount: dec!(1),
        },
    )
    .unwrap();
    fx.run(
        1,
        Command::AdjustCapital {
            asset: "USDT".into(),
            delta: dec!(500),
        },
    )
    .unwrap();

    let mut replayed = ExchangeState::new();
    for record in &fx.writer.sink().records {
        let json = serde_json::to_string(record).unwrap();
        let decoded: TransactionRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(&decoded, record);
        replayed.apply_record(&decoded).unwrap();
    }
    assert_eq!(replayed.fingerprint(), fx.state().fingerprint());
    assert_eq!(replayed.version(), fx.state().version());
    assert_eq!(replayed.next_order_id(), fx.state().next_order_id());
    replayed.check_invariants().unwrap();
    assert_eq!(replayed.capital("USDT"), dec!(500));
}

#[test]
fn record_contains_owners_even_after_the_ownership_index_forgot_the_maker() {
    let mut fx = Fixture::new();
    let maker = fx
        .place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();
    let maker_id = order_id_of(&maker);
    assert_eq!(fx.state().order_owner(maker_id), Some(2));

    let taker = fx
        .place(1, "BTC_USDT", Side::Buy, dec!(100), dec!(1))
        .unwrap();
    assert_eq!(fx.state().order_owner(maker_id), None);
    let fill = &fills_of(&taker)[0];
    assert_eq!((fill.maker_user_id, fill.taker_user_id), (2, 1));
}

#[test]
fn record_serialization_is_versioned() {
    let mut fx = Fixture::new();
    let tx = fx
        .place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();
    let json = serde_json::to_value(&tx.record).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["actor"]["user_id"], 2);
    assert_eq!(json["outcome"]["kind"], "OrderPlaced");

    let mut fresh = ExchangeState::new();
    let mut future = fx.writer.sink().records[0].clone();
    future.version = RECORD_VERSION + 1;
    future.seq = fresh.next_seq();
    assert_eq!(
        fresh.apply_record(&future),
        Err(ExchangeError::UnsupportedVersion(RECORD_VERSION + 1))
    );
    assert_eq!(fresh.version(), 0);
}

#[test]
fn stale_prepared_transaction_is_rejected() {
    let mut state = ExchangeState::new();
    let envelope = |key: &str, amount: Decimal| {
        CommandEnvelope::new(
            Actor::declared(1).unwrap(),
            RequestKey::new(key).unwrap(),
            Command::Deposit {
                asset: "USDT".into(),
                amount,
            },
        )
    };
    let a = state.prepare(&envelope("a", dec!(1)), 1).unwrap();
    let b = state.prepare(&envelope("b", dec!(2)), 2).unwrap();
    state.commit(a).unwrap();
    assert_eq!(
        state.commit(b).unwrap_err(),
        ExchangeError::SequenceMismatch {
            expected: 2,
            found: 1
        }
    );
    assert_eq!(state.ledger().balance(1, "USDT").free, dec!(1));
}

#[test]
fn sink_failure_happens_before_commit() {
    let mut fx = Fixture::new();
    let before = fx.state().fingerprint();
    fx.writer.sink_mut().fail_next = true;
    let result = fx.place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(1));
    assert_eq!(
        result.unwrap_err(),
        ExchangeError::SinkFailed("injected sink failure".into())
    );
    assert_eq!(fx.state().fingerprint(), before);
    // The next command still gets the sequence the failed one would have used.
    let ok = fx
        .place(2, "BTC_USDT", Side::Sell, dec!(100), dec!(1))
        .unwrap();
    assert_eq!(ok.record.seq, fx.state().version());
    fx.invariants();
}

#[test]
fn full_queue_rejects_before_acceptance() {
    let (tx, mut writer) = command_queue(ExchangeState::new(), NoopSink, 1, Box::new(|| 1));
    let deposit = |key: &str| {
        CommandEnvelope::new(
            Actor::declared(1).unwrap(),
            RequestKey::new(key).unwrap(),
            Command::Deposit {
                asset: "USDT".into(),
                amount: dec!(5),
            },
        )
    };
    let accepted = tx.submit(deposit("one")).unwrap();
    assert_eq!(
        tx.submit(deposit("two")).unwrap_err(),
        ExchangeError::QueueFull
    );
    assert_eq!(writer.state().version(), 0);

    assert_eq!(writer.drain(), 1);
    assert_eq!(accepted.recv().unwrap().unwrap().record.seq, 1);
    assert_eq!(writer.state().ledger().balance(1, "USDT").free, dec!(5));
    // Space is available again once the writer made progress.
    assert!(tx.submit(deposit("three")).is_ok());
    drop(writer);
    assert_eq!(
        tx.submit(deposit("four")).unwrap_err(),
        ExchangeError::QueueClosed
    );
}

#[test]
fn transaction_lifecycle_phases() {
    let mut state = ExchangeState::new();
    let envelope = CommandEnvelope::new(
        Actor::declared(1).unwrap(),
        RequestKey::new("lifecycle").unwrap(),
        Command::Deposit {
            asset: "USDT".into(),
            amount: dec!(1),
        },
    );
    let prepared = state.prepare(&envelope, 10).unwrap();
    assert_eq!(prepared.phase(), TransactionPhase::Prepared);
    assert_eq!(state.version(), 0, "prepare must not change state");
    let committed = state.commit(prepared).unwrap();
    assert_eq!(committed.phase(), TransactionPhase::Committed);
    assert_eq!(committed.publish().phase(), TransactionPhase::Published);
}

#[derive(Debug, Clone)]
enum Op {
    Place {
        user: u64,
        buy: bool,
        price: u32,
        amount: u32,
        market: bool,
    },
    Cancel {
        user: u64,
        order: u64,
    },
    Withdraw {
        user: u64,
        amount: u32,
    },
    Deposit {
        user: u64,
        amount: u32,
    },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => (1u64..=3, any::<bool>(), 95u32..=105, 1u32..=300, any::<bool>())
            .prop_map(|(user, buy, price, amount, market)| Op::Place { user, buy, price, amount, market }),
        2 => (1u64..=3, 1u64..=40).prop_map(|(user, order)| Op::Cancel { user, order }),
        1 => (1u64..=3, 1u32..=5000).prop_map(|(user, amount)| Op::Withdraw { user, amount }),
        1 => (1u64..=3, 1u32..=5000).prop_map(|(user, amount)| Op::Deposit { user, amount }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Random command sequences never panic, every committed state satisfies the accounting
    /// invariants, rejected commands change nothing, and replay equals the live state.
    #[test]
    fn random_sequences_preserve_invariants_and_replay(ops in proptest::collection::vec(op_strategy(), 1..60)) {
        let mut fx = Fixture::new();
        for op in ops {
            let before = fx.state().fingerprint();
            let result = match op {
                Op::Place { user, buy, price, amount, market } => fx.place(
                    user,
                    if market { "BTC_USDT" } else { "ETH_USDT" },
                    if buy { Side::Buy } else { Side::Sell },
                    Decimal::from(price),
                    Decimal::new(i64::from(amount), 2),
                ),
                Op::Cancel { user, order } => fx.run(user, Command::CancelOrder { order_id: order }),
                Op::Withdraw { user, amount } => fx.run(
                    user,
                    Command::Withdraw { asset: "USDT".into(), amount: Decimal::from(amount) },
                ),
                Op::Deposit { user, amount } => fx.run(
                    user,
                    Command::Deposit { asset: "USDT".into(), amount: Decimal::from(amount) },
                ),
            };
            if result.is_err() {
                prop_assert_eq!(fx.state().fingerprint(), before);
            }
            prop_assert!(fx.state().check_invariants().is_ok(), "{:?}", fx.state().check_invariants());
        }

        let mut replayed = ExchangeState::new();
        for record in &fx.writer.sink().records {
            prop_assert!(replayed.apply_record(record).is_ok());
        }
        prop_assert_eq!(replayed.fingerprint(), fx.state().fingerprint());
    }
}
