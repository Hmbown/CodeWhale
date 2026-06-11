# CodeWhale

> Harness agent local cho DeepSeek V4 và model mở: bản ngã vận hành, thứ bậc quyền lực, và vòng chứng cứ.

[English README](README.md) · [简体中文 README](README.zh-CN.md) · [日本語 README](README.ja-JP.md)

![codewhale screenshot](assets/screenshot.png)

## Ý tưởng chính

Phần lớn coding agent bắt đầu bằng sức mạnh: nhiều công cụ hơn, context dài hơn, tự động hóa nhiều hơn. CodeWhale bắt đầu bằng trách nhiệm.

Trước khi một agent sửa repo, nó cần một địa chỉ: terminal này, người dùng này, branch này, session này. Đó là lớp ego. Không phải khoe mẽ, mà là tính liên tục. Không phải mặt nạ personality, mà là nơi trách nhiệm bám vào.

Sau đó nó cần luật. Workspace thật là một chồng xung đột: ý định hiện tại của người dùng, hướng dẫn trong repo, output từ shell, memory cũ, handoff cũ, chính sách an toàn và thay đổi đang dang dở có thể va vào nhau trong cùng một lượt. Constitution của CodeWhale xếp thứ tự cho các nguồn đó: yêu cầu hiện tại cao hơn ngữ cảnh cũ; bằng chứng trực tiếp cao hơn phỏng đoán; kiểm chứng cao hơn sự tự tin; personality chỉ điều chỉnh giọng nói, không quyết định hành động.

Sản phẩm thật là lớp sắp thứ tự quanh model: ai đang hành động, luật nào thắng, chứng cứ nào tồn tại, và người hoặc agent tiếp theo có thể tiếp tục ra sao.

## CodeWhale cung cấp gì

- TUI chạy cục bộ trong terminal.
- Công cụ có schema cho file, Shell, Git, Web, MCP, RLM và sub-agent.
- Cổng phê duyệt, sandbox, snapshot side-git và rollback bằng `/restore`.
- Phản hồi diagnostics từ language server sau khi chỉnh sửa.
- Sub-agent chạy song song, session bền, fork, relay handoff và Runtime API.
- DeepSeek V4 là đường chính, cùng các provider rõ ràng như OpenRouter, Xiaomi MiMo, NVIDIA NIM, Arcee, SiliconFlow, Fireworks, Novita, SGLang/vLLM tự host, Ollama và các bề mặt Hugging Face khi chúng được hoàn thiện.

DeepSeek là first-class, nhưng không phải giới hạn duy nhất. Provider, model, base URL và credentials là các lựa chọn tách biệt.

## Cài đặt

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
codewhale --version
codewhale --model auto
```

Các đường khác:

```bash
# GitHub Releases có archive theo nền tảng:
# https://github.com/Hmbown/CodeWhale/releases

# Nếu GitHub không ổn định, dùng CNB mirror:
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.58 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.58 codewhale-tui --locked --force

# Homebrew legacy trong lúc formula vẫn dùng tên deepseek-tui
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

Wrapper npm `codewhale` cũng có thể được cài đặt qua `npm install -g codewhale`.

Docker, tải trực tiếp, mirror Trung Quốc, Windows/Scoop, Nix, checksum và troubleshooting nằm trong [docs/INSTALL.md](docs/INSTALL.md).

## Lần chạy đầu tiên

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

Các lệnh trong TUI thường dùng: `/provider`, `/model`, `/config`, `/statusline`, `/skills`, `/restore`. Bắt đầu dòng bằng `!` để chạy lệnh Shell qua cơ chế approval và sandbox bình thường.

## Tài liệu chi tiết

