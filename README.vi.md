# 🐳 CodeWhale

> **Agent lập trình gốc terminal dành cho DeepSeek V4. Chương trình chạy từ lệnh `codewhale`, hỗ trợ stream các khối suy nghĩ (reasoning blocks), chỉnh sửa workspace cục bộ thông qua các lớp phê duyệt, và đi kèm chế độ tự động để tự chọn mô hình cũng như mức độ suy nghĩ phù hợp cho mỗi lượt.**

[English README](README.md)
[简体中文 README](README.zh-CN.md)
[日本語 README](README.ja-JP.md)

## Cài đặt

`codewhale` được cài đặt dưới dạng một cặp binary tự chạy bằng Rust đồng bộ với nhau:
Lệnh điều phối `codewhale` (dispatcher) và môi trường chạy giao diện `codewhale-tui` (runtime) do nó khởi chạy để thực hiện các phiên làm việc tương tác. npm và Docker sẽ tự động cài đặt cả hai cho bạn; đối với Cargo hoặc cài đặt thủ công, bạn phải đặt cả hai tệp binary này trong cùng một thư mục (thông thường là một thư mục nằm trong biến môi trường `PATH` của bạn). Gói npm chỉ là một trình cài đặt/bao bọc (wrapper) cho các tệp binary phát hành này; agent không chạy trên môi trường Node.js.

