/// ENG-09 — Criterion benchmarks for the matching engine core.
///
/// Goal: confirm that hot-path operations stay well below 10 µs on
/// release-mode hardware, giving us a quantitative baseline before the
/// async API / DB layers are added.
///
/// Benchmark groups
/// ────────────────
/// add_order          – insert into an empty book (baseline allocation cost)
/// cancel_order       – O(log P + Q) remove by ID
/// match_order        – no crossing, single full fill, N-level walk, FIFO walk
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use matching_engine::engine::{Order, OrderBook, Side};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

// ── helpers ──────────────────────────────────────────────────────────────────

fn mk_order(id: u64, side: Side, price: Decimal, amount: Decimal) -> Order {
    // Distinct maker/taker identities keep self-trade prevention out of fill workloads.
    Order::new(id, id, "BTC_USDT", side, price, amount)
}

// ── add_order ────────────────────────────────────────────────────────────────

/// Baseline cost of inserting a single resting order into an empty book.
fn bench_add_order(c: &mut Criterion) {
    c.bench_function("add_order/empty_book", |b| {
        b.iter_batched(
            OrderBook::new,
            |mut book| {
                book.add_order(mk_order(1, Side::Buy, dec!(100), dec!(10)))
                    .unwrap()
            },
            BatchSize::SmallInput,
        );
    });
}

// ── cancel_order  ────────────────────────────────────────────────────────────

/// Cost of cancelling the only resting order (O(log P + Q) with P = Q = 1).
fn bench_cancel_order(c: &mut Criterion) {
    c.bench_function("cancel_order/single", |b| {
        b.iter_batched(
            || {
                let mut book = OrderBook::new();
                book.add_order(mk_order(1, Side::Buy, dec!(100), dec!(10)))
                    .unwrap();
                book
            },
            |mut book| book.cancel_order(1).unwrap(),
            BatchSize::SmallInput,
        );
    });
}

// ── match_order: no crossing ──────────────────────────────────────────────────

/// Taker bid (100) cannot cross a resting ask (101) → taker rests on the book.
/// No iteration over the opposite side; measures pure conditional + insert cost.
fn bench_match_no_crossing(c: &mut Criterion) {
    c.bench_function("match_order/no_crossing", |b| {
        b.iter_batched(
            || {
                let mut book = OrderBook::new();
                book.add_order(mk_order(1, Side::Sell, dec!(101), dec!(10)))
                    .unwrap();
                book
            },
            |mut book| {
                book.match_order(mk_order(2, Side::Buy, dec!(100), dec!(10)))
                    .unwrap()
            },
            BatchSize::SmallInput,
        );
    });
}

// ── match_order: single full fill ────────────────────────────────────────────

/// 1 resting ask at 100, taker buys 10 @ 100 → exact full fill.
/// This is the common hot-path: one BTreeMap lookup + one VecDeque pop.
fn bench_match_single_full_fill(c: &mut Criterion) {
    c.bench_function("match_order/single_full_fill", |b| {
        b.iter_batched(
            || {
                let mut book = OrderBook::new();
                book.add_order(mk_order(1, Side::Sell, dec!(100), dec!(10)))
                    .unwrap();
                book
            },
            |mut book| {
                book.match_order(mk_order(2, Side::Buy, dec!(100), dec!(10)))
                    .unwrap()
            },
            BatchSize::SmallInput,
        );
    });
}

// ── match_order: walking N distinct price levels ──────────────────────────────

/// N asks at prices 100, 101, …, 100+N−1 (one order each).
/// Taker buys N units at price 100+N, consuming every level.
/// Measures BTreeMap iteration + repeated pop + order_map removal × N.
fn bench_match_walk_n_levels(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_order/walk_n_levels");

    for n in [1_u64, 5, 10, 50, 100] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut book = OrderBook::new();
                    for i in 0..n {
                        let price = Decimal::from(100_u64 + i);
                        book.add_order(mk_order(i + 1, Side::Sell, price, dec!(1)))
                            .unwrap();
                    }
                    book
                },
                |mut book| {
                    let taker_price = Decimal::from(100_u64 + n);
                    let taker_qty = Decimal::from(n);
                    book.match_order(mk_order(n + 1, Side::Buy, taker_price, taker_qty))
                        .unwrap()
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ── match_order: FIFO N makers at the same price level ───────────────────────

/// N asks all at price 100, each with qty 1.
/// Taker buys N at 100 → must drain the entire VecDeque FIFO.
/// Isolates VecDeque traversal cost from BTreeMap key iteration.
fn bench_match_fifo_n_makers(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_order/fifo_n_makers");

    for n in [1_u64, 5, 10, 50, 100] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut book = OrderBook::new();
                    for i in 0..n {
                        book.add_order(mk_order(i + 1, Side::Sell, dec!(100), dec!(1)))
                            .unwrap();
                    }
                    book
                },
                |mut book| {
                    let taker_qty = Decimal::from(n);
                    book.match_order(mk_order(n + 1, Side::Buy, dec!(100), taker_qty))
                        .unwrap()
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ── entry point ───────────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_add_order,
    bench_cancel_order,
    bench_match_no_crossing,
    bench_match_single_full_fill,
    bench_match_walk_n_levels,
    bench_match_fifo_n_makers,
);
criterion_main!(benches);
