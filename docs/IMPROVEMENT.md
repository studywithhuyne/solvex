# Brainstorm cải thiện CEX ZKP

Ngày đánh giá: **01/10/2026**. Trạng thái: **đề xuất để thảo luận, chưa triển khai**.

Tài liệu phân tích bảy nhóm điểm yếu từ lần đọc mã nguồn hiện tại, đưa ra phương án khắc phục, lý do lựa chọn, giới hạn và tiêu chí nghiệm thu. Những thiết kế, tên trường, endpoint, cờ cấu hình và bài kiểm thử được đề xuất dưới đây chưa tồn tại mặc nhiên trong dự án. Việc tạo tài liệu này không bao gồm sửa mã nguồn, migration, cấu hình, dependency hay workflow.

## 0. Phạm vi và hướng tiếp cận

### 0.1. Giữ những nền tảng đang phù hợp

- Giữ backend Rust monolith, Axum/Tokio, PostgreSQL/SQLx, frontend Svelte/Vite và hệ sinh thái Arkworks.
- Giữ matching đồng bộ trong RAM, không gọi database hoặc mạng bên trong thuật toán khớp lệnh.
- Giữ Decimal cho tính toán tài sản; thống nhất đơn vị và quy tắc làm tròn giữa API, ledger, database và circuit.
- Giữ dummy auth bằng `x-user-id` trong phạm vi dev/test đã mô tả trong [techstack.md](techstack.md). Cơ chế này không cung cấp xác thực danh tính; tài liệu không coi việc thiếu JWT là lỗi làm sai yêu cầu hiện tại.
- Chưa cần microservices, Kafka, Redis, Kubernetes hay thay framework để giải quyết các vấn đề được nêu.

Điểm cần phân biệt: “matching không có I/O” và “API được báo thành công trước khi dữ liệu bền vững” là hai quyết định khác nhau. Có thể giữ thuật toán matching thuần trong RAM, nhưng chờ journal được ghi bền vững trước khi xác nhận một giao dịch đã được chấp nhận.

### 0.2. Mức độ ưu tiên đề xuất

| Nhóm | Điểm yếu | Ưu tiên | Kết quả cần đạt |
|---|---|---|---|
| 1 | Danh tính giả lập, quyền admin và trạng thái tài khoản chưa được bảo vệ đầy đủ | P0 nếu mở ra mạng ngoài; P1 trong demo cô lập | Ranh giới demo rõ ràng; admin kiểm soát tại server; tài khoản bị khóa không thực hiện được thao tác bị cấm |
| 2 | UI tin cờ `verified` thay vì tự xác minh proof | P0 cho mọi tuyên bố xác minh độc lập | Kết quả UI phụ thuộc vào phép kiểm tra mật mã thực sự |
| 3 | Statement ZKP, cách tính tổng và nguồn reserves chưa đủ để chứng minh solvency | P0 cho tính đúng của proof | Định nghĩa đúng điều được chứng minh; tổng liabilities đúng; công khai các giả định ngoài mật mã |
| 4 | Matching, ledger và persistence không tạo thành một giao dịch thống nhất | P0 | Không cập nhật một phần, không bỏ mất sự kiện, không ghi nợ khi request bị từ chối |
| 5 | Thiếu phục hồi trạng thái sau restart | P0 trước khi coi dữ liệu là bền vững | Khôi phục lệnh, thứ tự, reservation, phí và ID nhất quán |
| 6 | Chi phí clone, khóa chung và công việc nặng giới hạn hiệu năng | P1, sau khi chốt tính đúng | Đo được bottleneck, giảm chi phí theo số dữ liệu thực sự bị ảnh hưởng |
| 7 | Kiểm thử, CI, build và tài liệu chưa chứng minh được các luồng hoàn chỉnh | P1, bắt đầu cùng các nhóm P0 | Mỗi tuyên bố có bài kiểm tra và bằng chứng tương ứng |

P0 nghĩa là cần xử lý trước khi dựa vào tính năng tương ứng; P1 nghĩa là quan trọng nhưng không nên tối ưu bằng cách bỏ qua tính đúng.

### 0.3. Cơ sở đánh giá và giới hạn

Theo kết quả đã báo cáo ở lượt review trước, 44 Rust tests của core và 20 tests của zkp đều pass; frontend checker báo 0 lỗi/0 cảnh báo; production build của frontend thành công. Đây là kiểm tra các thành phần hiện có, không phải bằng chứng hệ thống chịu được crash, database lỗi, request đồng thời hoặc prover độc hại. Chưa có kết quả chạy integration test với PostgreSQL trong lượt review đó; các test này không được chạy lại chỉ để tạo tài liệu brainstorm.

Các lỗi bổ sung phát hiện khi brainstorm được đặt vào bảy nhóm cũ, đặc biệt là padding của Merkle Sum Tree, constraint chưa liên kết và trạng thái theo dõi workflow trong Git. Không có sửa chữa nào được thực hiện trong bước này.

## 1. Danh tính, quyền admin và ranh giới demo

### 1.1. Những chỗ đang yếu

Nguồn chính: [auth.rs](../core/src/api/auth.rs), [router.rs](../core/src/api/router.rs), [admin.rs](../core/src/api/admin.rs), [adminAuthStore.ts](../web/src/stores/adminAuthStore.ts), [docker-compose.yml](../docker-compose.yml).

- `UserId::from_request_parts` chỉ kiểm tra header `x-user-id` là số nguyên dương. Caller tự chọn được người dùng mà request đại diện.
- Register/login có Argon2 nhưng không tạo thông tin xác thực được kiểm tra ở các request tiếp theo. Có mật khẩu băm không đồng nghĩa các API wallet/order đã được bảo vệ.
- Admin login kiểm tra `admin/admin` trong JavaScript và lưu `admin_token=true` ở localStorage. Đây chỉ là trạng thái giao diện; sửa localStorage không cần vượt qua một kiểm tra phía server.
- Các handler admin, kể cả halt market, suspend user và điều chỉnh treasury, chưa có kiểm tra quyền quản trị tại backend.
- `is_suspended` được lưu và hiển thị trong admin nhưng chưa được dùng để chặn login/order/wallet. Cần phân biệt thao tác “đánh dấu bị khóa” với việc thực thi chính sách khóa.
- Compose công bố nhiều cổng với credential mẫu; simulator và mock deposit nằm trong cùng ứng dụng với các chức năng giao dịch. Ranh giới môi trường thử nghiệm chưa được cưỡng chế rõ.

Ví dụ tác động: việc frontend ẩn nút admin không ngăn một client gọi thẳng API admin. Tương tự, kiểm tra “order thuộc user hiện tại” không bảo vệ được ownership nếu chính caller được tự khai user hiện tại.

### 1.2. Các phương án

| Phương án | Lợi ích | Hạn chế | Khi phù hợp |
|---|---|---|---|
| A. Demo cô lập, tiếp tục dummy auth | Đúng phạm vi hiện tại, chi phí thấp | Không có danh tính đáng tin cậy giữa những người dùng cùng truy cập | Đồ án, chạy local, dữ liệu giả |
| B. Thêm kênh quản trị được bảo vệ ở server, giữ user auth giả lập | Bảo vệ các thao tác phá trạng thái demo | Người dùng thường vẫn có thể giả danh nhau; credential mới là mở rộng đặc tả auth hiện tại | Demo chia sẻ có kiểm soát |
| C. Thiết kế session thật và quyền theo vai trò | Tạo nền tảng cho môi trường nhiều người dùng không tin cậy | Cần thay đổi đặc tả dummy auth, quản lý session và kiểm thử bảo mật | Giai đoạn mở rộng ngoài dev/test |

**Khuyến nghị:** trong scope hiện tại, chọn A và policy phía server để mô phỏng quyền đúng. Policy này vẫn có giới hạn giả mạo identity bằng header. B là nhánh tăng bảo vệ admin cần ghi nhận là mở rộng đặc tả nếu thêm credential mới; C là quyết định cho môi trường người dùng thật. Không thêm JWT/OAuth chỉ vì dependency đã xuất hiện trong Cargo.toml.

### 1.3. Cách khắc phục đề xuất

