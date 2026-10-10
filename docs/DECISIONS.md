# Hợp đồng cải thiện — PLAN-01

Ngày: 10/10/2026. Phạm vi: [IMPROVE_PLAN.md](IMPROVE_PLAN.md), [techstack.md](techstack.md), [AGENTS.md](../AGENTS.md). Đây là đặc tả cho các task tiếp theo; chưa phải bảo đảm của runtime hiện tại.

## 1. Demo và policy

- Chỉ hỗ trợ `x-user-id` là u64 dương trong môi trường phát triển/test. Không thêm JWT/OAuth/wallet login. Header vẫn giả mạo được; role mô phỏng không phải authentication.
- Mode `maintenance` mặc định: chỉ đọc, chặn mọi mutation và simulator. Mode `demo` là opt-in trên dữ liệu demo có thể tạo lại, chưa qua G-MONEY. Mode khác phải lỗi cấu hình, không tự chuyển sang demo.
- Các cổng publish Compose bind loopback. Backend không publish ra host; Nginx là ingress. Không suy ra mức tin cậy từ X-Forwarded-For hay IP của Nginx.
- Admin được xác định bằng danh sách ID trong cấu hình server, không bằng localStorage. ID phải tồn tại. Policy suspension lấy từ server: user suspended được đọc/cancel, không được place/deposit/withdraw/transfer hoặc sửa hồ sơ. Admin suspended không được gọi chức năng đặc quyền. Admin API/simulator/mock deposit chỉ hoạt động trong demo.
- Đăng ký/login hiện có chỉ là tiện ích demo, không cấp credential truy cập. Mọi thay đổi vẫn phải chịu policy của server. Thứ tự suspend/halt đối với command được giải quyết tại PLAN-10; guard ở PLAN-03 chưa loại TOCTOU.
- Audit ghi actor khai báo, action, target route, status; không ghi password/header bí mật/body tài chính. Không gọi actor là đã xác thực.

## 2. Tiền, phí và ID

| Tiêu chí | Hợp đồng cho PLAN-06 | Ví dụ |
|---|---|---|
| Asset units | Decimal, scale tối đa 8 trong phiên bản đầu; precision nhỏ hơn phải theo metadata asset | `1.00000001` hợp lệ với asset 8; `0.000000001` bị từ chối |
| Price/tick và quantity/lot | Tick và lot mặc định `0.00000001`; market có precision nhỏ hơn phải cấu hình rõ | Reject trước mutation, không rescale để làm tròn input |
| Range | Nonnegative balances; amount/price giao dịch > 0; exact units dùng checked u128, tối đa `10^18` units cho một số dư/giá/lượng; tổng dùng checked arithmetic | Negative/overflow/không chia hết đơn vị nhỏ nhất → reject |
| Notional | Tính Decimal checked; tổng quote debit phải biểu diễn chính xác trong quote units | Input tạo notional dưới đơn vị quote → reject thay vì tạo tài sản |
| Fee | Rate Decimal trong [0,1]; tính trên asset nhận; phí thực thu round về 0 tới precision asset; phần còn lại ở người nhận | Net received + fee = gross received; dust không bị biến mất |
| Fee account | Account riêng theo asset, tính trong conservation; không âm thầm bỏ phí khi persist | Revenue user 0 nội bộ không phải caller hợp lệ |
| IDs | Caller/order/sequence dương, không vượt i64::MAX tại SQL boundary; wire dạng chuỗi | API giữ u64 parse, checked conversion trước SQL |

Asset có hơn 8 decimals chưa được bật cho tài chính/proof mới trước migration reviewed. Rounding cho fee chỉ thực hiện một lần trong quyết định settlement; projection và replay áp dụng delta, không tính lại. Đây là giới hạn đồ án có thể thay bằng version mới sau dữ liệu/workload; không tự lấy decimals ngoài hệ thống làm quyền đổi schema.

## 3. Atomicity, ACK và recovery

