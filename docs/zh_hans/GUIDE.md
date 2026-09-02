# Codewhale 用户指南

> 本文翻译自英文版 [GUIDE.md](../GUIDE.md)，与英文修订 `3c3630396`（2026-08-19）同步。

本指南面向你使用 Codewhale 的第一个小时。它涵盖了主要工作流程、重要安全控制，以及当你需要完整参考时接下来该看什么。

Codewhale 有更深入的参考文档，涵盖安装、配置、提供商（provider）、模式、快捷键、工具和运维。请将本页当作引导式走查，需要每个选项时再顺着"下一步"链接往下看。

## 1. 欢迎使用 Codewhale

Codewhale 是一个终端编码智能体（agent）。你从某个工作区运行它，交给它一个任务，它就能用结构化工具检查文件、运行命令、编辑代码，并带回证据汇报结果。

与普通聊天模型的重要区别在于，Codewhale 是围绕 “驾驭框架”（harness） 构建的：

- 它让活动工作区和会话保持可见。
- 它把每一轮都路由到明确的模式与审批规则。
- 它在对话记录中展示工具调用，而不是把工作藏起来。
- 它可以保存会话、分叉对话，并在之后继续。
- 它可以运行子智能体来执行专注的后台工作。

你可以用 Codewhale 回答小问题：

```text
解释此仓库中的身份验证流程。
```

也可以用它做多步工作：

```text
找到失败的验证路径，提出修复方案，等我批准了再编辑文件。
```

对于新仓库，请从保守的方式开始。在要求 Codewhale 修改文件之前，先让它探索和规划。这样会为您提供可审查的路径，并更容易及早发现错误的假设。

下一步：[ARCHITECTURE.md](../ARCHITECTURE.md) 讲解内部 harness 与运行时模型。

## 2. 首次启动

用适合你机器的路径安装 Codewhale。发布安装器在 `codewhale` 和 `codew` 两个命令名下提供同一运行时；每条受支持的安装路径都提供 `codewhale` 调度器，`codewhale-tui` 运行时已内置。

```bash
# npm
npm install -g codewhale

# Cargo
cargo install codewhale-cli --locked
# Cargo 安装后可选的短命令名：
ln -s "$(command -v codewhale)" "$(dirname "$(command -v codewhale)")/codew"

# Homebrew
brew tap Hmbown/deepseek-tui
brew install codewhale
```

当你想要隔离的运行时，也可以用 Docker：

