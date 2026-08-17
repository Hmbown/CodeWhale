<!-- source: README.md sha256:4fc19c5f9596 -->
# Codewhale

給終端機用的開源程式設計智能體——模型由你自備。

Codewhale 起初是為 DeepSeek 打造的原生體驗，如今已成長為社群驅動的專案：一套契合持續擴大的國際社群、並盡可能支援更多模型與 provider 的程式設計 harness——開放模型優先，託管或本機皆可，彼此之間沒有誰被特別優待。

給它一個 provider、一個模型和一項任務。它會讀你的程式碼、改檔案、跑指令、檢查自己的工作，並在任務完成或需要你介入時停下。任務進行中可用 `/model` 切換模型。互動式工作用 TUI，腳本和 CI 用 `codewhale exec`。以 Rust 撰寫，採 MIT 授權，跑在你自己的機器上。

和其他 harness 不一樣的地方在於：**每個角色用哪個模型由你決定，而且不必相同。** 一個 Fleet 會為每個角色分別釘住 provider、模型和推論層級——所以又快又便宜的模型可以指揮昂貴的推論模型，GLM 的 builder 也可以和 Kimi 的 reviewer 做同一份工作。寫下你自己的角色、你自己的 constitution，這套 harness 就是你的，而不是我們的。

我們一直在尋找貢獻者和改進的方式。如果你在用的某個模型或 provider 還沒支援，或有東西壞了，告訴我們就是你能做的最有用的事之一——見[貢獻](#貢獻)。

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Codewhale 在終端機中執行](assets/screenshot.png)

## 安裝

```bash
npm install -g codewhale
```

Cargo、Docker、Nix、Scoop、預先建置的封存檔、Android/Termux，以及給無法連上 GitHub 的人用的 CNB 鏡像，都寫在
[docs/INSTALL.md](docs/INSTALL.md)。從 `deepseek-tui` 過來？你的設定和工作階段會沿用——見
[docs/REBRAND.md](docs/REBRAND.md)。

## 使用

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

在 TUI 裡：`/model` 會同時切換 provider 與模型，`/fleet` 組建並執行團隊——一次一個角色，各自帶著自己的模型，`/undo` 還原上一回合，`/restore <N>` 把工作區捲回更早的快照（不帶參數的 `/restore` 只列出快照）。輸入區為空時，`Tab` 在 Plan / Work / Operate 之間循環；輸入區有文字時，`Tab` 改為補全斜線指令和 `@` 提及。`Shift+Tab` 隨時可循環切換 Ask / Auto-Review / Full Access 權限姿態。`!` 讓 shell 指令走一般的核准路徑。

## 功能

- **任意模型、任意 provider——也可以任意混搭。** DeepSeek、Claude、GPT、Kimi、GLM 等 30 多家 provider，以及你自己的 vLLM、SGLang 或 Ollama——不需要金鑰——全都跑在同一套執行環境和同一套工具上。目錄會追蹤每家 provider 的即時陣容——DeepSeek 的 V4 Pro 後端（標示為 `DeepSeek-V4-Pro-0813`）仍以 `deepseek-v4-pro` 呼叫，Grok 4.6 是 xAI 的直接預設，OrcaRouter 則經由 `orcarouter/auto` 路由。儲存下來的角色會明確記錄它的 `provider`、`model` 和推論層級，所以一個 Fleet 可以在同一次執行裡跨越多家廠商，角色的路由也不會取決於當時恰好啟用的是哪個 provider。上下文上限與價格取自真實路由；價格未知時顯示未知，而不是 $0。
- **由你親手寫就的 harness。** 角色就是你能讀、能改的檔案——每個角色一個模型、一套工具姿態和一份常駐指示——放在專案裡讓團隊共用，或放在你的其他個人設定旁邊，跟著你在不同倉庫之間走。constitution 記錄你希望智能體在每一次工作階段中如何行事，讓這套 harness 貼合你的做法，而不是我們的。
- **預設唯讀，你允許之後才再放寬。** Plan 模式不能改檔案，核准把關高風險指令。當作業系統沙箱確實包住一條指令時，Codewhale 會說出來：macOS 上是可用時啟用的 Seatbelt，Linux 上是需自行開啟的 bubblewrap。倉庫的 `constitution.json` 會編譯成寫入鎖定，連 Full Access 也無法略過。
- **隨時可以續跑的工作。** Fleet 把每一步記在只追加的帳本裡，`fleet resume` 從你停下的地方繼續。

## 整合

- **DeepSeek Harness（dsh）——透過 Codewhale 連接。**
  `codewhale integrations dsh connect` 會把既有的 `@deepseek-ai/dsh`
  安裝接到你的 Codewhale provider 路由、權限和工作區，而
  `integrations dsh install-bundle` 會加入可選的 DSH 外掛套件，讓
  `dsh --profile codewhale` 能獨自帶上同一身分。權限與生命週期由
  Codewhale 負責；dsh 保留自己的工作階段、設定檔和憑證，不會被改動。見
  [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md)。
- **VS Code。** 官方擴充功能鷹架（`extensions/vscode`）會在整合式終端機中開啟
  Codewhale，並透過本機執行環境提供唯讀的 Agent View。目前仍是本機開發預覽，尚未上架市集。

## 了解更多

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — 每一條 provider 路由：託管、閘道與本機
- [docs/FLEET.md](docs/FLEET.md) — Fleet、帳本與續跑
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — Workflow 內已凍結、對 provider 中立的實驗性搜尋
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`、hooks 與 constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — 模式、hook、權限規則、安全下限、倉庫規範、核准和沙箱如何組合
- [docs/HOOKS.md](docs/HOOKS.md) — 十一個 TUI 生命週期 hook 事件、其承載，以及其中可引導回合的三個事件（`codewhale exec` 和 CLI 子指令不會觸發 hooks）
- [docs/WEB.md](docs/WEB.md) — 僅限回環位址的瀏覽器用戶端及其一次性驗證邊界

其餘內容——模式、快捷鍵、沙箱細節、MCP、執行環境 API、架構——見 [docs](docs) 與 [codewhale.net](https://codewhale.net/)。

## 貢獻

Issue、PR、重現步驟、紀錄檔和功能請求，在這裡都算真實的專案工作，也歡迎第一次貢獻。當一個 PR 無法原樣合併時，維護者會擷取其中可用的部分，並保留作者的署名——在提交、變更紀錄和 [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) 中。

- [開放的 issue](https://github.com/Hmbown/CodeWhale/issues) —— 適合入門的貢獻在這裡
- [CONTRIBUTING.md](CONTRIBUTING.md) —— 開發環境建置與 PR 流程
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) —— 每一位形塑過這個專案的人
- [請我喝杯咖啡](https://www.buymeacoffee.com/hmbown)

感謝 [DeepSeek](https://github.com/deepseek-ai) 提供讓專案起步的模型與支援，感謝 [DataWhale](https://github.com/datawhalechina) 🐋 歡迎我們加入「鯨兄弟」大家庭，也感謝 [OpenWarp](https://github.com/zerx-lab/warp) 與 [Open Design](https://github.com/nexu-io/open-design) 在終端機智能體體驗上的合作。

## 授權

[MIT](LICENSE)。獨立的社群專案，與任何模型 provider 均無隸屬關係。

![Codewhale 在終端機中並行派出三個唯讀 scout 子代理](assets/fanout.gif)
