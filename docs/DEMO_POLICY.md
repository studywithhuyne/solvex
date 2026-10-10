# Policy môi trường demo — PLAN-03

`CEX_MODE=maintenance` mặc định: chỉ đọc; chặn mutation, registration/login tiện ích demo, admin, simulator và mock deposit. `demo` là opt-in cho dữ liệu có thể tạo lại. Mode hoặc danh sách admin sai khiến startup lỗi.

| Đối tượng | Đọc tài khoản | Hủy order của mình | Place/deposit/withdraw/transfer/profile | Admin/simulator |
|---|---|---|---|---|
| ID thiếu/sai/không tồn tại | Từ chối | Từ chối | Từ chối | Từ chối |
| User hoạt động | Cho phép | Demo | Demo | Chỉ ID trong `CEX_ADMIN_USER_IDS` |
| User suspended | Cho phép | Demo | Từ chối | Từ chối |
| Maintenance | Cho phép đọc thông thường | Từ chối | Từ chối | Từ chối |

Middleware router áp dụng policy cả request trực tiếp và qua Nginx. Header `x-user-id` vẫn giả mạo được; `localStorage` không cấp quyền server. UI admin yêu cầu ID khai báo và kiểm tra server, không còn credential `admin/admin`. Audit route admin/simulator chỉ ghi actor khai báo, method, path không có query/body và HTTP status.

Backend native mặc định bind `127.0.0.1:3000`. Compose bỏ publish backend; ingress web, database và observability bind loopback. Backend bind `0.0.0.0` trong mạng Docker để Nginx truy cập. Không dựa vào IP Nginx/forwarded headers để cấp quyền. Dummy identity không phù hợp truy cập công khai.

Simulator không tự chạy; maintenance không spawn worker, demo cần admin start. Guard chưa sắp thứ tự suspend/halt với command tài chính: TOCTOU được giải quyết ở PLAN-10. Chưa đạt G-MONEY; giữ maintenance cho dữ liệu cần bảo tồn.

Database test: `docker-compose.test.yml`, project `cex-zkp-tests`, database `cex_test`, loopback port 15432, volume riêng. Không reset database đang có, không sửa migration cũ. Evidence được cập nhật trong `IMPLEMENTATION_PROGRESS.md`.
