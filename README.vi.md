<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale là tác nhân lập trình mã nguồn mở dành cho terminal, được xây dựng bằng Rust và được cải thiện công khai cùng những người sử dụng nó.

![Codewhale đang chạy trong terminal](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Cài đặt

```bash
npm install -g codewhale
codewhale
```

Trong lần chạy đầu tiên, Codewhale sẽ giúp bạn kết nối với nhà cung cấp hoặc tiếp tục làm việc ngoại tuyến. Codewhale cũng hỗ trợ Cargo, Docker, Nix, Scoop, các gói dựng sẵn, Android/Termux và bản sao CNB. Xem [hướng dẫn cài đặt](docs/INSTALL.md).

Mỗi shell chỉ cần một lệnh để bật tính năng hoàn thành bằng phím Tab — `codewhale completion bash|zsh|fish|powershell|elvish`. Xem [tính năng hoàn thành của shell](docs/INSTALL.md#8-shell-completions).

## Sử dụng

Hãy trò chuyện với Codewhale như khi bạn trao đổi với một đồng đội:

```text
Fix the failing tests and explain what changed.
```

Hoặc chạy tác vụ mà không cần mở TUI:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale có thể đọc kho mã nguồn, chỉnh sửa tệp, chạy lệnh, kiểm tra kết quả và tiếp tục làm việc hướng đến mục tiêu. Bạn quyết định mức quyền truy cập dành cho nó.

## Vì sao chọn Codewhale

- **Dùng mô hình bạn muốn.** Kết nối với nhà cung cấp được lưu trữ hoặc với mô hình cục bộ thông qua Ollama, vLLM hay SGLang. Chuyển nhà cung cấp và mô hình bằng `/model`.
- **Luôn nắm quyền kiểm soát.** Plan chỉ cho phép đọc. Ask, Auto-Review và Full Access hiển thị rõ cách hoạt động của việc phê duyệt. `/undo` hoàn tác lượt gần nhất, còn `/restore` đưa không gian làm việc về một ảnh chụp trước đó.
- **Sắp xếp công việc dài hạn.** Lưu phiên, đặt `/goal` lâu dài, xem lại quy trình trước khi chạy và phối hợp các tác nhân mà không đưa chỉ dẫn nội bộ của chúng vào bản ghi hội thoại của bạn.
- **Mở rộng tác nhân bạn đang có.** Kết nối máy chủ MCP và kỹ năng, cấu hình hook, đồng thời lưu vai trò tác nhân dưới dạng các tệp dễ đọc trong dự án hoặc phần cài đặt cá nhân.

Chạy `/help` trong TUI để xem các lệnh và phím tắt.

## An toàn

Codewhale chạy trên máy của bạn với quyền truy cập do bạn cấp. Chế độ phê duyệt và quy tắc kho mã nguồn giới hạn những gì tác nhân được phép làm; cơ chế sandbox tùy chọn của hệ điều hành tạo thêm một ranh giới thực thi vững chắc hơn ở nơi được hỗ trợ. Giá mô hình chưa xác định sẽ vẫn được ghi là chưa xác định thay vì bị báo là miễn phí.

Đọc [thứ tự cấp quyền](docs/AUTHORIZATION_ORDER.md) để biết chính xác các lớp chính sách và [cấu hình](docs/CONFIGURATION.md) để biết các cài đặt cục bộ.

## Tài liệu

- [Nhà cung cấp và mô hình cục bộ](docs/PROVIDERS.md)
- [Nhóm tác nhân](docs/FLEET.md)
- [MCP](docs/MCP.md), [hook](docs/HOOKS.md) và [cấu hình](docs/CONFIGURATION.md)
- [Ứng dụng web cục bộ](docs/WEB.md)
- [Toàn bộ tài liệu](docs)

## Tham gia cộng đồng

Codewhale trở nên tốt hơn khi mọi người sử dụng, phản hồi những điểm chưa ổn và cùng khắc phục. Nếu thiếu một nhà cung cấp, quy trình còn bất tiện hoặc giao diện terminal cản trở công việc, hãy [mở issue](https://github.com/Hmbown/CodeWhale/issues). Nếu bạn biết cách cải thiện, hãy [mở pull request](CONTRIBUTING.md). Chúng tôi chào đón những đóng góp đầu tiên và người đóng góp luôn được ghi nhận cho phần việc đã được hợp nhất.

Tham gia [Discord](https://discord.gg/37gfS3ksug), hoặc thêm Hunter trên WeChat (`hunterbown`) và đề nghị tham gia nhóm Whale Brothers.

## Lịch sử dự án

Codewhale bắt đầu với tên `deepseek-tui` và vẫn duy trì khả năng tương thích với cấu hình cùng phiên làm việc của dự án đó. Hiện nay Codewhale không phụ thuộc vào nhà cung cấp nào, được duy trì độc lập và không liên kết với bất kỳ nhà cung cấp mô hình nào.

Cảm ơn mọi người đóng góp và các cộng đồng mã nguồn mở đã giúp dự án phát triển. Xem [danh sách người đóng góp](docs/CONTRIBUTORS.md).

## Giấy phép

[MIT](LICENSE). Các phần được điều chỉnh từ những dự án nguồn mở khác được ghi trong [thông báo của bên thứ ba](docs/THIRD_PARTY_NOTICES.md).
