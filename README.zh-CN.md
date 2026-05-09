# DeepSeek TUI

> **面向 [DeepSeek V4](https://platform.deepseek.com) 的终端原生编程智能体：100 万 token 上下文、思考模式流式推理、前缀缓存感知。自包含 Rust 二进制发布——开箱即带 MCP 客户端、沙箱和持久化任务队列。**

[English README](README.md)

> ⚠️ **安全警告**：发现仿冒仓库 `DeepSeek-TUI/DeepSeek-TUI` 冒充本项目，其发布资产含蠕虫病毒
> （VirusTotal 已确认）。**请仅从官方仓库下载：[Hmbown/DeepSeek-TUI](https://github.com/Hmbown/DeepSeek-TUI)。**
> 详见 [#1286](https://github.com/Hmbown/DeepSeek-TUI/issues/1286)。

## 安装

`deepseek` 是自包含 Rust 二进制——**运行时不依赖 Node.js 或 Python**。
下面几种方式装出来的是同一套二进制，按你已有的工具链选一个即可：

```bash
# 1. npm —— 已装 Node 的最方便方式。npm 包只是一个下载器，
#    会从 GitHub Releases 拉取对应平台的预编译二进制，
#    并不会让 deepseek 本身依赖 Node 运行时。
npm install -g deepseek-tui

# 2. Cargo —— 无需 Node。
cargo install deepseek-tui-cli --locked   # `deepseek` 入口
cargo install deepseek-tui     --locked   # `deepseek-tui` TUI 二进制

# 3. Homebrew —— macOS 包管理器。
brew tap Hmbown/deepseek-tui
brew install deepseek-tui

# 4. 直接下载 —— 无需任何工具链。
#    https://github.com/Hmbown/DeepSeek-TUI/releases
#    覆盖 Linux x64/ARM64、macOS x64/ARM64、Windows x64

# 5. Docker —— 预构建发布镜像。
docker run --rm -it \
  -e DEEPSEEK_API_KEY \
  -v "$PWD:/workspace" \
  ghcr.io/hmbown/deepseek-tui:latest
```

> 中国大陆访问较慢时，npm 可加 `--registry=https://registry.npmmirror.com`，
> 或使用下方的 [Cargo 镜像](#中国大陆--镜像友好安装)。

[![CI](https://github.com/Hmbown/DeepSeek-TUI/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/DeepSeek-TUI/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/deepseek-tui)](https://www.npmjs.com/package/deepseek-tui)
[![crates.io](https://img.shields.io/crates/v/deepseek-tui-cli?label=crates.io)](https://crates.io/crates/deepseek-tui-cli)

![DeepSeek TUI 截图](assets/screenshot.png)

---

## 这是什么？

DeepSeek TUI 是一个完全运行在终端里的编程智能体。它让 DeepSeek 前沿模型直接访问你的工作区：读写文件、运行 shell 命令、搜索浏览网页、管理 git、调度子智能体——全部通过快速、键盘驱动的 TUI 完成。

它面向 **DeepSeek V4**（`deepseek-v4-pro` / `deepseek-v4-flash`）构建，原生支持 100 万 token 上下文窗口和思考模式流式输出。

### 主要功能

- **原生 RLM**（`rlm_query`）—— 利用现有 API 客户端并行调度 1-16 个低成本 `deepseek-v4-flash` 子任务，用于批量分析和并行推理
- **思考模式流式输出** —— 实时观察模型在解决问题时的思维链展开
- **完整工具集** —— 文件操作、shell 执行、git、网页搜索/浏览、apply-patch、子智能体、MCP 服务器
- **100 万 token 上下文** —— 上下文接近上限时自动智能压缩，支持前缀缓存感知以降低成本
- **三种交互模式** —— Plan（只读探索）、Agent（带审批的默认交互）、YOLO（可信工作区自动批准）
- **推理强度档位** —— 用 `Shift+Tab` 在 `off → high → max` 之间切换
- **会话保存和恢复** —— 长任务的断点续作
- **工作区回滚** —— 通过 side-git 记录每轮前后快照，支持 `/restore` 和 `revert_turn`，不影响项目自己的 `.git`
- **持久化任务队列** —— 后台任务在重启后仍然存在，支持计划任务和长时间运行的操作
- **HTTP/SSE 运行时 API** —— `deepseek serve --http` 用于无界面智能体流程
- **MCP 协议** —— 连接 Model Context Protocol 服务器扩展工具，见 [docs/MCP.md](docs/MCP.md)
- **LSP 诊断** —— 每次编辑后通过 rust-analyzer、pyright、typescript-language-server、gopls、clangd 提供内联错误/警告
- **用户记忆** —— 可选的持久化笔记文件注入系统提示，实现跨会话偏好保持
- **多语言 UI** —— 支持 `en`、`ja`、`zh-Hans`、`pt-BR`，支持自动检测
- **实时成本跟踪** —— 按轮次和会话统计 token 用量与成本估算，含缓存命中/未命中明细
- **技能系统** —— 可通过 GitHub 安装的组合式指令包，无需后端服务

---

## 架构说明

`deepseek`（调度器 CLI）→ `deepseek-tui`（伴随二进制）→ ratatui 界面 ↔ 异步引擎 ↔ OpenAI 兼容流式客户端。工具调用通过类型化注册表（shell、文件操作、git、web、子智能体、MCP、RLM）路由，结果流式返回对话记录。引擎管理会话状态、轮次追踪、持久化任务队列和 LSP 子系统——它在下一步推理前将编辑后诊断反馈到模型上下文中。

详见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

---

## 快速开始

```bash
npm install -g deepseek-tui
deepseek --version
deepseek
```

预构建二进制覆盖 **Linux x64**、**Linux ARM64**（v0.8.8 起）、**macOS x64**、**macOS ARM64** 和 **Windows x64**。其他目标平台（musl、riscv64、FreeBSD 等）请见下方的[从源码安装](#从源码安装)或 [docs/INSTALL.md](docs/INSTALL.md)。

首次启动时会提示输入 [DeepSeek API key](https://platform.deepseek.com/api_keys)。密钥保存到 `~/.deepseek/config.toml`，在任意目录、IDE 终端和脚本中都能使用，不会触发系统密钥环弹窗。

也可以提前配置：

```bash
deepseek auth set --provider deepseek   # 保存到 ~/.deepseek/config.toml

export DEEPSEEK_API_KEY="YOUR_KEY"      # 环境变量方式；需要在非交互式 shell 中使用请放入 ~/.zshenv
deepseek

deepseek doctor                          # 验证安装
```

> 轮换或移除密钥：`deepseek auth clear --provider deepseek`。

### Linux ARM64（HarmonyOS 轻薄本、openEuler、Kylin、树莓派、Graviton 等）

从 v0.8.8 起，`npm i -g deepseek-tui` 直接支持 glibc 系的 ARM64 Linux。你也可以从 [Releases 页面](https://github.com/Hmbown/DeepSeek-TUI/releases) 下载预编译二进制，放到 `PATH` 目录中。

### 中国大陆 / 镜像友好安装

如果在中国大陆访问 GitHub 或 npm 下载较慢，可以通过 Cargo 注册表镜像安装：

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

然后安装两个二进制（调度器在运行时会调用 TUI）：

```bash
cargo install deepseek-tui-cli --locked   # 提供推荐入口 `deepseek`
cargo install deepseek-tui     --locked   # 提供交互式 TUI 伴随二进制
deepseek --version
```

也可以直接从 [GitHub Releases](https://github.com/Hmbown/DeepSeek-TUI/releases) 下载预编译二进制。`DEEPSEEK_TUI_RELEASE_BASE_URL` 可用于镜像后的 release 资产。

### Windows (Scoop)

[Scoop](https://scoop.sh) 是一个 Windows 软件包管理器。DeepSeek TUI 已进入
Scoop main bucket，但该 manifest 独立更新，可能滞后于 GitHub/npm/Cargo
release。先运行 `scoop update`，安装后用 `deepseek --version` 核对版本：

```bash
scoop update
scoop install deepseek-tui
deepseek --version
```

如果需要最新版本，请优先使用 npm 或直接下载 GitHub Release 资产。


<details id="install-from-source">
<summary>从源码安装</summary>

适用于任何 Tier-1 Rust 目标，包括 musl、riscv64、FreeBSD 以及尚无预编译包的 ARM64 发行版。

```bash
# Linux 构建依赖（Debian/Ubuntu/RHEL）：
#   sudo apt-get install -y build-essential pkg-config libdbus-1-dev
#   sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel

git clone https://github.com/Hmbown/DeepSeek-TUI.git
cd DeepSeek-TUI

cargo install --path crates/cli --locked   # 需要 Rust 1.88+；提供 `deepseek`
cargo install --path crates/tui --locked   # 提供 `deepseek-tui`
```

两个二进制都需要安装。交叉编译和平台特定说明见 [docs/INSTALL.md](docs/INSTALL.md)。

</details>

### 其他模型提供方

```bash
# NVIDIA NIM
deepseek auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
deepseek --provider nvidia-nim

# Fireworks
deepseek auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"
deepseek --provider fireworks --model deepseek-v4-pro

# 自托管 SGLang
SGLANG_BASE_URL="http://localhost:30000/v1" deepseek --provider sglang --model deepseek-v4-flash

# 自托管 vLLM
VLLM_BASE_URL="http://localhost:8000/v1" deepseek --provider vllm --model deepseek-v4-flash

# 自托管 Ollama
ollama pull deepseek-coder:1.3b
deepseek --provider ollama --model deepseek-coder:1.3b
```

---

## v0.8.29 新功能

维护版本，核心是修复 v0.8.27 / v0.8.28 引入的"滚动幽灵"回归
（#1085 类问题）和 Ctrl+R 会话恢复跨项目泄漏的问题（#1395），
外加 25 个社区 PR。[完整更新日志](CHANGELOG.md)。

- **"滚动幽灵"彻底修复**（#1085 回归）。并行子代理运行
  `exec_shell` 时，alt-screen 会被滚动出 ratatui 差分渲染器的
  视野，header 上方出现越来越大的空白带。三层防护一并上线：
  写入 `~/.deepseek/logs/tui-YYYY-MM-DD.log` 的 `tracing-subscriber`、
  alt-screen 生命周期内的 fd 级 stderr 重定向（Unix `dup2`）、
  以及 `tools/`、`core/`、`tui/`、`network_policy.rs`、
  `runtime_threads.rs` 模块的
  `#![deny(clippy::print_stdout, clippy::print_stderr)]`。今后在
  这些模块新增 `eprintln!` 会被 CI 拒绝。
- **Ctrl+R 会话恢复改为按当前工作区过滤**（#1395，PR #1397，
  来自 **@linzhiqin2003**）— 此前列出磁盘上所有会话，导致
  在项目 B 打开 DeepSeek-TUI 时按下 Ctrl+R 可能恢复项目 A 的
  历史记录。
- **运行时版本号直接显示在 header 中。** Header 右侧集群在
  provider / effort / Live / context 之后增加一个 `v0.8.29`
  小标签，在终端宽度紧张时最先收起。
- **MCP HTTP 传输现在尊重 HTTP(S)_PROXY**（#1408，来自
  **@hlx98007**）— 公司出口代理、国内 Clash / Shadowsocks 代理
  现在能正确应用于 MCP HTTP 连接，跟 box 上的其他工具
  （curl、npm、git 等）保持一致。同时支持 `NO_PROXY`。
- **MCP 发现接受不规范条目**（PR #1410，来自 **@Liu-Vince**）—
  一个错误的 tool / resource / prompt 条目不再让整页丢失；
  错误条目被跳过，目录的其余部分正常返回。
- **MCP SSE 接受 CRLF 分隔的 endpoint 事件**（#1309，PR #1358，
  来自 **@reidliu41**）— FastMCP / uvicorn 风格的 SSE 流不再因
  只等待 LF 分隔符而超时。
- **输入框会忽略泄漏的鼠标报告字节**（#1418，PR #1421，来自
  **@reidliu41**）— 某些 SSH / IDE 终端链路把 `[<35;44;18M`
  这类鼠标报告泄漏到 stdin 时，不再把输入区域填满。
- **Footer 芯片会遵守可用宽度**（#1357，PR #1417，来自
  **@Wenjunyun123**）— 窄终端下，过长的 cache / aux 芯片会先
  收起，而不是挤压左侧状态或 composer 区域。
- **笔记管理斜杠命令**（PR #1407，来自 **@reidliu41**）—
  `/note add`、`/note list` 等命令在 TUI 内提供持久笔记功能。
- **全局 `~/.deepseek/AGENTS.md` 与项目 AGENTS.md 合并**
  （#1157，PR #1399，来自 **@linzhiqin2003**）— 此前工作区
  自带 AGENTS.md 会完全遮蔽全局基准，现在分层叠加。
- **语言指令：thinking 跟随用户消息语言**（#1118，PR #1398，
  来自 **@linzhiqin2003**）— 此前项目上下文推断的 `lang`
  字段可能压制最新用户消息的语言，导致中文对话出现英文 thinking。
- **网络搜索过滤垃圾 SERP**（#964，PR #1396，来自
  **@linzhiqin2003**）— Bing / DDG 回退路径丢弃污染快速查找
  结果的 SEO 农场域名。
- **Auto 路由识别 CJK 调试 / 搜索关键词**（PR #1401、#1402，
  来自 **@linzhiqin2003**）— `--model auto` 和推理强度选择器
  现在能正确路由中文 / 日文技术查询，此前会回退到通用基准。
- **Deferred tools 首次执行前会先加载 schema**（#1419，PR #1429，
  来自 **@SamhandsomeLee**）— `edit_file` 等延迟加载工具现在会先
  展示期望字段并要求模型重试，而不是执行模型猜测出来的参数名。
- **DeepSeek 公开别名会正确回放 thinking-mode 工具轮次**（PR #1428，
  来自 **@Beltran12138**）— `deepseek-chat` 和
  `deepseek-reasoner` 现在与显式 V4 模型 ID 一样触发
  `reasoning_content` replay，避免工具调用后的第二轮 400。
- **技能补全收敛到 `/skill` 下**（#1437，PR #1442，来自
  **@reidliu41**）— 本地技能很多时不会再挤满根级 `/` 命令菜单。
- **`edit_file` 拒绝无变化替换**（PR #1460，来自
  **@xiluoduyu**）— `search` / `replace` 完全相同时会直接返回
  清晰的参数错误，而不是生成空 diff。
- **Windows 终端布局使用宽度稳定的字形**（#1314，PR #1465，来自
  **@CrepuscularIRIS**）— header 和文件树不再依赖 cmd /
  PowerShell 容易误判宽度的 SMP emoji。
- **Ghostty 默认启用低动态渲染**（#1445，PR #1468，来自
  **@CrepuscularIRIS**）— 受影响终端无需手动配置即可避开动画闪烁。
- **Docker buildx provenance 的 EPERM 失败会给出提示**（#1449，
  PR #1469，来自 **@CrepuscularIRIS**）— macOS shell 输出命中
  受限 metadata 写入失败时，会提示 provenance 相关开关。
- **Windows CMD 的鼠标滚轮回退会滚动 transcript**（#1443，
  PR #1471，来自 **@CrepuscularIRIS**）— 关闭 mouse capture 时，
  被终端映射成 Up / Down 的滚轮事件不再循环 composer 历史。
- **`sync-cnb.yml` 工作流加固** — 显式 `permissions: contents:
  read`、`actions/checkout` v3 → v4、触发器收紧到 `main` +
  `v*` 标签（不再镜像 feature 分支）。
- **新增 +438 LOC 测试覆盖** — `error_taxonomy`、
  `parse_pages_arg`、Web 搜索优先级、`sanitize_stream_chunk`
  控制字节过滤（PR #1403–#1406，来自 **@linzhiqin2003**）。

感谢本周期落地 10 个 PR 的 **@linzhiqin2003**、落地 5 个 PR 的
**@reidliu41**、落地 4 个 PR 的 **@CrepuscularIRIS**，以及
**@SamhandsomeLee**、**@Beltran12138**、**@Wenjunyun123**、
**@hlx98007**、**@Liu-Vince**、**@xiluoduyu**，和报告 #1395 的
**@shenxiaodaosanhua**。

---

## 使用方式

```bash
deepseek                                       # 交互式 TUI
deepseek "explain this function"              # 一次性提示
deepseek --model deepseek-v4-flash "summarize" # 指定模型
deepseek --yolo                                # 自动批准工具
deepseek auth set --provider deepseek         # 保存 API key
deepseek doctor                                # 检查配置和连接
deepseek doctor --json                         # 机器可读诊断
deepseek setup --status                        # 只读安装状态
deepseek setup --tools --plugins               # 创建本地工具和插件目录
deepseek models                                # 列出可用 API 模型
deepseek sessions                              # 列出已保存会话
deepseek resume --last                         # 恢复最近会话
deepseek resume <SESSION_ID>                   # 按 UUID 恢复指定会话
deepseek fork <SESSION_ID>                     # 在指定轮次分叉会话
deepseek serve --http                          # HTTP/SSE API 服务
deepseek run pr <N>                            # 获取 PR 并预填审查提示
deepseek mcp list                              # 列出已配置 MCP 服务器
deepseek mcp validate                          # 校验 MCP 配置和连接
deepseek mcp-server                            # 启动 dispatcher MCP stdio 服务器
deepseek update                                # 检查并应用二进制更新
```

### 常用快捷键

| 按键 | 功能 |
|---|---|
| `Tab` | 补全 `/` 或 `@`；运行中则把草稿排队；否则切换模式 |
| `Shift+Tab` | 切换推理强度：off → high → max |
| `F1` | 可搜索帮助面板 |
| `Esc` | 返回 / 关闭 |
| `Ctrl+K` | 命令面板 |
| `Ctrl+R` | 恢复旧会话 |
| `Alt+R` | 搜索提示历史和恢复草稿 |
| `Ctrl+S` | 暂存当前草稿（`/stash list`、`/stash pop` 恢复） |
| `@path` | 在输入框中附加文件或目录上下文 |
| `↑`（在输入框开头） | 选择附件行进行移除 |

完整快捷键目录：[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md)。

---

## 模式

| 模式 | 行为 |
|---|---|
| **Plan** 🔍 | 只读调查；模型先探索并提出计划（`update_plan` + `checklist_write`），然后再做更改 |
| **Agent** 🤖 | 默认交互模式；多步工具调用带审批门禁 |
| **YOLO** ⚡ | 在可信工作区自动批准工具；仍会维护计划和清单以保持可见性 |

---

## 配置

用户配置：`~/.deepseek/config.toml`。项目覆盖：`<workspace>/.deepseek/config.toml`（以下密钥被拒绝：`api_key`、`base_url`、`provider`、`mcp_config_path`）。完整选项见 [config.example.toml](config.example.toml)。

常用环境变量：

| 变量 | 用途 |
|---|---|
| `DEEPSEEK_API_KEY` | DeepSeek API key |
| `DEEPSEEK_BASE_URL` | API base URL |
| `DEEPSEEK_MODEL` | 默认模型 |
| `DEEPSEEK_PROVIDER` | `deepseek`（默认）、`nvidia-nim`、`fireworks`、`sglang`、`vllm`、`ollama` |
| `DEEPSEEK_PROFILE` | 配置 profile 名称 |
| `DEEPSEEK_MEMORY` | 设为 `on` 启用用户记忆 |
| `NVIDIA_API_KEY` / `FIREWORKS_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` | 提供商认证 |
| `SGLANG_BASE_URL` | 自托管 SGLang 端点 |
| `VLLM_BASE_URL` | 自托管 vLLM 端点 |
| `OLLAMA_BASE_URL` | 自托管 Ollama 端点 |
| `OLLAMA_MODEL` | 自托管 Ollama 模型标签 |
| `NO_ANIMATIONS=1` | 启动时强制无障碍模式 |
| `SSL_CERT_FILE` | 企业代理的自定义 CA 包 |

`locale` 会控制界面语言，并作为模型自然语言的兜底设置；最新用户消息的语言优先级更高。也就是说，即使系统 locale 是英文，用户用中文提问时，V4 的 `reasoning_content` 和最终回复也应该使用中文。可在 `config.toml` 中设置 `locale`、使用 `/config locale zh-Hans`、或依赖 `LC_ALL`/`LANG`。详见 [docs/LOCALIZATION.md](docs/LOCALIZATION.md) 和 [docs/CONFIGURATION.md](docs/CONFIGURATION.md)。

### 切换为中文界面

如果界面是其他语言，可以在 TUI 内一键切换为简体中文：

1. 在 Composer 里输入 `/config`，按 Tab 或 Enter 打开配置面板。
2. 选择 **Edit locale**，在 `New:` 字段输入 `zh-Hans`，按 Enter 应用。

可选语言：`auto` | `en` | `ja` | `zh-Hans` | `pt-BR`。

也可以在 `~/.deepseek/config.toml` 里直接设置 `locale = "zh-Hans"`，或通过 `LC_ALL` / `LANG` 环境变量自动选择：

```toml
# ~/.deepseek/config.toml
[tui]
locale = "zh-Hans"
```

或者通过环境变量（中文系统通常已自动生效）：

```bash
LANG=zh_CN.UTF-8 deepseek run
```

---

## 模型和价格

| 模型 | 上下文 | 输入（缓存命中） | 输入（缓存未命中） | 输出 |
|---|---|---|---|---|
| `deepseek-v4-pro` | 1M | $0.003625 / 1M* | $0.435 / 1M* | $0.87 / 1M* |
| `deepseek-v4-flash` | 1M | $0.0028 / 1M | $0.14 / 1M | $0.28 / 1M |

旧别名 `deepseek-chat` / `deepseek-reasoner` 映射到 `deepseek-v4-flash`。NVIDIA NIM 变体使用你的 NVIDIA 账号条款。

*DeepSeek Pro 价格是限时 75% 折扣，有效期到 2026-05-31 15:59 UTC；该时间之后 TUI 成本估算会回退到 Pro 基础价格。*

> [!Note]
> 关于 DeepSeek-V4-Pro 的最新定价信息，请参阅官方 [DeepSeek 定价页面](https://api-docs.deepseek.com/zh-cn/quick_start/pricing)，请注意目前可享受 75% 的折扣，该优惠有效期至 **2026 年 5 月 31 日 23:59（北京时间）**。此外，README 文档中所列出的所有价格，均与官方发布的数值保持一致。

---

## 创建和安装技能

DeepSeek TUI 从工作区目录（`.agents/skills` → `skills` → `.opencode/skills` → `.claude/skills`）和全局 `~/.deepseek/skills` 发现技能。每个技能是一个包含 `SKILL.md` 的目录：

```text
~/.deepseek/skills/my-skill/
└── SKILL.md
```

需要 YAML frontmatter：

```markdown
---
name: my-skill
description: 当 DeepSeek 需要遵循我的自定义工作流时使用这个技能。
---

# My Skill
这里写给智能体的指令。
```

常用命令：`/skills`（列出）、`/skill <name>`（激活）、`/skill new`（创建）、`/skill install github:<owner>/<repo>`（社区）、`/skill update` / `uninstall` / `trust`。社区技能直接从 GitHub 安装，无需后端服务。已安装技能在模型可见的会话上下文里列出；当任务匹配技能描述时，智能体可通过 `load_skill` 工具自动读取对应的 `SKILL.md`。

---

## 文档

| 文档 | 主题 |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | 代码库内部结构 |
| [CONFIGURATION.md](docs/CONFIGURATION.md) | 完整配置参考 |
| [MODES.md](docs/MODES.md) | Plan / Agent / YOLO 模式 |
| [MCP.md](docs/MCP.md) | Model Context Protocol 集成 |
| [RUNTIME_API.md](docs/RUNTIME_API.md) | HTTP/SSE API 服务 |
| [INSTALL.md](docs/INSTALL.md) | 各平台安装指南 |
| [MEMORY.md](docs/MEMORY.md) | 用户记忆功能指南 |
| [SUBAGENTS.md](docs/SUBAGENTS.md) | 子智能体角色分类与生命周期 |
| [KEYBINDINGS.md](docs/KEYBINDINGS.md) | 完整快捷键目录 |
| [RELEASE_RUNBOOK.md](docs/RELEASE_RUNBOOK.md) | 发布流程 |
| [LOCALIZATION.md](docs/LOCALIZATION.md) | UI 语言矩阵与切换 |
| [OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) | 运维和恢复 |

完整更新历史：[CHANGELOG.md](CHANGELOG.md)。

---

## 致谢

本项目由不断壮大的贡献者社区共同打造：

- **[merchloubna70-dot](https://github.com/merchloubna70-dot)** — 28 个 PR，涵盖功能、修复和 VS Code 扩展基础架构 (#645–#681)
- **[WyxBUPT-22](https://github.com/WyxBUPT-22)** — Markdown 表格、粗体/斜体和水平线渲染 (#579)
- **[loongmiaow-pixel](https://github.com/loongmiaow-pixel)** — Windows + 中国安装文档 (#578)
- **[20bytes](https://github.com/20bytes)** — 用户记忆文档和帮助优化 (#569)
- **[staryxchen](https://github.com/staryxchen)** — glibc 兼容性预检 (#556)
- **[Vishnu1837](https://github.com/Vishnu1837)** — glibc 兼容性改进 (#565)
- **[shentoumengxin](https://github.com/shentoumengxin)** — Shell `cwd` 边界验证 (#524)
- **[toi500](https://github.com/toi500)** — Windows 粘贴修复报告
- **[xsstomy](https://github.com/xsstomy)** — 终端启动重绘报告
- **[melody0709](https://github.com/melody0709)** — 斜杠前缀回车激活报告
- **[lloydzhou](https://github.com/lloydzhou)** 和 **[jeoor](https://github.com/jeoor)** — 压缩成本报告；lloydzhou 还贡献了确定性的环境上下文注入 (#813, #922) 和 KV 前缀缓存稳定化 (#1080)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README 清晰化改进 (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows 安装文档 (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — 价格/折扣信息更新 (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — 模型 ID 大小写兼容性报告 (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — `working...` 卡死状态 Bug 报告和 Windows 剪贴板兜底修复 (#738, #850)
- **[reidliu41](https://github.com/reidliu41)** — 退出后的恢复提示、工作区信任持久化、Ollama provider 支持，以及思考块流式终结修复 (#863, #870, #921, #1078)
- **[xieshutao](https://github.com/xieshutao)** — 纯 Markdown skill 兜底解析 (#869)
- **[GK012](https://github.com/GK012)** — npm wrapper 的 `--version` 兜底 (#885)
- **[y0sif](https://github.com/y0sif)** — 直接子智能体完成后唤醒父级 turn loop (#901)
- **[mac119](https://github.com/mac119)** 和 **[leo119](https://github.com/leo119)** — `deepseek update` 命令文档 (#838, #917)
- **[dumbjack](https://github.com/dumbjack)** / **浩淼的mac** — shell 命令空字节安全加固 (#706, #918)
- **macworkers** — fork 完成后显示新 session id (#600, #919)
- **zero** 和 **[zerx-lab](https://github.com/zerx-lab)** — 通知条件配置和更完整的 OSC 9 通知正文 (#820, #920)
- **[chnjames](https://github.com/chnjames)** — @mention 补全缓存、配置恢复优化，以及 Windows UTF-8 shell 输出修复 (#849, #927, #982, #1018)
- **[angziii](https://github.com/angziii)** — 配置安全、异步清理、Docker 加固和命令安全修复 (#822, #824, #827, #831, #833, #835, #837)
- **[elowen53](https://github.com/elowen53)** — UTF-8 解码和确定性测试覆盖 (#825, #840)
- **[wdw8276](https://github.com/wdw8276)** — 用于自定义 session 标题的 `/rename` 命令 (#836)
- **[banqii](https://github.com/banqii)** — `.cursor/skills` 发现路径支持 (#817)
- **[junskyeed](https://github.com/junskyeed)** — API 请求动态 `max_tokens` 计算 (#826)
- **Hafeez Pizofreude** — `fetch_url` 的 SSRF 保护和 Star History 图表
- **Unic (YuniqueUnic)** — 基于 schema 的配置 UI（TUI + web）
- **Jason** — SSRF 安全加固
- **[axobase001](https://github.com/axobase001)** — 快照孤儿文件清理、npm 安装守卫、会话遥测修复、模型作用域缓存清理、符号链接技能支持，以及 npm 镜像逃生路径指引 (#975, #1032, #1047, #1049, #1052, #1019, #1051, #1056)
- **[MengZ-super](https://github.com/MengZ-super)** — `/theme` 深色/浅色主题切换命令和 SSE gzip/brotli 解压支持 (#1057, #1061)
- **[DI-HUO-MING-YI](https://github.com/DI-HUO-MING-YI)** — Plan 模式只读沙箱安全修复 (#1077)
- **[bevis-wong](https://github.com/bevis-wong)** — 粘贴-回车自动提交问题的精确复现 (#1073)
- **[Duducoco](https://github.com/Duducoco)** 和 **[AlphaGogoo](https://github.com/AlphaGogoo)** — 技能斜杠菜单和 `/skills` 覆盖范围修复 (#1068, #1083)
- **[ArronAI007](https://github.com/ArronAI007)** — macOS Terminal.app 和 ConHost 窗口大小调整残留修复 (#993)
- **[THINKER-ONLY](https://github.com/THINKER-ONLY)** — OpenRouter 和自定义端点模型 ID 保留 (#1066)
- **[Jefsky](https://github.com/Jefsky)** — `deepseek-cn` 官方端点默认值 (#1079, #1084)
- **[wlon](https://github.com/wlon)** — NVIDIA NIM provider API key 优先级诊断 (#1081)

---

## 贡献

欢迎提交 pull request——请先查看 [CONTRIBUTING.md](CONTRIBUTING.md) 并留意[开放 issue](https://github.com/Hmbown/DeepSeek-TUI/issues) 中的好入门任务。

*本项目与 DeepSeek Inc. 无隶属关系。*

## 许可证

[MIT](LICENSE)

## Star 历史

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/DeepSeek-TUI&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FDeepSeek-TUI&type=date&logscale=&legend=top-left)
