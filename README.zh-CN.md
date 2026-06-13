# CodeWhale

> 一个社区共建的智能体终端，用最好的可用模型写代码。

[English README](README.md) · [日本語 README](README.ja-JP.md) · [Tiếng Việt README](README.vi.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![codewhale screenshot](assets/screenshot.png)

## CodeWhale 是什么

CodeWhale 是一个开源的终端智能体，你在本地运行它，在自己的仓库里干实事——读代码、跑命令、改文件、提交补丁——用的是**你自己选的模型**。它把一整套工具面（shell、文件编辑、git、网页、MCP 和子智能体）与审批闸门、可回滚的快照、可恢复的会话组合在一起。

它最初是一个受 DeepSeek 启发的 TUI。社区把它变成了更广阔的东西：一个能用大多数人手头最好的模型来干活的工具，无论你用的是哪家。DeepSeek 在这里依然是一等公民；但它不再是唯一的好选择，也并非必须。

一切都在你自己的机器上运行。密钥、仓库、判断力，始终握在你手里。

## 为什么用它

- **审批闸门工具。** 文件编辑、shell、git、网页、MCP 和子智能体调用，全部经由沙箱与由你掌控的审批策略。
- **子智能体与 Fleet。** 把并行的调查或实现分派到无头子智能体 worker 上，编排多步任务。
- **快照与回滚。** 每一轮都在 side-git 里留下快照，`/restore` 撤销改动时不会动到你仓库的 `.git`。
- **实时诊断。** 编辑后，语言服务器（在可用时）会即时亮起，让你第一时间看到类型错误与告警。
- **持久会话。** 在轮次、会话、机器之间恢复、分叉、中转交接——还有面向编辑器和 GUI 的运行时 API。
- **自带模型。** 把每个任务路由到最适合它的提供商。

## 支持的模型与提供商

CodeWhale 内置了大家真正在用的那些提供商的一等路由。带上你的密钥，按任务挑模型：

- **DeepSeek** —— V4 Pro / Flash，以及 DeepSeek 兼容的网关
- **GLM / Z.ai** —— GLM-5.1、GLM-5.2（Z.ai Coding Plan）
- **Kimi（Moonshot）** —— Kimi K2.6 / K2.7 Code
- **MiniMax** —— 一等路由
- **OpenRouter** —— 一把钥匙，数百个模型
- **NVIDIA NIM · Xiaomi MiMo · SiliconFlow · Fireworks · Novita · StepFun / StepFlash**
- **自托管** —— vLLM、SGLang、Ollama
- **任何 OpenAI 兼容的网关**

用 `/provider` 和 `/model` 切换。凭据、base URL 与能力边界见
[docs/PROVIDERS.md](docs/PROVIDERS.md)。

## 安装

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
codewhale --version
```

首次启动时，CodeWhale 会向你索要一个提供商密钥，并存入
`~/.codewhale/config.toml`；出于兼容，旧的 `~/.deepseek/` 配置仍会被读取。

其他安装方式：

```bash
# npm 封装
npm install -g codewhale

# 平台预编译包见 GitHub Releases
# https://github.com/Hmbown/CodeWhale/releases

# CNB 镜像（GitHub 难以访问时）
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.60 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.60 codewhale-tui --locked --force

# Homebrew（formula 改名期间的兼容方式）
brew tap Hmbown/deepseek-tui && brew install deepseek-tui
```

Docker、直接下载、国内镜像、Windows/Scoop、Nix、校验和与故障排查，见
[docs/INSTALL.md](docs/INSTALL.md)。

**从旧的 `deepseek-tui` 包升级？** 你的配置、会话、技能和 MCP 设置都会保留。见
[docs/REBRAND.md](docs/REBRAND.md)，然后跑一遍 `codewhale doctor` 确认。

## 快速开始

```bash
codewhale auth set --provider zai     # 或：deepseek、openrouter、kimi ……
codewhale auth status
codewhale doctor
codewhale                              # 启动 TUI
```

常用的会话内命令：

- `/provider` 与 `/model` —— 选择路由与模型。
- `/config` —— 编辑运行时设置。
- `/statusline` —— 当前的路由、成本与会话状态。
- `/skills` —— 从 `~/.codewhale/skills/` 加载可复用工作流。
- `/restore` —— 从 side-git 快照回滚之前的某轮。
- `! cargo test` —— 经由正常的审批与沙箱路径跑一条 shell 命令。

## 社区与贡献

CodeWhale 在公开环境中打造——这正是它的意义所在。目标很简单：用最多的眼睛和最多的手，为最多的人做出最好的智能体工具。它起初只是一个人受 DeepSeek 启发做的副业项目，在社区的塑造下，长成了超出它最初设想的东西。

**无论你觉得自己经验几何，我们都欢迎 issue 和 pull request。** bug 报告、功能想法、文档修正、"第一次 PR"、以及带着好奇的提问，全都算作真正的项目工作。即便最终补丁不得不收窄、延后、或并入维护者的提交，维护者也会把报告和 PR 当作贡献来对待——而反复贡献的人，会一直留在公开记录里被致谢。

- [开放的 issue](https://github.com/Hmbown/CodeWhale/issues) —— 适合作为首次贡献。
- [CONTRIBUTING.md](CONTRIBUTING.md) —— 搭好开发循环、提交一个 PR。
- [行为准则](CODE_OF_CONDUCT.md) —— 彼此友善。
- [贡献者](docs/CONTRIBUTORS.md) —— 塑造了 CodeWhale 的人。

## 文档

README 帮你起步；细节都在 [`docs/`](docs) 和 [codewhale.net](https://codewhale.net/)：

- [用户指南](docs/GUIDE.md) —— 与 CodeWhale 共度的第一个小时。
- [安装指南](docs/INSTALL.md) —— 每一种包路径与故障排查。
- [配置](docs/CONFIGURATION.md) —— 配置文件与提供商设置。
- [提供商](docs/PROVIDERS.md) —— 模型路由、凭据与能力。
- [模式](docs/MODES.md) —— Agent、Plan、YOLO。
- [子智能体](docs/SUBAGENTS.md) —— 角色、生命周期与恢复。
- [Fleet](docs/FLEET.md) —— 多 worker 运行与无头编排。
- [WhaleFlow 编写](docs/WHALEFLOW_AUTHORING.md) —— 声明式工作流。
- [运行时 API](docs/RUNTIME_API.md) —— HTTP/SSE、ACP 与编辑器/GUI 契约。
- [MCP](docs/MCP.md) —— Model Context Protocol 服务器。
- [架构](docs/ARCHITECTURE.md) —— crate 布局、运行时流程、安全模型。
- [快捷键](docs/KEYBINDINGS.md) —— 完整按键表。
- [沙箱与审批](docs/SANDBOX.md) · [无障碍](docs/ACCESSIBILITY.md)
  · [Docker](docs/DOCKER.md) · [记忆](docs/MEMORY.md)
- [完整文档索引](docs) —— 其余一切。

## 运行身份与宪法

CodeWhale 对一个智能体在真实工作区里**该如何行动**是有主张的，而不仅仅关心它能做什么。这套主张写成了一份
[CodeWhale 宪法](docs/AGENT_ETHOS.md)，归结起来是几条想法：

- **智能体要有地址。** 它是*这个*终端、*这个*工作区里的一个实例——不是一张模型卡片，也不是一个榜单分数。
- **证据高于叙述。** 工具输出胜过猜测；一条失败的命令就被如实报为失败的命令；验证是任务的一部分。
- **用户意图至上。** 你当下的请求，高于过时的仓库指引、记忆与之前的交接。
- **本地法规是显式的。** 仓库可以加一份 `.codewhale/constitution.json`，承载持久的项目权威、受保护的不变量与验证规则。
- **运行时策略是被强制执行的。** 模式、审批闸门、沙箱、回滚与工具 schema 是代码，而不是模型得去记住的劝告。

这个产品是环绕模型的那一层秩序：是谁在行动、适用谁的法规、存在哪些证据、以及下一个人或智能体如何接续。如果这套框架对你有用，那很好；如果没用，你完全可以无视它，只当工具用。

## 致谢

CodeWhale 之所以存在，是因为那些使用它、把它弄坏、又把它修好的人。

- **[DeepSeek](https://github.com/deepseek-ai)** —— 让这个项目得以起步的模型与支持。感谢 DeepSeek 提供模型与支持。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 —— 感谢支持，也感谢把我们迎进"鲸兄弟"大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** 与
  **[Open Design](https://github.com/nexu-io/open-design)** —— 感谢一同打造更好的终端智能体体验。
- **每一位贡献者** —— 完整的逐 PR 记录见
  [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md)。谢谢你们。

## 贡献

见 [CONTRIBUTING.md](CONTRIBUTING.md)。欢迎 pull request——可以从
[开放的 issue](https://github.com/Hmbown/CodeWhale/issues) 里挑适合的上手。

## 许可证

[MIT](LICENSE)

> *CodeWhale 是一个独立的社区项目，与任何模型提供商均无附属关系。*

## Star 历史

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