```bash
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

从你希望它工作的仓库或目录启动 Codewhale：

```bash
codewhale
```

首次启动时，Codewhale 只询问本次安装仍然需要的决定：无法推断语言时询问语言，未配置可用路由时询问提供商，文件夹需要决定时询问工作区信任。提供商步骤包含明确的离线路由。就绪界面随后打开真正的编辑器，保留命令行中提供的任务，或为当前文件夹建议第一个任务。

此后所有可选内容都保持可用。用 `/setup` 打开渐进式设置与修复指南，用 `/settings` 打开完整键入式编辑器，想自定义内置工作约定时用 `/constitution`。本地化遥测选择只在工作区就绪后出现，不会阻塞编辑器。

DeepSeek 是默认提供商。如果你想在首次启动之前或之后配置它的 key，最直接的设置路径是：

```bash
codewhale auth set --provider deepseek
```

你也可以通过环境变量提供 key：

```bash
export DEEPSEEK_API_KEY="your-key"
codewhale
```

新的 Codewhale 配置存放在 `~/.codewhale/config.toml`。旧的 `~/.deepseek/config.toml` 文件仍受支持，供从旧名称迁移的用户使用。

用 `/constitution` 查看或更改常驻指引。设置完成后，运行一次 doctor 检查：

```bash
codewhale doctor
```

当你需要机器可读的报告用于提交 issue 时，用 JSON 形式：

```bash
codewhale doctor --json
```

两种形式默认都是离线的。
它们报告结构配置和字面上的未知/未探测凭证状态，不会加载工作区的 `.env` 凭据、打开 secret/OAuth 文件、探测密钥串、联系提供商或启动 MCP 服务器。只有有意需要该实时边界时，才使用 `--check-updates`、`--probe-api`、`--probe-local` 或 `--probe-mcp`。JSON 保持离线，不接受实时标志。

JSON 把凭据的 `source`（来源）与字面的 `availability`（可用性）分开报告。配置的环境、外部认证、OAuth、consent 和 secret-store 来源仍为 `not_probed`；它们的声明本身并不会让 Setup 或 fleet 就绪。只有结构上存在的字面配置值，或一条不需要凭据的路由，才能证明离线就绪。对于无法使用共享存储的路由上的旧版密钥存储哨兵（secret-store sentinel），会单独报告为 `secret_store_unavailable`/`unavailable`，而不是简单的"符合条件"或"未知"。

`doctor` 和 `doctor --json` 都还包含一项会话恢复诊断，它把旧会话文件名与当前存储对比，不读取会话内容，并报告以下之一： `isolated`、`no_legacy_sessions`、`migration_pending`、`migration_incomplete`、`migration_complete` 或 `scan_failed` 。
使用 `migration_pending` 或 `migration_incomplete` 作为提示，完成把会话从 `~/.deepseek` 迁移到 `~/.codewhale` 的工作——就是上面提到的旧路径迁移。显式设置 `CODEWHALE_HOME` 会抑制此环境检查。

下一步：[INSTALL.md](INSTALL.md) 涵盖各平台的安装路径，[CONFIGURATION.md](CONFIGURATION.md) 涵盖配置解析，[PROVIDERS.md](PROVIDERS.md) 涵盖提供商 ID 与凭据。

## 3. 你的第一个任务

从一个真实工作区里的只读任务开始：

```text
映射仓库结构，并告诉我 CLI 入口点在哪里。
```

然后要一份有重点的计划：

```text
我想为空的配置值添加一个小型验证。
检查相关代码，并在编辑任何内容之前提出最小的安全更改。
```

当你准备好做编辑时，把验收标准说具体：

```text
实施你提出的验证。
将更改范围限制在配置解析内，添加或更新最窄的测试，并运行相关的检查。
```

好的首批提示词（prompt）包含四个要素：

- 你想要的结果。
- 你关心的文件、功能或行为。
- 哪些不在范围内。
- 什么算"验证通过"。

例如：

```text
修复配置加载器中损坏的提供程序错误消息。
不要更改提供程序注册表。添加回归测试，并且只运行 config 包的测试。
```

如果你不确定 bug 在哪，直说：

```text
调查为什么 `codewhale doctor` 报告了错误的提供程序。
暂时不要编辑文件。返回可能的原因、证据和提议的补丁计划。
```

面对不熟悉的代码，让调查和实现分步进行时 Codewhale 表现最好。对于很小且充分理解的改动，一个单独的实现请求就够了。

下一步：[MODES.md](MODES.md) 讲解何时使用 Plan、Act 和 Operate。

## 4. 了解界面

交互式 TUI 有几个稳定的区域：

- 头部（Header）：当前会话、活动模型、模式和总体状态。
- 转录区（对话记录，Transcript）：对话、工具调用、命令输出摘要和模型回复。
- 输入区（Composer）：你在这里输入提示、斜杠命令和文件提及。
- 工作栏（Work bar）：转录区上方的一条（或可选的侧栏），承载活动目标、待办列表和子智能体。行会保持整个会话——已完成的工作显示为"已完成"而不是消失——点击某一行（或对它按 `Enter`）会打开它的详情。
- 状态与底部区域：实时活动、排队的后续动作和简短命令提示。

底部状态行可配置。运行 `/statusline` 选择哪些底部的片区可见，或在 `config.toml` 里设置 `[tui].status_items` 同时控制选择和顺序。
当前支持的键包括 `mode`、`model`、`cost`、`balance`（仅 DeepSeek / DeepSeekCN）、`status`、`agents`、`reasoning_replay`、`prefix_stability`、`cache`、`context_percent`、`git_branch`、`last_tool_elapsed`（保留）、`rate_limit`（保留）、`tokens` 和 `session_metrics`。
省略 `status_items` 以保持内置默认顺序；把它设为 `[]` 以隐藏可配置的片区。

`session_metrics`（默认开启）在阶段行上绘制会话指标条带：
`4 turns · 108 steps │ LLM 11m46s · Tool call 1m52s │ TTFT avg 1.5s · 120 tok/s │ Cache hit 99% │ Input 9.3M`
Turns 是用户回合；steps 是模型调用加工具调用；`LLM` 是模型调用墙钟时间的总和，`Tool call` 是工具墙钟时间的总和；`TTFT avg` 是到首个流式 token 的平均时间；`tok/s` 是提供商报告的输出 token 除以流式秒数；`Cache hit` 和 `Input` 是提供商报告的 token 类别。提供商或运行时证据尚未到达的单元格会被省略而不是估算，在窄行上，指标条会丢弃价值最低的组（先是 steps 和工具时间，然后是延迟、turns、LLM 时间），而不是截断某个数字。`/status` 打印未裁剪的完整行。

转录区（对话记录）就是审计轨迹。当 Codewhale 读文件、跑命令或改代码时，动作会出现在那里。如果某条命令失败，把可见的失败输出作为你下一条指令的一部分，而不是从头再来。

输入区接受普通提示和斜杠命令。输入 `/` 可以发现可用命令。想让模型专注于某个特定文件或目录而不是广泛搜索时，使用文件提及。

当一个回合跨越多个步骤时，工作栏很有用。它让目标、待办列表和智能体状态保持可见，同时转录区继续增长——包括在工作落定之后，这样你仍然可以打开看看发生了什么。

键盘快捷键因上下文、终端和平台而异。本指南不重复完整的快捷键目录，以免与 TUI 脱节。

下一步：[KEYBINDINGS.md](KEYBINDINGS.md) 是完整的快捷键参考。

## 5. 模式

Codewhale 有三种可见的 TUI 模式：

| 模式 | 用于 | 默认姿态 |
| --- | --- | --- |
| Plan | 改动前的探索、设计与审查 | 只读调查 |
| Act | 常规的多步编码工作 | 带审批门禁的工具使用 |
| Operate | 直接工作，外加并行或后台协调 | 工具遵循活动姿态；需要时委派 |

从 TUI 里用模式选择器切换模式：

```text
/mode
```

或直接切换：

```text
/mode plan
/mode act
/mode operate
```

Plan 模式是在陌生仓库里开始的最安全位置。它用于检查和决策，不做文件编辑。对于非平凡的工作，Plan 模式的确认提示可以显示有依据的计划工件（PlanArtifact）：目标、上下文、使用的来源、关键文件、约束、方法、验证计划、风险和交接说明。
当智能体（agent）使用富工件形态时，空章节也是可见的，所以你可以要求修订，而不是接受一份说明不足的计划。

Act 模式是大多数贡献工作的默认模式。它允许 Codewhale 读文件、跑检查、编辑文件，同时把有风险的动作留在审批门禁之后。

Operate 保持直接的工具面及其审批、沙箱、shell、ask 规则和仓库保护。它的区别在于编排重点：Codewhale 优先把独立、并行、后台或长时间运行的工作交给 fleet worker，而小型或紧密耦合的工作可以留在父进程中。

对于你信任的工作区，如果你确实希望动作不经审批提示就继续，可以用 `Shift+Tab` 选择 Full Access 权限姿态。不要在你不信任的仓库里使用 Full Access。

模式与模型路由是分开的。输入区空闲时 `Tab` 循环切换可见模式，而 `/model auto` 控制回合的模型与思考选择。

你也可以在 `/config` 里通过编辑审批模式来改变审批行为。只有当你理解它会如何改变工具执行时才使用它。

下一步：[MODES.md](MODES.md) 有完整的模式、审批和信任模式参考。

## 6. 斜杠命令

斜杠命令在输入区里输入。当你想要直接改变 Codewhale 状态，而不是用自然语言让模型去做时，它们很有用。

对首次用户常用的命令：

| 命令 | 用途 |
| --- | --- |
| `/mode` | 打开模式选择器，或用 `/mode agent` 切换 |
| `/model` | 选择模型，或用 `/model auto` |
| `/provider` | 选择活动的 API 提供商|
| `/fleet` | 打开当前所选 fleet 的成员花名册 |
| `/fleet saved` | 选择或切换已命名保存的 fleet |
| `/goal` | 设置一个智能体跨回合持续追求的持久目标；裸 `/goal` 显示进度 |
| `/workflow` | 把当前工作编排为 Workflow；`status`、`cancel`、`settings` 无需模型回合即可回答 |
| `/workflows` | 打开实时 Workflow 运行仪表盘：该工作区日志记录的每一次运行，含阶段、子项、进度和主机侧取消 |
| `/config` | 编辑运行时与提供商设置 |
| `/statusline` | 选择哪些底部状态芯片可见 |
| `/compact` | 压缩长上下文以回收 token 预算 |
| `/review` | 请求结构化的审查工作流 |
| `/memory` | 启用时检查或管理记忆 |
| `/mcp` | 配置或检查 MCP 服务器集成 |
| `/plugin` | 审查和管理默认禁用的本地插件包 |
| `/rc` | 把此确切会话交给已登录的 Codewhale 网页应用 |

工具箱命令直接输入即可搜索：`/models` 拉取实时端点 ID，`/modeldb` 打开内置模型参考，`/rlm` 把文件或一段文本加载进工作上下文，在会话剩余时间里保持可用。

想切离默认的 DeepSeek 路由时用 `/provider`。Provider ID、环境变量、模型默认值和能力说明都保留在提供商注册表文档里。

软自动多智能体工作：[AUTOMATIC_WORKFLOWS.md](../AUTOMATIC_WORKFLOWS.md)。

面向持久多 worker 工作的下一步：[FLEET_WORKFLOW_TUTORIAL.md](../FLEET_WORKFLOW_TUTORIAL.md) 带你走一遍 fleet 任务规格、监控和 Workflow 编写。

想让 Codewhale 每回合自己选模型和思考级别时，用 `/model auto`。当 DeepSeek 路由模型可用时，Auto 可以在脱敏清单中选取任何可运行的 provider/模型组合。该分类会把最新请求（上限 4,000 字符）加上最多六条最近上下文行的有界摘要（每条 900 字符）发送到 `DeepSeek / deepseek-v4-flash`。凭据、端点和提供商错误文本不会包含在清单里。没有该路由器时，Auto 使用本地的、感知提供商的启发式方法，不发送任何路由请求。如果分类尝试未通过验证或出错，Auto 回退到该启发式方法，同时把尝试过的分类器数据路径保留在回合回执中。

`/model` 选择器会说明哪条数据路径可用，并显示最后解析的路由。`Ctrl+O` 打开所选或当前回合的推理详情；`Ctrl+Alt+O`（或 `/turn inspect`）打开整回合的回合检查器（Turn Inspector），其模型路由区记录具体的 provider/模型、strong/fast 配对、所选层级、选择范围、路由原因，以及分类器是否收到了路由上下文。当你需要可重复的比较、严格的提供商边界或完全不要分类请求时，使用固定模型。

会话变长、模型开始承载太多历史记录时，用 `/compact`。压缩会用简洁的工作摘要换取原始转录细节。

本指南有意不列出每条命令。命令面比上手流程变化更频繁，你在会话里时，TUI 命令面板才是事实来源。

下一步：[CONFIGURATION.md](CONFIGURATION.md) 涵盖运行时设置，[MCP.md](MCP.md) 涵盖模型上下文协议（MCP，Model Context Protocol）集成。[PLUGIN_BUNDLES.md](../PLUGIN_BUNDLES.md) 涵盖默认禁用的包清单、能力审查和带命名空间的 Skill/MCP 激活边界。

## 7. 使用工具

Codewhale 的工具是结构化操作。模型不只是产出文字，还能调用工具来检查和改变工作区。

工具支撑的工作示例包括：

- 解释文件之前先读它。
- 提出重构之前先搜索调用点。
- 运行一条有重点的测试命令。
- 应用一个小补丁。
- 为并行调查打开一个子智能体。

工具使用由模式、审批和沙箱策略约束。确切行为取决于当前模式和配置，但基本规则很简单：只读探索用 Plan 开始，常规改动用 Act，Full Access 留给受信任的自动化。

工作区边界很重要。Codewhale 应该在你启动它的目录或你配置的工作区里工作。当任务应该留在仓库内时要说清楚：

```text
就检查并编辑此仓库下的文件。别触父目录和全局配置。
```

当命令需要网络、在工作区外写入或有风险的 shell 操作时，除非你配置了更宽松的行为，否则期待一个审批提示。

好的工具指令是具体的：

```text
运行覆盖此解析器更改的最窄测试。
如果失败，报告失败并在扩大测试范围之前停止。
```

避免在专注修复期间要求广泛的清理。较小的工具范围使对话记录更易于审查，最终的差异更易于合并。

下一步：[TOOL_SURFACE.md](../TOOL_SURFACE.md) 列出工具面，[SANDBOX.md](../SANDBOX.md) 讲解沙箱行为。

## 8. 子智能体与并行工作

子智能体是后台子代理。父会话给子代理一个专注的任务，收到一个 agent id，然后可以在子代理运行时继续工作。

主要的编排工具是：

- `agent`：带任务和角色启动一个专注的子代理。子代理在后台运行，返回一份紧凑回执加转录句柄。

你通常不需要直接调用这些工具。用自然语言请求并行工作：

```text
为 config 包打开一个只读探索器，为 TUI 提供商选择器打开另一个。让两者在规划修复之前返回文件引用和风险。
```

有用的角色包括：

| 角色 | 适合 |
| --- | --- |
| `general` | 多步任务；未指定角色时的默认值 |
| `explore` | 只读代码梳理 |
| `plan` | 设计与迁移规划 |
| `review` | 对已有改动的 bug 聚焦审查 |
| `implementer` | 规格明确的编辑 |
| `verifier` | 运行检查并报告通过/失败证据 |

子智能体在可以干净切分工作的时候最有用。不要为微小编辑使用它们，也不要让多个智能体同时写入相同文件。

### 长时间工作如何保持连贯

跨越多个回合的工作不依赖无限增长的聊天转录。这是普通 Agent 行为——不需要打开任何东西，也没有单独的工作流要学：

- 工作上下文在整个会话中保持加载。大段源材料和持久转录作为数据保存，智能体可以搜索和切片，有用的变量与导入跨回合存活。
- Workflow 组合独立的 `task(...)` 调用和并行扇出。
- `agent` 消息与后续动作直接协调活动的子代理。
- 目标（Goals）在工作期间保留持久目标。

`/rlm <file-or-text>` 把工作上下文指向一个特定文件或一段文本。历史上一度存在的动作形态 `rlm` 工具仍然注册着，只为了让旧会话能回放，并且刻意不教给新的模型回合。

Codewhale 还可以在 `.codewhale/harness/state.json` 维护一个小型项目级账本：有证据支撑的提示备注、可复用的子代理简报和 skill 路由提示。之后的回合会把它当作不受信任的补充指导接收，绝不是权威或可执行指令。读取它是自动的；添加或删除条目要走正常的审批回执。它和个人记忆是分开的，绝不能保存密钥、草稿转录或未经证实的说法。

下一步：[SUBAGENTS.md](SUBAGENTS.md) 涵盖角色、生命周期、并发和输出契约。

## 9. 技能（Skills）

技能是可复用的指令包。一个技能通常是 `SKILL.md` 文件，教 Codewhale 如何执行某个重复工作流、使用某类工具，或遵循某项项目约定。

当任务有可重复的流程时使用技能：

- 审查某一类 PR。
- 处理某种文档或电子表格格式。
- 遵循团队发布检查清单。
- 使用项目特定的记忆或 wiki 工作流。

在 TUI 里，`/skill <name>` 在可用时激活技能，裸 `/skills` 打开技能管理器（仅限自有清单，无网络）。用 `/skills <prefix>`、`/skills inspect`、`/skills --remote`、`/skills suggest <task>` 或 `/skills sync` 走文本/注册表路径。建议会对远程目录排序，但绝不安装或激活任何东西。命令面板也能把技能条目和普通斜杠命令一起展示。

知识贵广，技能贵精。它们应该告诉模型遵循什么工作流、收集什么证据、避免什么。它们不应该隐藏凭据或取代正常的仓库文档。

如果仓库有自己的指令，把请将其当作活动工作的一部分。编辑前先读本地指南，并让你的贡献保持在仓库约定之内。

下一步：见 [SKILLS.md](SKILLS.md) 了解管理器、所有权和来源规则；[CLAUDE_PLUGIN_COMPAT.md](../CLAUDE_PLUGIN_COMPAT.md) 了解 Claude Code 技能/插件兼容性；[CONFIGURATION.md](CONFIGURATION.md) 了解配置路径与项目权威。

## 10. 获取帮助

从 doctor 输出开始：

```bash
codewhale doctor
```

提交详细 issue 时用 JSON：

```bash
codewhale doctor --json
```

对于认证问题，用结构化的来源状态确认声明了什么。Doctor 刻意不检查环境、secret-store、钥匙串或 OAuth token 的值。当实时检查合适时，用 `codewhale doctor --probe-api` 选择加入（本地端点用 `--probe-local`）。

对于提供商问题，确认活动的提供商和模型：

```text
/provider
/model
```

会话又长又乱时，用 `/compact` 减轻上下文压力，或在同一工作区开一个新会话并总结你需要的东西。

报告 issue 时，请包含：

- Codewhale 版本。
- 安装方式。
- 操作系统和终端。
- 提供商和模型。
- 确切的命令或提示。
- 相关的 doctor 输出。
- 问题是否在新工作区里也出现。

不要把 API key、私有源码或密钥粘贴进公开 issue。

下一步：[OPERATIONS_RUNBOOK.md](../OPERATIONS_RUNBOOK.md) 有运维分诊与恢复步骤。

## 常见问题（FAQ）

### Codewhale 只支持 DeepSeek 吗？

DeepSeek 是默认且一等的路由，但 Codewhale 也支持其他托管和本地的 OpenAI 兼容供应商。用 `/provider` 或 `codewhale --provider <id>` 选择供应商。配置非默认路由时，请打开提供商注册表参考。

### 我应该先用哪个模式？

陌生代码用 Plan，常规实现用 Act，只有在你信任、可以接受自动执行的仓库里才用 Full Access。

### 为什么 Codewhale 运行命令前要问我？

审批是安全模型的一部分。Shell 命令、付费工具、写入以及预期工作区之外的动作都可能产生副作用。审批提示让你在让模型做有用工作的同时保持控制。

### 我如何在 macOS 上运行一个 Python 文件？

在包含该文件的文件夹里打开终端并运行：

```bash
python3 your_file.py
```

如果 macOS 提示 `python3` 缺失，从 [python.org](https://www.python.org/downloads/macos/) 或 Homebrew 安装 Python：

```bash
brew install python
```

在 Codewhale 里，让智能体检查文件并用 `python3 your_file.py` 运行它。如果脚本需要包，先在虚拟环境里安装：

```bash
python3 -m venv .venv
source .venv/bin/activate
python3 -m pip install -r requirements.txt
python3 your_file.py
```

### 我的配置存放在哪里？

新的 Codewhale 配置使用 `~/.codewhale/config.toml`。旧的 `~/.deepseek/config.toml` 为兼容性仍然受支持。当工作区配置存在时，项目覆盖也可能影响行为。

### 如何让成本可预测？

用 `/model auto` 做路由，需要严格配置时选择固定模型，并压缩长会话。对更大的任务，让 Codewhale 先规划再实现，这样你就不会把 token 花在错误的路线上。

### 如何继续之前的工作？

Codewhale 会保存会话。用 README 和模式指南里讲到的会话选择器或 resume/continue CLI 路径。对于有风险的实验，在改变方向前先分叉（fork）会话。

`/sessions` 选择器以当前工作区为范围启动，这样恢复会保持挂在打开的项目上。在选择器里按 `a` 显示所有工作区的会话，或在恢复某个特定 id 之前运行 `codewhale sessions` 列出所有已保存会话及其最后更新时间。

要从网页应用继续当前正在运行的会话，输入 `/rc` 或用 `codewhale rc` 启动。在系统浏览器里批准一次性代码。租赁期生效期间，浏览器拥有新的提示和审批，终端是可读的安全面。连接后，横幅和一条转录备注会显示实时会话链接（`https://app.codewhale.net/session?run=…`）；`/rc open` 在浏览器里打开它，`/rc link` 打印它。`/rc status` 显示归属，`/rc stop` 把它交回终端，interrupt 仍然可用。断开的连接会保持本地输入锁定，直到最后一个网页租赁过期，这样两个控制器永远不会竞争。从一个终端登记的每个文件夹共享同一个稳定的设备 id，因此网页应用每台机器列出一台电脑，而不是每个会话一台。

### 模型糊涂了，我该怎么办？

停下来，重新陈述目标、约束和当前证据。如果转录很长，用 `/compact`，或带简短交接开一个新会话。如果是运维问题，运行 `codewhale doctor` 并检查报告的配置与提供商状态。

### 项目规则应该放在提示里还是文件里？

持久性的项目规则用仓库文件，回合特定的意图用提示。如果某个工作流跨项目重复出现，考虑把它做成技能。

### Codewhale 能编辑当前仓库之外的文件吗？

这取决于工作区边界、沙箱设置、信任模式和审批策略。做贡献工作时，让指令保持在当前仓库范围内，除非你确实需要别的。

### 学完本指南后我该去哪？

读与你正在改动的东西相关的重点参考。对大多数用户，接下来的页面是安装、配置、提供商、模式、快捷键、工具和子智能体。

下一步：[INSTALL.md](INSTALL.md)、[CONFIGURATION.md](CONFIGURATION.md)、[PROVIDERS.md](PROVIDERS.md)、[MODES.md](MODES.md) 和 [TOOL_SURFACE.md](../TOOL_SURFACE.md)。