1. **Định nghĩa chế độ chạy rõ ràng.** Ví dụ cấu hình demo cho phép simulator, mock deposit và dummy identity; chế độ hạn chế không đăng ký các route này. Kiểm tra tại server, không chỉ ẩn trang frontend. Mặc định local bind hoặc mạng nội bộ; không mặc nhiên coi một biến tên “production” là đủ an toàn.
2. **Phân biệt policy demo và bảo vệ admin thực sự.** Trong scope dummy auth, tra user tồn tại, role và trạng thái từ server để mô phỏng policy, nhưng không coi `x-user-id == admin_id` là chống giả danh. Nếu chọn nhánh B, dùng điều kiện độc lập: tắt admin từ xa và chỉ vận hành qua kênh local được kiểm soát, hoặc thêm credential quản trị riêng do server kiểm tra. Credential lấy từ môi trường, không nhúng vào bundle hay commit; đây là một mở rộng đặc tả auth cần được ghi nhận trước đợt triển khai tương ứng. Hạn chế truy cập local phải được áp dụng ở published ingress/firewall/network route; backend thấy địa chỉ nội bộ của Nginx không chứng minh caller gốc là local.
3. **Gom chính sách truy cập tại backend.** Handler quản trị dùng một guard/extractor chung để tránh bỏ sót route. Chính sách phải áp dụng cả HTTP trực tiếp qua cổng backend, không chỉ đường qua Nginx.
4. **Thực thi suspension tại thời điểm xử lý command.** Quy định rõ tài khoản bị suspend có được đọc số dư, hủy lệnh để giải phóng reservation, hoặc rút tiền hay không. Đề xuất chặn lệnh mới và thao tác tăng rủi ro; quyền hủy/rút cần quyết định nghiệp vụ riêng. Nếu suspend đồng thời với place order, thứ tự command quyết định kết quả một cách nhất quán.
5. **Ghi audit cho thao tác đặc quyền.** Lưu người thực hiện, hành động, đối tượng, thời điểm, request ID và kết quả. Không ghi password, secret hay cookie. Audit log không tự làm cho danh tính dummy trở nên đáng tin cậy.
6. **Nếu sau này chọn session thật:** cân nhắc opaque session do server quản lý, cookie HttpOnly/Secure/SameSite phù hợp, expiry/revocation/logout và CSRF cho request thay đổi trạng thái. Quyền phải được lấy từ dữ liệu phía server. Đây là nhánh thiết kế tương lai, chưa được coi là thay đổi đặc tả đã chốt.

### 1.4. Vì sao hướng này giải quyết vấn đề

Ranh giới demo giúp sản phẩm không hứa một mức bảo vệ mà nó chưa có. Guard ở server khiến việc sửa UI/localStorage không quyết định policy thực hiện thao tác; chống giả danh đòi hỏi chọn thêm kênh quản trị độc lập hoặc auth thật. Một nguồn chính sách dùng chung giảm khả năng login, order và wallet diễn giải trạng thái tài khoản khác nhau.

### 1.5. Tiêu chí kiểm chứng

- Với dummy auth: ID không tồn tại hoặc role không phù hợp bị từ chối theo policy; khả năng giả mạo ID hợp lệ vẫn là giới hạn được ghi nhận của demo.
- Nếu đã chọn kênh quản trị độc lập/credential của B: gọi trực tiếp API admin khi thiếu hoặc sai điều kiện quản trị phải bị từ chối; giả `x-user-id` không đủ để vượt guard.
- Chỉ sửa `admin_token` trong localStorage không làm thay đổi quyền trên server.
- Trong demo, UI ghi rõ identity là giả lập; ngoài demo, route simulator/mock deposit không tồn tại hoặc bị từ chối rõ ràng.
- Suspension được kiểm tra với request bình thường và request đồng thời; hành vi hủy/rút khớp chính sách đã thống nhất.

## 2. Browser chưa thực hiện xác minh proof thực sự

### 2.1. Những chỗ đang yếu

Nguồn chính: [ZkpVerifierPanel.svelte](../web/src/components/zkp/ZkpVerifierPanel.svelte), [zkp-verify.ts](../web/src/lib/zkp-verify.ts), [zkp-wasm.ts](../web/src/lib/zkp-wasm.ts), [lib.rs](../zkp/src/lib.rs), [verifier.rs](../zkp/src/verifier.rs), [API zkp.rs](../core/src/api/zkp.rs).

- Panel và hàm có tên `runWasmVerification` hiện suy ra kết quả từ `parsed.snark?.verified === true`.
- Parser không bắt buộc proof bytes hợp lệ trước khi hiển thị valid. Một JSON có đủ các trường sơ bộ và cờ true có thể vượt qua luồng UI hiện tại.
- Module WASM hiện có chỉ kiểm tra Merkle path bằng cách tính lại hash. `snark` bị loại khỏi target wasm32; module này chưa phải Groth16 verifier.
- API tính Merkle path nhưng response hiện tại không trả `merkle_path`. Vì vậy chỉ nối loader WASM hiện có vào UI sẽ chưa làm luồng API–verifier hoạt động đúng.
- Root kỳ vọng được trả cùng payload với root cần kiểm tra. Hai trường giống nhau trong dữ liệu do cùng một bên gửi không tạo ra một nguồn tin cậy độc lập.

### 2.2. Hai hướng khả thi

| Hướng | Điều thực sự kiểm chứng | Ưu điểm | Đánh đổi |
|---|---|---|---|
| A. Hoàn thiện Merkle inclusion verifier hiện có | Số dư/người dùng được mở ra dẫn đến một root đã được chọn | Ít thay đổi hơn, tái sử dụng Rust/WASM hiện có | Lộ số dư và sibling sums; không tự chứng minh solvency hoặc toàn bộ tính hợp lệ của cây |
| B. Groth16 verifier thực sự trong browser | Proof hợp lệ cho circuit và public inputs cụ thể | Phù hợp hướng ZK dài hạn | Phải sửa statement ở nhóm 3, thêm verify-only WASM, quản lý VK và đo hiệu năng |

**Khuyến nghị:** A là mốc ngắn hạn có ý nghĩa nếu đặt tên đúng là “xác minh inclusion”. B là mốc tiếp theo sau khi circuit được thiết kế và kiểm tra. Không dùng kết quả server trả về làm fallback thành công khi WASM không tải được.

### 2.3. Cách triển khai dự kiến

1. Định nghĩa schema có version và loại proof rõ ràng: Merkle inclusion và Groth16 là hai định dạng khác nhau. Không dùng một boolean chung để che sự khác biệt này.
2. Với A, trả đủ leaf, path, sibling hash/sum, direction, asset, snapshot ID và root metadata. Browser lấy public inputs kỳ vọng từ snapshot đã chọn rồi gọi hàm Rust/WASM.
3. Với B, tách phần verify-only đủ điều kiện biên dịch sang WASM; deserialize proof/public inputs đúng định dạng; kiểm tra encoding, điểm đường cong/subgroup và kích thước bằng API của phiên bản thư viện đã chọn. Không tải proving key xuống browser.
4. Pin verification key hoặc hash của key theo circuit version trong một bản phát hành verifier đáng tin cậy. Không nhận bất kỳ VK nào đính kèm proof rồi tự tuyên bố proof có ý nghĩa với circuit mong muốn.
5. Lấy root/snapshot kỳ vọng từ manifest đã xác thực: ít nhất root công bố được lưu cố định theo snapshot; tốt hơn có chữ ký với public key được pin và bản lưu độc lập/auditor. Chữ ký của chính sàn xác định nguồn phát hành, không tự ngăn sàn công bố hai root khác nhau.
6. Hiển thị các kết quả tách biệt: dữ liệu không hợp lệ, lỗi tải verifier, proof không hợp lệ, proof hợp lệ cho snapshot X, và trạng thái reserves nếu có bằng chứng riêng. “Proof hợp lệ” không tự chuyển thành “sàn có khả năng thanh toán”.
7. Đưa verify vào Web Worker nếu đo thấy chặn main thread. Giới hạn dung lượng file, độ dài path, thời gian và số lần verify để dữ liệu không hợp lệ không làm treo giao diện.
8. Gom logic parse/verify dùng chung giữa panel và thư viện để tránh hai bản triển khai lệch nhau. Với hex đầu vào, kiểm tra ASCII/hex trước khi cắt chuỗi; cách cắt theo byte hiện tại có thể panic với Unicode không nằm trên biên ký tự.

### 2.4. Vì sao hướng này giải quyết vấn đề

Kết quả được tính lại trên máy người kiểm tra, dựa trên thuật toán và public inputs đã biết, thay vì tin vào phát biểu của server. Pin circuit/VK và root context ngăn tình huống một proof đúng cho dữ liệu hoặc circuit khác vẫn được trình bày là hợp lệ cho snapshot người dùng đang xem.

Một giới hạn cần ghi rõ: nếu sàn có thể thay toàn bộ JavaScript/WASM được phát tới người dùng, nó cũng có thể thay verifier. Muốn giảm giả định này cần bản verifier độc lập, build có thể tái lập, artifact có hash và hướng dẫn xác minh offline. Đây là lớp tin cậy bổ sung, không phải tính chất tự có của việc dùng WASM.

### 2.5. Tiêu chí kiểm chứng

- JSON chỉ có `verified: true` nhưng thiếu proof phải bị từ chối.
- Sửa proof bytes, root, user commitment, asset, snapshot ID hoặc circuit version phải làm xác minh thất bại khi các trường đó thuộc statement đã thiết kế.
- Proof đúng nhưng dùng VK khác phải thất bại; các public inputs không được bỏ qua hoặc hoán đổi thứ tự âm thầm.
- Native verifier và browser WASM cho cùng kết quả trên bộ vector đúng/sai chung.
- Thiếu hoặc hỏng WASM phải hiển thị trạng thái không xác minh được, không hiển thị valid.

## 3. Statement ZKP và dữ liệu solvency còn thiếu điều kiện

### 3.1. Những chỗ đang yếu

