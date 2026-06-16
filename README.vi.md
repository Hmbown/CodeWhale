# CodeWhale

> Coding agent trong terminal cho mọi model — ưu tiên model mở.

Một TUI và CLI viết bằng Rust, 24 provider. DeepSeek, OpenRouter, Hugging Face
và vLLM/SGLang/Ollama chạy cục bộ là các đường first-class, và CodeWhale nói
chuyện native với Anthropic Claude và OpenAI khi đó là thứ bạn đang có. Công cụ
qua cổng phê duyệt, sandbox cấp hệ điều hành, và rollback bằng `/restore` cho
mọi lượt.

[English README](README.md) · [简体中文 README](README.zh-CN.md) · [日本語 README](README.ja-JP.md) · [codewhale.net](https://codewhale.net/) · [Hướng dẫn cài đặt](docs/INSTALL.md) · [Danh mục provider](docs/PROVIDERS.md) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![CodeWhale chạy trong terminal](assets/screenshot.png)

## Cài đặt

```bash
npm install -g codewhale
codewhale --version   # 0.8.61
```

Wrapper npm (Node 18+) tải binary đã xác minh SHA-256 từ GitHub Releases và
cài các lệnh `codewhale`, `codew` và `codewhale-tui`. Muốn tự build từ source?
Dùng cargo (Rust 1.88+):

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

> **Người dùng Linux:** cài đặt các gói build trước:
> `sudo apt-get install -y build-essential pkg-config libdbus-1-dev`.
> Xem [INSTALL.md](docs/INSTALL.md#4-install-via-cargo-any-tier-1-rust-target).

Mọi đường cài đặt khác:

```bash
# Docker
docker pull ghcr.io/hmbown/codewhale:latest

# Nix
nix run github:Hmbown/CodeWhale

# Windows
scoop install codewhale        # hoặc trình cài NSIS từ GitHub Releases

# CNB mirror cho người dùng khó truy cập GitHub ổn định
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.61 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.61 codewhale-tui --locked --force

# Homebrew legacy trong lúc formula đang được đổi tên
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

Archive dựng sẵn cho mọi nền tảng — bao gồm cả Linux riscv64 — được đính kèm
trong [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases).
Checksum, mirror Trung Quốc, chi tiết riêng cho Windows và troubleshooting nằm
trong [docs/INSTALL.md](docs/INSTALL.md).

## Lần chạy đầu tiên

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

Mọi provider đều cùng một dạng lệnh một dòng: `--provider openrouter`,
`--provider moonshot`, hoặc trỏ `vllm`, `sglang`, `ollama` vào runtime
localhost của riêng bạn mà không cần key nào cả. Có key Claude? Chạy
`codewhale auth set --provider anthropic` — hoặc chỉ cần export
`ANTHROPIC_API_KEY` — và adapter Messages native sẽ lo phần còn lại.

Key được lưu trong `~/.codewhale/config.toml`; cấu hình cũ trong
`~/.deepseek/` vẫn được đọc để giữ tương thích.

Các lệnh hữu ích trong session:

- `/provider` và `/model` đổi đường định tuyến và model ngay giữa session.
- `/restore` quay lui một lượt trước đó từ snapshot side-git.
- `/skills` nạp các workflow tái sử dụng từ `~/.codewhale/skills/`.
- `/config` chỉnh cài đặt runtime; `/statusline` hiển thị route hiện tại,
  chi phí và trạng thái session.
- `! cargo test -p codewhale-tui` chạy bất kỳ lệnh shell nào qua đường
  approval và sandbox bình thường.

Chế độ headless, cho script và CI:

```bash
codewhale exec --allowed-tools read_file,exec_shell --max-turns 10 "fix the failing test"
```

## CodeWhale cung cấp gì

Một harness agent thuần terminal — TUI + CLI, 16 crate Rust — nơi các rào an
toàn là cơ chế runtime, không phải lời dặn mà model phải tự nhớ:

- **Công cụ qua cổng phê duyệt với sandbox cấp HĐH.** Công cụ file, shell,
  git, web, MCP và sub-agent chạy sau cổng phê duyệt tường minh và các backend
  sandbox (bwrap, Landlock, Seatbelt, seccomp).
- **Rollback đáng tin cậy.** Snapshot side-git và `/restore`, giữ bên ngoài
  `.git` của repo — hoàn tác một lượt không bao giờ chạm vào lịch sử của bạn.
- **Hooks v2** *(0.8.58)*. Hook `tool_call_before` trả về quyết định JSON
  `allow`/`deny`/`ask` với quy tắc deny thắng, matcher dạng glob, và
  `.codewhale/hooks.toml` riêng cho từng dự án.
- **Sub-agent chạy song song với định tuyến theo provider** *(0.8.58)*. Điều
  tra và triển khai song song, với các tier model lớn/rẻ được phân giải theo
  từng provider — không hardcode model id.
- **Session bền.** Fork, relay handoff, và prompt cache lưu trên đĩa dùng
  chung giữa các session, ổn định từng byte khi chuyển qua lại giữa chế độ
  Plan/Agent/YOLO *(0.8.56)*. Lượt chạy sống sót qua sleep hệ thống
  *(0.8.57)*: máy ngủ giữa stream, thức dậy, request được âm thầm gửi lại
  thay vì làm hỏng lượt.
- **Chế độ headless.** `codewhale exec` với `--allowed-tools`,
  `--disallowed-tools` (deny thắng), `--max-turns` và
  `--append-system-prompt` *(0.8.58)* cho script và CI.
- **Nhúng được ở mọi nơi.** Runtime API HTTP/SSE và ACP, extension VS Code
  (Phase 0), và cầu nối Telegram/Feishu.
- **Độ hoàn thiện để dùng hằng ngày.** Vừa là MCP client *vừa* là MCP server,
  skill tái sử dụng, bản địa hóa 7 ngôn ngữ (gồm cả hộp thoại phê duyệt từ
  0.8.56), và speech/TTS qua Xiaomi MiMo.

### Mọi model, ưu tiên model mở

Hai mươi bốn provider đi qua cùng một harness, cùng một constitution, cùng
một bộ công cụ:

- **Model mở, dạng hosted:** `deepseek` (đứng đầu trong nhóm ngang hàng),
  `openrouter`, `huggingface` (Inference Providers), `moonshot` (Kimi),
  `volcengine` (Ark), `nvidia-nim`, `together`, `fireworks`, `novita`,
  `siliconflow` / `siliconflow-CN`, `arcee`, `xiaomi-mimo`, `atlascloud`,
  `wanjie-ark`, cộng thêm một đường `openai`-compatible tổng quát cho bất kỳ
  gateway nào.
- **Model mở, tự host:** `vllm`, `sglang` và `ollama` trỏ vào endpoint
  localhost của riêng bạn — không cần key.
- **Provider đóng, hỗ trợ native:** `anthropic` qua adapter `/v1/messages`
  chuyên dụng *(0.8.58)* với adaptive thinking, breakpoint prompt-cache và
  phát lại signed-thinking — không phải shim giả giọng OpenAI — và
  `openai-codex`, tái sử dụng phiên đăng nhập ChatGPT/Codex CLI sẵn có.

Định tuyến không chỉ là đổi base URL: mức effort của `/reasoning` được dịch
sang phương ngữ wire của từng provider, tier sub-agent phân giải theo
provider, và phần facts về model trong system prompt được template theo từng
model thay vì hardcode *(0.8.58)*. Đổi giữa session bằng `/provider` và
`/model`. Danh mục đầy đủ — credentials, base URL, ranh giới năng lực — nằm
trong [docs/PROVIDERS.md](docs/PROVIDERS.md).

Các nhãn phiên bản ở trên đánh dấu những gì đã hạ cánh trong ba bản phát hành
gần nhất (0.8.56 → 0.8.58). Chi tiết đầy đủ trong [CHANGELOG.md](CHANGELOG.md).

## Ý tưởng chính

Phần lớn coding agent bắt đầu bằng việc thêm sức mạnh: nhiều công cụ hơn,
context dài hơn, tự chủ nhiều hơn. CodeWhale bắt đầu bằng việc gán trách
nhiệm.

Một agent sửa repo của bạn cần có một địa chỉ — terminal này, người dùng này,
branch này, session này. Không phải một persona; một địa chỉ để truy hồi. Khi
có gì đó hỏng, "model làm đấy" không phải là câu trả lời. "Instance này, trong
session này, sau lần phê duyệt này" mới là câu trả lời.

Sau đó nó cần luật. Một phiên làm việc thật là một chồng xung đột: yêu cầu
hiện tại của bạn, chỉ dẫn trong repo, output shell vừa chạy, memory cũ, và
bản handoff của agent trước đó cùng tranh nhau trong một lượt. **Constitution
của CodeWhale** cố định thứ tự quyền lực:

1. **Ý định người dùng là tối thượng.** Yêu cầu hiện tại của bạn đứng trên
   hướng dẫn repo đã cũ, memory, handoff trước đó và các lớp personality.
2. **Luật của repo phải tường minh.** Thêm `.codewhale/constitution.json` để
   khai báo quyền lực bền vững của dự án: các bất biến cần bảo vệ, chính sách
   branch, quy tắc kiểm chứng.
3. **Bằng chứng đứng trên lời kể.** Output của công cụ thắng một phỏng đoán
   tự tin. `cargo test` thất bại được báo cáo đúng là `cargo test` thất bại,
   không bao giờ bị tóm tắt thành lạc quan. Kiểm chứng là một phần của nhiệm
   vụ, không phải phần vĩ thanh.
4. **Memory xếp cuối.** Hữu ích, nhưng không bao giờ có thẩm quyền.

Phần chính sách quan trọng được thực thi bằng code, không phải bằng prompt:
cổng phê duyệt, sandbox, snapshot, rollback và schema công cụ là các cơ chế
runtime mà model không thể nói khéo để lách qua.

Và không phần nào của bộ luật đó nằm trong model — vì thế model mới thay
được. Harness mang constitution; model cung cấp khả năng suy luận. DeepSeek
và thế giới open-weight là công dân hạng nhất, một chiếc máy trong LAN của
bạn chạy vLLM hay Ollama là một peer đầy đủ, và khi thứ bạn có là key Claude
hay OpenAI, CodeWhale cũng nói các API đó một cách native.

Đó chính là sản phẩm: không phải một model lớn hơn, mà một harness nghiêm
khắc hơn quanh bất kỳ model nào bạn chọn. Đổi model; luật vẫn đứng vững.

## Tài liệu chi tiết

README giữ phần ý tưởng và con đường đầu tiên. Chi tiết nằm trong docs và
trên [codewhale.net](https://codewhale.net/):

- [User guide](docs/GUIDE.md) — giờ đầu tiên với CodeWhale.
- [Install guide](docs/INSTALL.md) — mọi đường cài đặt và troubleshooting.
- [Configuration](docs/CONFIGURATION.md) — file cấu hình, constitution của
  repo và cài đặt provider.
- [Provider registry](docs/PROVIDERS.md) — đường model, credentials, base URL
  và ranh giới năng lực.
- [Sub-agents](docs/SUBAGENTS.md) — vai trò, vòng đời, hợp đồng output và
  hành vi phục hồi.
- [MCP](docs/MCP.md) — kết nối tool server bên ngoài và chạy CodeWhale như
  một MCP server.
- [Runtime API](docs/RUNTIME_API.md) — hợp đồng tích hợp HTTP/SSE, ACP,
  mobile và GUI/editor.
- [Model Lab](docs/MODEL_LAB.md) — roadmap khám phá và đánh giá model mở.
- [Architecture](docs/ARCHITECTURE.md) — bố cục crate, luồng runtime, hệ
  thống công cụ, điểm mở rộng và mô hình bảo mật.

## Track v0.9.0

v0.9.0 là làn tích hợp hiện tại. Những việc đang tụ về đó:

- bề mặt relay và handoff mạnh hơn giữa các session và agent;
- transcript gọn gàng hơn cho các chuỗi công cụ dày đặc;
- runtime API cho VS Code và các client GUI;
- điều phối workflow branch/leaf với WhaleFlow.

Chi tiết theo từng bản phát hành nằm trong [CHANGELOG.md](CHANGELOG.md).

## Lời cảm ơn

- **[DeepSeek](https://github.com/deepseek-ai)** — Xin cảm ơn các model và sự
  hỗ trợ đã tiếp sức cho mọi lượt tương tác.
  感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — Xin cảm ơn sự hỗ trợ
  nhiệt tình và đã chào đón chúng tôi vào đại gia đình "Whale Brother".
  感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — Cảm ơn vì đã ưu tiên hỗ
  trợ codewhale và hợp tác để mang lại trải nghiệm agent terminal tốt hơn.
- **[Open Design](https://github.com/nexu-io/open-design)** — Cảm ơn vì sự hỗ
  trợ và hợp tác xung quanh quy trình làm việc chú trọng thiết kế của agent.

Dự án này được phát hành với sự giúp sức của một cộng đồng đóng góp ngày càng
lớn. Nguyên tắc của maintainer rất đơn giản: báo cáo lỗi và PR là công việc
thực sự của dự án, kể cả khi bản vá cuối cùng phải được thu hẹp, hoãn lại,
hoặc harvest vào một nhánh của maintainer.

Danh sách ghi công đầy đủ theo từng người đóng góp — và luôn được cập nhật —
nằm trong [mục Thanks của README tiếng Anh](README.md#thanks), hồ sơ ghi nhận
chính thức của dự án.

---

## Đóng góp cho dự án

Xem [CONTRIBUTING.md](CONTRIBUTING.md). Hoan nghênh các Pull Request — hãy xem
[danh sách issue đang mở](https://github.com/Hmbown/CodeWhale/issues) để tìm
những đóng góp đầu tiên phù hợp.

CodeWhale nhận được rất nhiều báo cáo và PR chất lượng. Lập trường của
maintainer là giữ cánh cửa đó luôn mở trong khi vẫn bảo vệ chất lượng phát
hành:

- Issue nên dễ đọc với con người và có thể hành động được. Tự động hóa khâu
  tiếp nhận chỉ mang tính tư vấn, trừ khi maintainer chủ động bật chế độ
  cưỡng chế.
- PR được review từ code, test, issue liên quan và hành vi runtime, không chỉ
  từ tiêu đề.
- Nếu một PR quá rộng để merge trực tiếp, maintainer có thể harvest phần an
  toàn vào một nhánh hẹp hơn, sau đó ghi công tác giả và giải thích phần nào
  đã được đưa vào.
- Trailer co-author nên dùng danh tính GitHub noreply có thể ánh xạ từ
  `.github/AUTHOR_MAP`; người báo cáo và người viết bước tái hiện lỗi nên
  được cảm ơn trong changelog, release notes và bình luận khi đóng issue.
- Người đóng góp thường xuyên có thể được thêm vào
  `.github/APPROVED_CONTRIBUTORS` để các cổng dry-run không cản đường họ.

Ủng hộ dự án: [Buy me a coffee](https://www.buymeacoffee.com/hmbown).

> [!NOTE]
> *Dự án này không trực thuộc DeepSeek Inc.*

## Giấy phép

[MIT](LICENSE)

## Star History

[![Biểu đồ Star History](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