```bash
# 1. npm — dễ nhất nếu bạn đã cài đặt Node. Gói này sẽ tự động tải các
#    binary Rust dựng sẵn tương ứng từ GitHub Releases.
npm install -g codewhale

# 2. Cargo — không cần Node. Yêu cầu phiên bản Rust từ 1.88 trở lên (các crate sử dụng
#    phiên bản Rust edition 2024; các toolchain cũ hơn sẽ báo lỗi "feature `edition2024` is
#    required"). Hãy chạy lệnh `rustup update` trước, hoặc sử dụng các cách cài đặt không qua Cargo ở dưới.
cargo install codewhale-cli --locked   # cài đặt `codewhale` (điểm truy cập CLI chính)
cargo install codewhale-tui     --locked   # cài đặt `codewhale-tui` (giao diện TUI)

# 3. Homebrew — chỉ dành cho khả năng tương thích với cài đặt cũ.
#    Tap/formula vẫn dùng tên deepseek-tui cũ. Với cài đặt mới, hãy ưu tiên
#    npm, Cargo, Docker hoặc tải trực tiếp cho đến khi formula được đổi tên.
brew tap Hmbown/deepseek-tui
brew install deepseek-tui

# 4. Tải xuống trực tiếp — các gói lưu trữ theo nền tảng từ GitHub Releases.
#    https://github.com/Hmbown/CodeWhale/releases
#    Gói nén bao gồm cả codewhale và codewhale-tui cùng một tập lệnh cài đặt.
#    Các binary riêng lẻ cũng được đính kèm cho các tập lệnh; hãy giữ cặp này ở cùng một nơi.

# 5. Docker — hình ảnh phát hành dựng sẵn.
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

> Tại Trung Quốc đại lục, bạn có thể tăng tốc độ tải qua npm bằng tham số
> `--registry=https://registry.npmmirror.com`, hoặc sử dụng
> [Cargo mirror](#china--cai-dat-than-thien-qua-mirror) bên dưới.
>
> An toàn tải xuống: Các binary phát hành chính thức chỉ nằm tại
> `https://github.com/Hmbown/CodeWhale/releases`. Nếu tải thủ công,
> vui lòng xác minh mã băm SHA-256 manifest và tránh các kho lưu trữ giả mạo hoặc các
> trang web mirror trên kết quả tìm kiếm. Xem [an toàn tải xuống và mã xác thực](docs/INSTALL.md#2-download-safety-and-checksums).

Đã cài đặt từ trước? Sử dụng lệnh cập nhật tương ứng với cách bạn đã cài đặt:

```bash
codewhale update                         # trình cập nhật binary phát hành trực tiếp
npm install -g codewhale@latest      # thông qua trình bao bọc npm
brew update && brew upgrade deepseek-tui  # chỉ cho cài đặt Homebrew cũ
cargo install codewhale-cli --locked --force
cargo install codewhale-tui     --locked --force
```

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/codewhale)](https://www.npmjs.com/package/codewhale)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[Mục lục dự án DeepWiki](https://deepwiki.com/Hmbown/CodeWhale)

![ảnh chụp màn hình codewhale](assets/screenshot.png)

---

## CodeWhale là gì?

Mô hình AI chỉ trả lời câu hỏi. Agent hoàn thành một nhiệm vụ. Sự khác biệt nằm ở
**khung ràng buộc (harness)** — một hệ thống các quy tắc, bằng chứng và phản hồi giúp giữ cho
mô hình đi đúng hướng thay vì bị trôi lệch mục tiêu.

CodeWhale chính là khung ràng buộc đó, được xây dựng xung quanh DeepSeek V4 và được dẫn dắt bởi ba ý tưởng chính:

| Nguyên tắc | Cách thức hoạt động |
|---|---|
| **Bắt đầu với sự tin tưởng** | Mỗi lượt bắt đầu bằng chữ "A" — tìm kiếm khả năng trước khi khẳng định chắc chắn, chú trọng chất lượng trước sự tiện lợi |
| **Thẩm quyền rõ ràng** | Một bản Hiến pháp bằng văn bản với chín cấp bậc thẩm quyền. Ý định của người dùng quan trọng hơn các hướng dẫn cũ kỹ. Sự xác minh quan trọng hơn sự tự tin. |
| **Cải tiến đệ quy** | V4 đã tham gia viết nên một phần của khung ràng buộc này. Khi khung ràng buộc tốt lên, V4 hoạt động hiệu quả hơn — và giúp cải tiến khung ràng buộc hơn nữa. Mỗi lượt chạy mới đều bắt đầu mạnh mẽ hơn. |

Dự án này là mã nguồn mở, hoạt động trực tiếp trên terminal và được đóng gói thành một cặp binary Rust đồng bộ là `codewhale` / `codewhale-tui`.

## Khung Ràng Buộc Hoạt Động Thế Nào?

Các mô hình dạng Agent phải xử lý lượng thông tin xung đột rất lớn trên quy mô lớn: ý định của người dùng, quy tắc dự án, cấu hình mặc định của hệ thống, đầu ra của công cụ và bộ nhớ cũ đều cạnh tranh thẩm quyền trong một lượt chạy duy nhất. LLM hoạt động như một thẩm phán cần có thẩm quyền rõ ràng — nguồn thông tin nào sẽ thắng thế khi xảy ra xung đột?

CodeWhale giải quyết vấn đề này bằng một bản **Hiến pháp** (`prompts/base.md`). Đây là một hệ thống phân cấp luật chính thức — Điều VII xếp hạng chín nguồn thông tin từ các điều khoản của chính Hiến pháp xuống đến thông tin bàn giao từ phiên làm việc trước. Tin nhắn hiện tại của người dùng có thẩm quyền cao hơn các hướng dẫn dự án cũ kỹ. Đầu ra trực tiếp từ công cụ có thẩm quyền cao hơn các giả định. Việc xác minh thực tế có thẩm quyền cao hơn sự tự tin của mô hình. Mô hình kế thừa một chuỗi thẩm quyền rõ ràng qua từng lượt và không bao giờ phải đoán xem nên làm theo chỉ thị nào.

Có bảy điều khoản đứng đầu hệ thống phân cấp này, định nghĩa danh tính, nghĩa vụ và quyền hạn của mô hình: yêu cầu xác minh (Điều V — mọi hành động phải để lại bằng chứng thực tế, không bao giờ tuyên bố thành công dựa trên niềm tin mơ hồ), di sản điều phối (Điều VI — giữ cho workspace dễ đọc để trí tuệ tiếp theo có thể tiếp quản), và điều khoản ưu tiên sự thật (Điều II — không có quy tắc cấp dưới nào được phép ghi đè lên nó).

Bộ nhớ đệm tiền tố (prefix caching) của DeepSeek V4 làm cho điều này trở nên khả thi và thực tế. Bản Hiến pháp rất dài và chi tiết, nhưng một khi đã được cache, nó sẽ tốn ít hơn khoảng 100 lần chi phí cho mỗi lượt so với một lần đọc mới hoàn toàn. Mô hình tham chiếu nó một cách đệ quy — xem qua, quét và truy vấn thông qua các phiên RLM — truy cập lại thông tin theo nhu cầu thay vì chỉ dựa trên một lượt ghi nhớ duy nhất. Nó hoạt động giống như một bài kiểm tra mở sách hơn là kiểm tra đóng sách.

Bởi vì cấu trúc thẩm quyền là tường minh, các lỗi và thất bại không bao giờ bị che giấu. Các mã thoát (exit codes) khác không, lỗi kiểu dữ liệu từ rust-analyzer trả về giữa các lượt, từ chối của sandbox — tất cả đều được đưa ngược lại như các vectơ sửa lỗi. Mô hình sử dụng chính sự chệch hướng của mình để tự sửa sai.

Ba chế độ kiểm soát không gian hành động: **Plan** là chế độ chỉ đọc. **Agent** chặn các thao tác can thiệp thay đổi file đằng sau quyền phê duyệt của người dùng. **YOLO** tự động phê duyệt tất cả các công cụ trong các workspace đáng tin cậy. Chế độ Sandbox hoạt động trên macOS Seatbelt; Linux Landlock đã được phát hiện nhưng chưa được áp dụng bắt buộc; chế độ sandboxing trên Windows hiện chưa được hỗ trợ.

**Fin** — một cuộc gọi Flash giá rẻ và tắt chức năng suy nghĩ — xử lý việc tự động định tuyến mô hình cho mỗi lượt. Tham số mặc định là `--model auto`.

Mỗi lượt chạy đều ghi lại một ảnh chụp nhanh side-git bên ngoài thư mục `.git` của repo. Các lệnh `/restore` và `revert_turn` giúp khôi phục nhanh workspace về trạng thái trước đó.

Các sub-agent chạy đồng thời (tối đa 20). Lệnh `agent_open` trả về kết quả ngay lập tức; kết quả trả về nội tuyến dưới dạng các sentinel hoàn thành kèm theo bản tóm tắt. Nhật ký chi tiết của sub-agent được lưu trữ và truy cập thông qua `agent_eval`. Xem chi tiết tại [docs/SUBAGENTS.md](docs/SUBAGENTS.md).

Các tính năng khác của hệ thống bao gồm: chẩn đoán lỗi LSP sau mỗi lần chỉnh sửa file (rust-analyzer, pyright, typescript-language-server, gopls, clangd), các phiên làm việc RLM để phân tích hàng loạt, giao thức MCP, API runtime HTTP/SSE, hàng đợi tác vụ liên tục, adapter ACP cho trình soạn thảo Zed, xuất kết quả định dạng SWE-bench và theo dõi chi phí trực tiếp với bảng phân tích chi tiết lượt hit/miss cache.

---

## Khung Kết Nối (Harness)

`codewhale` (CLI điều phối) → `codewhale-tui` (binary giao diện) → giao diện ratatui ↔ công cụ bất đồng bộ ↔ máy khách streaming tương thích với OpenAI. Các lượt gọi công cụ được định tuyến qua một registry có phân loại (shell, thao tác file, git, web, sub-agent, MCP, RLM) và kết quả được truyền trực tuyến trở lại transcript. Công cụ quản lý trạng thái phiên làm việc, theo dõi lượt chạy, hàng đợi tác vụ bền bỉ và một phân hệ LSP cung cấp thông tin chẩn đoán sau khi chỉnh sửa vào ngữ cảnh của mô hình trước bước suy nghĩ tiếp theo.

Xem tài liệu [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) để biết chi tiết toàn bộ luồng hoạt động.

### Sub-agents: Khởi chạy Tác vụ Nền Đồng thời

CodeWhale có thể điều phối nhiều sub-agent chạy song song — hoạt động giống như một hàng đợi tác vụ đồng thời:

- **Khởi chạy không chặn:** Lệnh `agent_open` trả về ngay lập tức. Sub-agent con có một ngữ cảnh độc lập mới và hệ thống đăng ký công cụ riêng để chạy tự chủ. Agent cha vẫn tiếp tục làm việc bình thường.
- **Thực thi dưới nền:** Các sub-agent chạy đồng thời (giới hạn mặc định: 10, có thể cấu hình lên đến 20). Hệ thống tự quản lý pool tài nguyên này mà không cần vòng lặp thăm dò (polling loop).
- **Thông báo hoàn thành:** Khi một sub-agent hoàn thành, hệ thống sẽ chèn một khóa sentinel `<codewhale:subagent.done>` vào transcript của agent cha. Một bản tóm tắt thân thiện với con người — bao gồm phát hiện của sub-agent con, các file đã thay đổi và các rủi ro có thể xảy ra — nằm ngay dòng phía trên khóa sentinel. Mô hình cha sẽ đọc tóm tắt đó và tích hợp kết quả thu được mà không cần phải thực hiện thêm bất kỳ lệnh gọi công cụ nào khác.
- **Truy xuất kết quả có giới hạn:** Nhật ký chi tiết của agent con nằm dưới dạng một `transcript_handle` có thể truy cập qua `agent_eval`. Khi bản tóm tắt là chưa đủ, agent cha có thể gọi `handle_read` để đọc một phần, các dòng cụ thể hoặc lọc qua JSONPath — giúp ngữ cảnh của agent cha luôn tinh gọn mà không làm mất đi các chi tiết quan trọng.

Xem thêm tài liệu [docs/SUBAGENTS.md](docs/SUBAGENTS.md) để tham khảo thông tin đầy đủ về sub-agent.

---

## Khởi động nhanh

```bash
npm install -g codewhale
codewhale --version
codewhale --model auto
```

Cặp binary dựng sẵn và gói nén nền tảng được phát hành cho các kiến trúc **Linux x64**, **Linux ARM64** (từ v0.8.8 trở lên), **macOS x64**, **macOS ARM64**, và **Windows x64**. Đối với các mục tiêu khác (musl, riscv64, FreeBSD, v.v.), xem phần [Cài đặt từ nguồn](#install-from-source) hoặc tài liệu [docs/INSTALL.md](docs/INSTALL.md).

Trong lần chạy đầu tiên, bạn sẽ được nhắc nhập [API key của DeepSeek](https://platform.deepseek.com/api_keys). Khóa này được lưu vào tệp cấu hình `~/.codewhale/config.toml` (tương thích cả tệp cũ `~/.deepseek/config.toml`) để nó hoạt động từ bất kỳ thư mục nào mà không cần nhắc thông tin đăng nhập của hệ điều hành.

Bạn cũng có thể thiết lập trước:

```bash
codewhale auth set --provider deepseek   # lưu vào ~/.codewhale/config.toml
codewhale auth status                    # hiển thị nguồn thông tin đăng nhập đang hoạt động

export DEEPSEEK_API_KEY="YOUR_KEY"      # cách thiết lập qua biến môi trường thay thế; sử dụng ~/.zshenv cho terminal không tương tác
codewhale

codewhale doctor                         # kiểm tra và xác minh thiết lập
```

Nếu lệnh `codewhale doctor` báo lỗi API key bị từ chối đến từ biến môi trường `DEEPSEEK_API_KEY`, hãy xóa cấu hình xuất biến môi trường cũ trong tệp khởi chạy shell của bạn, mở một shell mới hoặc chạy lệnh `codewhale auth set --provider deepseek`. Sử dụng `codewhale auth status` để xem trạng thái của cấu hình, keyring hệ thống và biến môi trường mà không hiển thị trực tiếp khóa API. Các khóa lưu trong file cấu hình sẽ được ưu tiên cao hơn keyring và môi trường để dễ dàng thay đổi khi cần.

> Để thay đổi hoặc xóa khóa đã lưu: `codewhale auth clear --provider deepseek`.

### Tencent Cloud / CNB Remote-First Path

Đối với không gian làm việc luôn trực tuyến mà bạn có thể điều khiển từ điện thoại, hãy sử dụng đường dẫn gốc của Tencent: CNB mirror/source, Tencent Lighthouse HK, cầu kết nối dài hạn Feishu/Lark, và EdgeOne tùy chọn cho một cổng HTTPS công cộng có kiểm soát. API runtime luôn được giới hạn chạy tại localhost; EdgeOne không được sử dụng để hiển thị công khai đường dẫn `/v1/*`.

Bắt đầu với tài liệu [docs/TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md), sau đó xem thêm tài liệu [docs/TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) để biết các vận hành máy chủ.

### Chế độ Tự động (Auto Mode)

Sử dụng `codewhale --model auto` hoặc gõ lệnh `/model auto` khi bạn muốn hệ thống tự động quyết định sức mạnh của mô hình và cấp độ suy nghĩ cần thiết cho mỗi lượt.

Chế độ tự động điều khiển hai cài đặt cùng nhau:

- Mô hình: `deepseek-v4-flash` hoặc `deepseek-v4-pro`
- Cấp độ suy nghĩ: `off`, `high`, hoặc `max`

Trước khi lượt gửi chính thức được thực hiện, ứng dụng sẽ thực hiện một cuộc gọi định tuyến nhỏ thông qua mô hình `deepseek-v4-flash` tắt chế độ suy nghĩ. Trình định tuyến đó sẽ đánh giá yêu cầu mới nhất và ngữ cảnh gần đây, từ đó chọn mô hình cụ thể và cấp độ suy nghĩ phù hợp cho lượt gọi thực tế. Các lượt tương tác ngắn/đơn giản sẽ được chạy trên mô hình Flash tắt suy nghĩ; các công việc lập trình phức tạp, gỡ lỗi, phát hành, kiến trúc phần mềm, kiểm tra bảo mật hoặc các tác vụ nhiều bước mơ hồ sẽ được đẩy lên mô hình Pro với cấp độ suy nghĩ cao hơn.

Cơ chế `auto` hoạt động hoàn toàn cục bộ trên máy của bạn. API ở máy chủ upstream không bao giờ nhận được chuỗi `model: "auto"`; nó luôn nhận được mô hình cụ thể và cấu hình suy nghĩ đã được chọn cho lượt chạy đó. Giao diện TUI hiển thị tuyến đường định tuyến được chọn và bộ theo dõi chi phí sẽ tính tiền cho mô hình thực tế đã chạy. Nếu cuộc gọi định tuyến thất bại hoặc trả về câu trả lời không hợp lệ, ứng dụng sẽ chuyển sang thuật toán phỏng đoán cục bộ. Các sub-agent con sẽ kế thừa chế độ tự động này trừ khi bạn chỉ định rõ một mô hình cho chúng.

Hãy chỉ định mô hình hoặc cấp độ suy nghĩ cố định nếu bạn muốn chạy benchmark lặp lại nhất quán, kiểm soát nghiêm ngặt chi phí trần hoặc có cấu hình ánh xạ nhà cung cấp/mô hình tùy chỉnh cụ thể.

### Linux ARM64 (Raspberry Pi, Asahi, Graviton, HarmonyOS PC)

Lệnh cài đặt `npm i -g codewhale` hoạt động trên môi trường Linux ARM64 nền glibc từ phiên bản v0.8.8 trở đi. Bạn cũng có thể tải trực tiếp các tệp binary dựng sẵn từ [trang phát hành Releases](https://github.com/Hmbown/CodeWhale/releases) và đặt chúng cạnh nhau trong một thư mục thuộc biến `PATH`.

Xem [docs/HarmonyOS.md](docs/HarmonyOS.md) để cấu hình HarmonyOS PC và cross-build OpenHarmony.

### Cài đặt thân thiện qua Mirror (Tại Trung Quốc)

Nếu việc tải xuống từ GitHub hoặc npm bị chậm từ Trung Quốc đại lục, bạn hãy sử dụng mirror registry cho Cargo:

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

Sau đó cài đặt cả hai binary (trình điều phối sẽ ủy quyền cho TUI tại thời điểm chạy):

```bash
cargo install codewhale-cli --locked   # cung cấp lệnh `codewhale`
cargo install codewhale-tui     --locked   # cung cấp giao diện `codewhale-tui`
codewhale --version
```

Các binary dựng sẵn cũng có thể được tải từ [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases). Thiết lập biến `DEEPSEEK_TUI_RELEASE_BASE_URL` để sử dụng mirror tải các tệp tài nguyên phát hành.

### Windows (Scoop)

[Scoop](https://scoop.sh) là một trình quản lý gói phổ biến trên Windows. Gói `codewhale` đã được liệt kê trong bucket chính của Scoop, tuy nhiên gói cài đặt này hoạt động độc lập và đôi khi cập nhật chậm hơn các bản phát hành chính thức trên GitHub/npm/Cargo. Chạy lệnh `scoop update` trước, sau đó xác minh phiên bản đã cài bằng `codewhale --version`:

```bash
scoop update
scoop install codewhale
codewhale --version
```

Vui lòng sử dụng phương pháp npm hoặc tải trực tiếp từ GitHub Releases nếu bạn muốn trải nghiệm phiên bản mới nhất trước khi Scoop cập nhật.

<details id="install-from-source">
<summary>Cài đặt từ mã nguồn</summary>

Cách này hoạt động trên bất kỳ kiến trúc mục tiêu Tier-1 nào được Rust hỗ trợ — bao gồm cả musl, riscv64, FreeBSD và các bản phân phối ARM64 Linux cũ.

```bash
# Các thư viện phụ thuộc để build trên Linux (Debian/Ubuntu/RHEL):
#   sudo apt-get install -y build-essential pkg-config libdbus-1-dev
#   sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel

git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale

cargo install --path crates/cli --locked   # yêu cầu Rust 1.88+; cung cấp `codewhale`
cargo install --path crates/tui --locked   # cung cấp giao diện `codewhale-tui`
```

Cả hai tệp binary đều bắt buộc phải cài đặt. Xem hướng dẫn biên dịch chéo và ghi chú riêng theo nền tảng tại: [docs/INSTALL.md](docs/INSTALL.md).

</details>

### Các Nhà Cung Cấp API Khác

Để xem danh sách đầy đủ tất cả các nhà cung cấp được hỗ trợ chính thức, bao gồm mã định danh mô hình, biến xác thực, URL cơ sở và ranh giới tính năng, xem thêm tài liệu [docs/PROVIDERS.md](docs/PROVIDERS.md).

```bash
# NVIDIA NIM
codewhale auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
codewhale --provider nvidia-nim

# AtlasCloud
codewhale auth set --provider atlascloud --api-key "YOUR_ATLASCLOUD_API_KEY"
codewhale --provider atlascloud

# Wanjie Ark
codewhale auth set --provider wanjie-ark --api-key "YOUR_WANJIE_API_KEY"
codewhale --provider wanjie-ark --model deepseek-reasoner

# OpenRouter
codewhale auth set --provider openrouter --api-key "YOUR_OPENROUTER_API_KEY"
codewhale --provider openrouter --model deepseek/deepseek-v4-pro

# Novita
codewhale auth set --provider novita --api-key "YOUR_NOVITA_API_KEY"
codewhale --provider novita --model deepseek/deepseek-v4-pro

# Fireworks
codewhale auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"
codewhale --provider fireworks --model deepseek-v4-pro

# Các endpoint tương thích định dạng OpenAI chung
codewhale auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"
OPENAI_BASE_URL="https://openai-compatible.example/v4" codewhale --provider openai --model glm-5

# Tự host bằng SGLang
SGLANG_BASE_URL="http://localhost:30000/v1" codewhale --provider sglang --model deepseek-v4-flash

# Tự host bằng vLLM
VLLM_BASE_URL="http://localhost:8000/v1" codewhale --provider vllm --model deepseek-v4-flash
# Sử dụng vLLM qua kết nối HTTP trong mạng LAN đáng tin cậy
DEEPSEEK_ALLOW_INSECURE_HTTP=1 VLLM_BASE_URL="http://192.168.0.110:8000/v1" codewhale --provider vllm --model deepseek-v4-flash

# Tự host bằng Ollama
ollama pull codewhale-coder:1.3b
codewhale --provider ollama --model codewhale-coder:1.3b

# Hugging Face Inference Providers
codewhale auth set --provider huggingface --api-key "YOUR_HF_TOKEN"
codewhale --provider huggingface --model deepseek-ai/DeepSeek-V4-Pro
```

Bên trong giao diện TUI, lệnh `/provider` mở bảng chọn nhà cung cấp và `/model` mở bảng chọn mô hình/cấp độ suy nghĩ cục bộ. Lệnh `/provider openrouter` và `/model <id>` chuyển đổi trực tiếp, trong khi lệnh `/models` sẽ truy vấn trực tiếp và hiển thị danh sách các mô hình API trực tuyến từ nhà cung cấp (nếu nhà cung cấp hỗ trợ tính năng liệt kê mô hình).

---

## Nhật ký thay đổi (Release Notes)

Chi tiết thay đổi giữa các phiên bản được cập nhật tại [CHANGELOG.md](CHANGELOG.md). File README này chỉ tập trung vào các đường dẫn cài đặt hiện tại, quy trình làm việc cốt lõi, thiết lập nhà cung cấp API, giao diện và các điểm mở rộng tính năng của dự án.

---

## Cách sử dụng

```bash
codewhale                                         # giao diện tương tác TUI chính
codewhale "explain this function"                 # thực thi prompt nhanh một lượt
codewhale exec --auto --output-format stream-json "fix this bug"  # truyền phát luồng dữ liệu NDJSON backend
codewhale exec --resume <SESSION_ID> "follow up"  # tiếp tục phiên làm việc không tương tác cũ
codewhale --model deepseek-v4-flash "summarize"   # ghi đè mô hình chạy chỉ định
codewhale --model auto "fix this bug"             # tự động chọn mô hình và cấp độ suy nghĩ thích hợp
codewhale --yolo                                  # tự động phê duyệt chạy các công cụ
codewhale auth set --provider deepseek            # lưu trữ API key
codewhale doctor                                  # tự động kiểm tra cài đặt và kết nối mạng
codewhale doctor --json                           # trả về chuẩn đoán định dạng máy đọc được
codewhale setup --status                          # chỉ đọc trạng thái thiết lập hiện tại
codewhale setup --tools --plugins                 # tạo sẵn cấu trúc thư mục tool/plugin
codewhale models                                  # liệt kê các mô hình khả dụng trực tuyến
codewhale sessions                                # liệt kê các phiên làm việc đã lưu
codewhale resume --last                           # tiếp tục phiên làm việc gần nhất trong thư mục này
codewhale resume <SESSION_ID>                     # tiếp tục một phiên làm việc cụ thể theo mã UUID
codewhale fork <SESSION_ID>                       # tạo một nhánh (fork) phiên làm việc đã lưu sang đường dẫn mới
codewhale serve --http                            # khởi chạy máy chủ API định dạng HTTP/SSE
codewhale serve --acp                             # khởi chạy adapter ACP qua stdio cho trình soạn thảo Zed/agent tùy chỉnh
codewhale run pr <N>                              # tải PR về và nạp sẵn vào prompt đánh giá
codewhale mcp list                                # liệt kê các máy chủ MCP đã cấu hình
codewhale mcp validate                            # kiểm tra cấu hình và kết nối máy chủ MCP
codewhale mcp-server                              # khởi chạy máy chủ MCP điều phối qua cổng stdio
codewhale update                                  # kiểm tra và cài đặt phiên bản binary mới nhất
```

### Tạo nhánh phiên làm việc (Branching)

Các phiên làm việc được lưu có thể được phân nhánh một cách có chủ đích. Lệnh `codewhale fork <SESSION_ID>` sao chép toàn bộ phiên làm việc cũ sang một phiên mới song song, lưu trữ mã ID của phiên cha trong siêu dữ liệu (metadata) và mở phiên fork đó ra để bạn có thể thử nghiệm hướng phát triển mới mà không làm ảnh hưởng đến lịch sử phiên làm việc gốc. Trình chọn phiên làm việc và danh sách `codewhale sessions` sẽ đánh dấu rõ ràng các phiên được fork kèm theo mã ID của phiên cha.

Bên trong giao diện TUI, bạn có thể nhấn phím `Esc` hai lần (`Esc-Esc`) để quay ngược lại transcript và đưa prompt cũ về lại phần soạn thảo để chỉnh sửa lại nội dung. Các lệnh `/restore` và `revert_turn` là công cụ khôi phục workspace độc lập: chúng khôi phục lại các tệp tin dựa trên ảnh chụp nhanh side-git nhưng không làm thay đổi hay ghi đè lịch sử trò chuyện của phiên làm việc.

Các hình ảnh Docker được phát hành lên GHCR cho các bản dựng phát hành chính thức:

```bash
docker volume create codewhale-home

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

Xem tài liệu [docs/DOCKER.md](docs/DOCKER.md) để biết thêm thông tin về thẻ phiên bản (pinned tags), cách tự dựng image cục bộ, lưu ý quyền sở hữu volume và cách sử dụng cho pipeline không tương tác.

### Zed / ACP

DeepSeek có thể chạy dưới dạng một máy chủ Agent Client Protocol (ACP) cục bộ cho các trình soạn thảo mã nguồn hỗ trợ giao tiếp ACP qua cổng stdio. Trong trình soạn thảo Zed, bạn hãy thêm cấu hình máy chủ agent tùy chỉnh sau:

```json
{
  "agent_servers": {
    "DeepSeek": {
      "type": "custom",
      "command": "codewhale",
      "args": ["serve", "--acp"],
      "env": {}
    }
  }
}
```

Phân hệ ACP ban đầu hỗ trợ khởi tạo phiên làm việc mới và nhận phản hồi prompt qua cấu hình và API key hiện tại của DeepSeek. Tính năng chỉnh sửa tích hợp công cụ và phát lại checkpoint hiện chưa được hỗ trợ qua giao diện ACP.

Adapter do cộng đồng phát triển: [acp-codewhale-adapter](https://github.com/rockeverm3m/acp-codewhale-adapter) hỗ trợ cầu nối lệnh `codewhale exec --auto` với `cc-connect` cho người dùng cần quy trình làm việc ACP có tích hợp công cụ bên ngoài trình soạn thảo Zed.

### Phím Tắt Tiêu Biểu

| Phím | Hành động |
|---|---|
| `Tab` | Hoàn thành gợi ý lệnh `/` hoặc các nhãn tệp `@`; khi đang chạy, xếp tin nhắn nháp vào hàng đợi chạy tiếp theo; hoặc chuyển đổi qua lại giữa các chế độ |
| `Shift+Tab` | Thay đổi nhanh cấp độ suy nghĩ: off → high → max |
| `F1` | Mở màn hình trợ giúp phím tắt có thanh tìm kiếm |
| `Esc` | Quay lại / đóng cửa sổ popup |
| `Ctrl+K` | Mở bảng lệnh nhanh (Command palette) |
| `Ctrl+R` | Tiếp tục một phiên làm việc cũ |
| `Alt+R` | Tìm kiếm lịch sử prompt cũ để khôi phục tin nháp đã xóa |
| `Ctrl+S` | Cất tin nháp hiện tại vào bộ nhớ tạm (dùng `/stash list`, `/stash pop` để lấy lại) |
| `@path` | Đính kèm ngữ cảnh file hoặc thư mục trực tiếp tại trình soạn thảo văn bản |
| `↑` (tại đầu composer) | Chọn hàng tệp tin đính kèm để xóa |

Xem danh sách phím tắt đầy đủ tại: [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md).

---

## Chế độ hoạt động (Modes)

| Chế độ | Hành vi hoạt động |
| --- | --- |
| **Plan** 🔍 | Chế độ khảo sát chỉ đọc — mô hình tìm hiểu cấu trúc và đề xuất kế hoạch hành động cụ thể trước khi sửa đổi file; các cuộc khảo sát nhiều bước sử dụng công cụ `checklist_write` |
| **Agent** 🤖 | Chế độ tương tác mặc định — thực thi tác vụ nhiều bước có kiểm soát đằng sau các cổng phê duyệt; các tác vụ lớn sẽ được theo dõi qua `checklist_write` |
| **YOLO** ⚡ | Tự động phê duyệt tất cả các lệnh gọi công cụ trong các workspace tin cậy; các tác vụ nhiều bước vẫn duy trì checklist hiển thị trực quan |

---

## Cấu hình

Cấu hình của người dùng lưu tại: `~/.codewhale/config.toml` (tự động fallback về tệp cũ `~/.deepseek/config.toml` nếu có). Cấu hình riêng của dự án ghi đè tại: `<workspace>/.codewhale/config.toml` (hoặc `<workspace>/.deepseek/config.toml`) (lưu ý các trường sau bị cấm ghi đè ở cấp dự án: `api_key`, `base_url`, `provider`, `mcp_config_path`). Tham khảo tệp [config.example.toml](config.example.toml) để xem đầy đủ tất cả cấu hình mẫu.

Các biến môi trường chính:

| Biến môi trường | Mục đích sử dụng |
|---|---|
| `DEEPSEEK_API_KEY` | Khóa API key chính |
| `DEEPSEEK_BASE_URL` | Địa chỉ URL cơ sở của máy chủ API |
| `DEEPSEEK_HTTP_HEADERS` | Các header tùy chỉnh gửi kèm yêu cầu API, ví dụ `X-Model-Provider-Id=your-model-provider` |
| `DEEPSEEK_MODEL` | Mô hình mặc định |
| `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` | Thời gian chờ tối đa khi stream bị rảnh (giây), mặc định là `300`, giới hạn trong khoảng `1..=3600` |
| `CODEWHALE_PROVIDER` / `DEEPSEEK_PROVIDER` | Các nhà cung cấp: `deepseek` (mặc định), `nvidia-nim`, `openai`, `atlascloud`, `wanjie-ark`, `volcengine`, `openrouter`, `xiaomi-mimo`, `novita`, `fireworks`, `siliconflow`, `moonshot`, `sglang`, `vllm`, `ollama`, `huggingface` |
| `DEEPSEEK_PROFILE` | Tên cấu hình profile sử dụng |
| `DEEPSEEK_MEMORY` | Thiết lập là `on` để kích hoạt tính năng tự ghi nhớ thông tin người dùng |
| `DEEPSEEK_ALLOW_INSECURE_HTTP=1` | Cho phép sử dụng các đường dẫn API dạng `http://` không mã hóa trong các mạng LAN tin cậy |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `ATLASCLOUD_API_KEY` / `WANJIE_ARK_API_KEY` / `VOLCENGINE_API_KEY` / `ARK_API_KEY` / `OPENROUTER_API_KEY` / `XIAOMI_MIMO_API_KEY` / `MIMO_API_KEY` / `NOVITA_API_KEY` / `FIREWORKS_API_KEY` / `SILICONFLOW_API_KEY` / `MOONSHOT_API_KEY` / `KIMI_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` | Thông tin đăng nhập theo từng nhà cung cấp tương ứng |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | Điểm cuối (endpoint) và mã mô hình cho nhà cung cấp tương thích định dạng OpenAI chung |
| `ATLASCLOUD_BASE_URL` / `ATLASCLOUD_MODEL` | Endpoint và mô hình ghi đè cho AtlasCloud |
| `WANJIE_ARK_BASE_URL` / `WANJIE_ARK_MODEL` | Endpoint và mô hình ghi đè cho Wanjie Ark |
| `VOLCENGINE_BASE_URL` / `ARK_BASE_URL` / `VOLCENGINE_MODEL` / `ARK_MODEL` | Endpoint và mô hình ghi đè cho Volcengine Ark |
| `OPENROUTER_BASE_URL` | Endpoint ghi đè cho OpenRouter |
| `XIAOMI_MIMO_BASE_URL` / `MIMO_BASE_URL` / `XIAOMI_MIMO_MODEL` / `MIMO_MODEL` | Endpoint và mô hình ghi đè cho Xiaomi MiMo |
| `NOVITA_BASE_URL` | Endpoint ghi đè cho Novita |
| `FIREWORKS_BASE_URL` | Endpoint ghi đè cho Fireworks |
| `SILICONFLOW_BASE_URL` / `SILICONFLOW_MODEL` | Endpoint và mô hình ghi đè cho SiliconFlow |
| `MOONSHOT_BASE_URL` / `MOONSHOT_MODEL` / `KIMI_BASE_URL` / `KIMI_MODEL` | Endpoint và mô hình ghi đè cho Moonshot/Kimi |
| `SGLANG_BASE_URL` | Endpoint cho máy chủ SGLang tự host |
| `SGLANG_MODEL` | Mã mô hình cho máy chủ SGLang tự host |
| `VLLM_BASE_URL` | Endpoint cho máy chủ vLLM tự host |
| `VLLM_MODEL` | Mã mô hình cho máy chủ vLLM tự host |
| `OLLAMA_BASE_URL` | Endpoint cho máy chủ Ollama tự host |
| `OLLAMA_MODEL` | Thẻ mô hình (model tag) cho máy chủ Ollama tự host |
| `HUGGINGFACE_API_KEY` / `HF_TOKEN` | Xác thực Hugging Face |
| `HUGGINGFACE_BASE_URL` / `HUGGINGFACE_MODEL` | Ghi đè endpoint và mô hình Hugging Face |
| `NO_ANIMATIONS=1` | Bắt buộc chạy ở chế độ hỗ trợ khả năng tiếp cận (Accessibility mode), tắt hiệu ứng khi khởi động |
| `SSL_CERT_FILE` | Đường dẫn file CA bundle tùy chỉnh khi sử dụng proxy nội bộ doanh nghiệp |

Thiết lập thuộc tính `locale` trong file `settings.toml`, sử dụng lệnh `/config locale vi`, hoặc dựa vào cài đặt biến `LC_ALL`/`LANG` của hệ điều hành để lựa chọn ngôn ngữ cho giao diện TUI và ngôn ngữ nhắc nhở gửi kèm tới các mô hình V4. Tin nhắn mới nhất của người dùng vẫn có mức độ ưu tiên cao nhất để mô hình tự động chọn ngôn ngữ phản hồi tương ứng, do đó các câu hỏi bằng Tiếng Việt của người dùng vẫn sẽ luôn nhận được câu trả lời bằng Tiếng Việt ngay cả khi hệ điều hành đang thiết lập giao diện hiển thị mặc định bằng tiếng Anh. Xem tài liệu hướng dẫn cấu hình tại [docs/CONFIGURATION.md](docs/CONFIGURATION.md) và [docs/MCP.md](docs/MCP.md).

---

## Mô hình & Giá cả

| Mô hình | Ngữ cảnh | Đầu vào (Hit Cache) | Đầu vào (Miss Cache) | Đầu ra |
|---|---|---|---|---|
| `deepseek-v4-pro` | 1M | $0.003625 / 1M | $0.435 / 1M | $0.87 / 1M |
| `deepseek-v4-flash` | 1M | $0.0028 / 1M | $0.14 / 1M | $0.28 / 1M |

Nền tảng DeepSeek mặc định sử dụng đường dẫn `https://api.deepseek.com/beta` để bạn có thể trải nghiệm các tính năng API beta mà không cần thiết lập cấu hình phức tạp. Thiết lập thuộc tính `base_url = "https://api.deepseek.com"` nếu muốn tắt tính năng này.

Các tên định danh cũ `deepseek-chat` / `deepseek-reasoner` sẽ được tự động ánh xạ đến `deepseek-v4-flash` và sẽ chính thức dừng hoạt động sau ngày 24 tháng 7 năm 2026. Các biến thể NVIDIA NIM sẽ áp dụng theo điều khoản tài khoản NVIDIA của bạn.

> [!Note]
> Trang cấu trúc giá của DeepSeek hiện đã cập nhật bảng giá trên của dòng V4 Pro làm mức giá cố định vĩnh viễn: Chương trình khuyến mãi giảm giá 75% trước đó đã được chính thức tích hợp thẳng vào giá cơ sở từ sau khi thời hạn khuyến mãi kết thúc vào lúc 15:59 UTC ngày 31 tháng 5 năm 2026. Trình tính toán chi phí trên giao diện TUI của CodeWhale đã cập nhật các giá trị mới này, do đó bạn không cần thực hiện thêm thay đổi nào. Để theo dõi các thay đổi giá trong tương lai, vui lòng tham khảo [trang giá chính thức của DeepSeek](https://api-docs.deepseek.com/zh-cn/quick_start/pricing).

---

## Chia Sẻ Skill Tự Viết

CodeWhale sẽ tự động quét và tìm kiếm các skill được định nghĩa từ các thư mục của dự án (`.agents/skills` → `skills` → `.opencode/skills` → `.claude/skills` → `.cursor/skills`) và các thư mục cấu hình toàn cục (`~/.agents/skills` → `~/.claude/skills` → `~/.codewhale/skills` → `~/.deepseek/skills`). Mỗi skill là một thư mục chứa một tệp tin `SKILL.md`:

```text
~/.agents/skills/my-skill/
└── SKILL.md
```

Yêu cầu định nghĩa phần Frontmatter ở đầu file:

```markdown
---
name: my-skill
description: Sử dụng skill này khi bạn muốn DeepSeek tuân thủ theo quy trình làm việc tùy chỉnh của tôi.
---

# My Skill
Các hướng dẫn chi tiết dành cho agent được viết tại đây.
```

Các lệnh tương tác: `/skills` (liệt kê), `/skill <name>` (kích hoạt), `/skill new` (tạo khung mẫu), `/skill install github:<owner>/<repo>` (cài đặt từ cộng đồng GitHub), `/skill update` / `uninstall` / `trust` để quản lý. Cài đặt các skill từ cộng đồng GitHub không yêu cầu chạy thêm bất kỳ dịch vụ nền nào. Các skill sau khi cài đặt sẽ hiển thị trong phần ngữ cảnh phiên làm việc mà mô hình AI có thể đọc được; agent có thể tự chọn skill phù hợp qua công cụ `load_skill` khi nhiệm vụ của bạn khớp với phần mô tả của skill.

Trong lần chạy đầu tiên, chương trình cũng tự động cài đặt sẵn một số skill hệ thống cho các quy trình phổ biến:
`skill-creator`, `delegate`, `v4-best-practices`, `plugin-creator`, `skill-installer`, `mcp-builder`, `documents`, `presentations`, `spreadsheets`, `pdf`, và `feishu`. Các skill này nằm trong thư mục `~/.codewhale/skills` (hoặc thư mục cũ `~/.deepseek/skills`) và được quản lý phiên bản để các bản nâng cấp mới được cài đặt tự động mà không làm ảnh hưởng đến các skill do người dùng tự chủ động xóa trước đó.

---

## Tài liệu hướng dẫn

| Tài liệu | Chủ đề chi tiết |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Cấu trúc bên trong của cơ sở mã nguồn |
| [CONFIGURATION.md](docs/CONFIGURATION.md) | Hướng dẫn cấu hình chi tiết và đầy đủ nhất |
| [MODES.md](docs/MODES.md) | Các chế độ hoạt động: Plan / Agent / YOLO |
| [MCP.md](docs/MCP.md) | Tích hợp giao thức Model Context Protocol |
| [RUNTIME_API.md](docs/RUNTIME_API.md) | Hướng dẫn sử dụng máy chủ API HTTP/SSE |
| [INSTALL.md](docs/INSTALL.md) | Hướng dẫn cài đặt riêng theo từng nền tảng |
| [DOCKER.md](docs/DOCKER.md) | Sử dụng Docker image trên GHCR, volume lưu trữ |
| [CNB_MIRROR.md](docs/CNB_MIRROR.md) | CNB mirror và các lưu ý cài đặt tại Trung Quốc |
| [TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md) | Hướng dẫn kết nối Tencent/CNB/Lighthouse/Feishu từ xa |
| [TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) | Thiết lập máy chủ Lighthouse Hồng Kông |
| [MEMORY.md](docs/MEMORY.md) | Hướng dẫn tính năng tự ghi nhớ thông tin người dùng |
| [SUBAGENTS.md](docs/SUBAGENTS.md) | Phân loại vai trò và vòng đời của các sub-agent con |
| [KEYBINDINGS.md](docs/KEYBINDINGS.md) | Danh sách phím tắt đầy đủ |
| [RELEASE_RUNBOOK.md](docs/RELEASE_RUNBOOK.md) | Quy trình đóng gói và phát hành phiên bản mới |
| [LOCALIZATION.md](docs/LOCALIZATION.md) | Ma trận đa ngôn ngữ giao diện & cách chuyển đổi |
| [OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) | Vận hành và phục hồi hệ thống |

Lịch sử cập nhật chi tiết: [CHANGELOG.md](CHANGELOG.md).

---

## Lời cảm ơn

- **[DeepSeek](https://github.com/deepseek-ai)** — Xin chân thành cảm ơn sự hỗ trợ và các mô hình AI mạnh mẽ giúp tiếp sức cho mọi tương tác trong dự án. 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — Xin cảm ơn sự hỗ trợ nhiệt tình và đã chào đón chúng tôi gia nhập gia đình lớn "Whale Brother". 感谢 DataWhale 的支持，并欢迎 chúng tôi gia nhập “鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — Cảm ơn vì đã ưu tiên hỗ trợ codewhale và hợp tác để mang lại trải nghiệm agent terminal tốt hơn.
- **[Open Design](https://github.com/nexu-io/open-design)** — Cảm ơn vì sự hỗ trợ và hợp tác xung quanh quy trình làm việc chú trọng thiết kế của agent.

Dự án này được phát triển và vận hành trơn tru với sự đóng góp của cộng đồng các nhà phát triển ngày càng lớn mạnh:

Các đóng góp đã được merge hoặc được harvest trong v0.8.48: **[@cy2311](https://github.com/cy2311)**, **[@LING71671](https://github.com/LING71671)**, **[@axobase001](https://github.com/axobase001)**, **[@dzyuan](https://github.com/dzyuan)**, **[@mvanhorn](https://github.com/mvanhorn)**, **[@malsony](https://github.com/malsony)**, **[@gaord](https://github.com/gaord)**, **[@yuanchenglu](https://github.com/yuanchenglu)**, **[@idling11](https://github.com/idling11)**, **[@h3c-hexin](https://github.com/h3c-hexin)**, **[@AdityaVG13](https://github.com/AdityaVG13)**, **[@Sskift](https://github.com/Sskift)**, **[@cyq1017](https://github.com/cyq1017)**, **[@HUQIANTAO](https://github.com/HUQIANTAO)**, **[@New2Niu](https://github.com/New2Niu)**, **[@AiurArtanis](https://github.com/AiurArtanis)**, **[@Lee-take](https://github.com/Lee-take)**, **[@nightt5879](https://github.com/nightt5879)**, **[@AresNing](https://github.com/AresNing)**, **[@AccMoment](https://github.com/AccMoment)**, **[@reidliu41](https://github.com/reidliu41)**, **[@aboimpinto](https://github.com/aboimpinto)**, **[@zhuangbiaowei](https://github.com/zhuangbiaowei)**, **[@donglovejava](https://github.com/donglovejava)**, **[@hongqitai](https://github.com/hongqitai)**, **[@zlh124](https://github.com/zlh124)**, **[@encyc](https://github.com/encyc)**, **[@Implementist](https://github.com/Implementist)**, **[@lihuan215](https://github.com/lihuan215)**, **[@LeoAlex0](https://github.com/LeoAlex0)**, **[@jimmyzhuu](https://github.com/jimmyzhuu)**, **[@rockyzhang](https://github.com/rockyzhang)**, **[@mo-vic](https://github.com/mo-vic)**, **[@hufanexplore](https://github.com/hufanexplore)**, **[@hoclaptrinh33](https://github.com/hoclaptrinh33)** và **[@BryonGo](https://github.com/BryonGo)**.

Xin cảm ơn các báo cáo, bước tái hiện lỗi và xác minh từ **[@buko](https://github.com/buko)**, **[@yyyCode](https://github.com/yyyCode)**, **[@gaslebinh-glitch](https://github.com/gaslebinh-glitch)**, **[@Dr3259](https://github.com/Dr3259)**, **[@lpeng1711694086-lang](https://github.com/lpeng1711694086-lang)**, **[@VerrPower](https://github.com/VerrPower)**, **[@yan-zay](https://github.com/yan-zay)**, **[@jretz](https://github.com/jretz)**, **[@Neo-millunnium](https://github.com/Neo-millunnium)**, **[@caeserchen](https://github.com/caeserchen)**, **[@T-Phuong-Nguyen](https://github.com/T-Phuong-Nguyen)**, **[@zhyuzhyu](https://github.com/zhyuzhyu)**, **[@0gl20shk0sbt36](https://github.com/0gl20shk0sbt36)**, **[@hatakes](https://github.com/hatakes)**, **[@goodvecn-dev](https://github.com/goodvecn-dev)**, **[@bevis-wong](https://github.com/bevis-wong)**, **[@PurplePulse](https://github.com/PurplePulse)** và **[@nbiish](https://github.com/nbiish)** đã giúp định hình v0.8.48.

- **[merchloubna70-dot](https://github.com/merchloubna70-dot)** — Đóng góp 28 PR bao gồm tính năng mới, sửa lỗi và dựng sẵn extension cho VS Code (#645–#681)
- **[WyxBUPT-22](https://github.com/WyxBUPT-22)** — Xây dựng trình kết xuất Markdown hỗ trợ bảng biểu, chữ đậm/nghiêng và đường kẻ ngang (#579)
- **[loongmiaow-pixel](https://github.com/loongmiaow-pixel)** — Tài liệu cài đặt cho Windows và Trung Quốc (#578)
- **[20bytes](https://github.com/20bytes)** — Cải tiến tài liệu tính năng tự ghi nhớ và giao diện trợ giúp (#569)
- **[staryxchen](https://github.com/staryxchen)** — Kiểm tra độ tương thích của thư viện glibc trước khi chạy (#556)
- **[Vishnu1837](https://github.com/Vishnu1837)** — Tối ưu hóa tính tương thích glibc và tự phục hồi trạng thái terminal khi nhận tín hiệu SIGINT/SIGTERM (#565, #1586)
- **[shentoumengxin](https://github.com/shentoumengxin)** — Kiểm tra hợp lệ ranh giới thư mục làm việc `cwd` của Shell (#524)
- **[toi500](https://github.com/toi500)** — Báo cáo và sửa lỗi dán văn bản trên hệ điều hành Windows
- **[xsstomy](https://github.com/xsstomy)** — Báo cáo lỗi vẽ lại màn hình khi khởi động terminal
- **Melody0709** — Báo cáo lỗi kích hoạt phím Enter với tiền tố lệnh gạch chéo
- **[lloydzhou](https://github.com/lloydzhou)** và **[jeoor](https://github.com/jeoor)** — Báo cáo lỗi chi phí nén dữ liệu; lloydzhou cũng đóng góp ngữ cảnh môi trường xác định (#813, #922) và ổn định bộ nhớ đệm KV prefix-cache (#1080)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — Tinh chỉnh diễn đạt rõ ràng cho file giới thiệu README (#685)
- **[woyxiang](https://github.com/woyxiang)** — Tài liệu hướng dẫn cài đặt qua Scoop trên Windows (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — Cập nhật thông tin giá cả và chương trình khuyến mãi (#692)
- **[zichen0116](https://github.com/zichen0116)** — Xây dựng tài liệu quy tắc ứng xử cộng đồng CODE_OF_CONDUCT.md (#686)
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — Báo cáo tính tương thích chữ hoa/thường của ID mô hình (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — Báo cáo lỗi trạng thái `working...` bị kẹt, cơ chế dự phòng khay nhớ tạm (clipboard) trên Windows, sửa lỗi phiên kết nối HTTP dạng MCP Streamable, và tự động hóa brew tap (#738, #850, #1643, #1631)
- **[reidliu41](https://github.com/reidliu41)** — Ý tưởng gợi ý tiếp tục phiên, lưu trữ độ tin cậy workspace, hỗ trợ nhà cung cấp Ollama, hoàn thiện stream khối suy nghĩ, tăng cường cache cho CI, xử lý wrap dòng stream, và hoàn thành tính năng autocomplete cho DeepSeek (#863, #870, #921, #1078, #1603, #1628, #1601)
- **[xieshutao](https://github.com/xieshutao)** — Cơ chế dự phòng skill dạng Markdown thuần (#869)
- **[GK012](https://github.com/GK012)** — Cơ chế dự phòng lệnh `--version` của wrapper npm (#885)
- **[y0sif](https://github.com/y0sif)** — Xử lý đánh thức vòng lặp agent cha sau khi các sub-agent con hoàn thành tác vụ (#901)
- **[mac119](https://github.com/mac119)** và **[leo119](https://github.com/leo119)** — Viết tài liệu hướng dẫn cho lệnh `codewhale update` (#838, #917)
- **[dumbjack](https://github.com/dumbjack)** / **浩淼的mac** — Tăng cường bảo mật chống mã độc qua lệnh shell byte rỗng (#706, #918)
- **macworkers** — Cải tiến xác nhận rẽ nhánh (fork) kèm mã phiên làm việc mới (#600, #919)
- **zero** và **[zerx-lab](https://github.com/zerx-lab)** — Cấu hình điều kiện nhận thông báo và làm phong phú nội dung thông báo qua OSC 9 (#820, #920)
- **[chnjames](https://github.com/chnjames)** — Gợi ý hoàn thành @mentions từ cache, cải tiến phục hồi file cấu hình lỗi, và hiển thị chuẩn UTF-8 cho Shell trên Windows (#849, #927, #982, #1018)
- **[angziii](https://github.com/angziii)** — Bảo mật cấu hình, dọn dẹp tài nguyên bất đồng bộ, tăng cường bảo mật Docker và vá lỗi an toàn thực thi lệnh (#822, #824, #827, #831, #833, #835, #837)
- **[elowen53](https://github.com/elowen53)** — Giải mã UTF-8 và bổ sung các ca kiểm thử xác định (#825, #840)
- **[wdw8276](https://github.com/wdw8276)** — Bổ sung lệnh `/rename` để đổi tên tiêu đề phiên làm việc tùy chỉnh (#836)
- **[banqii](https://github.com/banqii)** — Hỗ trợ đường dẫn tìm kiếm skill dạng `.cursor/skills` (#817)
- **[junskyeed](https://github.com/junskyeed)** — Tính toán động giá trị `max_tokens` cho các yêu cầu API (#826)
- **Hafeez Pizofreude** — Triển khai cơ chế chống tấn công SSRF trong công cụ `fetch_url` và biểu đồ lịch sử Star History.
- **Unic (YuniqueUnic)** — Xây dựng giao diện cấu hình tự động dựa trên schema (cả TUI và web).
- **Jason** — Tăng cường bảo mật an toàn mạng chống tấn công giả mạo yêu cầu từ phía máy chủ (SSRF).
- **[axobase001](https://github.com/axobase001)** — Dọn dẹp snapshot mồ côi, bổ sung bộ bảo vệ khi cài npm, sửa lỗi đo lường phiên làm việc, xóa cache phạm vi mô hình, hỗ trợ các liên kết tượng trưng (symlinks) cho skill, hướng dẫn cơ chế thoát lỗi cài đặt npm mirror, và duy trì cấu hình proxy cho các tác vụ con (#975, #1032, #1047, #1049, #1052, #1019, #1051, #1056, #1608)
- **[MengZ-super](https://github.com/MengZ-super)** — Xây dựng nền tảng cho lệnh `/theme` và giải nén dữ liệu nén dạng gzip/brotli cho kết nối SSE (#1057, #1061)
- **[DI-HUO-MING-YI](https://github.com/DI-HUO-MING-YI)** — Vá lỗi bảo mật sandbox chỉ đọc trong chế độ Plan (#1077)
- **[bevis-wong](https://github.com/bevis-wong)** — Cung cấp ca tái hiện chính xác lỗi tự động gửi tin khi dán văn bản kèm ký tự xuống dòng (#1073)
- **[Duducoco](https://github.com/Duducoco)** và **[AlphaGogoo](https://github.com/AlphaGogoo)** — Xây dựng thanh menu gạch chéo cho skill và sửa lỗi bao phủ lệnh `/skills` (#1068, #1083)
- **[ArronAI007](https://github.com/ArronAI007)** — Sửa lỗi hiển thị tài nguyên artifact khi thay đổi kích thước cửa sổ trên macOS Terminal.app và ConHost (#993)
- **[THINKER-ONLY](https://github.com/THINKER-ONLY)** — Duy trì mã mô hình tùy chỉnh cho OpenRouter và endpoint riêng (#1066)
- **[Jefsky](https://github.com/Jefsky)** — Báo cáo sửa lỗi địa chỉ endpoint chính thức của DeepSeek (#1079, #1084)
- **[wlon](https://github.com/wlon)** — Chẩn đoán và ưu tiên lựa chọn khóa xác thực cho nhà cung cấp NVIDIA NIM (#1081)
- **[Horace Liu](https://github.com/liuhq)** — Đóng gói hỗ trợ Nix package và viết tài liệu hướng dẫn cài đặt (#1173)
- **[jieshu666](https://github.com/jieshu666)** — Giảm thiểu hiện tượng nhấp nháy màn hình khi vẽ lại giao diện TUI (#1563)
- **[gordonlu](https://github.com/gordonlu)** — Sửa lỗi nhận dạng phím Enter / mã nhập CSI-u trên Windows (#1612)
- **[mdrkrg](https://github.com/mdrkrg)** — Vá lỗi sập ứng dụng trong lần chạy đầu tiên khi thiếu khóa API (#1598)
- **[Aitensa](https://github.com/Aitensa)** — Xử lý tự động xuống dòng CJK cho các khối diff và kết quả đầu ra trang giấy (#1622)
- **[qiyan233](https://github.com/qiyan233)** — Đảm bảo tương thích với các bí danh cũ của nhà cung cấp DeepSeek Trung Quốc (#1645)
- **[zlh124](https://github.com/zlh124)** — Báo cáo khởi động không đầu WSL2 và sửa lỗi khay nhớ tạm (#1772, #1773)
- **[aboimpinto](https://github.com/aboimpinto)** — Sửa lỗi ghi nhật ký màn hình phụ trên Windows, hoàn thiện phím Home/End tại bộ soạn thảo và theo dõi log runtime (#1774, #1776, #1748, #1749, #1782, #1783)
- **[LeoLin990405](https://github.com/LeoLin990405)** — Bổ sung cơ chế truyền thẳng mô hình qua provider, phát lại luồng suy nghĩ, tối ưu lượt chạy chỉ suy nghĩ, và sửa lỗi trích dẫn trên Windows (#1740, #1743, #1742, #1744)
- **[nightt5879](https://github.com/nightt5879)** — Khắc phục lỗi khôi phục giao diện nhắc nhở khi bấm phím Ctrl+C (#1764)
- **[donglovejava](https://github.com/donglovejava)** — Hợp nhất kéo thả dán tệp `@file`, vá lỗi sập chữ CJK, thu thập phản hồi người dùng, định tuyến RLM, và thử lại khi `edit_file` bị kẹt (#2154–#2168)
- **[encyc](https://github.com/encyc)** — Hiển thị chi tiết số lượng token tiêu thụ ở chân trang và lệnh `/status` (#2152)
- **[saieswar237](https://github.com/saieswar237)** — Bổ sung tài liệu hướng dẫn về quy trình review code (#2178)
- **[sximelon](https://github.com/sximelon)** — Chặn sự kiện tự gửi tin khi dán văn bản và tách phân hệ quản lý phím bấm (#2174, #2042)
- **[nanookclaw](https://github.com/nanookclaw)** — Bổ sung hiển thị nhà cung cấp tìm kiếm trong kết quả của lệnh doctor (#2135)
- **[Sskift](https://github.com/Sskift)** — Ngăn chặn việc ghi đè biến môi trường mặc định trên CLI (#2119)
- **[xin1104](https://github.com/xin1104)** — Tạo brew formula cài binary codewhale độc lập (#2105)
- **[mrluanma](https://github.com/mrluanma)** — Bổ sung nhà cung cấp dịch vụ tìm kiếm Metaso (#2059)
- **[Lellansin](https://github.com/Lellansin)** — Bỏ qua việc gộp cấu hình tại thư mục home người dùng (#2055)
- **[zhuangbiaowei](https://github.com/zhuangbiaowei)** — Cập nhật các kênh phát hành chính thức của sản phẩm (#2145)

---

## Đóng góp cho dự án

Xem tài liệu hướng dẫn đóng góp tại [CONTRIBUTING.md](CONTRIBUTING.md). Chúng tôi luôn hoan nghênh các yêu cầu kéo Pull Requests — vui lòng xem danh sách các [vấn đề mở (open issues)](https://github.com/Hmbown/CodeWhale/issues) để bắt đầu đóng góp những phần việc đầu tiên.

Ủng hộ nhà phát triển: [Buy me a coffee](https://www.buymeacoffee.com/hmbown).

> [!Note]
> *Dự án này độc lập và không trực thuộc công ty DeepSeek Inc.*

## Bản quyền

[MIT](LICENSE)

## Biểu đồ Star History

[![Biểu đồ lịch sử sao](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
