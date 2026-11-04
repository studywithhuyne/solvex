mod support;

use matching_engine::{
    engine::{Engine, Order, Side},
    ledger::InMemoryLedger,
};
use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use support::*;

#[derive(Deserialize)]
struct Fixture {
    version: u8,
    balances: Vec<(i64, String, String)>,
}

fn fixture() -> InMemoryLedger {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/baseline-v1.json")).unwrap();
    assert_eq!(fixture.version, 1);
    let rows = fixture
        .balances
        .into_iter()
        .map(|(user, asset, free)| (user, asset, free.parse::<Decimal>().unwrap(), Decimal::ZERO))
        .collect::<Vec<_>>();
    InMemoryLedger::from_rows(&rows).unwrap()
}

fn open_orders(engine: &Engine) -> Vec<Order> {
    (1..=3)
        .flat_map(|user| engine.open_orders_by_user(user))
        .collect()
}

fn scenario(quantity: Decimal, expected_makers: &[u64]) {
    let mut ledger = fixture();
    let mut engine = Engine::new();
    let expected = totals(&observe(&ledger, vec![]));
    for maker in [
        Order::new(10, 2, "BTC_USDT", Side::Sell, dec!(100), dec!(1)),
        Order::new(11, 3, "BTC_USDT", Side::Sell, dec!(100), dec!(1)),
    ] {
        ledger.reserve_for_new_order(&maker, "BTC", "USDT").unwrap();
        assert!(engine.match_order(maker).unwrap().is_empty());
    }
    let taker = Order::new(12, 1, "BTC_USDT", Side::Buy, dec!(105), quantity);
    ledger.reserve_for_new_order(&taker, "BTC", "USDT").unwrap();
    let fills = engine.match_order(taker).unwrap();
    assert_eq!(
        fills
            .iter()
            .map(|fill| fill.maker_order_id)
            .collect::<Vec<_>>(),
        expected_makers
    );
    for fill in &fills {
        assert_eq!(fill.price, dec!(100));
        ledger.settle_trade(fill, dec!(0.001), dec!(0.002)).unwrap();
    }
    let state = observe(&ledger, open_orders(&engine));
    check_accounting(&state, &expected).unwrap();
    let filled: Decimal = fills.iter().map(|fill| fill.amount).sum();
    let resting: Decimal = state
        .orders
        .iter()
        .filter(|order| order.id == 12)
        .map(|order| order.remaining)
        .sum();
    assert!(check_volume(quantity, filled, resting));
}

#[test]
fn full_match_conserves_each_asset_including_fees() {
    scenario(dec!(1), &[10]);
}
#[test]
fn partial_match_preserves_remaining_reservation() {
    scenario(dec!(0.5), &[10]);
}
#[test]
fn walking_book_preserves_fifo_and_accounts_for_fees() {
    scenario(dec!(1.5), &[10, 11]);
}

#[test]
fn oracle_detects_balance_fee_and_reservation_corruption() {
    let initial = observe(&fixture(), vec![]);
    let expected = totals(&initial);
    check_accounting(&initial, &expected).unwrap();
    let mut bad = initial.clone();
    bad.balances.get_mut(&(1, "USDT".into())).unwrap().0 = dec!(-1);
    assert!(check_accounting(&bad, &expected).is_err());
    let mut bad = initial.clone();
    bad.fees.insert("BTC".into(), dec!(1));
    assert!(check_accounting(&bad, &expected).is_err());
    let mut bad = initial;
    let balance = bad.balances.get_mut(&(1, "USDT".into())).unwrap();
    balance.0 -= dec!(1);
    balance.1 += dec!(1);
    assert!(check_accounting(&bad, &expected).is_err());
    assert!(!check_volume(dec!(2), dec!(1), dec!(2)));
}

#[test]
fn failpoint_script_distinguishes_storage_and_ack_boundaries() {
    let boundaries = [
        FaultBoundary::SqlStatement,
        FaultBoundary::JournalWrite,
        FaultBoundary::JournalSync,
        FaultBoundary::SnapshotPublish,
        FaultBoundary::BeforeAck,
    ];
    for target in boundaries {
        let script = FaultScript { stop_at: target };
        for boundary in boundaries {
            assert_eq!(script.visit(boundary).is_err(), boundary == target);
        }
    }
}

#[test]
fn odd_tree_does_not_duplicate_funded_leaf() {
    use zkp::tree::{build_poseidon_merkle_sum_tree, BalanceSnapshot};
    let leaves = [10, 5, 7]
        .into_iter()
        .enumerate()
        .map(|(index, balance)| BalanceSnapshot {
            user_id: index as u64 + 1,
            balance: Decimal::from(balance),
        })
        .collect::<Vec<_>>();
    let tree = build_poseidon_merkle_sum_tree(&leaves).unwrap();
    // PLAN-07: zero padding replaces the legacy duplicated-leaf behaviour (was 29).
    assert_eq!(tree.root().balance, dec!(22));
}

proptest! {
    #[test]
    fn random_partial_and_multifill_conserve_assets(units in 1u32..=250) {
        let quantity = Decimal::new(i64::from(units), 2);
        scenario(quantity, if units <= 100 { &[10] } else { &[10, 11] });
    }
}
