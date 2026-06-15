# CodeWhale

> 面向任意模型的终端编程智能体——开放模型优先。

一套 Rust TUI 与 CLI，支持 25 个 provider。DeepSeek、OpenRouter、Hugging Face
以及本地 vLLM/SGLang/Ollama 都是一等路由；当你手里是 Anthropic Claude 或
OpenAI 的 key 时，CodeWhale 也以原生协议直连。工具经审批放行、操作系统级沙箱，
每一轮都可用 `/restore` 回滚。

[English README](README.md) · [日本語 README](README.ja-JP.md) · [Tiếng Việt README](README.vi.md) · [codewhale.net](https://codewhale.net/) · [安装指南](docs/INSTALL.md) · [Provider 注册表](docs/PROVIDERS.md) · [更新日志](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![CodeWhale 在终端中运行](assets/screenshot.png)

## 安装

```bash
npm install -g codewhale
codewhale --version   # 0.8.61
```

npm wrapper（Node 18+）会从 GitHub Releases 下载经 SHA-256 校验的二进制，并安装
`codewhale`、`codew` 和 `codewhale-tui` 三个命令。更想从源码构建？用 cargo
（Rust 1.88+）：

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

如果访问 GitHub 不稳定，推荐直接走下面的 CNB 镜像。其他安装路径：

```bash
# Docker
docker pull ghcr.io/hmbown/codewhale:latest

# Nix
nix run github:Hmbown/CodeWhale

# Windows
scoop install codewhale        # 或使用 GitHub Releases 中的 NSIS 安装包

# CNB 镜像：适合无法稳定访问 GitHub 的用户
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.61 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.61 codewhale-tui --locked --force

# 旧 Homebrew 兼容路径：formula 改名期间仍沿用 deepseek-tui
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

各平台的预编译归档——包括 Linux riscv64——都附在
[GitHub Releases](https://github.com/Hmbown/CodeWhale/releases)。校验和、
中国大陆镜像、Windows 细节与故障排查见 [docs/INSTALL.md](docs/INSTALL.md)。

## 第一次运行

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

每个 provider 都是同样的一行写法：`--provider openrouter`、
`--provider moonshot`，或者把 `vllm`、`sglang`、`ollama` 指向你自己的
localhost 运行时——完全不需要 key。手里是 Claude 的 key？运行
`codewhale auth set --provider anthropic`，或直接导出 `ANTHROPIC_API_KEY`，
原生 Messages 适配器会接管后续。

密钥保存在 `~/.codewhale/config.toml`；旧的 `~/.deepseek/` 配置仍会被读取，
保持兼容。

常用会话内命令：

- `/provider` 与 `/model` 在会话中途切换路由和模型。
- `/restore` 从 side-git 快照回滚之前的某一轮。
- `/skills` 从 `~/.codewhale/skills/` 加载可复用的工作流。
- `/config` 编辑运行时设置；`/statusline` 显示当前路由、费用和会话状态。
- `! cargo test -p codewhale-tui` 让任意 shell 命令走正常的审批与沙箱路径。

无头模式，用于脚本和 CI：

```bash
codewhale exec --allowed-tools read_file,exec_shell --max-turns 10 "fix the failing test"
```

## 已交付的能力

一个终端原生的 Agent 运行框架——TUI + CLI、16 个 Rust crate——安全护栏是
运行时机制，而不是要靠模型自己记住的提醒：

- **审批门控工具 + 操作系统级沙箱。** 文件、Shell、Git、Web、MCP 和子 Agent
  工具都运行在显式审批门和沙箱后端（bwrap、Landlock、Seatbelt、seccomp）之后。
- **值得信任的回滚。** side-git 快照与 `/restore`，存放在仓库 `.git` 之外——
  撤销一轮永远不会动你的提交历史。
- **Hooks v2** *(0.8.58)*。`tool_call_before` 钩子返回 JSON 形式的
  `allow`/`deny`/`ask` 决策，deny 优先，支持 glob 匹配器和项目级
  `.codewhale/hooks.toml`。
- **并发子 Agent + 按 provider 感知的路由** *(0.8.58)*。并行调查与实现，
  大模型/轻量模型分档按 provider 解析——不写死任何模型 id。
- **持久会话。** fork、relay 交接，以及跨会话的磁盘级 prompt 缓存，在
  Plan/Agent/YOLO 模式切换之间保持字节级稳定 *(0.8.56)*。轮次可挺过系统休眠
  *(0.8.57)*：流式途中休眠，唤醒后请求会被静默重发，而不是整轮失败。
- **无头模式。** `codewhale exec` 支持 `--allowed-tools`、
  `--disallowed-tools`（deny 优先）、`--max-turns` 和
  `--append-system-prompt` *(0.8.58)*，面向脚本和 CI。
- **随处可嵌入。** HTTP/SSE 与 ACP 运行时 API、VS Code 扩展（Phase 0），
  以及 Telegram/飞书桥接。
- **日常主力的打磨。** MCP 客户端*和*服务器、可复用 skills、7 种语言本地化
  （0.8.56 起覆盖审批对话框），以及基于小米 MiMo 的语音/TTS。

### 任意模型，开放模型优先

25 个 provider 共用同一套运行框架、同一部宪法、同一组工具：

- **开放模型，托管服务：** `deepseek`（同侪之首）、`openrouter`、
  `huggingface`（Inference Providers）、`deepinfra`、`moonshot`（Kimi）、`volcengine`
  （火山方舟）、`nvidia-nim`、`together`、`fireworks`、`novita`、
  `siliconflow` / `siliconflow-CN`、`arcee`、`xiaomi-mimo`、`atlascloud`、
  `wanjie-ark`，外加一条通用的 `openai` 兼容路由，可接任意网关。
- **开放模型，自托管：** `vllm`、`sglang`、`ollama` 直连你自己的 localhost
  端点——无需任何 key。
- **闭源 provider，原生直连：** `anthropic` 走专用的 `/v1/messages` 适配器
  *(0.8.58)*，支持自适应思考、prompt-cache 断点和签名思考重放——不是
  OpenAI 方言的转译垫片；还有 `openai-codex`，复用已有的 ChatGPT/Codex CLI
  登录。

路由不只是换个 base URL：`/reasoning` 努力档位会翻译成各 provider 的协议方言，
子 Agent 分档按 provider 解析，系统提示中的模型事实也按模型模板化而非写死
*(0.8.58)*。会话中途用 `/provider` 和 `/model` 即可切换。完整注册表——凭据、
base URL、能力边界——见 [docs/PROVIDERS.md](docs/PROVIDERS.md)。

上面的版本标注对应最近三个版本（0.8.56 → 0.8.58）落地的内容。完整细节见
[CHANGELOG.md](CHANGELOG.md)。

## 核心想法

多数编程 Agent 从加码开始：更多工具、更长上下文、更多自主性。CodeWhale
从落实责任开始。

一个会改你仓库的 Agent 应该有一个地址——这个终端、这个用户、这个分支、
这个会话。不是人格面具，而是一个回信地址。出了问题，“是模型干的”不是答案；
“这个实例，在这个会话里，经过这次审批”才是。

接下来它需要法律。真实的工作会话是一摞冲突：你当前的请求、仓库的指令、
新鲜的 shell 输出、过期的记忆、上一个 Agent 的交接，全都挤在同一轮里。
**CodeWhale Constitution** 把权威次序固定下来：

1. **用户意图至上。** 你当前的请求高于过期的仓库指引、记忆、先前的交接和
   人格叠加层。
2. **仓库法律必须显式。** 添加 `.codewhale/constitution.json`，声明项目的
   持久权威：受保护的不变量、分支策略、验证规则。
3. **证据高于叙述。** 工具输出胜过自信的猜测。`cargo test` 失败就如实报告
   `cargo test` 失败，绝不被总结成乐观措辞。验证是任务的一部分，不是尾声。
4. **记忆排在最后。** 有用，但永不具备权威。

真正起作用的策略由代码强制执行，而非靠提示词：审批门、沙箱、快照、回滚和
工具 schema 都是模型无法靠话术绕过的运行时机制。

而这些法律没有一条住在模型里——这正是模型可以随时更换的原因。运行框架承载
宪法；模型提供推理。DeepSeek 和开放权重世界是一等公民，你局域网里那台跑着
vLLM 或 Ollama 的机器是完全平等的一员；当你手里是 Claude 或 OpenAI 的 key
时，CodeWhale 同样以原生协议对话。

这就是产品本身：不是更大的模型，而是围绕你所选模型的一套更严格的运行框架。
换掉模型，法律不变。

## 更多文档

README 承载理念和最快路径，细节放在文档和 [codewhale.net](https://codewhale.net/)：

- [用户指南](docs/GUIDE.md) —— 上手 CodeWhale 的第一个小时。
- [安装指南](docs/INSTALL.md) —— 所有安装路径与故障排查。
- [配置](docs/CONFIGURATION.md) —— 配置文件、仓库 constitution 和 provider 设置。
- [Provider 注册表](docs/PROVIDERS.md) —— 模型路由、凭据、base URL 与能力边界。
- [子 Agent](docs/SUBAGENTS.md) —— 角色、生命周期、输出契约与恢复行为。
- [MCP](docs/MCP.md) —— 接入外部工具服务器，或让 CodeWhale 自己作为 MCP 服务器运行。
- [Runtime API](docs/RUNTIME_API.md) —— HTTP/SSE、ACP、移动端及 GUI/编辑器集成契约。
- [Model Lab](docs/MODEL_LAB.md) —— 开放模型发现与评测路线图。
- [架构](docs/ARCHITECTURE.md) —— crate 布局、运行时流程、工具系统、扩展点与安全模型。

## v0.9.0 轨道

v0.9.0 是当前的集成轨道，正在那里汇聚的工作包括：

- 更强的跨会话、跨 Agent 的 relay 与交接界面；
- 高密度工具运行时更安静的转录；
- 面向 VS Code 与 GUI 客户端的运行时 API；
- WhaleFlow 分支/叶子工作流编排。

逐版本的细节见 [CHANGELOG.md](CHANGELOG.md)。

## 致谢

- **[DeepSeek](https://github.com/deepseek-ai)** — 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — 感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — 感谢 OpenWarp 优先支持 codewhale，并一起打磨更好的终端智能体体验。
- **[Open Design](https://github.com/nexu-io/open-design)** — 感谢 Open Design 围绕设计导向的智能体工作流给予的支持与协作。

本项目由不断壮大的贡献者社区共同打造。维护者的原则很简单：报告和 PR 都是
真实的项目工作——即使最终补丁需要收窄、延后，或被吸收进维护者分支，也是如此。

完整且持续更新的逐位贡献者名单，请见
[英文 README 的 Thanks 章节](README.md#thanks)——那里是权威的致谢记录。

---

## 贡献

参见 [CONTRIBUTING.md](CONTRIBUTING.md)。欢迎提交 Pull Request——可以先从
[开放 issue](https://github.com/Hmbown/CodeWhale/issues) 中寻找适合入门的任务。

CodeWhale 收到很多高质量的报告和 PR。维护者的姿态是把这扇门一直敞开，
同时守住发布质量：

- Issue 应保持人类可读、可执行。除非维护者主动启用强制模式，接收自动化只起建议作用。
- 评审 PR 看代码、测试、关联 issue 和运行时行为，而不只看标题。
- 如果某个 PR 范围太大无法直接合并，维护者可能把安全的部分吸收进更窄的分支，
  然后为作者署名并说明实际落地了什么。
- Co-author 署名应使用 `.github/AUTHOR_MAP` 中可映射的 GitHub noreply 身份；
  报告者和复现作者应在 changelog、release notes 和关闭评论中得到致谢。
- 经常性贡献者可加入 `.github/APPROVED_CONTRIBUTORS`，让 dry-run 门禁不再打扰他们。

支持项目：[Buy me a coffee](https://www.buymeacoffee.com/hmbown)。

> [!NOTE]
> *本项目与 DeepSeek Inc. 无隶属关系。*

## 许可证

[MIT](LICENSE)

## Star 历史

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
