<!-- source: README.md sha256:ddc9cd7b5aea -->
# Codewhale

一个运行在终端里的编程智能体。适配任意模型；开放模型优先。

你给它一个 provider、一个模型和一个任务。它读代码、改文件、跑命令、检查结果，一直做到任务完成或需要你介入为止。交互式工作用 TUI，脚本和 CI 用 `codewhale exec`。Rust 编写，MIT 许可，完全在你自己的机器上运行。

它最初叫 `deepseek-tui`。围绕它形成的社区需要更多 provider，于是现在 DeepSeek、Claude、GPT、Kimi、GLM 以及其他 30 多个模型都跑在同一套运行时和工具之上。

[English](README.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)

![Codewhale 在终端中运行](assets/screenshot.png)

## 安装

```bash
npm install -g codewhale
```

Cargo、Docker、Nix、Scoop、预编译归档、Android/Termux，以及面向无法访问 GitHub 用户的 CNB 镜像，均见 [docs/INSTALL.md](docs/INSTALL.md)。从 `deepseek-tui` 迁移过来？你的配置和会话可以直接沿用——见 [docs/REBRAND.md](docs/REBRAND.md)。

## 使用

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
```

在 TUI 中：`/model` 同时切换 provider 和模型，`/fleet` 运行一组 worker，`/restore` 撤销某一轮，`Tab` 在 Plan / Act / Operate 之间循环切换，`Shift+Tab` 在 Ask / Auto-Review / Full Access 审批姿态之间循环切换，`!` 让 shell 命令经由正常的审批路径运行。

## 它做什么

- 把你选定的 provider + 模型解析为一条具体路由：端点、传输协议、上下文上限、价格。上下文预算与费用显示都取自真实路由；价格未知时就显示未知，而不是 $0。（[docs/PROVIDERS.md](docs/PROVIDERS.md)）
- 可连接托管的开放模型 provider（`deepseek`、`openrouter`、`moonshot`、`zai`、`minimax`、`nvidia-nim` 等），可无 key 直连你自己的 `vllm` / `sglang` / `ollama`，也能通过 Messages API 原生对接 Anthropic，支持 thinking 与 prompt 缓存。
- 以可持久化的方式运行多个 worker：Fleet 把工作记录在只追加的账本里，运行不会因重启而丢失，`fleet resume` 能从中断处继续。Workflow 把更大的任务规划成可恢复、可验证的 lane。（[docs/FLEET.md](docs/FLEET.md)）
- 风险把关靠代码，不靠感觉：三种模式（Plan 为只读）、独立的审批姿态、操作系统级沙箱（Seatbelt、Landlock + seccomp、bwrap）、可对每次工具调用做 allow/deny/ask 决策的 hooks，以及 side-git 快照——`/restore` 永远不会碰你真正的提交历史。
- 允许仓库声明自己的法律：`.codewhale/constitution.json` 中的不变量会编译成写入拦截，连 Full Access 也无法跳过。（[docs/CONFIGURATION.md](docs/CONFIGURATION.md)）
- 双向支持 MCP，可加载可复用的 skills，对外提供 HTTP/SSE 与 ACP 运行时 API，并支撑社区维护的 [VS Code GUI](https://github.com/HengQuWorld/CodeWhale-VSCode)。
- TUI 把工作显示为可逐条查验的回执，同一时间只让一行保持动态，内置真实的上下文查看器、12 套主题、减弱动效与 ASCII 安全模式，界面语言覆盖英语、简体中文、日语、越南语、西班牙语、葡萄牙语和韩语，繁体中文为部分翻译。

其余内容——配置、键位绑定、沙箱细节、架构——见 [docs](docs) 与 [codewhale.net](https://codewhale.net/)。

## 贡献

所有反馈都是礼物。Issue、PR、复现步骤、日志、功能请求和第一次贡献，在这里都算真实的项目工作。当一个 PR 无法原样合并时，维护者会吸收其中可用的部分，作者的署名会保留——在提交、更新日志和 [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) 中。如果你在用的某个模型或 provider 还不支持，或者有什么东西在你机器上坏了，告诉我们就是你能做的最有用的事。

- [开放 issue](https://github.com/Hmbown/CodeWhale/issues) —— 适合入门的贡献在这里
- [CONTRIBUTING.md](CONTRIBUTING.md) —— 开发环境搭建与 PR 流程
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) —— 每一位塑造过这个项目的人
- [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

感谢 [DeepSeek](https://github.com/deepseek-ai) 提供让项目起步的模型与支持，感谢 [DataWhale](https://github.com/datawhalechina) 🐋 欢迎我们加入“鲸兄弟”大家庭，也感谢 [OpenWarp](https://github.com/zerx-lab/warp) 与 [Open Design](https://github.com/nexu-io/open-design) 在终端智能体体验上的协作。

## 许可证

[MIT](LICENSE)。独立的社区项目，与任何模型 provider 均无隶属关系。

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
