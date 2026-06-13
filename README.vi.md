# CodeWhale

> Một terminal agent mã nguồn mở do cộng đồng xây dựng, để viết code với những mô hình tốt nhất hiện có.

[English README](README.md) · [简体中文 README](README.zh-CN.md) · [日本語 README](README.ja-JP.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![codewhale screenshot](assets/screenshot.png)

## CodeWhale là gì

CodeWhale là một terminal agent mã nguồn mở, chạy cục bộ trên máy của bạn để làm
việc thật trong các repository — đọc code, chạy lệnh, sửa file và đóng góp bản vá —
với **mô hình do chính bạn chọn**. Nó kết hợp một bộ công cụ đầy đủ (shell, sửa
file, git, web, MCP và sub-agent) với các cổng phê duyệt, snapshot có thể khôi
phục, và session có thể tiếp tục.

Nó bắt đầu như một TUI lấy cảm hứng từ DeepSeek. Cộng đồng đã biến nó thành thứ
rộng lớn hơn: một harness hoạt động với những mô hình tốt nhất cho đại đa số mọi
người, bất kể provider đó là ai với bạn. DeepSeek vẫn là hạng nhất ở đây; nhưng nó
không còn là lựa chọn tốt duy nhất, và cũng không phải là bắt buộc.

Mọi thứ đều chạy trên chính máy của bạn. Khóa, repo và phán đoán của bạn luôn nằm
trong tay bạn.

## Tại sao dùng nó

- **Công cụ có cổng phê duyệt.** Sửa file, shell, git, web, MCP và các lệnh gọi
  sub-agent đều đi qua một sandbox và chính sách phê duyệt do bạn kiểm soát.
- **Sub-agent & Fleet.** Phân tán điều tra hay triển khai song song qua các
  sub-agent worker headless, và điều phối các chạy nhiều bước.
- **Snapshot & khôi phục.** Mỗi lượt đều được chụp snapshot trong side-git, nên
  `/restore` hoàn tác một thay đổi mà không động tới `.git` của repo bạn.
- **Chẩn đoán trực tiếp.** Sau khi sửa, các language server (nếu có) sẽ sáng lên,
  để bạn thấy lỗi kiểu và cảnh báo ngay khi chúng xảy ra.
- **Session bền vững.** Tiếp tục, phân nhánh và bàn giao giữa các lượt, session,
  và máy — cùng các runtime API cho editor và GUI.
- **Tự mang mô hình của bạn.** Điều hướng mỗi tác vụ tới provider phù hợp nhất.

## Mô hình & provider được hỗ trợ

CodeWhale đi kèm các tuyến hạng nhất cho những provider mà người ta thực sự dùng.
Mang theo key của bạn và chọn mô hình phù hợp với tác vụ:

- **DeepSeek** — V4 Pro / Flash, cùng các gateway tương thích DeepSeek
- **GLM / Z.ai** — GLM-5.1, GLM-5.2 (Z.ai Coding Plan)
- **Kimi (Moonshot)** — Kimi K2.6 / K2.7 Code
- **MiniMax** — tuyến hạng nhất
- **OpenRouter** — hàng trăm mô hình sau một key
- **NVIDIA NIM · Xiaomi MiMo · SiliconFlow · Fireworks · Novita · StepFun / StepFlash**
- **Tự host** — vLLM, SGLang, Ollama
- **Bất kỳ gateway tương thích OpenAI nào**

Chuyển đổi bằng `/provider` và `/model`. Xem [docs/PROVIDERS.md](docs/PROVIDERS.md)
về thông tin xác thực, base URL và giới hạn năng lực.

## Cài đặt

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
codewhale --version
```

Khi khởi động lần đầu, CodeWhale sẽ hỏi một provider key và lưu vào
`~/.codewhale/config.toml`; vì tương thích, cấu hình `~/.deepseek/` cũ vẫn được
đọc.

Các đường dẫn cài đặt khác:

```bash
# npm wrapper
npm install -g codewhale

# Các bản nén theo nền tảng đính kèm ở GitHub Releases
# https://github.com/Hmbown/CodeWhale/releases

# CNB mirror, nếu khó tiếp cận GitHub
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.60 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.60 codewhale-tui --locked --force

# Homebrew (tương thích trong khi formula được đổi tên)
brew tap Hmbown/deepseek-tui && brew install deepseek-tui
```

Về Docker, tải trực tiếp, mirror cho Trung Quốc, Windows/Scoop, Nix, checksum và
khắc phục sự cố, xem [docs/INSTALL.md](docs/INSTALL.md).

**Nâng cấp từ gói `deepseek-tui` cũ?** Cấu hình, session, skill và cài đặt MCP
của bạn được giữ nguyên. Xem [docs/REBRAND.md](docs/REBRAND.md), rồi chạy
`codewhale doctor` để xác nhận.

## Bắt đầu nhanh

```bash
codewhale auth set --provider zai     # hoặc: deepseek, openrouter, kimi, ...
codewhale auth status
codewhale doctor
codewhale                              # khởi động TUI
```

Các lệnh hữu ích trong session:

- `/provider` và `/model` — chọn tuyến và mô hình.
- `/config` — sửa các cài đặt runtime.
- `/statusline` — tuyến hiện tại, chi phí và trạng thái session.
- `/skills` — nạp các workflow dùng lại được từ `~/.codewhale/skills/`.
- `/restore` — khôi phục một lượt trước đó từ snapshot side-git.
- `! cargo test` — chạy một lệnh shell qua đường phê duyệt và sandbox bình thường.

## Cộng đồng & Đóng góp

CodeWhale được xây dựng công khai — và đó chính là điểm cốt lõi. Mục tiêu thật
đơn giản: với nhiều ánh mắt và nhiều bàn tay nhất, xây nên harness agent tốt nhất
cho nhiều người nhất. Những gì bắt đầu như một dự án phụ lấy cảm hứng từ DeepSeek
của một người đã được cộng đồng nhào nặn thành thứ vượt xa ý định ban đầu.

**Chúng tôi rất hoan nghênh issue và pull request, bất kể bạn tự thấy mình giàu
kinh nghiệm đến đâu.** Báo cáo bug, ý tưởng tính năng, sửa tài liệu, "PR đầu tiên",
và cả những câu hỏi tò mò đều được tính là công việc dự án thật. Ngay cả khi bản
vá cuối cùng phải thu hẹp, trì hoãn, hay gộp vào một commit của maintainer, các
maintainer vẫn xem báo cáo và PR là những đóng góp — và những người đóng góp thường
xuyên được ghi nhận dài lâu trong hồ sơ công khai.

- [Các issue đang mở](https://github.com/Hmbown/CodeWhale/issues) — có nhiều thứ
  phù hợp để đóng góp lần đầu.
- [CONTRIBUTING.md](CONTRIBUTING.md) — dựng vòng lặp phát triển và mở một PR.
- [Quy tắc ứng xử](CODE_OF_CONDUCT.md) — hãy tử tế với nhau.
- [Những người đóng góp](docs/CONTRIBUTORS.md) — những người đã nhào nặn CodeWhale.

## Tài liệu

README giúp bạn khởi động; chi tiết nằm ở [`docs/`](docs) và trên
[codewhale.net](https://codewhale.net/):

- [Hướng dẫn người dùng](docs/GUIDE.md) — giờ đầu tiên của bạn với CodeWhale.
- [Hướng dẫn cài đặt](docs/INSTALL.md) — mọi đường dẫn gói và khắc phục sự cố.
- [Cấu hình](docs/CONFIGURATION.md) — file cấu hình và cài đặt provider.
- [Provider](docs/PROVIDERS.md) — tuyến mô hình, thông tin xác thực và năng lực.
- [Các chế độ](docs/MODES.md) — Agent, Plan, và YOLO.
- [Sub-agent](docs/SUBAGENTS.md) — vai trò, vòng đời và phục hồi.
- [Fleet](docs/FLEET.md) — chạy đa worker và điều phối headless.
- [Tác giả WhaleFlow](docs/WHALEFLOW_AUTHORING.md) — workflow khai báo.
- [Runtime API](docs/RUNTIME_API.md) — hợp đồng HTTP/SSE, ACP và editor/GUI.
- [MCP](docs/MCP.md) — các máy chủ Model Context Protocol.
- [Kiến trúc](docs/ARCHITECTURE.md) — bố cục crate, luồng runtime, bảo mật.
- [Phím tắt](docs/KEYBINDINGS.md) — bản đồ phím đầy đủ.
- [Sandbox & phê duyệt](docs/SANDBOX.md) · [Trợ năng](docs/ACCESSIBILITY.md)
  · [Docker](docs/DOCKER.md) · [Bộ nhớ](docs/MEMORY.md)
- [Toàn bộ mục lục tài liệu](docs) — mọi thứ khác.

## Bản sắc vận hành & Hiến pháp

CodeWhale có quan điểm rõ ràng về việc một agent **nên hành xử thế nào** trong một
workspace thật, chứ không chỉ là nó có thể làm gì. Quan điểm đó được viết ra thành
[Hiến pháp CodeWhale](docs/AGENT_ETHOS.md), và tóm lại bằng vài ý sau:

- **Agent có một địa chỉ.** Nó là một instance trong *terminal này* và *workspace
  này* — không phải một model card hay một điểm số trên bảng xếp hạng.
- **Bằng chứng hơn tường thuật.** Đầu ra công cụ thắng một phỏng đoán; một lệnh
  thất bại được báo cáo là thất bại; xác minh là một phần của tác vụ.
- **Ý định người dùng là tối thượng.** Yêu cầu hiện tại của bạn thắng các hướng
  dẫn repo cũ, bộ nhớ, và các lần bàn giao trước đó.
- **Luật địa phương là tường minh.** Các repository có thể thêm
  `.codewhale/constitution.json` cho thẩm quyền dự án bền vững, các bất biến được
  bảo vệ và quy tắc xác minh.
- **Chính sách runtime được thực thi.** Các chế độ, cổng phê duyệt, sandbox,
  khôi phục và schema công cụ là code, không phải lời khuyên mà model phải nhớ.

Sản phẩm là lớp thứ tự bao quanh model: ai đang hành động, luật của ai được áp
dụng, bằng chứng nào tồn tại, và làm sao con người hay agent tiếp theo có thể tiếp
nối. Nếu cách đóng khung này hữu ích với bạn, tuyệt; nếu không, bạn có thể bỏ qua
nó và chỉ dùng các công cụ.

## Lời cảm ơn

CodeWhale tồn tại nhờ những người dùng nó, làm hỏng nó, và sửa nó.

- **[DeepSeek](https://github.com/deepseek-ai)** — những mô hình và sự hỗ trợ đã
  giúp dự án này khởi đầu. 感谢 DeepSeek 提供模型与支持。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — vì sự hỗ trợ và vì đã
  đón chúng tôi vào gia đình Whale Brother. 感谢 DataWhale 的支持。
- **[OpenWarp](https://github.com/zerx-lab/warp)** và
  **[Open Design](https://github.com/nexu-io/open-design)** — vì đã hợp tác xây
  dựng một trải nghiệm terminal-agent tốt hơn.
- **Mọi người đóng góp** — bản ghi chép đầy đủ theo PR nằm ở
  [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md). Cảm ơn các bạn.

## Đóng góp

Xem [CONTRIBUTING.md](CONTRIBUTING.md). Hoan nghênh pull request — hãy xem
[các issue đang mở](https://github.com/Hmbown/CodeWhale/issues) để tìm nơi khởi
đầu phù hợp.

## Giấy phép

[MIT](LICENSE)

> *CodeWhale là một dự án cộng đồng độc lập và không liên kết với bất kỳ provider
> mô hình nào.*

## Lịch sử Star

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