README chỉ giữ ý tưởng và đường đi nhanh nhất. Chi tiết nằm trong docs và [codewhale.net](https://codewhale.net/):

- [User guide](docs/GUIDE.md)
- [Install guide](docs/INSTALL.md)
- [Configuration](docs/CONFIGURATION.md)
- [Provider registry](docs/PROVIDERS.md)
- [Sub-agents](docs/SUBAGENTS.md)
- [Runtime API](docs/RUNTIME_API.md)
- [Model Lab](docs/MODEL_LAB.md)
- [Architecture](docs/ARCHITECTURE.md)
- [v0.9.0 release acceptance](docs/V0_9_0_RELEASE_ACCEPTANCE.md)

## Track v0.9.0

v0.9.0 vẫn là nhánh tích hợp, chưa phải release công khai cho đến khi tag, GitHub Release, npm, Cargo và artifact phát hành thật sự được tạo và kiểm chứng. Trọng tâm hiện tại: relay/handoff, transcript gọn hơn, kiến trúc command/provider, Runtime API cho VS Code/GUI, HarnessProfile, WhaleFlow và credit hygiene cho cộng đồng đóng góp.

## Lời cảm ơn

- **[DeepSeek](https://github.com/deepseek-ai)** — Xin chân thành cảm ơn sự hỗ trợ và các mô hình AI mạnh mẽ giúp tiếp sức cho mọi tương tác trong dự án. 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — Xin cảm ơn sự hỗ trợ nhiệt tình và đã chào đón chúng tôi gia nhập gia đình lớn "Whale Brother". 感谢 DataWhale 的支持，并欢迎 chúng tôi gia nhập “鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — Cảm ơn vì đã ưu tiên hỗ trợ codewhale và hợp tác để mang lại trải nghiệm agent terminal tốt hơn.
- **[Open Design](https://github.com/nexu-io/open-design)** — Cảm ơn vì sự hỗ trợ và hợp tác xung quanh quy trình làm việc chú trọng thiết kế của agent.

Dự án này được phát triển và vận hành trơn tru với sự đóng góp của cộng đồng các nhà phát triển ngày càng lớn mạnh:

Các đóng góp đã được merge hoặc được harvest trong v0.8.48: **[@cy2311](https://github.com/cy2311)**, **[@LING71671](https://github.com/LING71671)**, **[@axobase001](https://github.com/axobase001)**, **[@dzyuan](https://github.com/dzyuan)**, **[@mvanhorn](https://github.com/mvanhorn)**, **[@malsony](https://github.com/malsony)**, **[@gaord](https://github.com/gaord)**, **[@yuanchenglu](https://github.com/yuanchenglu)**, **[@idling11](https://github.com/idling11)**, **[@h3c-hexin](https://github.com/h3c-hexin)**, **[@AdityaVG13](https://github.com/AdityaVG13)**, **[@Sskift](https://github.com/Sskift)**, **[@cyq1017](https://github.com/cyq1017)**, **[@HUQIANTAO](https://github.com/HUQIANTAO)**, **[@New2Niu](https://github.com/New2Niu)**, **[@AiurArtanis](https://github.com/AiurArtanis)**, **[@Lee-take](https://github.com/Lee-take)**, **[@nightt5879](https://github.com/nightt5879)**, **[@AresNing](https://github.com/AresNing)**, **[@AccMoment](https://github.com/AccMoment)**, **[@reidliu41](https://github.com/reidliu41)**, **[@aboimpinto](https://github.com/aboimpinto)**, **[@zhuangbiaowei](https://github.com/zhuangbiaowei)**, **[@donglovejava](https://github.com/donglovejava)**, **[@hongqitai](https://github.com/hongqitai)**, **[@zlh124](https://github.com/zlh124)**, **[@encyc](https://github.com/encyc)**, **[@Implementist](https://github.com/Implementist)**, **[@lihuan215](https://github.com/lihuan215)**, **[@LeoAlex0](https://github.com/LeoAlex0)**, **[@jimmyzhuu](https://github.com/jimmyzhuu)**, **[@rockyzhang](https://github.com/rockyzhang)**, **[@mo-vic](https://github.com/mo-vic)**, **[@hufanexplore](https://github.com/hufanexplore)**, **[@hoclaptrinh33](https://github.com/hoclaptrinh33)** , **[@BryonGo](https://github.com/BryonGo)**, **[@gordonlu](https://github.com/gordonlu)** và **[@hongchen1993](https://github.com/hongchen1993)**.

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