Nguồn chính: [snark.rs](../zkp/src/snark.rs), [tree.rs](../zkp/src/tree.rs), [circuit.rs](../zkp/src/circuit.rs), [poseidon.rs](../zkp/src/poseidon.rs), [API zkp.rs](../core/src/api/zkp.rs), [admin.rs](../core/src/api/admin.rs).

| Phát hiện từ mã nguồn | Hệ quả |
|---|---|
| `MembershipCircuit` chỉ ràng buộc user ID với leaf commitment của balance | Proof không chứng minh leaf nằm trong root, cũng không chứng minh tổng liabilities |
| `liabilities_leq_assets` được tính ngoài circuit | Kết quả so sánh chỉ là một tính toán phía server, không thuộc statement Groth16 hiện tại |
| Reserves có thể lấy từ `cold_wallet_assets`, biến môi trường hoặc treasury mô phỏng | Chưa có bằng chứng độc lập về tài sản thực sự được kiểm soát |
| Tree nhân đôi node cuối khi một tầng có số node lẻ | Tổng liabilities bị cộng dư; dữ liệu 10, 5, 7 tạo tổng 29 thay vì 22; trường hợp một leaf cũng bị nhân đôi |
| Test padding hiện kỳ vọng 29 cho ví dụ trên | Test pass đang củng cố hành vi tính tổng sai đối với mục tiêu cộng liabilities thật |
| `MerkleNodeRelationCircuit` cấp phát UInt128 và FpVar riêng mà chưa ràng buộc hai biểu diễn bằng nhau; carry đầu chưa bị ép bằng 0 | Range/overflow constraints chưa được liên kết đầy đủ với giá trị đi vào hash/sum; witness do chương trình hiện tại tạo đúng không đủ chứng minh circuit sound |
| Groth16 setup dùng `StdRng::seed_from_u64(42)` | Randomness của trusted setup có thể tái tạo từ thông tin công khai |
| MDS Poseidon được tạo bằng ma trận ngẫu nhiên rồi cộng đường chéo | Chưa có bằng chứng trong code rằng ma trận đáp ứng tính chất MDS và bộ tham số đạt mức an toàn mong muốn |
| Balance được rescale về 8 chữ số khi hash, trong khi đầu vào có thể có độ chính xác khác | Có nguy cơ số được cam kết khác số mà API/ledger hiểu; cần chính sách conversion chặt chẽ |
| API trả leaf balance; Merkle path có thể lộ sibling sums | Không thể mô tả toàn bộ gói trả về là giữ bí mật số dư chỉ vì kèm SNARK |
| Snapshot admin tạo chuỗi hash từ thời gian thay vì root thật | Lịch sử snapshot không phải bằng chứng đã cam kết dữ liệu liabilities |

Các phát hiện về constraints và MDS là thiếu điều kiện cần được kiểm tra chuyên biệt, không phải kết quả của một cuộc audit mật mã hoàn chỉnh hay một exploit đã được chạy.

### 3.2. Chốt điều cần chứng minh trước khi chọn circuit

Nên tách ba statement, với tên và bằng chứng riêng:

1. **Inclusion của một tài khoản:** commitment số dư của người dùng nằm trong snapshot S, cho asset A, dưới root R.
2. **Tính hợp lệ của tập liabilities đã cam kết:** mỗi số dư nằm trong miền hợp lệ, tổng không tràn và tổng của toàn bộ tập được cam kết bằng L.
3. **So sánh liabilities với reserves được xác nhận:** L không vượt quá R_assets, trong cùng đơn vị tài sản và bối cảnh thời gian phù hợp.

Một inclusion proof với các sibling sums không tự chứng minh rằng mọi leaf trong các sibling subtree đều không âm. Kể cả proof của toàn bộ cây đúng, mật mã cũng không tự biết sàn đã bỏ sót người dùng hoặc khoản nợ ngoài hệ thống. “Đầy đủ tập dữ liệu thực tế” là giả định/kiểm soát bổ sung cần được công bố.

### 3.3. Phương án ngắn hạn: chứng minh inclusion đúng và trình bày giới hạn

- Sửa padding thành các leaf rỗng có balance bằng 0, với domain riêng, pad tới kích thước chuẩn đã chốt; không nhân đôi một node có tiền. Quy định rõ cây rỗng và cây một leaf.
- Đặt leaf count, asset, đơn vị, tree depth, schema version và snapshot ID vào manifest; ràng buộc các giá trị liên quan vào statement hoặc commitment được proof xác nhận.
- Dùng snapshot có sequence cụ thể để mọi người trong cùng kỳ nhận proof dưới cùng root. Không build một root khác cho mỗi request rồi gọi chúng là cùng snapshot.
- Trình bày reserves nhập tay như một giả định mô phỏng; kết quả so sánh mang nhãn “theo reserves khai báo”.
- Snapshot admin lấy root thật từ cùng pipeline snapshot, không tạo root giả để lấp chỗ trống giao diện.

**Vì sao chọn mốc này:** sửa được các lỗi tính toán và tính trung thực dễ kiểm chứng trước, tạo test vectors dùng chung cho API, circuit và browser. Nó chưa đủ để tuyên bố solvency độc lập.

### 3.4. Phương án mục tiêu: statement có ràng buộc đầy đủ

**Thiết kế membership:** witness gồm balance, giá trị mở commitment nếu có, index/path và sibling data. Circuit tự tính leaf rồi tính từng parent theo direction đã ràng buộc, cuối cùng buộc kết quả bằng public root. User/account commitment, asset, snapshot ID và các tham số miền phải được gắn vào statement. Một public input được khai báo nhưng không tham gia constraint không tạo ra ràng buộc.

**Thiết kế tổng liabilities:** với phạm vi đồ án nhỏ, có thể dùng một circuit kích thước cố định hoặc nhiều mức capacity để kiểm tra toàn bộ leaf, range, padding và phép cộng của cây; public root và tổng phải thuộc cùng một tập constraints. Chi phí tăng theo số leaf và setup phụ thuộc hình dạng circuit. Một membership proof riêng dùng cùng root giúp người dùng kiểm tra phần của mình. Chưa ưu tiên recursion vì làm tăng đáng kể độ phức tạp.

Quy định một bản ghi duy nhất cho mỗi account/asset trong snapshot, cùng thứ tự canonical của leaf. Nếu statement tuyên bố không trùng tài khoản, circuit phải ràng buộc tính duy nhất/thứ tự tương ứng hoặc dựa vào một attestation được nêu rõ. Unique constraint của SQL hữu ích cho pipeline dữ liệu nhưng không tự ràng buộc một prover có thể tự tạo witness ngoài pipeline.

**Phương án thay thế:** nếu full aggregate circuit quá tốn, dùng audited snapshot/attestation về tổng và các điều kiện của dataset kết hợp inclusion proof, đồng thời ghi rõ auditor là một bên được tin cậy. Không gắn tên “full ZK solvency” cho tổ hợp có phần còn dựa vào attestation.

Nếu L và R_assets đã công khai và được xác thực bằng các bằng chứng đúng context, verifier có thể so sánh hai số nguyên ngoài circuit một cách hợp lệ. Circuit inequality cần thiết khi statement yêu cầu giấu tổng hoặc ràng buộc phép so sánh với các commitment riêng tư. Vấn đề của cách hiện tại là các giá trị và quan hệ cần thiết chưa được xác thực độc lập; không phải mọi phép so sánh ngoài circuit đều sai.

**Range và đơn vị:** chuyển amount thành số nguyên theo đơn vị nhỏ nhất đã được định nghĩa cho từng asset; từ chối phần thập phân vượt precision thay vì âm thầm làm tròn trong hasher. Đặt giới hạn cho balance, tổng và notional để không tràn Decimal, integer hay vòng modulo field. FpVar dùng để hash phải bằng biểu diễn được range-check; carry vào đầu bằng 0 và carry ra cuối bằng 0. Cân nhắc dùng gadget cộng/range đã được kiểm chứng thay cho full-adder tự viết, sau khi kiểm tra tính tương thích với phiên bản Arkworks hiện tại.

**Privacy:** nếu mục tiêu là giấu số dư đối với người quan sát, phải thiết kế response không công khai witness đó. Có thể dùng commitment có blinding và domain separation; phải giải quyết cách người dùng giữ/nhận giá trị mở commitment và cách ràng buộc với tài khoản của họ. Balance có miền nhỏ có thể bị đoán khi hash không có blinding. Không mặc định đổi user ID thành hash là đã ẩn danh.

### 3.5. Trusted setup và tham số hash

- Xóa fixed seed khỏi mọi setup được dùng để tuyên bố an toàn; seed cố định chỉ phù hợp test fixture được gắn nhãn rõ.
- Chọn circuit/version trước, tạo và lưu bộ tham số tương ứng; không tạo setup mới trên mỗi lần khởi động server.
- Một setup bằng randomness mật mã tạo offline có thể dùng làm bước nghiên cứu nhưng vẫn phải tin bên chạy setup đã hủy trapdoor. Khi cần giảm giả định tin cậy, dùng quy trình ceremony có transcript và nhiều bên đóng góp phù hợp Groth16.
- Proving key và verification key không phải “toxic waste”; chúng có thể công khai theo giao thức. Các giá trị bí mật dùng để sinh setup mới là phần phải bảo vệ/hủy. Lưu metadata liên kết circuit hash, VK hash, phiên bản tham số và transcript.
- Với Poseidon, dùng bộ tham số có nguồn gốc rõ ràng cho đúng field, width, round counts và alpha; đưa constants/version cùng vectors kiểm tra vào quá trình review. Không kết luận rằng mọi ma trận ngẫu nhiên cộng identity đều là MDS.