- Matching vẫn sync/I/O-free trong RAM. Một writer điều phối toàn bộ mutation ở boundary; PostgreSQL chỉ async projection. File journal thuộc volume dự án, không thêm dịch vụ.
- Command gồm actor, request key, digest của payload canonical. Prepare toàn bộ book/ledger/reservation/fee trước commit; thất bại không đổi state. Journal sync bền vững trước publish RAM/ACK; không giữ engine lock qua `.await` hoặc I/O.
- Success = durable commit + state publish. Timeout không có nghĩa chưa commit: retry cùng key và payload trả outcome đã lưu; key khác payload → conflict. Không tự cấp key mới cho retry.
- Outcome và key được giữ theo lifetime journal/snapshot trong mốc đầu; không tự expire. Compaction chỉ sau snapshot chứa dedup và policy retention được nghiệm thu. Queue full/storage lỗi phải fail trước ACK thành công.
- Mốc đầu bảo vệ process crash khi volume còn nguyên, không hứa chống mất host/volume. Journal + dedup + projection + recovery cùng vượt G-MONEY mới bật write mới.

## 4. Statement và disclosure

1. Inclusion: account/balance nằm trong root đã pin cho asset/snapshot/sequence. Merkle path làm lộ sibling sums; phiên bản đầu không hứa privacy của tổng hay balance trong payload.
2. Aggregate: tổng đúng của dataset đã commit, mọi leaf/range/sum bị constrained; không tự chứng minh không bỏ sót nợ ngoài dataset.
3. Coverage: authenticated aggregate và reserves evidence cùng asset/mốc quy chiếu. Tổng công khai nên so sánh exact integers ngoài circuit. Không có evidence → unverified, dù query có `cold_wallet_assets` rất lớn.

Capacity nghiên cứu ban đầu 4096 leaves/asset (zero padding), chưa freeze circuit/setup trước PLAN-07/20/21 và benchmark; tăng capacity phải version lại. Manifest phải có publisher pin độc lập; nguồn trust/key provisioning là đầu ra PLAN-17, chưa chọn khóa/setup cuối trong đợt này. Poseidon parameters phải có provenance/vectors; Groth16 setup seed 42 là legacy không trusted.

## 5. Dữ liệu cũ và invariant

Dữ liệu đang có mặc định cần giữ: không reset DB, không sửa migrations lịch sử, không suy ra FIFO/reservations từ balance thiếu thông tin. Trước G-MONEY giữ maintenance nếu không xác nhận là dữ liệu demo. PLAN-15 inventory/reconcile/backup/restore rồi cutover; rollback không được xóa command đã ACK.

- Free/locked không âm; locked = reservations + holds theo asset/user.
- Order: 0 ≤ remaining ≤ amount, filled + remaining = amount; price-time FIFO, maker price, STP theo quy tắc hiện tại.
- Tổng tài sản users + fees/dust/capital thay đổi đúng external inflow/outflow; checker order volume độc lập checker tài sản.
- Command nguyên tử, sequence duy nhất; projection/replay idempotent; snapshot exact sequence không đồng nghĩa đọc DB ở thời điểm tùy ý.

## 6. Quyết định chưa freeze

| Mục | Default | Khi cần thay / task chịu trách nhiệm |
|---|---|---|
| Durable format/compaction | File journal, giữ dedup; chưa freeze bytes | PLAN-08/12/14 với checksum/version/crash evidence |
| Proof manifest trust | Publisher pin, unavailable nếu chưa có pin | PLAN-17; không dùng root cùng proof làm nguồn trust độc lập |
| Reserves thực | Simulated/unverified | PLAN-24 nguồn evidence được kiểm tra; không thêm wallet authentication |
| Capacity/targets | 4096 nghiên cứu; không SLA chưa đo | PLAN-21/26; đổi shape kéo theo key/version mới |
| SQL compile checking | Dedicated test PostgreSQL hoặc metadata project-local | PLAN-05 chuẩn bị job; PLAN-11 xác minh query mới, không kết nối DB chung |

Không có hai ý nghĩa ACK hoặc hai policy rounding. Default ở đây là đặc tả triển khai trong phạm vi AGENTS; mọi thay đổi default phải ghi rationale/version/test ở PR.
