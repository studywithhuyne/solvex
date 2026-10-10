# Baseline và test matrix — PLAN-02

Ngày 10/10/2026. Fixture `core/tests/fixtures/baseline-v1.json` có 3 users, BTC/USDT/ETH; kiểm chứng settlement maker/taker fees, price improvement, full/partial/FIFO multifill. Checker độc lập tính locked từ resting orders và tổng tài sản bao gồm fee account; volume được kiểm tra riêng. Mutation tests phải bắt balance âm, fee giả, locked không có reservation và volume sai.

## Phân biệt evidence với known failures

- Test `known_failure_odd_tree_duplicates_funded_leaf` chỉ ghi nhận 10+5+7 đang ra 29. **Không phải tiêu chí Merkle đúng đã pass**; G-MATH vẫn blocked cho tới PLAN-07 sửa ra 22 và thay characterization bằng regression acceptance.
- `FaultScript` là harness test-only mô tả điểm tiêm lỗi, chưa wire vào runtime. PLAN-11: lỗi sau từng SQL; PLAN-12: write/sync/truncated record; PLAN-14: snapshot/checksum/replay; PLAN-13/15: durable-before-ACK/timeout/hard crash. Không claim đã kiểm thử process kill bằng script này.
- Whole-order atomicity, concurrent withdraw và restart replay chưa có contract/runtime mới; cần PLAN-09–15. Không dùng unit settlement atomic làm bằng chứng whole command.

## Matrix tiếp tục

| Nhóm | Kiểm chứng | Hiện tại / task |
|---|---|---|
| Engine/ledger | Full/partial/FIFO, fees, conservation, reservation; proptest | `core/tests/baseline.rs` |
| Money boundaries | Negative, precision, overflow, tick/lot/ID | PLAN-06 |
| Whole command | Late fill failure, simultaneous markets/withdraw | PLAN-09/10 |
| PostgreSQL | Migrations và rollback transaction trên database test riêng | PLAN-05 smoke; PLAN-11 fault projection |
| Durable state | write/sync/ACK crash, duplicate retry, replay FIFO/fees/IDs | PLAN-12–15; Not run trước khi có adapter |
| Proof | Tree total/constraints, malformed inputs, hostile witness | PLAN-07/20/21 |
| Browser | Fake verified flag, unsupported/legacy/WASM unavailable | PLAN-04 tests; PLAN-19/23 WASM thật |

Baseline workload Criterion: empty add, single cancel, no-crossing, single full fill, level walk/FIFO walk (`core/benches/engine_benchmark.rs`). Chạy release trên host hiện có, giữ report trong `target/criterion`; ghi toolchain, CPU và intervals thực đo. Không diễn giải matching-only thành API/journal latency, không đưa SLA trước PLAN-26.

## Kết quả thực chạy

- `cargo test --workspace` ngày 10/10/2026: **71 tests passed** (44 core unit tests, 7 baseline integration/property/characterization tests, 20 ZKP unit tests), 0 failed. Một ví dụ doc-test được đánh dấu ignore từ trước; không tính là test đã chạy.
- Host toolchain: `rustc 1.90.0`, `cargo 1.90.0`, Windows. Có cảnh báo không dọn được một số cache incremental trong `target`; build và tests vẫn hoàn tất, không sửa quyền host để xử lý cảnh báo.
- Bộ randomized baseline dùng input Decimal chính xác, không dùng float cho giá trị tài chính. Các mutation kiểm tra riêng conservation, reservation và volume.
- PostgreSQL/container/browser thật: **Not run** ở bước baseline này. Lệnh Docker chưa có trong PATH của terminal; không suy ra database hiện có là database test và không kết nối dữ liệu cần giữ.
- Benchmark Criterion sau khi sửa identity maker/taker khác nhau để tránh STP: release `opt-level=3`, LTO bật, 1 codegen unit; riêng tiến trình bench dùng `CARGO_PROFILE_RELEASE_PANIC=unwind` để tránh xung đột artifacts trên Windows. Lệnh mặc định trước đó **failed** với E0463 và cảnh báo output collision, không tính là lần đo thành công.
- Lần đo thành công: `cargo bench -p core --bench engine_benchmark -- --warm-up-time 1 --measurement-time 1 --sample-size 10`, 14 workloads; report trong `target/criterion`. Host `x86_64-pc-windows-msvc`, Rust 1.90.0 / LLVM 20.1.8; model CPU chưa thu thập. Kết quả là khoảng ước lượng Criterion, không phải p95/p99 request; sample ngắn và có outliers.

| Workload | Khoảng ước lượng thời gian |
|---|---|
| Empty add | 239.19–250.00 ns |
| Single cancel | 492.64–548.74 ns |
| No crossing | 330.64–397.65 ns |
| Single full fill | 479.00–559.08 ns |
| Walk 10 levels | 2.5243–2.6454 µs |
| Walk 50 levels | 13.108–15.313 µs |
| Walk 100 levels | 28.447–74.851 µs (outlier lớn) |
| FIFO 10 makers | 3.0118–3.1577 µs |
| FIFO 50 makers | 13.051–13.983 µs |
| FIFO 100 makers | 26.144–27.143 µs |

Không áp dụng mục tiêu <10 µs cho mọi kích thước lệnh và không lấy matching-only làm latency API/journal hay SLA. PostgreSQL test được bổ sung trong các task kế tiếp khi Docker sẵn sàng.