Seed dùng để tạo constants công khai của Poseidon không có cùng yêu cầu bí mật với randomness của Groth16 setup. Vấn đề ở Poseidon hiện tại là phương pháp tạo và kiểm chứng tham số, không phải bản thân việc constants hoặc seed được công khai.

Lý do fixed seed không phù hợp được đối chiếu với [mã sinh tham số của Arkworks](https://docs.rs/ark-groth16/latest/src/ark_groth16/generator.rs.html): các giá trị setup được lấy từ RNG. Link minh họa nguyên lý; dự án đang dùng Arkworks 0.3 nên API cụ thể cần đối chiếu theo phiên bản khi triển khai.

### 3.6. Xác nhận reserves và tính đầy đủ của liabilities

1. Xác định asset theo network/contract hoặc định danh duy nhất, không chỉ ticker. Không so tổng BTC với tổng USDT hoặc dùng quy đổi tùy ý để che thiếu hụt một tài sản.
2. Xác định nguồn reserves: số dư tại block cụ thể, địa chỉ được tuyên bố kiểm soát, bằng chứng quyền kiểm soát phù hợp blockchain; với tài sản lưu ký ngoài chuỗi có thể cần attestation riêng.
3. Liên kết reserves evidence với snapshot/time/asset; phát hiện dữ liệu cũ, sai chain và cùng tài sản bị đếm nhiều lần. Nếu có quy đổi, phải công bố giá, thời điểm và giả định của phương pháp định giá.
4. Công bố giới hạn: chữ ký từ ví không chứng minh tài sản không bị thế chấp, không vay tạm hoặc không gánh nghĩa vụ khác. Snapshot không phải bảo đảm liên tục trong tương lai.
5. Đối với liabilities, dùng một pipeline snapshot có tiêu chí chọn tài khoản rõ ràng, đối soát ledger với hồ sơ người dùng/nghĩa vụ và hỗ trợ người dùng kiểm tra inclusion. Một auditor hoặc quy trình đối soát bên ngoài có thể giảm rủi ro omission nhưng không biến phép chứng minh thành toàn tri.
6. Public manifest nên có snapshot ID, ledger sequence, asset/unit, count, root, tổng/commitment tổng, circuit/VK version, nguồn reserves và thời hạn. Cơ chế công bố/lưu manifest độc lập giúp phát hiện việc công bố nhiều root mâu thuẫn.

### 3.7. Tiêu chí kiểm chứng

- Tổng root bằng tổng các số dư thật với 0, 1, 2, 3, 5, 6 và nhiều số lượng ngẫu nhiên theo quy ước cây rỗng đã chốt; 10 + 5 + 7 phải ra 22.
- Test witness đối kháng: FpVar không khớp bits, carry đầu bằng 1, số âm được mã hóa trong field, overflow, direction/index sai, padding khác 0 đều bị từ chối.
- Sửa root/asset/snapshot/tổng đã ràng buộc phải làm proof thất bại; verify không chỉ chạy lại witness do honest prover sinh.
- Bộ vector hash và encoding thống nhất giữa native/WASM; decimal vượt precision bị từ chối có chủ đích.
- Reserves giả lập không được hiển thị như reserves đã xác nhận; thiếu reserve evidence không cho ra trạng thái solvency verified.
- Snapshot admin được tái tính từ đúng boundary dữ liệu và khớp manifest, không chỉ có hình thức giống hash.

## 4. Tính nhất quán giữa order book, ledger RAM và PostgreSQL

### 4.1. Những chỗ đang yếu

Nguồn chính: [orders.rs](../core/src/api/orders.rs), [ledger/mod.rs](../core/src/ledger/mod.rs), [worker.rs](../core/src/db/worker.rs), [wallet.rs](../core/src/api/wallet.rs).

- Place order giữ tiền, khớp lệnh và settlement ở ba pha khóa riêng. Không có một ranh giới nguyên tử cho cả thay đổi book lẫn ledger.
- `settle_trade` staging bằng cách clone toàn bộ ledger chỉ bảo vệ một fill. Nó không rollback book đã thay đổi hoặc các fill trước đó của một order nhiều fill.
- Handler bỏ qua lỗi gửi persistence event. Worker ghi log khi SQL lỗi rồi tiếp tục; event lỗi không được giữ lại để replay bền vững.
- Các bước insert trade, cập nhật filled, thay đổi các số dư và candles không cùng một transaction.
- Reservation và fee revenue chưa được project đầy đủ; công thức phí còn được tính lại ở worker từ các hằng riêng.
- Withdraw ghi nợ DB trước khi kiểm tra/trừ ledger RAM. Transfer có transaction DB nhưng commit trước khi cập nhật RAM; vì vậy transaction đó chưa bảo vệ sự nhất quán giữa hai lớp.
- Một số UPDATE không kiểm tra số dòng bị tác động. UPDATE không trúng balance row không nhất thiết phát sinh SQL error.
- Candle dùng thời gian worker xử lý thay vì thời gian khớp lệnh đã được ghi lại; backlog/replay có thể chuyển volume sang kỳ khác.

**Ví dụ lỗi cụ thể:** người dùng có 100 USDT và đặt buy cần giữ 80. Ledger RAM có free = 20, locked = 80, còn DB có thể vẫn available = 100. Withdraw 50 vượt qua kiểm tra DB và trừ còn 50; ledger từ chối vì free chỉ có 20. Client nhận lỗi nhưng DB đã ghi nợ. Chỉ đổi thứ tự thành “trừ RAM trước” cũng không giải quyết triệt để: DB lỗi sau đó sẽ tạo sự lệch theo chiều ngược lại.

**Ví dụ race:** sau khi matching giảm remaining của maker nhưng trước khi settlement cập nhật reservation, một request cancel khác có thể giải phóng reservation. Settlement sau đó thiếu dữ liệu cần thiết. Những lock riêng lẻ bảo vệ cấu trúc bộ nhớ, nhưng chưa bảo vệ toàn bộ nghiệp vụ.

### 4.2. Các phương án và mức bảo đảm

| Phương án | Giải quyết được | Chưa giải quyết/đánh đổi |
|---|---|---|
| A. Bọc SQL của worker trong transaction, bổ sung retry và event ID | Ghi DB một phần, một số trường hợp retry trùng | Event vẫn có thể mất trước khi xuống DB; RAM book/ledger vẫn có thể lệch |
| B. Một bộ xử lý command theo thứ tự + journal bền vững + PostgreSQL projection | Nguyên tử nghiệp vụ, thứ tự, replay và durability trong phạm vi storage đã định | Cần thiết kế journal/recovery; latency xác nhận có chi phí sync |
| C. Bộ xử lý command + PostgreSQL làm journal/nguồn commit bền vững | Tận dụng transaction/durability của DB, ít tự viết storage hơn | API ghi phụ thuộc latency/availability DB; cần thay đổi ràng buộc DB chỉ dùng async persistence |

**Khuyến nghị:** B là hướng mục tiêu phù hợp nhất nếu giữ PostgreSQL ngoài đường xác nhận giao dịch. A là giảm rủi ro ngắn hạn, không được gọi là đã xử lý hết nhóm 4. C đáng cân nhắc nếu nhóm ưu tiên giảm độ phức tạp storage và chấp nhận điều chỉnh đặc tả. Dù chọn C, hàm matching vẫn có thể thuần sync trong RAM; việc chờ DB thuộc lớp điều phối bên ngoài.

### 4.3. Một nơi quyết định mọi thay đổi giá trị

Đề xuất một bộ xử lý ghi theo thứ tự trong cùng tiến trình, sở hữu hoặc điều phối toàn bộ state giao dịch. Đây là single writer trong monolith, không phải một dịch vụ mới.

Mọi đường thay đổi tài sản đi qua cùng command pipeline: place/cancel, deposit/withdraw, transfer, simulator, thay đổi vốn, cấu hình phí và các trạng thái ảnh hưởng quyền giao dịch. API chỉ parse, kiểm tra sơ bộ và gửi command vào hàng đợi có giới hạn; validate quyết định dựa trên state tại lúc thực thi.

Luồng mục tiêu:

```text
Request + idempotency key
  -> bounded command queue
  -> validate against committed state
  -> prepare complete state transition
  -> durable journal commit
  -> publish committed state in RAM
  -> response + WebSocket

Durable journal
  -> asynchronous PostgreSQL projection
  -> query views / history / snapshot pipeline
```

Mỗi command phải bao trùm reservation, tất cả fills, giá khớp, phí, hoàn tiền do giá tốt hơn, resting remainder và index liên quan. Staging trên tập record bị tác động hoặc execution plan có kiểm tra đầy đủ; không làm mutation một nửa rồi hy vọng rollback một fill sẽ khôi phục cả hệ thống.

Sau durable commit, apply state đã được kiểm tra không được phát sinh lỗi nghiệp vụ bình thường. Nếu phát hiện lỗi nội bộ ở bước này, chuyển sang dừng nhận ghi và phục hồi từ journal; không tiếp tục phục vụ giao dịch trên state chưa khớp record đã commit.

### 4.4. Hợp đồng durability và idempotency

1. **Một record nghiệp vụ nguyên tử:** có sequence tăng đơn điệu, schema/logic version, command ID, request key, payload digest, execution timestamp, order/trade IDs và các delta đã quyết định. Ghi cả kết quả response cần thiết để retry trả về cùng kết quả.
2. **Ghi bền vững trước ACK:** dữ liệu chỉ nằm trong queue hoặc OS buffer chưa đáp ứng hợp đồng này. Với file journal, phải có framing/checksum, xử lý record ghi dở và yêu cầu đồng bộ storage phù hợp. `flush` bộ đệm không thay thế `sync_all`/cơ chế durability tương ứng. [Rust File documentation](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all)
3. **Định nghĩa ACK rõ:** HTTP success nghĩa là command đã commit theo hợp đồng storage. Khi timeout hoặc sync có kết quả chưa xác định, client không được suy ra command chắc chắn chưa xảy ra; tra cứu/retry bằng cùng idempotency key.
4. **Retry an toàn:** key có scope theo actor và loại nghiệp vụ. Cùng key, cùng payload trả lại cùng outcome; cùng key với payload khác bị từ chối. Lưu kết quả dedup bền vững để restart không xóa lịch sử chống trùng. Chốt thời hạn lưu và hành vi với retry sau thời hạn.
5. **Ghi kết quả đã quyết định:** lưu fee amount, rounding result, execution time và giá đã dùng. Replay không lấy `now()`, cấu hình phí hoặc giá thị trường mới để tính lại giao dịch cũ.
6. **Group commit nếu cần:** nhiều command có thể chia sẻ một lần sync, nhưng tất cả ACK tương ứng phải chờ durable boundary. Dữ liệu được commit phải là một prefix hợp lệ của sequence; không công bố state chưa commit.
7. **Một writer active:** khóa ownership của journal/state để hai instance không cùng cấp ID hoặc ghi vào cùng trạng thái. Nginx có khả năng proxy không đồng nghĩa có thể nhân đôi backend giao dịch tùy ý.
8. **Storage lỗi:** disk full, lỗi sync hoặc checksum bất thường làm hệ thống ngừng nhận ghi và báo trạng thái; không ACK rồi bỏ lỗi. Cần volume bền vững trong Docker, giám sát dung lượng và kế hoạch backup.

Chi phí cần chấp nhận: đường ACK có thêm thời gian durable write. Mục tiêu “không mất command đã ACK khi process crash” chỉ có nghĩa trong giả định volume/journal còn nguyên và storage thực hiện đúng hợp đồng đồng bộ. Mất máy hoặc mất cả volume đòi hỏi backup/replication và một mục tiêu phục hồi riêng; local journal không tự cung cấp bảo đảm đó.

### 4.5. PostgreSQL trở thành bản phục vụ truy vấn có thể dựng lại

Project một transaction record trong một DB transaction:

1. Claim event ID/sequence trong bảng dedup có unique constraint.
2. Nếu record đã được áp dụng, bỏ qua toàn bộ side effects của record đó.
3. Ghi orders, trades, ledger entries, reservation, free/locked và phí theo delta đã quyết định; kiểm tra các row kỳ vọng tồn tại.
4. Cập nhật checkpoint đã áp dụng trong cùng transaction rồi commit.

Transaction bảo đảm các thay đổi trong DB cùng thành công hoặc cùng rollback. Cơ chế này được mô tả trong [PostgreSQL Transactions](https://www.postgresql.org/docs/current/tutorial-transactions.html). Nó không tự tạo transaction xuyên RAM và DB; journal cùng state transition ở trên giải quyết ranh giới đó.

Không chỉ thêm `ON CONFLICT DO NOTHING` vào insert trade rồi vẫn chạy `filled += amount` hoặc cập nhật số dư: cách đó vẫn cộng trùng khi replay. Dedup phải bao quanh cả đơn vị nghiệp vụ và commit cùng các thay đổi.

- Lỗi tạm thời: giữ record, retry với backoff/jitter; không tiến checkpoint qua event bị lỗi.
- Lỗi schema, invariant hoặc dữ liệu không hợp lệ: dừng projection ở sequence đó, báo lỗi cần xử lý; không lặng lẽ bỏ qua để tiếp tục.
- Queue RAM chỉ là thông báo/cache; journal mới là nơi đọc lại sự kiện sau crash.
- Có thể tách candle/chart thành projection riêng với checkpoint riêng; lỗi chart không nên làm mất hoặc chặn vĩnh viễn sổ cái.
- Candle lấy execution timestamp gốc, tránh thay đổi kết quả khi backlog/replay.

### 4.6. Một mô hình kế toán và một policy precision

Thiết kế ledger entries có debit/credit hoặc delta đối ứng cho từng asset. Phí chuyển vào tài khoản phí của sàn; nạp/rút đi qua tài khoản nguồn/đích được định nghĩa. Có thể giữ balances là cache nhanh, nhưng cần entries có thể đối soát và dựng lại.

Với mỗi asset, giao dịch nội bộ phải bảo toàn tổng của người dùng cộng tài khoản phí; nạp/rút làm thay đổi tổng đúng lượng vào/ra đã ghi nhận. Không cộng BTC và USDT như những số cùng đơn vị. Nếu có withdrawal thật trong tương lai, pending withdrawal phải có trạng thái và liability/hold riêng, không biến mất khỏi nghĩa vụ chỉ vì đã gửi yêu cầu.

Tick size, lot size, precision và quy tắc làm tròn fee/notional phải được định nghĩa tập trung. `assets.decimals`, Decimal và `NUMERIC(30,8)` chưa tự bảo đảm tương thích. Dùng cùng kết quả đã quyết định cho RAM, event, DB và snapshot ZKP; tránh hai implementation settlement chạy song song.

### 4.7. Đọc dữ liệu trong lúc projection bị trễ

Balance tức thời và order state nên đọc committed state hoặc immutable read snapshot của writer. History đọc từ DB được phép trễ nhưng phải biết watermark, ví dụ `state_seq` và `last_applied_seq`, để không khiến người dùng hiểu nhầm lệnh đã mất.

Snapshot ZKP phải chốt tại sequence S. Có hai cách: chụp immutable state trực tiếp ở writer tại S; hoặc chốt một read snapshot DB nhất quán cùng checkpoint của chính transaction đọc đó. Nếu cần chính xác S, phải có cơ chế dừng tại S, versioned history hoặc snapshot materialization. Chỉ chờ DB đạt ít nhất S rồi SELECT số dư đang tiếp tục thay đổi không bảo đảm đọc đúng S.

### 4.8. Tiêu chí kiểm chứng

- Ví dụ 100 USDT, giữ 80, rút 50 phải bị từ chối mà không làm thay đổi bất kỳ state đã commit nào.
- Order nhiều fills lỗi ở fill cuối phải giữ nguyên cả book/ledger/reservations nếu command chưa commit; không chỉ rollback fill cuối.
- Place/cancel/withdraw cạnh tranh cùng số dư, kể cả trên nhiều market dùng chung USDT, không thể overdraft.
- SQL lỗi sau bất kỳ bước nào phải rollback cả projection record; retry không nhân đôi trade, fills, số dư, phí hoặc candle volume.
- Client timeout sau commit rồi retry cùng key nhận cùng order/result.
- Replay với đồng hồ hoặc cấu hình phí hiện tại đã thay đổi vẫn dựng lại đúng kết quả lịch sử.
- DB outage không làm mất event đã ACK; khả năng tiếp tục nhận ghi phải tuân theo giới hạn journal/backlog đã công bố.

## 5. Phục hồi sau restart và định danh bền vững

### 5.1. Những chỗ đang yếu

Nguồn chính: [state.rs](../core/src/api/state.rs), [main.rs](../core/src/main.rs), [ledger/mod.rs](../core/src/ledger/mod.rs), [worker.rs](../core/src/db/worker.rs), [orders.rs](../core/src/api/orders.rs).

- Startup tạo engine rỗng, order counter bắt đầu lại từ 1 và ownership/reference-price maps rỗng.
- Ledger chỉ nạp các hàng balance; reservations, thứ tự lệnh và các state khác chưa được dựng lại.
- Fee revenue chưa được lưu đầy đủ; vốn nền được khởi tạo từ hằng số. Điều chỉnh vốn trong RAM không phải một lịch sử vốn bền vững.
- Simulator tự chạy sau bootstrap, nên có thể tạo state mới trước khi một quy trình recovery đầy đủ tồn tại.
- `ON CONFLICT (order_id) DO NOTHING` có thể che việc tái dùng ID: lệnh mới tồn tại trong RAM nhưng DB giữ hàng của lệnh cũ.
- API dùng u64 còn PostgreSQL BIGINT là signed; cast `as i64` cần giới hạn/checked conversion, không chỉ chống trùng ID.

### 5.2. Phương án đề xuất: snapshot nhất quán + journal replay

Snapshot tại sequence S cần đủ dữ liệu để quyết định các command tiếp theo giống một hệ thống chưa restart:

| Nhóm state | Dữ liệu cần lưu hoặc dựng lại có kiểm chứng |
|---|---|
| Order book | Market, side, price, original amount, remaining, acceptance sequence/FIFO |
| Ledger | Free/locked theo asset, reservation theo order, holds khác, fee accounts, vốn |
| Index | Ownership, user-to-orders, user-to-balances; có thể rebuild từ dữ liệu chuẩn |
| Định danh | High-water mark order/trade/event, idempotency keys và outcomes cần giữ |
| Quy tắc | Market status, asset precision, fee/config versions và reference price cần cho validation |
| Metadata | Snapshot/schema version, sequence S, checksum/digest và journal position |

Quy trình recovery dự kiến:

1. Vào trạng thái recovering; chưa nhận command ghi và chưa khởi động simulator.
2. Chọn snapshot hoàn chỉnh hợp lệ mới nhất; kiểm tra version/checksum và sự liên kết với journal.
3. Khôi phục state tại S, replay các record đã commit từ S+1 theo thứ tự.
4. Record cuối ghi dở được xử lý theo quy tắc framing/commit marker đã thiết kế; checksum hỏng hoặc sequence gap ở giữa log phải làm recovery thất bại rõ ràng, không tự bỏ qua.
5. Đối soát book, reservation, balances, fee accounts và các invariant ở mục 4.
6. Khôi phục bộ đếm và dedup; resume projection; chỉ bật readiness và nhận ghi khi state đạt điều kiện đã chốt.

Không dùng luồng `match_order` thông thường để nạp resting orders, vì có thể tạo thêm trade ngoài ý muốn. Dùng đường restore không khớp lại, giữ acceptance sequence cho FIFO, hoặc replay event đã quyết định trên snapshot chuẩn.

### 5.3. ID và retry qua restart

Lấy high-water mark từ durable state/journal. `MAX(order_id) + 1` từ DB chỉ là cách tạm khi DB đã đồng bộ hoàn toàn và có đúng một writer; trong mô hình async projection, DB có thể chưa biết ID đã ACK.

Chốt miền ID phù hợp cả Rust, SQL và frontend; dùng checked conversion và xử lý overflow. API nên truyền ID dạng chuỗi khi có thể vượt miền số nguyên an toàn của JavaScript. ID tăng đơn điệu trong một writer là đủ cho scope hiện tại; UUID không tự giải quyết thứ tự replay hoặc mất event.

Key chống trùng phải sống qua restart. Nếu client mất response sau khi server commit, request gửi lại phải tìm thấy outcome cũ thay vì cấp một order mới.

### 5.4. Ghi snapshot, giữ journal và backup

- Snapshot phải đại diện một boundary nhất quán. Có thể chụp immutable state nhanh rồi serialize ở tác vụ riêng; không đọc lần lượt book và ledger ở hai thời điểm khác nhau.
- Ghi artifact mới, hoàn tất nội dung và durability, rồi mới công bố manifest/checkpoint tương ứng. Giữ snapshot cũ để fallback. Cách publish phải được kiểm thử trên filesystem/OS chạy thực tế; không mặc định mọi rename đều có cùng bảo đảm sau mất điện.
- Chỉ xóa journal segment khi snapshot recovery, tất cả projection cần thiết và chính sách backup đã vượt boundary đó. Phải còn một đường dựng lại history nếu journal cũ bị prune.
- Backup phải được kiểm tra bằng restore rehearsal. Cần phân biệt phục hồi process crash, volume corruption và mất toàn bộ máy.
- Thêm readiness cho recovery/projection health bên cạnh liveness; một endpoint trả `ok` không chứng minh backend đã sẵn sàng nhận lệnh.

### 5.5. Phương án tạm cho demo và chuyển đổi dữ liệu cũ

Nếu chưa làm replay, có thể chọn chính sách startup vào maintenance, đối soát dữ liệu và hủy các open order cũ theo quy trình có log rồi giải phóng đúng reservation đã xác định. Phương án này chấp nhận mất tính liên tục của thị trường và phải được ghi rõ; không được gọi là restore order book.

DB hiện tại có thể đã thiếu event hoặc không có reservation tương ứng, nên không thể suy ra chắc chắn mọi state cũ. Khi chuyển thiết kế, cần một lần dừng giao dịch, backup, đối soát balances/orders/fees, giải quyết chênh lệch và tạo baseline đã xác nhận. Không “repair” bằng cách bịa số dư hoặc xóa volume để che chênh lệch.

### 5.6. Tiêu chí kiểm chứng

- Crash trước journal write, giữa record, sau durable commit trước RAM apply, trước ACK, giữa DB transaction và lúc publish snapshot đều có kết quả recovery đã định nghĩa.
- Restart nhiều lần giữ nguyên remaining, FIFO, reservation, fee revenue, capital và ID high-water mark; không sinh trade mới do restore.
- State sau replay khớp state chạy liên tục tại cùng committed sequence.
- Journal gap/checksum sai, snapshot sai version, disk full hoặc volume không sẵn sàng phải chặn nhận ghi rõ ràng.
- Mục tiêu thời gian phục hồi được đo theo số lệnh/event thực tế; chưa đặt con số RTO/RPO nếu chưa có workload và storage assumptions.

## 6. Hiệu năng của toàn bộ đường xử lý

### 6.1. Những chỗ đang yếu

Nguồn chính: [ledger/mod.rs](../core/src/ledger/mod.rs), [engine.rs](../core/src/engine/engine.rs), [state.rs](../core/src/api/state.rs), [orders.rs](../core/src/api/orders.rs), [worker.rs](../core/src/db/worker.rs), [API zkp.rs](../core/src/api/zkp.rs), [ws.rs](../core/src/api/ws.rs).

- Engine và ledger có khóa dùng chung toàn hệ thống; read lock vẫn có thể cản write lock.
- Mỗi fill clone toàn bộ ledger, gồm balances và reservations; chi phí phụ thuộc tổng state thay vì chỉ các tài khoản của trade.
- Truy vấn balances/open orders theo user quét các cấu trúc lớn rồi lọc.
- “Batch” worker hiện gom event vào Vec rồi chạy tuần tự nhiều SQL; chưa tương đương một bulk operation giảm round trips.
- Resolve market dùng DB trên đường place order; metadata ít đổi vẫn có thể tạo độ trễ theo request.
- Mỗi mutation phát depth snapshot, mọi connection nhận sự kiện toàn thị trường; client chậm bị bỏ message nhưng chưa có protocol sequence/resync đầy đủ.
- API proof đọc toàn bộ số dư, xây cây và thực hiện công việc mật mã trong request async. Cache có root trong key kiểm tra nhưng root thường đổi không làm mất chi phí xây cây trước đó.
- Ownership/cache cần lifecycle rõ; giữ metadata cho order đã hoàn tất mãi sẽ làm RAM tăng theo lịch sử.

### 6.2. Thứ tự tối ưu đề xuất

| Thứ tự | Thay đổi | Cơ chế giảm chi phí | Đánh đổi cần kiểm tra |
|---|---|---|---|
| 1 | Staging/write set theo toàn command và chỉ record bị tác động | Tránh clone toàn ledger cho mỗi fill | Phải giữ tính nguyên tử và invariant của cả order |
| 2 | Single writer + read snapshots | Giảm tranh chấp khóa, thứ tự state rõ ràng | Throughput ghi có trần; command lớn có thể chặn hàng đợi |
| 3 | Index theo user, cleanup active-order metadata | Tránh quét toàn bộ state và tăng RAM theo lịch sử | Mọi mutation/restore phải cập nhật hoặc rebuild index đúng |
| 4 | Cache market metadata có version | Giảm DB lookup lặp lại | Halt/config change phải đi qua cùng thứ tự command |
| 5 | Transaction/batch projection hợp lý | Giảm round trips, dedup tập trung | Batch lớn tăng lock time và độ trễ nhìn thấy dữ liệu |
| 6 | Snapshot ZKP theo kỳ, bounded worker cho proof | Không xây lại cùng cây trên từng request; không chặn HTTP executor | Có độ cũ snapshot phải hiển thị; cache cần giới hạn dung lượng |
| 7 | Gộp depth update, subscription theo market, sequence/resync | Giảm serialize/broadcast và phục hồi client chậm | Không làm mất semantics của trade/order outcome |

### 6.3. Các chi tiết cần giữ đúng

- Write set phải bao trùm tất cả maker bị khớp trong cùng order; tối ưu clone từng fill mà không sửa boundary nghiệp vụ vẫn để lại lỗi nhóm 4.
- Order/trade event phải tự chứa ownership cần thiết trước khi xóa active-order index; worker không phụ thuộc map đã cleanup.
- Các công việc CPU nặng chạy trong worker pool hoặc `spawn_blocking` có giới hạn concurrency; không mở vô hạn tác vụ chứng minh cho mọi request. Việc dừng chờ response không mặc nhiên hủy được công việc CPU đã bắt đầu.
- Cache proof/tree theo snapshot ID, asset, circuit/VK version và dữ liệu witness liên quan; hết hạn có chủ đích. Không trả proof của root khác để tăng cache hit.
- Depth snapshot có sequence của state. Client phát hiện gap hoặc reconnect thì lấy snapshot mới; tránh snapshot REST cũ ghi đè một WebSocket update mới hơn.
- Với broadcast full snapshots, lần update tiếp theo có thể tự sửa depth, nhưng recent trades bị mất vẫn cần cơ chế lấy lại nếu UI hứa hiển thị đầy đủ.
- Giới hạn queue, kích thước request và outstanding orders; reject quá tải trước khi mutate state. Không âm thầm cắt bớt fills của một order mà chưa định nghĩa lại order semantics.
- Các số tiền trong API/ledger/proof giữ dạng decimal string hoặc integer units. JavaScript Number chỉ dùng cho hiển thị/chart khi chấp nhận mất precision, không làm nguồn số tiền quyết định settlement.

### 6.4. Vì sao chưa nên chia engine theo market ngay

BTC_USDT và ETH_USDT cùng tiêu một số dư USDT. Chia book thành nhiều writer mà không điều phối reservation chung có thể tạo double spend dù từng market chạy đúng. Khuyến nghị tối ưu write set, read snapshots và pipeline một writer trước; chỉ xét partition khi benchmark cho thấy cần, với thiết kế tài khoản dùng chung và atomicity được chứng minh.

### 6.5. Cách đo và tiêu chí kiểm chứng

Tách thời gian khớp thuần khỏi end-to-end latency. Đo p50/p95/p99 cho queue wait, matching, settlement, durable commit, response; đo projection lag, proof generation/verification time, memory growth và recovery time.

Workload cần gồm nhiều user, nhiều market, order không khớp, partial fills, order khớp nhiều maker, cancel/withdraw cạnh tranh, DB chậm và client WebSocket chậm. Ghi phần cứng, build profile, kích thước state và phiên bản code; chưa khẳng định SLA microsecond cho toàn API chỉ từ Criterion của order book.

Tiêu chí đề xuất: tăng mạnh số tài khoản không tham gia trade không được làm chi phí settlement tăng tuyến tính như full clone hiện tại; RAM ổn định khi active orders ổn định dù tổng lịch sử tăng; proof load không làm độ trễ đặt lệnh tăng không kiểm soát. Ngưỡng số cụ thể được chốt sau baseline, không tự đặt thành cam kết chưa đo.

## 7. Kiểm thử, CI, build và tài liệu chưa phản ánh đủ mức hoàn thiện

### 7.1. Những chỗ đang yếu

Nguồn chính: [wbs.csv](wbs.csv), [README](../README.md), [.gitignore](../.gitignore), [core Dockerfile](../core/Dockerfile), [web Dockerfile](../web/Dockerfile), [package.json](../web/package.json), [order_book.rs](../core/src/engine/order_book.rs).

- Roadmap ghi WASM verifier/UI verification là DONE trong khi luồng giao diện đang tin boolean. Thành phần có tồn tại chưa đồng nghĩa đã được tích hợp và kiểm chứng end-to-end.
- Snapshot admin giả chưa được phân biệt đủ rõ với artifact mật mã thật.
- README dùng ảnh badge build-passing/coverage tĩnh, không phải bằng chứng từ một pipeline đang chạy.
- Trong checkout đã kiểm tra, `.github/workflows/deploy.yml` có ở máy nhưng cả thư mục workflow bị `.gitignore` bỏ qua; `git ls-files` không trả workflow hoặc Cargo.lock. Vì thế chưa có cơ sở kết luận workflow local đang chạy trên remote.
- Workflow local chủ yếu build/push image trên main/manual, chưa có PR quality gate. Không thể chỉ thêm lệnh test vào một file bị ignore rồi coi CI đã được thiết lập.
- Cargo.lock bị ignore; Docker backend không copy lock hoặc dùng `--locked`. Docker frontend dùng `npm install` sau khi copy riêng package.json, chưa dùng package-lock để khóa layer dependency.
- Frontend package scripts chưa có check/test; phần lớn tests hiện có thuộc engine, ledger hoặc primitive ZKP, chưa bao phủ đầy đủ API/database/restart/browser.
- Property test của order book kiểm tra bảo toàn khối lượng lệnh/matching; chưa phải kiểm tra bảo toàn tài sản của toàn ledger gồm phí/nạp/rút như mô tả rộng trong roadmap.
- README đưa `docker compose down -v` vào hướng dẫn khi migration đổi. Đây là xóa dữ liệu demo, không phải chiến lược nâng cấp giữ dữ liệu.

### 7.2. Định nghĩa hoàn thành dựa trên bằng chứng

Đề xuất mỗi hạng mục có ba mức: **prototype** (thành phần tồn tại), **integrated** (luồng thật gọi thành phần đó), **verified** (acceptance tests và artifact xác nhận luồng). Không thay mọi dòng bằng DONE chỉ vì build thành công.

Mỗi hạng mục ghi: điều người dùng nhận được, đoạn mã thực thi, test kiểm chứng, trạng thái demo/thật và giới hạn còn lại. Ví dụ WASM crate tồn tại là prototype; panel gọi verifier là integrated; browser production bundle từ chối proof giả là verified.

Snapshot pipeline nên có trạng thái queued/building/ready/failed; chỉ ready khi artifact thật đã được tạo và kiểm tra. Nếu vẫn cần mock để demo UI, dùng loại/status riêng và không cho verifier lấy mock làm trusted snapshot.

### 7.3. Kiểm thử theo rủi ro

| Tầng | Tình huống cần bổ sung | Vì sao cần |
|---|---|---|
| Unit/domain | Validate precision, fees, price improvement, reserve/cancel, ledger entries | Giữ quy tắc nghiệp vụ độc lập với HTTP/DB |
| Property test ledger | Chuỗi random deposit/order/cancel/fill/withdraw nhiều user/asset | Kiểm tra bảo toàn từng asset, không âm, reservation khớp lệnh |
| API/policy | Gọi admin trực tiếp, suspension, route demo, retry key | UI không phải ranh giới quyền và client có thể retry |
| PostgreSQL integration | Migrations, transaction rollback, missing rows, dedup, DB outage | Mock không tái hiện đầy đủ transaction/isolation/constraint |
| Concurrency | Place/cancel/withdraw cùng tài khoản, hai market cùng quote asset | Bắt lỗi do interleaving giữa book và ledger |
| Crash/recovery | Dừng process ở các boundary journal/snapshot/ACK | Kiểm chứng durability và replay thay vì chỉ kiểm tra shutdown đẹp |
| Cryptographic constraints | Witness đối kháng, padding, overflow, metadata/VK sai | Honest witness tests dễ bỏ sót constraint thiếu |
| Browser integration | Bundle release tải WASM, proof đúng/sai, lỗi loader | Bảo đảm UI chạy verifier thật, không chỉ pass kiểm tra TypeScript |
| Performance | End-to-end, memory, queue/projection lag | Tránh suy diễn benchmark engine thành hiệu năng toàn sàn |

Dùng PostgreSQL thật trong môi trường test riêng bằng Docker Compose hoặc cấu hình test tương đương. Fault injection đặt tại các ranh giới quan trọng và phải có kết quả mong đợi rõ ràng. Không chạy thử nghiệm phá dữ liệu trên database demo đang dùng để trình diễn.

### 7.4. Pipeline đề xuất

1. Đưa workflow và các lockfile phù hợp vào Git; phân biệt file local với file thực sự nằm trong repository.
2. Pull request chạy format/lint Rust, Rust tests, Svelte/TypeScript check và frontend production build. Các cảnh báo cũ cần baseline/lộ trình xử lý rõ để gate có thể được duy trì.
3. Chạy integration stage với PostgreSQL, apply migrations và kiểm tra các luồng tài chính quan trọng.
4. Build WASM từ Rust source tương ứng; kiểm tra một proof vector trên artifact đã build, sau đó kiểm tra browser ở luồng cần thiết.
5. Chỉ publish image sau khi các gate bắt buộc pass. Job test dùng quyền đọc tối thiểu; quyền ghi package thuộc job publish được giới hạn đúng nhánh/ngữ cảnh.
6. Tách benchmark/fault-injection nặng sang lịch hoặc trigger phù hợp, nhưng giữ các regression tests quan trọng đủ nhanh để chạy trên PR.

**Vì sao chọn:** giữ CI đủ nhanh để nhóm dùng thường xuyên, đồng thời đặt các kiểm tra có khả năng bắt lỗi dữ liệu trước việc phát hành. “Build image được” và “nghiệp vụ đúng” cần hai loại bằng chứng khác nhau.

### 7.5. Build và vận hành có thể tái lập

- Với workspace ứng dụng này, đề xuất commit Cargo.lock và dùng `--locked` ở CI/Docker; pin toolchain theo quyết định của dự án.
- Frontend dùng package-lock và `npm ci`; copy manifest cùng lock trước bước cài dependency trong Docker.
- Gắn JS wrapper, WASM, circuit parameters và VK với version/digest tương ứng; không ghép artifact mới/cũ không tương thích. Lock dependency không thay thế audit correctness của circuit.
- Badge build dẫn tới workflow thật; coverage chỉ xuất khi có báo cáo. Dùng coverage để tìm vùng chưa được kiểm tra, không thay thế acceptance tests bằng một phần trăm đẹp.
- README tách hướng dẫn reset dữ liệu demo khỏi migration giữ dữ liệu; ghi quy trình backup/restore và điều kiện chạy destructive reset rõ ràng.
- Tài liệu kiến trúc cập nhật theo code thật: scope auth, nguồn state chuẩn, ý nghĩa ACK, độ trễ projection, loại proof và giới hạn solvency.

### 7.6. Tiêu chí kiểm chứng

- Clone sạch có đủ workflow/lockfile, build được backend/frontend/WASM theo tài liệu.
- Một test bắt buộc fail làm pipeline không thể publish image.
- Browser release từ chối proof giả dù có `verified: true`.
- Test SQL failure/restart chứng minh state ở cùng committed sequence và không ghi trùng.
- Mỗi trạng thái verified/DONE dẫn tới một acceptance test hoặc bằng chứng tương đương; mock có nhãn và đường dữ liệu riêng.

## 8. Lộ trình thảo luận và triển khai đề xuất

Tất cả các mốc trong bảng dưới đây **chưa được triển khai trong yêu cầu hiện tại**. Ước lượng công sức chỉ mang tính tương đối; cần chốt workload, độ sâu proof và hợp đồng durability trước khi ước lượng ngày công.

| Mốc | Nội dung | Phụ thuộc | Điều kiện kết thúc | Công sức tương đối |
|---|---|---|---|---|
| A. Chốt đặc tả | Demo boundary, invariants, precision/fees, scope proof, ý nghĩa ACK | Không | Có quyết định và các ví dụ đúng/sai cụ thể | Nhỏ–vừa |
| B. Loại kết quả gây hiểu nhầm | Đánh dấu mock; ngừng coi boolean là verify; sửa padding; mô tả đúng roadmap | A về scope proof | Tổng đúng; JSON giả không được báo valid; mock không vào trusted snapshot | Vừa |
| C. Nguyên tử nghiệp vụ | Command pipeline dùng chung, whole-command staging, reservation/fees nhất quán | A về accounting | Concurrency và lỗi fill cuối không tạo state một phần | Lớn |
| D. Durability và recovery | Journal, idempotency, DB transaction/dedup, snapshot/replay, readiness | C | Fault-injection/restart đạt hợp đồng đã chốt | Lớn |
| E. Snapshot và verifier thật | Snapshot theo sequence, manifest, Merkle inclusion native/WASM | B; boundary state nhất quán từ C/D | Browser xác minh độc lập inclusion dưới root đã chọn | Vừa–lớn |
| F. Solvency có phạm vi rõ | Circuit membership/aggregate, tham số chuẩn, setup, reserve evidence | D/E và threat model | Các statement được kiểm chứng; giả định completeness/reserves được công bố | Lớn, cần review chuyên biệt |
| G. Đo và tối ưu | Write set, indexes, bounded proof workers, projection/broadcast | Baseline và correctness từ C/D | Đạt mục tiêu hiệu năng đã đo, không làm yếu invariants | Theo kết quả đo |

CI và tài liệu của nhóm 7 bắt đầu từ mốc A/B, phát triển cùng các mốc sau. Rào chắn admin/demo có thể làm sớm độc lập. Transaction/retry worker có thể giảm rủi ro trước D, nhưng phải gắn nhãn biện pháp tạm: chưa bảo đảm không mất event đã ACK.

Không nên làm đầy đủ Groth16 browser verifier trước khi chốt circuit/public inputs/VK lifecycle, vì việc đổi statement sau đó sẽ làm thay định dạng và key. Không nên tối ưu nhiều writer trước khi giải quyết reservation dùng chung giữa các market.

## 9. Các quyết định còn mở để cùng brainstorm

| Câu hỏi | Hướng mặc định đề xuất | Lý do / điều kiện cần đổi |
|---|---|---|
| Dự án chỉ demo local hay demo chia sẻ cho người không tin cậy? | Demo cô lập, dữ liệu giả | Dummy auth không xác thực danh tính; nếu mở rộng cần đặc tả auth mới |
| Mốc gần nhất cần inclusion hay full proof of solvency? | Inclusion đúng trước, công bố giới hạn | Có thể kiểm chứng sớm mà không gọi quá phạm vi bảo đảm |
| Ưu tiên giữ DB ngoài ACK hay giảm độ phức tạp storage? | Journal riêng là thiết kế mục tiêu; so sánh với PostgreSQL journal trước khi chốt | Journal riêng giữ async projection nhưng yêu cầu năng lực kiểm thử durability; DB journal đơn giản hơn nhưng đổi ràng buộc kiến trúc |
| Chấp nhận mất dữ liệu nào sau lỗi? | Không mất command đã ACK khi process crash và volume còn nguyên | Lỗi cả volume/máy cần mục tiêu backup/replication riêng; không thể suy ra từ test restart |
| Reserves chỉ là số giả lập hay cần chứng cứ bên ngoài? | Gắn nhãn giả lập cho dữ liệu hiện có | Nguồn ngoài cần network/asset/block, quyền kiểm soát và giả định nghiệp vụ rõ |
| Tổng liabilities và số dư cá nhân có cần giấu không? | Chốt đối tượng quan sát và dữ liệu cần giấu trước | Quyết định này ảnh hưởng commitment, response, circuit comparison và độ phức tạp |
| Tài khoản suspended còn được hủy/rút/đọc không? | Cho đọc, có đường hủy an toàn; quyền rút cần chốt riêng | Tránh đóng băng reservation vô hạn hoặc âm thầm thay đổi chính sách |
| Snapshot phát hành theo kỳ hay theo request? | Theo kỳ/sự kiện với ID bất biến | Tái sử dụng cây, root chung để đối chiếu; phải công bố độ cũ |
| Tải và độ trễ mục tiêu là bao nhiêu? | Đo baseline trước, chốt p95/p99 theo workload | Không có số đo để cam kết một con số microsecond cho toàn hệ thống |
| Giữ dữ liệu demo cũ khi chuyển kiến trúc như thế nào? | Backup, maintenance, đối soát rồi tạo baseline | Schema hiện tại không chứa đủ mọi reservation/event để tự suy diễn lịch sử hoàn hảo |

## 10. Checklist đánh giá một đợt triển khai tương lai

Các mục này là tiêu chí dự kiến, chưa được đánh dấu hoàn thành:

- [ ] Cấu hình môi trường và phạm vi dummy auth được cưỡng chế tại server.
- [ ] Admin có cơ chế bảo vệ phù hợp; localStorage và user ID tự khai không cấp quyền thật.
- [ ] Book, ledger và reservation thay đổi nguyên tử theo một command.
- [ ] Event đã ACK có hợp đồng durability rõ, retry và replay không nhân đôi tác động.
- [ ] Restart giữ FIFO, remaining, fees, capital, idempotency và ID.
- [ ] Snapshot lấy từ một committed boundary; root sum bằng liabilities đã chọn.
- [ ] Circuit ràng buộc đúng statement; range, encoding, setup và hash parameters được review.
- [ ] Browser thực sự xác minh artifact, public inputs và key/context đã chọn.
- [ ] Kết quả inclusion, aggregate và reserves được trình bày đúng phạm vi.
- [ ] Integration/fault-injection/browser tests chạy trên artifact và storage thực.
- [ ] Workflow/lockfiles có trong Git; quality gate chặn phát hành khi test thất bại.
- [ ] Benchmark thể hiện chi phí toàn hệ thống và các giới hạn vận hành đã đo.

## 11. Tài liệu tham chiếu

- [Đặc tả công nghệ hiện tại](techstack.md) và [roadmap hiện tại](wbs.csv): nguồn xác định scope ban đầu; các đề xuất thay đổi scope trong tài liệu này cần được ghi nhận thành quyết định riêng ở giai đoạn triển khai.
- [Arkworks Groth16 parameter generator](https://docs.rs/ark-groth16/latest/src/ark_groth16/generator.rs.html): cơ sở phân biệt setup randomness với proving/verifying keys.
- [PostgreSQL Transactions](https://www.postgresql.org/docs/current/tutorial-transactions.html): tính nguyên tử của các thao tác trong một DB transaction.
- [Rust File synchronization](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all): phân biệt ghi bộ đệm và yêu cầu đồng bộ dữ liệu xuống storage.

Các liên kết chính thức minh họa nguyên lý thiết kế. Dự án dùng PostgreSQL 17 và Arkworks 0.3; khi triển khai phải xác nhận API, feature flags, encoding và bảo đảm vận hành theo đúng phiên bản thực tế. Tài liệu này không phải chứng nhận audit bảo mật hay kết quả kiểm thử các phương án chưa triển khai.


