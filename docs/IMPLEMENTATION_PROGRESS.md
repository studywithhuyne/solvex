# Tiến độ cải thiện

Nguồn: [IMPROVE_PLAN.md](IMPROVE_PLAN.md). Mỗi đợt tối đa 5 task; branch riêng, PR xếp nối tiếp theo phụ thuộc, không merge tự động. Tài liệu không ghi gate đã đạt khi chưa có bằng chứng thực thi.

| Task | Branch | Trạng thái | Bằng chứng / giới hạn |
|---|---|---|---|
| PLAN-01 | `codex/plan-01-contracts` | Implemented | [DECISIONS.md](DECISIONS.md): policy, money/ACK/privacy/migration; runtime chưa thực thi các hợp đồng tương lai |
| PLAN-02 | `codex/plan-02-baseline` | Implemented, validated | [BASELINE.md](BASELINE.md): 71 Rust tests pass; 14 Criterion workloads; storage fault harness chưa wire runtime |
| PLAN-03 | `codex/plan-03-demo-policy` | Implemented, validation in progress | [DEMO_POLICY.md](DEMO_POLICY.md); HTTP router thật + PostgreSQL test pass; TOCTOU chờ PLAN-10 |
| PLAN-04 | `codex/plan-04-proof-status` | Implemented, validated locally | [PROOF_STATUS.md](PROOF_STATUS.md); local SNARK verification chưa có; browser E2E còn bổ sung |
| PLAN-05 | `codex/plan-05-quality-gates` | Implemented locally, CI chưa chạy trên GitHub | [CI.md](CI.md): Cargo.lock/workflow tracked, `rust-toolchain.toml`, clippy + test blocking, fmt non-blocking (repo chưa rustfmt-clean); Docker build và PostgreSQL job **Not run** |
| PLAN-06 | `codex/plan-06-money-policy` | Implemented, validated locally | [MONEY_AND_EXCHANGE.md](MONEY_AND_EXCHANGE.md): `core/src/money.rs`, ledger `ChangeSet`; không cần migration (NUMERIC(30,8)) |
| PLAN-07 | `codex/plan-07-merkle-hash` | Implemented, validated locally | [PLAN07_MERKLE.md](PLAN07_MERKLE.md): format v2, zero padding (10+5+7=22), domain separation, parser chặt, circuit primitives; Poseidon params chưa có provenance chuẩn (regression pins), chưa có circuit Merkle path/trusted setup |
| PLAN-08 | `codex/plan-08-command-model` | Implemented, validated locally | `core/src/exchange/{command,record,writer}.rs`; chưa nối vào HTTP |
| PLAN-09 | `codex/plan-09-atomic-transition` | Implemented, validated locally | `plan_match` read-only + `ChangeSet` toàn lệnh + replay; chưa nối vào HTTP (PLAN-10) |

Đợt 2 (PLAN-05–09): `cargo test --workspace --locked` pass (core 78 + 7 baseline, zkp 77), `cargo clippy --workspace --all-targets --locked -- -D warnings` sạch.

PLAN-10–32 chưa triển khai. G-MONEY/G-INCLUSION/G-ZK chưa đạt. G-MATH: PLAN-06/07 đã có code và test cục bộ, chưa review độc lập. Đường HTTP vẫn dùng ledger/engine khóa riêng lẻ cho tới PLAN-10.
