# Tiến độ cải thiện

Nguồn: [IMPROVE_PLAN.md](IMPROVE_PLAN.md). Mỗi đợt tối đa 5 task; branch riêng, PR xếp nối tiếp theo phụ thuộc, không merge tự động. Tài liệu không ghi gate đã đạt khi chưa có bằng chứng thực thi.

| Task | Branch | Trạng thái | Bằng chứng / giới hạn |
|---|---|---|---|
| PLAN-01 | `codex/plan-01-contracts` | Implemented | [DECISIONS.md](DECISIONS.md): policy, money/ACK/privacy/migration; runtime chưa thực thi các hợp đồng tương lai |
| PLAN-02 | `codex/plan-02-baseline` | Pending | Fixtures, invariant checker, baseline và fault matrix |
| PLAN-03 | `codex/plan-03-demo-policy` | Pending | Mode/policy/ingress và audit |
| PLAN-04 | `codex/plan-04-proof-status` | Pending | Proof UI fail-closed và simulated snapshots |
| PLAN-05 | `codex/plan-05-quality-gates` | Pending | Locks, CI, Docker build và integration job |

PLAN-06–32 chưa triển khai. G-MONEY/G-INCLUSION/G-ZK chưa đạt. Đợt 1 không sửa matching algorithm hay tạo verifier/circuit mới.
