# Trạng thái proof — PLAN-04

Đợt này chỉ sửa phạm vi tuyên bố. Payload version 1 là **prototype leaf commitment**; chưa phải membership vào root, aggregate liabilities hoặc reserve coverage. Seed 42/setup cũ và root gửi kèm proof không phải nguồn trust độc lập.

- JSON thiếu version: legacy, chỉ xem dữ liệu, chưa xác minh.
- Version/statement không hỗ trợ: unsupported; không fallback qua cờ server.
- Version 1/leaf commitment: verifier/context độc lập chưa sẵn sàng, trạng thái unavailable; không gọi là proof cryptographically invalid khi chưa kiểm tra.
- JSON/type/ID/hash shape sai hoặc quá giới hạn: invalid structure. Kiểm tra shape không thay kiểm tra field/group/circuit.
- `snark.verified`, `liabilities_leq_assets`, giá trị reserves từ query/env/RAM đều không tạo trạng thái verified. Không hiển thị SOLVENT/INSOLVENT từ comparison prototype.
- Panel proof và reserve comparison dùng logic chung. File/payload bị giới hạn; dữ liệu đổi sẽ xóa kết quả cũ. PLAN-18/19/23 bổ sung verifier thật và artifact/browser gate.

Admin tạo record `simulated_*` với marker `simulated:no-cryptographic-root`, không chế tạo chuỗi timestamp trông như Merkle root. History tách `simulated` và `legacy_unverified`; mọi record `trusted=false`. `users_observed` là số user quan sát được, không phải số leaf đã được inclusion chứng minh. Không tồn tại trusted snapshot trong prototype này.

Không sửa circuit/tree trong PLAN-04. Lỗi tổng cây lẻ và giới hạn completeness còn được ghi trong BASELINE/IMPROVE_PLAN. Node tests kiểm tra fake flag/legacy/unsupported/malformed/unavailable; kiểm tra browser thật chỉ được ghi pass khi chạy trên production bundle.
