# CodeWhale

> **DeepSeek V4 的最强智能体运行框架。规则、工具、证据和反馈循环——帮助模型持续工作直到任务完成，并且越用越好。**

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/codewhale)](https://www.npmjs.com/package/codewhale)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![Sponsor](https://img.shields.io/badge/Sponsor-GitHub%20Sponsors-ea4aaa?logo=githubsponsors&logoColor=white)](https://github.com/sponsors/Hmbown)
[DeepWiki project index](https://deepwiki.com/Hmbown/CodeWhale)

[English README](README.md)
[日本語 README](README.ja-JP.md)

[安装](#安装) · [快速开始](#快速开始) · [使用方式](#使用方式) · [文档](#文档) · [贡献](#贡献) · [支持](#支持)

## 安装

`codewhale` 是自包含 Rust 二进制——**运行时不依赖 Node.js 或 Python**。
下面几种方式装出来的是同一套二进制，按你已有的工具链选一个即可：

```bash
# 1. npm —— 已装 Node 的最方便方式。npm 包只是一个下载器，
#    会从 GitHub Releases 拉取对应平台的预编译二进制，
#    并不会让 codewhale 本身依赖 Node 运行时。
npm install -g codewhale

# 2. Cargo —— 无需 Node。
cargo install codewhale-cli --locked   # `codewhale` 入口
cargo install codewhale-tui     --locked   # `codewhale-tui` TUI 二进制

# 3. Homebrew —— macOS 包管理器。
brew tap Hmbown/deepseek-tui
brew install deepseek-tui

# 4. 直接下载 —— 无需任何工具链。
#    https://github.com/Hmbown/CodeWhale/releases
#    覆盖 Linux x64/ARM64、macOS x64/ARM64、Windows x64

# 5. Docker —— 预构建发布镜像。
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.deepseek \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

> 中国大陆访问较慢时，npm 可加 `--registry=https://registry.npmmirror.com`，
> 或使用下方的 [Cargo 镜像](#中国大陆--镜像友好安装)。
>
> 下载安全：官方二进制只发布在
> `https://github.com/Hmbown/CodeWhale/releases`。手动下载时请校验
> SHA-256 manifest，并避免相似仓库名或搜索结果里的镜像站。详见
> [下载安全与校验](docs/INSTALL.md#2-download-safety-and-checksums)。

已经安装过？按你的安装方式更新：

```bash
codewhale update                         # release 二进制更新器
npm install -g codewhale@latest      # npm 包装器
brew update && brew upgrade deepseek-tui
cargo install codewhale-cli --locked --force
cargo install codewhale-tui     --locked --force
```

![codewhale 截图](assets/screenshot.png)

---

## 这是什么？

模型回答问题。智能体完成任务。区别在于运行框架——包围模型的规则、工具、证据和反馈循环。

CodeWhale 就是这套框架，围绕 DeepSeek V4 Pro 和 Flash 构建。它最初是一个个人工具，因为维护者受够了模型在任务中途迷失方向、服从过时指令而非用户当前请求、或者命令失败就放弃。结果诞生了一个让模型保持方向的系统：宪政提示层级、结构化信任边界、并行子智能体、前缀缓存感知的上下文管理、以及让模型有足够信号来自我校正的验证节拍。

DeepSeek V4 参与了这套框架的部分编写。这很重要——它意味着 CodeWhale 已经是使用 V4 最有效的方式，并且随着 V4 的改进，框架也会随之改进。每一轮都留下更好的提示、更好的规则、更好的交接。下一轮从一个更强的位置开始。

### 框架如何工作

**宪政层级。** 系统提示词是一个冲突法引擎。用户意图优先于陈旧记忆。实时证据优先于假设。验证优先于自信。每一轮继承清晰的权威链，模型永远不需要猜测该服从哪条指令。

**结构化信任。** 三种模式，硬边界——Plan（只读）、Agent（审批门控）、YOLO（可信工作区自动批准）。OS 级沙箱支撑边界：macOS 的 Seatbelt、Linux 的 Landlock、Windows 的 Job Objects。危险命令无论在哪种模式下都被分类和门控。

**反馈节拍。** 失败的命令、失败的测试、LSP 诊断——这些不是死胡同。它们是模型可以调谐的信号。非零退出码是信息。类型错误是修正向量。框架让失败变得可读，模型可以在用户不必不断重新掌舵的情况下恢复。

**连续性。** 记忆跨会话持久化。交接在上下文压缩后存活。会话可以保存、恢复和分叉到兄弟路径。下一位智能体——人或机器——从上一位置继续，无需重新发现已经学到的东西。

**分布式工作。** 子智能体并发运行，一次最多 20 个。`agent_open` 立即返回，父进程继续工作。结果以结构化完成事件形式到达，带有有界句柄——无需用完整对话记录淹没父上下文。详见 [docs/SUBAGENTS.md](docs/SUBAGENTS.md)。

**适度智能。** Fin——关闭思考的廉价 Flash——处理模型自动路由、RLM 子调用、摘要和协调。Pro 用于架构、调试和安全审查。`--model auto` 每轮选择模型和思考强度。每个问题匹配恰当的智能量。

**长程注意力经济学。** 100 万 token 上下文窗口，前缀缓存遥测。缓存命中比未命中便宜约 100 倍。`/statusline` 芯片实时显示前缀稳定性，让你能看到某次变更是否即将破坏缓存预算。

**自由与恢复。** 每轮记录 side-git 快照，不影响仓库 `.git`。`/restore` 和 `revert_turn` 即刻回滚工作区。危险操作在 OS 级沙箱化。你可以让模型放手尝试。

其余功能面：**RLM 会话**（持久化 Python REPL，配合 `peek`、`search`、`chunk`、`sub_query_batch` 辅助函数进行批量分析）、**LSP 诊断**（每次编辑后 rust-analyzer、pyright、tsserver、gopls、clangd 的内联错误）、**MCP 协议**、**HTTP/SSE 运行时 API**、重启后仍存活的**持久化任务队列**、Zed 等编辑器的 **ACP 适配器**、**SWE-bench 导出**、**可安装技能**、**主题选择器**、**桌面通知**、以及带缓存命中/未命中明细的**实时成本追踪**。

---

## 运行框架

`codewhale`（调度器 CLI）→ `codewhale-tui`（伴随二进制）→ ratatui 界面 ↔ 异步引擎 ↔ OpenAI 兼容流式客户端。工具调用通过类型化注册表（shell、文件操作、git、web、子智能体、MCP、RLM）路由，结果流式返回对话记录。引擎管理会话状态、轮次追踪、持久化任务队列和 LSP 子系统——它在下一步推理前将编辑后诊断反馈到模型上下文中。

详见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

### 子智能体：并发后台执行

codewhale 可以同时调度多个子智能体并行运行——类似于并发任务队列：

- **非阻塞启动。** `agent_open` 立即返回。子智能体获得独立的上下文和工具注册表，独立运行。父进程继续工作。
- **后台执行。** 子智能体并发运行（默认上限 10，可配置至 20）。引擎管理线程池——无需轮询循环。
- **完成通知。** 子智能体完成后，运行时发送结构化的 `<codewhale:subagent.done>` 事件，包含摘要、证据列表和执行指标。父模型读取 `summary` 字段并整合结果。
- **按需读取结果。** 大型对话记录暂存为 `var_handle` 引用。模型通过 `handle_read` 按切片、范围或 JSONPath 投影读取——保持父上下文精简。

详见 [docs/SUBAGENTS.md](docs/SUBAGENTS.md)。

---

## 快速开始

```bash
npm install -g codewhale
codewhale --version
codewhale --model auto
```

预构建二进制覆盖 **Linux x64**、**Linux ARM64**（v0.8.8 起）、**macOS x64**、**macOS ARM64** 和 **Windows x64**。其他目标平台（musl、riscv64、FreeBSD 等）请见下方的[从源码安装](#从源码安装)或 [docs/INSTALL.md](docs/INSTALL.md)。

首次启动时会提示输入 [DeepSeek API key](https://platform.deepseek.com/api_keys)。密钥保存到 `~/.deepseek/config.toml`，在任意目录、IDE 终端和脚本中都能使用，不会触发系统密钥环弹窗。

也可以提前配置：

```bash
codewhale auth set --provider deepseek   # 保存到 ~/.deepseek/config.toml

codewhale auth status                    # 显示当前活跃的凭证来源
export DEEPSEEK_API_KEY="YOUR_KEY"      # 环境变量方式；需要在非交互式 shell 中使用请放入 ~/.zshenv
codewhale

codewhale doctor                          # 验证安装
```

> 轮换或移除密钥：`codewhale auth clear --provider deepseek`。

### 腾讯云 / CNB 远程优先路径

如果你想要一个长期在线、可从手机控制的工作区，推荐使用腾讯云原生路径：
CNB 镜像/源码，腾讯云 Lighthouse 香港实例，飞书/Lark 长连接桥接，
以及可选的 EdgeOne 公网 HTTPS 边缘。运行时 API 必须绑定在 localhost；
不要通过 EdgeOne 暴露 `/v1/*`。

先看 [docs/TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md)，
再按 [docs/TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) 配置服务器。

### 模型自动路由与 Fin

使用 `codewhale --model auto` 或 `/model auto` 让 codewhale 自行决定每轮需要多少模型和推理能力。

模型自动路由同时控制两个设置：

- 模型：`deepseek-v4-flash` 或 `deepseek-v4-pro`
- 推理强度：`off`、`high` 或 `max`

在真实请求发出之前，应用会先用关闭推理的 `deepseek-v4-flash` 进行一次小型路由调用。这条快速路径叫 **Fin**：用于模型选择、摘要、RLM 子任务、上下文维护以及其他不该消耗完整推理轮次的协调工作。Fin 审视最新请求和最近的上下文，然后为真实请求选定具体的模型和推理强度。简短/简单的轮次保持在 Flash + 关闭推理；编码、调试、发布、架构、安全审查或模糊的多步骤任务可升级到 Pro 和/或更高推理强度。

`--model auto` 和 `/model auto` 是 codewhale 本地行为。上游 API 永远不会收到 `model: "auto"`，它只会收到为当前轮次选定的具体模型和推理强度设置。TUI 会显示选定的路由，成本跟踪按实际运行的模型计费。如果 Fin 路由失败或返回无效答案，应用会回退到本地启发式规则。子智能体会继承模型自动路由，除非你为它们指定了显式模型。

需要可重复基准测试、严格控制成本上限或特定提供商/模型映射时，请使用固定模型或固定推理强度。

### Linux ARM64（HarmonyOS 轻薄本、openEuler、Kylin、树莓派、Graviton 等）

从 v0.8.8 起，`npm i -g codewhale` 直接支持 glibc 系的 ARM64 Linux。你也可以从 [Releases 页面](https://github.com/Hmbown/CodeWhale/releases) 下载预编译二进制，放到 `PATH` 目录中。

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
cargo install codewhale-cli --locked   # 提供推荐入口 `codewhale`
cargo install codewhale-tui     --locked   # 提供交互式 TUI 伴随二进制
codewhale --version
```

也可以直接从 [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases) 下载预编译二进制。`DEEPSEEK_TUI_RELEASE_BASE_URL` 可用于镜像后的 release 资产。

### Windows (Scoop)

[Scoop](https://scoop.sh) 是一个 Windows 软件包管理器。codewhale 已进入
Scoop main bucket，但该 manifest 独立更新，可能滞后于 GitHub/npm/Cargo
release。先运行 `scoop update`，安装后用 `codewhale --version` 核对版本：

```bash
scoop update
scoop install deepseek-tui
codewhale --version
```

如果需要最新版本，请优先使用 npm 或直接下载 GitHub Release 资产。


<details id="install-from-source">
<summary>从源码安装</summary>

适用于任何 Tier-1 Rust 目标，包括 musl、riscv64、FreeBSD 以及尚无预编译包的 ARM64 发行版。

```bash
# Linux 构建依赖（Debian/Ubuntu/RHEL）：
#   sudo apt-get install -y build-essential pkg-config libdbus-1-dev
#   sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel

git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale

cargo install --path crates/cli --locked   # 需要 Rust 1.88+；提供 `codewhale`
cargo install --path crates/tui --locked   # 提供 `codewhale-tui`
```

两个二进制都需要安装。交叉编译和平台特定说明见 [docs/INSTALL.md](docs/INSTALL.md)。

</details>

### 其他模型提供方

```bash
# NVIDIA NIM
codewhale auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
codewhale --provider nvidia-nim

# AtlasCloud
codewhale auth set --provider atlascloud --api-key "YOUR_ATLASCLOUD_API_KEY"
codewhale --provider atlascloud

# Wanjie Ark
codewhale auth set --provider wanjie-ark --api-key "YOUR_WANJIE_API_KEY"
codewhale --provider wanjie-ark --model deepseek-reasoner

# OpenRouter
codewhale auth set --provider openrouter --api-key "YOUR_OPENROUTER_API_KEY"
codewhale --provider openrouter --model deepseek/deepseek-v4-pro

# Novita
codewhale auth set --provider novita --api-key "YOUR_NOVITA_API_KEY"
codewhale --provider novita --model deepseek/deepseek-v4-pro

# Fireworks
codewhale auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"
codewhale --provider fireworks --model deepseek-v4-pro

# 通用 OpenAI 兼容端点
codewhale auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"
OPENAI_BASE_URL="https://openai-compatible.example/v4" codewhale --provider openai --model glm-5

# 自托管 SGLang
SGLANG_BASE_URL="http://localhost:30000/v1" codewhale --provider sglang --model deepseek-v4-flash

# 自托管 vLLM
VLLM_BASE_URL="http://localhost:8000/v1" codewhale --provider vllm --model deepseek-v4-flash

# 自托管 Ollama
ollama pull codewhale-coder:1.3b
codewhale --provider ollama --model codewhale-coder:1.3b
```

在 TUI 内，`/provider` 打开提供方选择器，`/model` 打开本地模型/思考模式
选择器。`/provider openrouter` 和 `/model <id>` 可直接切换；`/models` 会在
当前提供方支持模型列表时显式请求并列出 API 返回的实时模型。

---

## 版本说明

每个版本的具体变更见 [CHANGELOG.md](CHANGELOG.md)。README 只保留当前
安装方式、核心工作流、模型提供方配置、运行时接口和扩展入口。

---

## 使用方式

```bash
codewhale                                       # 交互式 TUI
codewhale "explain this function"              # 一次性提示
codewhale exec --auto --output-format stream-json "fix this bug" # 自动批准工具的 agentic exec
codewhale exec --resume <SESSION_ID> "follow up" # 继续非交互会话
codewhale --model deepseek-v4-flash "summarize" # 指定模型
codewhale --model auto "fix this bug"          # 自动路由模型 + 推理强度
codewhale --yolo                                # 自动批准工具
codewhale auth set --provider deepseek         # 保存 API key
codewhale doctor                                # 检查配置和连接
codewhale doctor --json                         # 机器可读诊断
codewhale setup --status                        # 只读安装状态
codewhale setup --tools --plugins               # 创建本地工具和插件目录
codewhale models                                # 列出可用 API 模型
codewhale sessions                              # 列出已保存会话
codewhale resume --last                         # 恢复最近会话
codewhale resume <SESSION_ID>                   # 按 UUID 恢复指定会话
codewhale fork <SESSION_ID>                     # 将已保存会话分叉为兄弟路径
codewhale serve --http                          # HTTP/SSE API 服务
codewhale serve --acp                           # Zed/自定义智能体的 ACP stdio 适配器
codewhale run pr <N>                            # 获取 PR 并预填审查提示
codewhale mcp list                              # 列出已配置 MCP 服务器
codewhale mcp validate                          # 校验 MCP 配置和连接
codewhale mcp-server                            # 启动 dispatcher MCP stdio 服务器
codewhale update                                # 检查并应用二进制更新
```

Docker 镜像发布在 GHCR 上：

```bash
docker volume create codewhale-home

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.deepseek \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

固定 tag、本地构建、volume 权限和非交互管道用法见 [docs/DOCKER.md](docs/DOCKER.md)。

### Zed / ACP

DeepSeek 可作为自定义 Agent Client Protocol 服务器运行，供 Zed 等编辑器通过 stdio 调用本地 ACP 智能体。在 Zed 中添加自定义智能体服务器：

```json
{
  "agent_servers": {
    "DeepSeek": {
      "type": "custom",
      "command": "codewhale",
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

模式与模型自动路由是两个概念。`Tab` 切换 Plan / Agent / YOLO，
`/model auto` 选择模型和思考强度。`/goal` 当前用于追踪会话目标和
token 预算；未来如果扩展成 Goal 工作区，也应与 `--model auto` 保持独立。

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
| `DEEPSEEK_PROVIDER` | `codewhale`（默认）、`nvidia-nim`、`openai`、`atlascloud`、`wanjie-ark`、`openrouter`、`novita`、`fireworks`、`sglang`、`vllm`、`ollama` |
| `DEEPSEEK_PROFILE` | 配置 profile 名称 |
| `DEEPSEEK_MEMORY` | 设为 `on` 启用用户记忆 |
| `DEEPSEEK_ALLOW_INSECURE_HTTP=1` | 在可信网络上允许非本机 `http://` API base URL |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `ATLASCLOUD_API_KEY` / `WANJIE_ARK_API_KEY` / `OPENROUTER_API_KEY` / `NOVITA_API_KEY` / `FIREWORKS_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` | 提供商认证 |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | 通用 OpenAI 兼容端点和模型 ID |
| `ATLASCLOUD_BASE_URL` / `ATLASCLOUD_MODEL` | AtlasCloud 端点和模型覆盖 |
| `WANJIE_ARK_BASE_URL` / `WANJIE_ARK_MODEL` | Wanjie Ark 端点和模型覆盖 |
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
LANG=zh_CN.UTF-8 codewhale run
```

---

## 模型和价格

| 模型 | 上下文 | 输入（缓存命中） | 输入（缓存未命中） | 输出 |
|---|---|---|---|---|
| `deepseek-v4-pro` | 1M | $0.003625 / 1M | $0.435 / 1M | $0.87 / 1M |
| `deepseek-v4-flash` | 1M | $0.0028 / 1M | $0.14 / 1M | $0.28 / 1M |

旧别名 `deepseek-chat` / `deepseek-reasoner` 映射到 `deepseek-v4-flash`。NVIDIA NIM 变体使用你的 NVIDIA 账号条款。

> [!Note]
> 上表的 V4 Pro 单价现已成为官方长期价格：DeepSeek 已宣布在 75% 限时折扣窗口于 **2026 年 5 月 31 日 23:59（北京时间）** 结束后，正式将原始价格调整为约四分之一。TUI 的成本估算已使用这些数值，因此无需任何代码改动。后续价格变动请参阅官方 [DeepSeek 定价页面](https://api-docs.deepseek.com/zh-cn/quick_start/pricing)。

---

## 创建和安装技能

codewhale 从工作区目录（`.agents/skills` → `skills` → `.opencode/skills` → `.claude/skills`）和全局 `~/.deepseek/skills` 发现技能。每个技能是一个包含 `SKILL.md` 的目录：

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

## 支持

CodeWhale 采用 MIT 许可证，使用和参与贡献都不需要赞助。如果它帮你节省了时间，
最直接的长期支持方式是 [GitHub Sponsors](https://github.com/sponsors/Hmbown)。
一次性支持也可以通过 [Buy Me a Coffee](https://www.buymeacoffee.com/hmbown) 完成。

赞助会用于发布构建、CI/运行时测试、包发布，以及维护者处理 issue 和 review 的时间。
功能请求、Bug 报告和 pull request 不需要赞助。

---

## 致谢

- **[DeepSeek](https://github.com/deepseek-ai)** — 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** — 感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — 感谢 OpenWarp 优先支持 codewhale，并一起打磨更好的终端智能体体验。
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
- **[lloydzhou](https://github.com/lloydzhou)** 和 **[jeoor](https://github.com/jeoor)** — 压缩成本报告和 npm 安装器流暂停竞态修复 (#1860)；lloydzhou 还贡献了确定性的环境上下文注入 (#813, #922) 和 KV 前缀缓存稳定化 (#1080)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README 清晰化改进 (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows 安装文档 (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — 价格/折扣信息更新 (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — 模型 ID 大小写兼容性报告 (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — `working...` 卡死状态 Bug 报告和 Windows 剪贴板兜底修复 (#738, #850)
- **[reidliu41](https://github.com/reidliu41)** — 退出后的恢复提示、工作区信任持久化、Ollama provider 支持、思考块流式终结修复，以及帮助选择器选中行可见性优化 (#863, #870, #921, #1078, #1964)
- **[cyq1017](https://github.com/cyq1017)** — Unicode `git_status` 路径、本地/配置技能发现，以及模式切换 toast 去重 (#1953, #1956, #1957)
- **[xieshutao](https://github.com/xieshutao)** — 纯 Markdown skill 兜底解析 (#869)
- **[GK012](https://github.com/GK012)** — npm wrapper 的 `--version` 兜底 (#885)
- **[y0sif](https://github.com/y0sif)** — 直接子智能体完成后唤醒父级 turn loop (#901)
- **[mac119](https://github.com/mac119)** 和 **[leo119](https://github.com/leo119)** — `codewhale update` 命令文档 (#838, #917)
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
- **[Horace Liu](https://github.com/liuhq)** — Nix 包支持和安装文档 (#1173)
- **[jieshu666](https://github.com/jieshu666)** — 终端重绘闪烁修复 (#1563)
- **[gordonlu](https://github.com/gordonlu)** — Windows Enter / CSI-u 输入修复 (#1612)
- **[mdrkrg](https://github.com/mdrkrg)** — 首次运行 API key 缺失时的启动崩溃修复 (#1598)
- **[Aitensa](https://github.com/Aitensa)** — diff 和 pager 输出的 CJK 换行支持 (#1622)
- **[qiyan233](https://github.com/qiyan233)** — 遗留 DeepSeek CN provider 别名兼容 (#1645)
- **[zlh124](https://github.com/zlh124)** — WSL2/headless 启动报告和剪贴板初始化修复 (#1772, #1773)
- **[aboimpinto](https://github.com/aboimpinto)** — Windows alt-screen 日志、Home/End 编辑器，以及运行时日志跟进 (#1774, #1776, #1748, #1749, #1782, #1783)
- **[LeoLin990405](https://github.com/LeoLin990405)** — provider 模型透传、reasoning 重放、thinking-only turn 和 Windows 引用修复 (#1740, #1743, #1742, #1744)
- **[nightt5879](https://github.com/nightt5879)** — Ctrl+C 提示恢复修复 (#1764)
- **[h3c-hexin](https://github.com/h3c-hexin)** — 流式批量工具调用保留和 CLI reasoning-effort 透传 (#1686, #1511)
- **[hxy91819](https://github.com/hxy91819)** — 工具结果裁剪时的前缀缓存保留 (#1514)
- **[JiarenWang](https://github.com/JiarenWang)** — Plan 模式只读执行、审批接管优化、Ctrl+H 删除修复和 undo 上下文同步 (#1123, #962, #958, #1150)
- **[Liu-Vince](https://github.com/Liu-Vince)** — MCP 分页、markdown 缩进保留、zh-Hans i18n 优化和环境变量文档 (#1256, #1179, #1274, #1178)
- **[linzhiqin2003](https://github.com/linzhiqin2003)** — `--model auto` 成本节约偏好、执行纪律提示和声明式事实记忆指导 (#1385, #1384, #1381)
- **[lbcheng888](https://github.com/lbcheng888)** — 跨保存/恢复的成本持久化和对话滚动修复 (#1192, #1211)
- **[pengyou200902](https://github.com/pengyou200902)** — UTF-8 安全记忆截断、截断标记精确化和快捷键文档 (#968, #1122, #1095)
- **[ChaceLyee2101](https://github.com/ChaceLyee2101)** — 推理 token 成本统计和 zh-Hans 自动 CNY 显示，以及 zh-CN README 同步 (#1505, #1504)
- **[CrepuscularIRIS](https://github.com/CrepuscularIRIS)** — Termius/SSH 低动画模式和 npx MCP 服务器沙箱修复 (#1479, #1346)
- **[laoye2020](https://github.com/laoye2020)** — Catppuccin、Tokyo Night、Dracula 和 Gruvbox 主题及 `/theme` 选择器 (#1534)
- **[punkcanyang](https://github.com/punkcanyang)** — Kitty (OSC 99) 和 Ghostty (OSC 777) 桌面通知支持 (#1426)
- **[Rene-Kuhm](https://github.com/Rene-Kuhm)** — 西班牙语（es-419）拉丁美洲本地化 (#1452)
- **[sternelee](https://github.com/sternelee)** — DeepSeek 前缀缓存稳定性追踪 (#1517)
- **[ComeFromTheMars](https://github.com/ComeFromTheMars)** — Shift+Up/Down 对话滚动快捷键 (#1432)
- **[sockerch](https://github.com/sockerch)** — 所有斜杠命令的拼音别名 (#1306)
- **[Apeiron0w0](https://github.com/Apeiron0w0)** — Tabby 终端闪烁循环的 FocusGained 去抖动 (#1560)
- **[greyfreedom](https://github.com/greyfreedom)** — 跳转到最新对话按钮 (#969)
- **[SamhandsomeLee](https://github.com/SamhandsomeLee)** — 显式隐藏文件提及补全 (#1270)
- **[dst1213](https://github.com/dst1213)** — 配额错误 HTTP 400 重试 (#1203)
- **[fuleinist](https://github.com/fuleinist)** — `--yolo` 标志从 CLI 转发到 TUI (#1233)
- **[heloanc](https://github.com/heloanc)** — Home/End 键编辑器支持 (#1246)
- **[jinpengxuan](https://github.com/jinpengxuan)** — 入职期间活动 provider 凭据保留 (#1265)
- **[lixiasky-back](https://github.com/lixiasky-back)** — 已验证 npm 二进制采用 (#1339)
- **[J3y0r](https://github.com/J3y0r)** — 工作区切换命令 (#1065)
- **[KhalidAlnujaidi](https://github.com/KhalidAlnujaidi)** — delegate 技能打包 (#1144)
- **[Wenjunyun123](https://github.com/Wenjunyun123)** — 文档锚点偏移保留 (#1282)
- **[whtis](https://github.com/whtis)** — zh-CN README 调度程序路径同步 (#1235)
- **[aqilaziz](https://github.com/aqilaziz)** — memory 技能链接修复 (#1095)
- **[wuwuzhijing](https://github.com/wuwuzhijing)** — rsproxy rustup 变通安装文档 (#1011)
- **[eltociear](https://github.com/eltociear)** — 日语 README 翻译 (#746)
- **[Ling](https://github.com/LING71671)** — `grep_files` 取消令牌支持和 Ctrl+Z 编辑器草稿恢复 (#1839, #1911)
- **[Ben Younes](https://github.com/ousamabenyounes)** — Linux Wayland（非 wlroots）剪贴板支持 (#1938)
- **[Matt Van Horn](https://github.com/mvanhorn)** — Docker 首次运行权限修复和运行时系统提示回归测试 (#1699, #1702)
- **[Kristopher Clark](https://github.com/krisclarkdev)** — compaction 用户查询保留修复 (#1704)
- **[tdccccc](https://github.com/tdccccc)** — 编辑器滚动修复和 pager 鼠标滚轮支持 (#1715, #1716)
- **[LittleBlacky](https://github.com/LittleBlacky)** — provider gated `reasoning_content` 流式修复 (#1680)
- **[Anaheim](https://github.com/AnaheimEX)** — `rlm_open` 空 source schema 校验报告 (#1712)
- **[THatch26](https://github.com/THatch26)** — 终端 resize 后翻页修复 (#1724)
- **[Alvin](https://github.com/alvin1)** — Zed ACP id 兼容性报告 (#1696)
- **[knqiufan](https://github.com/knqiufan)** — sub-agent 文件写入委派工作 (#1833)
- **[IIzzaya](https://github.com/IIzzaya)** — slash 补全精确 alias 优先排序想法 (#1811)
- **[DC](https://github.com/duanchao-lab)** — 终端清理 guard 思路 (#1630)
- **[imkingjh999](https://github.com/imkingjh999)** — provider/model 切换修复 (#1642)
- **[Photo](https://github.com/eng2007)** — provider-aware `/model` picker catalog 工作 (#1201)
- **[chennest](https://github.com/chennest)** — diagnostics schema 报告 (#1685)
- **[kunpeng-ai-lab](https://github.com/kunpeng-ai-lab)** — Windows 编辑器滚动修复 (#1578)
- **[WuMing](https://github.com/asdfg314284230)** — Windows PowerShell 闪烁修复 (#1591)
- **[maker316](https://github.com/maker316)** — LoopGuard/checklist 循环报告 (#1574)
- **[lalala](https://github.com/lalala-233)** — approval denial 回归报告 (#1617)
- **[muyuliyan](https://github.com/muyuliyan)** — `pandoc_convert` 校验修复 (#1523)
- **[czf0718](https://github.com/czf0718)** — resize 和 turn-completion 闪烁修复 (#1537)
- **[MeAiRobot](https://github.com/MeAiRobot)** — toast 覆盖编辑器输入的修复 (#1485)
- **[tiger-dog](https://github.com/tiger-dog)** — approval modal 折叠和 markdown identifier 修复 (#1455)
- **[MMMarcinho](https://github.com/MMMarcinho)** — opt-in `image_analyze` 视觉工具 (#1467)
- **[lucaszhu-hue](https://github.com/lucaszhu-hue)** — AtlasCloud provider 集成 (#1436)
- **[sandofree](https://github.com/sandofree)** — Tavily 和 Bocha `web_search` 后端 (#1294)
- **[zhuangbiaowei](https://github.com/zhuangbiaowei)** — `/change` release notes 命令 (#1416)
- **[NorethSea](https://github.com/NorethSea)** — updater 同步刷新 companion binary 的修复 (#1492)
- **[Jianfengwu2024](https://github.com/Jianfengwu2024)** — Windows MSVC toolchain 环境保留 (#1487)
- **[Fire-dtx](https://github.com/Fire-dtx)** — npm postinstall 可恢复性工作 (#1059)
- **[oooyuy92](https://github.com/oooyuy92)** — 长会话配色可读性报告 (#1070, #936)
- **[qinxianyuzou](https://github.com/qinxianyuzou)** — zh-Hans destructive approval 文案 (#1087, #1091)
- **[tyouter](https://github.com/tyouter)** — session title/history preview 清理 (#1510)
- **[xulongzhe](https://github.com/xulongzhe)** — issue template 和 vision boundary follow-up (#1530, #1544)
- **[YaYII](https://github.com/YaYII)** — trusted media path 工作 (#1462)
- **[47Cid](https://github.com/47Cid)** 和 **[Jafar Akhondali](https://github.com/JafarAkhondali)** — 负责任安全披露和加固报告

---

## 贡献

欢迎提交 pull request——请先查看 [CONTRIBUTING.md](CONTRIBUTING.md) 并留意[开放 issue](https://github.com/Hmbown/CodeWhale/issues) 中的好入门任务。

*本项目与 DeepSeek Inc. 无隶属关系。*

## 许可证

[MIT](LICENSE)

## Star 历史

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
