<!-- source: README.md sha256:797a23968c31 -->
# Codewhale

一个面向终端的开源编程智能体——模型由你自带。

Codewhale 最初是为 DeepSeek 打造的原生体验,如今已成长为一个由社区驱动的项目:一套契合日益壮大的国际社区需求的编程工具,尽可能支持更多的模型与 provider——开放模型优先,托管或本地皆可,彼此之间没有谁被优先对待。

给它一个 provider、一个模型和一个任务:它会读你的代码、改文件、跑命令、检查自己的工作,并在任务完成或需要你介入时停下。任务中途用 `/model` 切换模型。交互式工作用 TUI,脚本和 CI 用 `codewhale exec`。它用 Rust 编写,采用 MIT 许可,运行在你自己的机器上。

我们一直在寻找贡献者和改进的方式。如果你在用的某个模型或 provider 还不支持,或者有什么东西坏了,告诉我们就是你能做的最有用的事之一——见[贡献](#贡献)。

[English](README.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)

![Codewhale 在终端中运行](assets/screenshot.png)

## 安装

```bash
npm install -g codewhale
```

Cargo、Docker、Nix、Scoop、预编译归档、Android/Termux,以及面向无法访问 GitHub 用户的 CNB 镜像,均见 [docs/INSTALL.md](docs/INSTALL.md)。从 `deepseek-tui` 迁移过来?你的配置和会话可以直接沿用——见 [docs/REBRAND.md](docs/REBRAND.md)。

## 使用

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

在 TUI 中:`/model` 同时切换 provider 和模型,`/fleet` 运行一组 worker,`/restore` 撤销某一轮。输入区空闲时,`Tab` 在 Plan / Act / Operate 之间循环切换,`Shift+Tab` 在 Ask / Auto-Review / Full Access 权限姿态之间循环切换。`!` 让 shell 命令经由正常的审批路径运行。

## 功能

- **任意模型,任意 provider。** DeepSeek、Claude、GPT、Kimi、GLM 等 30 多家 provider,以及你自己的 vLLM、SGLang、Ollama——无需 key——全都跑在同一套运行时和同一套工具之上。上下文预算与价格取自真实路由;价格未知时显示未知,而不是 $0。
- **默认只读,放开权限才更进一步。** Plan 模式不改动文件,审批把关每一次高风险命令。只有当命令确实被 OS 沙箱包装时,Codewhale 才会如实标明:macOS 上是可用时启用的 Seatbelt,Linux 上是需显式启用的 bubblewrap。仓库的 `constitution.json` 会编译成写入拦截,连 Full Access 也无法跳过。
- **随时可以续跑的工作。** Fleet 把每一步记录在只追加的账本里,`fleet resume` 从你停下的地方继续。

## 了解更多

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — 每一条 provider 路由:托管、网关与本地
- [docs/FLEET.md](docs/FLEET.md) — Fleet、账本与恢复
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`、hooks 与 constitution
- [docs/WEB.md](docs/WEB.md) — 仅限回环地址的内置浏览器客户端及其一次性身份验证边界

其余内容——模式、键位绑定、沙箱细节、MCP、运行时 API、架构——见 [docs](docs) 与 [codewhale.net](https://codewhale.net/)。

## 贡献

Issue、PR、复现步骤、日志和功能请求,在这里都算真实的项目工作,也欢迎第一次贡献。当一个 PR 无法原样合并时,维护者会吸收其中可用的部分,并保留作者的署名——在提交、更新日志和 [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) 中。

- [开放 issue](https://github.com/Hmbown/CodeWhale/issues) —— 适合入门的贡献在这里
- [CONTRIBUTING.md](CONTRIBUTING.md) —— 开发环境搭建与 PR 流程
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) —— 每一位塑造过这个项目的人
- [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

感谢 [DeepSeek](https://github.com/deepseek-ai) 提供让项目起步的模型与支持,感谢 [DataWhale](https://github.com/datawhalechina) 🐋 欢迎我们加入“鲸兄弟”大家庭,也感谢 [OpenWarp](https://github.com/zerx-lab/warp) 与 [Open Design](https://github.com/nexu-io/open-design) 在终端智能体体验上的协作。

## 许可证

[MIT](LICENSE)。独立的社区项目,与任何模型 provider 均无隶属关系。

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
