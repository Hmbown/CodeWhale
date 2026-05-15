# DeepSeek TUI

> **面向 [DeepSeek V4](https://platform.deepseek.com) 的终端原生编程智能体：100 万 token 上下文、思考模式流式推理、前缀缓存感知。自包含 Rust 二进制发布——开箱即带 MCP 客户端、沙箱和持久化任务队列。**

[English README](README.md)
[日本語 README](README.ja-JP.md)

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
docker volume create deepseek-tui-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v deepseek-tui-home:/home/deepseek/.deepseek \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/deepseek-tui:latest
```

> 中国大陆访问较慢时，npm 可加 `--registry=https://registry.npmmirror.com`，
> 或使用下方的 [Cargo 镜像](#中国大陆--镜像友好安装)。
>
> 下载安全：官方二进制只发布在
> `https://github.com/Hmbown/DeepSeek-TUI/releases`。手动下载时请校验
> SHA-256 manifest，并避免相似仓库名或搜索结果里的镜像站。详见
> [下载安全与校验](docs/INSTALL.md#2-download-safety-and-checksums)。

已经安装过？按你的安装方式更新：

```bash
deepseek update                         # release 二进制更新器
npm install -g deepseek-tui@latest      # npm 包装器
brew update && brew upgrade deepseek-tui
cargo install deepseek-tui-cli --locked --force
cargo install deepseek-tui     --locked --force
```

[![CI](https://github.com/Hmbown/DeepSeek-TUI/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/DeepSeek-TUI/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/deepseek-tui)](https://www.npmjs.com/package/deepseek-tui)
[![crates.io](https://img.shields.io/crates/v/deepseek-tui-cli?label=crates.io)](https://crates.io/crates/deepseek-tui-cli)
[DeepWiki project index](https://deepwiki.com/Hmbown/DeepSeek-TUI)

![DeepSeek TUI 截图](assets/screenshot.png)

---

## 这是什么？

DeepSeek TUI 是一个完全运行在终端里的编程智能体。它让 DeepSeek 前沿模型直接访问你的工作区：读写文件、运行 shell 命令、搜索浏览网页、管理 git、调度子智能体——全部通过快速、键盘驱动的 TUI 完成。

它面向 **DeepSeek V4**（`deepseek-v4-pro` / `deepseek-v4-flash`）构建，原生支持 100 万 token 上下文窗口和思考模式流式输出。

### 主要功能

- **Auto 模式** —— `--model auto` / `/model auto` 每轮自动选择模型和推理强度
- **原生 RLM**（`rlm_open`/`rlm_eval`）—— 持久化 REPL 会话用于批量分析；使用带界面的辅助函数（`peek`、`search`、`chunk`、`sub_query_batch`）运行低成本 `deepseek-v4-flash` 子任务
- **思考模式流式输出** —— 实时观察模型在解决问题时的思维链展开
- **完整工具集** —— 文件操作、shell 执行、git、网页搜索/浏览、apply-patch、子智能体、MCP 服务器
- **100 万 token 上下文** —— 上下文跟踪、手动或配置驱动的压缩，以及前缀缓存遥测
- **前缀缓存稳定性跟踪** —— 可选 `/statusline` footer chip 显示最近轮次缓存前缀的稳定程度
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
- **实时成本跟踪** —— 按轮次和会话统计 token 用量与成本估算，含缓存命中/未命中明细；简体中文 locale 下显示 CNY
- **技能系统** —— 可通过 GitHub 安装的组合式指令包；首次启动自带 `skill-creator`、`mcp-builder`、`documents`、`presentations`、`spreadsheets`、`pdf`、`feishu` 等 starter skills
- **终端原生通知** —— OSC 9、OSC 99、OSC 777，以及桌面通知兜底
- **内置主题选择器** —— Catppuccin、Tokyo Night、Dracula、Gruvbox 和原有亮/暗色主题，可用 `/theme` 实时切换

---

## 架构说明

`deepseek`（调度器 CLI）→ `deepseek-tui`（伴随二进制）→ ratatui 界面 ↔ 异步引擎 ↔ OpenAI 兼容流式客户端。工具调用通过类型化注册表（shell、文件操作、git、web、子智能体、MCP、RLM）路由，结果流式返回对话记录。引擎管理会话状态、轮次追踪、持久化任务队列和 LSP 子系统——它在下一步推理前将编辑后诊断反馈到模型上下文中。

详见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

### 子智能体：并发后台执行

DeepSeek TUI 可以同时调度多个子智能体并行运行——类似于并发任务队列：

- **非阻塞启动。** `agent_open` 立即返回。子智能体获得独立的上下文和工具注册表，独立运行。父进程继续工作。
- **后台执行。** 子智能体并发运行（默认上限 10，可配置至 20）。引擎管理线程池——无需轮询循环。
- **完成通知。** 子智能体完成后，运行时发送结构化的 `<deepseek:subagent.done>` 事件，包含摘要、证据列表和执行指标。父模型读取 `summary` 字段并整合结果。
- **按需读取结果。** 大型对话记录暂存为 `var_handle` 引用。模型通过 `handle_read` 按切片、范围或 JSONPath 投影读取——保持父上下文精简。

详见 [docs/SUBAGENTS.md](docs/SUBAGENTS.md)。

---

## 快速开始

```bash
npm install -g deepseek-tui
deepseek --version
deepseek --model auto
```

预构建二进制覆盖 **Linux x64**、**Linux ARM64**（v0.8.8 起）、**macOS x64**、**macOS ARM64** 和 **Windows x64**。其他目标平台（musl、riscv64、FreeBSD 等）请见下方的[从源码安装](#从源码安装)或 [docs/INSTALL.md](docs/INSTALL.md)。

首次启动时会提示输入 [DeepSeek API key](https://platform.deepseek.com/api_keys)。密钥保存到 `~/.deepseek/config.toml`，在任意目录、IDE 终端和脚本中都能使用，不会触发系统密钥环弹窗。

也可以提前配置：

```bash
deepseek auth set --provider deepseek   # 保存到 ~/.deepseek/config.toml

deepseek auth status                    # 显示当前活跃的凭证来源
export DEEPSEEK_API_KEY="YOUR_KEY"      # 环境变量方式；需要在非交互式 shell 中使用请放入 ~/.zshenv
deepseek

deepseek doctor                          # 验证安装
```

> 轮换或移除密钥：`deepseek auth clear --provider deepseek`。

### 腾讯云 / CNB 远程优先路径

如果你想要一个长期在线、可从手机控制的工作区，推荐使用腾讯云原生路径：
CNB 镜像/源码，腾讯云 Lighthouse 香港实例，飞书/Lark 长连接桥接，
以及可选的 EdgeOne 公网 HTTPS 边缘。运行时 API 必须绑定在 localhost；
不要通过 EdgeOne 暴露 `/v1/*`。

先看 [docs/TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md)，
再按 [docs/TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) 配置服务器。

### Auto 模式

使用 `deepseek --model auto` 或 `/model auto` 让 DeepSeek TUI 自行决定每轮需要多少模型和推理能力。

Auto 模式同时控制两个设置：

- 模型：`deepseek-v4-flash` 或 `deepseek-v4-pro`
- 推理强度：`off`、`high` 或 `max`

在真实请求发出之前，应用会先用关闭推理的 `deepseek-v4-flash` 进行一次小型路由调用。路由器审视最新请求和最近的上下文，然后为真实请求选定具体的模型和推理强度。简短/简单的轮次保持在 Flash + 关闭推理；编码、调试、发布、架构、安全审查或模糊的多步骤任务可升级到 Pro 和/或更高推理强度。

`auto` 是 DeepSeek TUI 本地行为。上游 API 永远不会收到 `model: "auto"`，它只会收到为当前轮次选定的具体模型和推理强度设置。TUI 会显示选定的路由，成本跟踪按实际运行的模型计费。如果路由调用失败或返回无效答案，应用会回退到本地启发式规则。子智能体会继承 auto 模式，除非你为它们指定了显式模型。

需要可重复基准测试、严格控制成本上限或特定提供商/模型映射时，请使用固定模型或固定推理强度。

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

# AtlasCloud
deepseek auth set --provider atlascloud --api-key "YOUR_ATLASCLOUD_API_KEY"
deepseek --provider atlascloud

# OpenRouter
deepseek auth set --provider openrouter --api-key "YOUR_OPENROUTER_API_KEY"
deepseek --provider openrouter --model deepseek/deepseek-v4-pro

# Novita
deepseek auth set --provider novita --api-key "YOUR_NOVITA_API_KEY"
deepseek --provider novita --model deepseek/deepseek-v4-pro

# Fireworks
deepseek auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"
deepseek --provider fireworks --model deepseek-v4-pro

# 通用 OpenAI 兼容端点
deepseek auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"
OPENAI_BASE_URL="https://openai-compatible.example/v4" deepseek --provider openai --model glm-5

# 自托管 SGLang
SGLANG_BASE_URL="http://localhost:30000/v1" deepseek --provider sglang --model deepseek-v4-flash

# 自托管 vLLM
VLLM_BASE_URL="http://localhost:8000/v1" deepseek --provider vllm --model deepseek-v4-flash

# 自托管 Ollama
ollama pull deepseek-coder:1.3b
deepseek --provider ollama --model deepseek-coder:1.3b
```

在 TUI 内，`/provider` 打开提供方选择器，`/model` 打开模型选择器。
`/provider openrouter` 和 `/model <id>` 可直接切换；`/models` 会列出
API 返回的实时模型。`/model` 选择器会优先使用当前提供方的实时模型
目录，不可用时再回退到 provider-aware 默认模型列表。

---

## 版本说明

每个版本的具体变更见 [CHANGELOG.md](CHANGELOG.md)。README 只保留当前
安装方式、核心工作流、模型提供方配置、运行时接口和扩展入口。

---

## 使用方式

```bash
deepseek                                       # 交互式 TUI
deepseek "explain this function"              # 一次性提示
deepseek exec --auto --output-format stream-json "fix this bug" # 面向后端集成的 NDJSON 流
deepseek exec --resume <SESSION_ID> "follow up" # 继续非交互会话
deepseek --model deepseek-v4-flash "summarize" # 指定模型
deepseek --model auto "fix this bug"          # 自动选择模型 + 推理强度
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
deepseek serve --acp                           # Zed/自定义智能体的 ACP stdio 适配器
deepseek run pr <N>                            # 获取 PR 并预填审查提示
deepseek mcp list                              # 列出已配置 MCP 服务器
deepseek mcp validate                          # 校验 MCP 配置和连接
deepseek mcp-server                            # 启动 dispatcher MCP stdio 服务器
deepseek update                                # 检查并应用二进制更新
```

Docker 镜像发布在 GHCR 上：

```bash
docker volume create deepseek-tui-home

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v deepseek-tui-home:/home/deepseek/.deepseek \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/deepseek-tui:latest
```

固定 tag、本地构建、volume 权限和非交互管道用法见 [docs/DOCKER.md](docs/DOCKER.md)。

### Zed / ACP

DeepSeek 可作为自定义 Agent Client Protocol 服务器运行，供 Zed 等编辑器通过 stdio 调用本地 ACP 智能体。在 Zed 中添加自定义智能体服务器：

```json
{
  "agent_servers": {
    "DeepSeek": {
      "type": "custom",
      "command": "deepseek",
      "args": ["serve", "--acp"],
      "env": {}
    }
  }
}
```

首个 ACP 切片支持通过现有 DeepSeek 配置/API 密钥创建新会话和提示响应。工具支持的编辑和检查点回放尚未通过 ACP 暴露。

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
| `DEEPSEEK_HTTP_HEADERS` | 可选模型请求头，例如 `X-Model-Provider-Id=your-model-provider` |
| `DEEPSEEK_MODEL` | 默认模型 |
| `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` | 流式响应空闲超时秒数，默认 `300`，限制在 `1..=3600` |
| `DEEPSEEK_PROVIDER` | `deepseek`（默认）、`nvidia-nim`、`openai`、`openrouter`、`novita`、`atlascloud`、`fireworks`、`sglang`、`vllm`、`ollama` |
| `DEEPSEEK_PROFILE` | 配置 profile 名称 |
| `DEEPSEEK_MEMORY` | 设为 `on` 启用用户记忆 |
| `DEEPSEEK_ALLOW_INSECURE_HTTP=1` | 在可信网络上允许非本机 `http://` API base URL |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `OPENROUTER_API_KEY` / `NOVITA_API_KEY` / `ATLASCLOUD_API_KEY` / `FIREWORKS_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` | 提供商认证 |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | 通用 OpenAI 兼容端点和模型 ID |
| `ATLASCLOUD_BASE_URL` / `ATLASCLOUD_MODEL` | AtlasCloud 端点和模型覆盖 |
| `OPENROUTER_BASE_URL` | OpenRouter 端点覆盖 |
| `NOVITA_BASE_URL` | Novita 端点覆盖 |
| `FIREWORKS_BASE_URL` | Fireworks 端点覆盖 |
| `SGLANG_BASE_URL` | 自托管 SGLang 端点 |
| `SGLANG_MODEL` | 自托管 SGLang 模型 ID |
| `VLLM_BASE_URL` | 自托管 vLLM 端点 |
| `VLLM_MODEL` | 自托管 vLLM 模型 ID |
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
| [DOCKER.md](docs/DOCKER.md) | GHCR 镜像、volume 和 Docker 用法 |
| [CNB_MIRROR.md](docs/CNB_MIRROR.md) | CNB 镜像和中国大陆友好安装说明 |
| [TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md) | 腾讯云/CNB/Lighthouse/飞书远程优先路径 |
| [TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) | 腾讯云 Lighthouse 香港实例配置 |
| [MEMORY.md](docs/MEMORY.md) | 用户记忆功能指南 |
| [SUBAGENTS.md](docs/SUBAGENTS.md) | 子智能体角色分类与生命周期 |
| [KEYBINDINGS.md](docs/KEYBINDINGS.md) | 完整快捷键目录 |
| [RELEASE_RUNBOOK.md](docs/RELEASE_RUNBOOK.md) | 发布流程 |
| [LOCALIZATION.md](docs/LOCALIZATION.md) | UI 语言矩阵与切换 |
| [OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) | 运维和恢复 |

完整更新历史：[CHANGELOG.md](CHANGELOG.md)。

---

## 致谢

- **[DeepSeek](https://github.com/deepseek-ai)** — 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** — 感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — 感谢 OpenWarp 优先支持 DeepSeek TUI，并一起打磨更好的终端智能体体验。
- **[Open Design](https://github.com/nexu-io/open-design)** — 感谢 Open Design 对面向设计的智能体工作流提供支持与协作。

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
- **[MengZ-super](https://github.com/MengZ-super)** — `/theme` 命令基础和 SSE gzip/brotli 解压支持 (#1057, #1061)
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
