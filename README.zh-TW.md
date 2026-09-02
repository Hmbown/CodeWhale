<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale 是一款在終端機中使用的開源程式設計代理，以 Rust 打造，並與使用者一起透過公開協作持續改進。

![Codewhale 在終端機中執行](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## 安裝

```bash
npm install -g codewhale
codewhale
```

第一次執行時，系統會協助你連線至供應商，也可以選擇保持離線。Codewhale 亦支援 Cargo、Docker、Nix、Scoop、預先建置的封存檔、Android/Termux 與 CNB 映像。請參閱[安裝指南](docs/INSTALL.md)。

每種 shell 只需一個指令即可啟用 Tab 自動完成——`codewhale completion bash|zsh|fish|powershell|elvish`。請參閱 [shell 自動完成](docs/INSTALL.md#8-shell-completions)。

## 使用

像和隊友交談一樣告訴 Codewhale 你的需求：

```text
Fix the failing tests and explain what changed.
```

你也可以不開啟 TUI，直接執行任務：

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale 可以讀取你的程式碼儲存庫、編輯檔案、執行指令、檢查結果，並持續朝目標推進。你可以決定要授予它多少存取權限。

## 為何選擇 Codewhale

- **使用你想要的模型。** 連線至託管供應商，或透過 Ollama、vLLM、SGLang 使用本機模型。使用 `/model` 切換供應商與模型。
- **掌控權始終在你手中。** Plan 模式為唯讀。Ask、Auto-Review 與 Full Access 會清楚呈現核准行為。`/undo` 可復原上一輪操作，`/restore` 可將工作區還原至較早的快照。
- **讓長時間工作井然有序。** 儲存工作階段、設定持久的 `/goal`、在工作流程執行前加以審查，並協調多個代理，同時避免其內部指示混入你的對話記錄。
- **擴充你已有的代理。** 連接 MCP 伺服器與技能、設定掛鉤，並將代理角色以可讀檔案保存在專案或個人設定中。

在 TUI 中執行 `/help` 可查看指令與鍵盤快速鍵。

## 安全性

Codewhale 在你的電腦上執行，且只擁有你授予的存取權限。核准模式與儲存庫規則會限制代理可以執行的操作；在支援的平台上，選用的作業系統沙箱可提供更強的執行邊界。未知的模型價格會維持顯示為未知，而不會被誤報為免費。

閱讀[授權順序](docs/AUTHORIZATION_ORDER.md)以了解確切的政策層級，並閱讀[設定](docs/CONFIGURATION.md)以了解本機設定。

## 文件

- [供應商與本機模型](docs/PROVIDERS.md)
- [代理團隊](docs/FLEET.md)
- [MCP](docs/MCP.md)、[掛鉤](docs/HOOKS.md)與[設定](docs/CONFIGURATION.md)
- [本機網頁用戶端](docs/WEB.md)
- [所有文件](docs)

## 加入社群

當人們使用 Codewhale、回報不順手之處並協助修正問題時，它就會變得更好。如果缺少某個供應商、工作流程操作不便，或終端機介面妨礙了你，請[提出 issue](https://github.com/Hmbown/CodeWhale/issues)。如果你知道如何改善，請[提出 pull request](CONTRIBUTING.md)。我們歡迎首次貢獻，貢獻者也會保留已合併工作的署名。

加入 [Discord](https://discord.gg/37gfS3ksug)，或在微信加入 Hunter（`hunterbown`）並申請加入 Whale Brothers 群組。

## 專案歷史

Codewhale 最初名為 `deepseek-tui`，至今仍保留與其設定及工作階段的相容性。現在它不偏向任何供應商，由社群獨立維護，也不隸屬於任何模型供應商。

感謝每一位貢獻者，以及協助專案成長的開源社群。請參閱[貢獻者記錄](docs/CONTRIBUTORS.md)。

## 授權條款

[MIT](LICENSE)。從其他開放原始碼專案改編的部分記錄於[第三方聲明](docs/THIRD_PARTY_NOTICES.md)。
