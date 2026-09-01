# Fleet worker 与子代理兼容性

> 本文翻译自英文版 [SUBAGENTS.md](../SUBAGENTS.md)，与英文修订 `66816e6cc`（2026-08-16）同步。

Fleet 角色是面向用户的委派工作词汇：父代理通过 `agent` 启动一个专注的 `worker`、`scout`、`planner`、`reviewer`、`builder`、`verifier` 或 `consultant`，在 worker 运行期间拿回一个 `agent_id` 加上 transcript 句柄。内部运行时类型是 `FleetRole`（以前叫 `SubAgentType`）；旧的角色拼写（`general`、`explore`、`plan`、`review`、`implementer`、`oracle`、…）在 v0.9.x 期间仅作为持久化/反序列化兼容适配器被接受。新的提示和配置应使用 Fleet 名称。

从架构上讲，子代理不应成为第二种执行基质。持久化原语是 [`AGENT_RUNTIME.md`](AGENT_RUNTIME.md) 中描述的 fleet 支撑的 worker 运行：重试、终态、回执、工件引用、检查与重启行为都归属于那里。面向模型的启动器是单一的 `agent` 工具，detached 工作应收敛到与 Agent Fleet 相同的生命周期。

当前 `agent` 实现在此切换完成期间委托给持久化子代理运行时。它对于会话内的短期委派仍然有用。瞬态的 provider header/stream/超时失败会在子运行时内部先以退避方式重试，然后才把 worker 标记为 interrupted；如果重试预算耗尽，Codewhale 会保留一个检查点并返回延续句柄，而不是让父代理去猜测发生了什么。对于必须跨进程重启、休眠或远程执行的工作，优先选择 Fleet 或 Workflow 支撑的 fleet 运行。

子代理默认继承父代理的工具注册表，其中包括 `agent` 本身：子代理用 `with_full_agent_surface_options`（`crates/tui/src/tools/subagent/mod.rs:12164`）构建，因此它们可以递归。只有当深度预算耗尽时，`agent` 才会从子代理的目录中过滤掉——`can_spawn_child = !runtime.would_exceed_depth()`（`mod.rs:12145`），在 `mod.rs:12324` 和 `:12469` 强制执行。默认深度为 3（`DEFAULT_SPAWN_DEPTH`，`crates/config/src/lib.rs:1671`）时，子代理可以生成孙代理。已移除的 `agent_open`/`agent_eval`/`agent_close` 生命周期工具已从每个注册表中消失，父代理和子代理皆然。

`agent` 启动 detached 后台工作：取消父代理的回合会停止父代理的等待路径，但不会杀死已经打开的 child 运行。

本文档涵盖角色分类和当前兼容性控制。活动的编排面是 `agent`；参见 `crates/tui/src/prompts/text.rs`（`AGENT_MODE`）中的子代理指南以及行内工具描述。

## 角色分类

`agent` 上的 `type` 字段为子代理选择一种 Fleet 姿态（`agent_type` 作为兼容别名被接受）。每个角色都是对工作的一种独特立场——不只是标签不同。

## 维护者姿态

子代理帮助 Codewhale 更快地前进，但父代理仍然拥有维护者的决策权。使用子代理收集证据、审查补丁、运行验证，同时保持 [`AGENT_ETHOS.md`](../AGENT_ETHOS.md) 中的社区姿态：issue 是开放的接收入口，PR 门禁是审查负载控制，收割的工作需要明确的贡献者署名。

当子代理审查社区工作时，父代理在合并、收割、关闭或推迟之前，仍应检查 PR diff、关联 issue、测试和 CI。子代理的结果是一个工作集，而不是管家责任的替代品。

| 角色 | 姿态 | 可写？ | 联网？ | Shell 姿态 | 典型用途 |
|---|---|---|---|---|---|
| `worker` | 灵活；父代理说什么就做什么 | 是 | 是 | 是 | 默认角色；多步任务 |
| `scout` | 只读；快速摸清相关代码 | 否 | 是 | 只读（网络 + 有界验证） | "找到 `Foo` 的每个调用点；用 gh 检查这个 PR" |
| `planner` | 分析并产出策略 | 否 | 是 | 只读探针 | "设计迁移方案；不要执行" |
| `reviewer` | 带严重度评分的阅读与评审 | 否 | 是 | 只读（网络 + 有界验证） | "审计这个 PR 的 bug" |
| `builder` | 以最小改动落地某个具体变更 | 是 | 是 | 是 | "把 `bar.rs::Foo::bar` 重写为做 X" |
| `verifier` | 运行测试/验证并报告结果 | 否 | 是 | 测试导向 | "运行 cargo test --workspace，并报告" |
| `consultant` | 短期、高推理密度的咨询 | 否 | 是 | 无 | "这个设计我们漏掉了什么？" |
| `custom` | 显式的窄工具 allowlist | 继承 | 继承 | 继承 | 在父代理姿态上精选的工具 |

角色的默认值就是该角色*想要*的姿态，而父代理的有效姿态永远是天花板（子代理绝不会扩得比父代理更宽）。只读角色按意图扣留**工作区写入**；默认不拿走任何其他东西——每个角色都保留网络读取，`custom` 继承父代理的写入/网络/shell 姿态，并且只被它的显式工具列表或发起调用收窄。被聚焦的 worker 的头部会依据运行时自身的权限快照声明有效姿态（`scout · read-only · network · read-only shell`）。

**委派移动的是工作，绝不是权限**（#5426 的遏制答案）。只读角色委派给可写角色（scout → builder）是*工作容量*受支持的逃生舱——子代理自带其模型、路由和步骤预算——但子代理的权限被钳制在委派父代理的实时姿态上，而不是操作者的姿态上：scout 的 builder 子代理以只读落地，raw shell 和可变工具被拒绝，规范的 `Bash` 对它也被拒绝（只有有界检查角色保留分类的只读 shell）。因此通过委派来获得 shell 在机制上是无用的——scout 自带的受限 shell（`git -C … log`、`find … | head`、`npm view …`，分类器门控）是只读父代理唯一的 shell 路径。只读通过任何委派链都是传递的：钳制（`fleet/exact.rs` 中的 `ChildAuthority::clamp`）把每个字段与更窄的一侧求交，拒绝列表的并集意味着后代永远无法去掉祖先的限制，而 `inherit_disallowed_tools: false` 无法去掉姿态拒绝（`is_posture_denial`）。这一点由 `crates/tui/src/fleet/exact.rs` 测试中的 `a_read_only_parents_delegation_never_widens_authority` 钉死。

会话的**权限姿态**在每个子代理内部的应用方式与父代理回合完全一致：在 Auto-Review 下，同一个确定性底线和一次性模型守护者决定 worker 的被扣留调用（绝不是提示词；守护者不可用时拒绝，fail closed）；在 Ask 下，角色无法委派的被扣留调用会作为审批提示在父代理的 UI 中弹出，worker 可见地等待（`waiting for user`），或者在无法提示的主机上带着原因被拒绝；Full Access 仍然在不可绕过的安全底线上 fail closed。每一次没有人被提示的决策都是该 worker 转录中的一行备注（聚焦时可见）和一条审计日志记录。参见 `docs/MODES.md`。

每个角色的完整系统提示词位于 `crates/tui/src/tools/subagent/mod.rs`（搜索 `*_AGENT_INTRO`）。提示词前缀在子代理启动时自动加载；父代理的委派提示词成为第一个回合的用户消息。

## 上下文分叉

`agent` 默认全新启动：子代理拿到它的角色提示词加上你传入的任务。当子代理应从父代理当前的请求前缀继续时，使用 `fork_context: true`。（`fork_context` 不在对外公布的 v0.9.9 schema 中——它对兼容调用方保持解析接受，只读角色的自动分叉继续不变。）在分叉模式下，运行时会尽可能保持父代理 prefill/提示词前缀逐字节一致，追加一个结构化的状态快照，然后在尾部加上子代理角色指令和任务。这样既保留了 DeepSeek 前缀缓存的复用，又给了子代理做延续、审查、总结或压缩工作所需的上下文。

独立探索使用全新会话。当任务依赖父代理转录中已有的决策、文件、todo 或 plan 状态时，使用分叉会话。

分叉状态显示父代理的 To-do 快照——由 `todo_write` 写入的唯一的 Work 表面。子代理的 `<codewhale:fork_state>` 块携带由 `crates/tui/src/todo_snapshot.rs` 渲染的有界体，因此分叉是从父代理真实的进度位置继续，而不是一段转述。该 To-do 段在发起时解析，所以父代理回合内更早的 `todo_write` 会被包含进来。

**该列表只在发起那一刻显示一次，之后绝不会重新发送。** 没有哪个子代理请求会重新陈述 To-do 列表，父代理请求也不会。每个代理保留它自己的私有列表（#4810）；它对列表的了解来自自己 `todo_write` 调用返回的工具结果，这些结果是它自己转录中的普通消息。因此 worker 无法读取或写入父代理或兄弟代理的列表，分叉的子代理也无法修改它被交到手里的快照，或持续读取之后父代理的变化。

同一个私有列表就是子代理转录内卡片所显示的。一张委派卡片渲染**它自己**代理的 To-do 的有界投影——settled/total 计数、始终包含进行中的条目、最多三行，当界限省略其余部分时带一个显式的 `… +N more`——由 `card_todo_projection` 用与面向模型的主体相同的快照、优先级顺序和清洗器构建。卡片只消费 `agent_id` 与它匹配的信封，所以父代理的列表绝不会出现在子代理名下，也没有兄弟代理的列表会出现在另一个名下。没有声明任何工作的代理完全不显示 To-do 行，而不是显示占位任务；终态卡片保留它自己的代理实际发布的最后一个快照。扇出卡片保持圆点网格，不显示子代理 To-do：一张卡片后面有多个 worker 时，没有可以如实挂起单一列表的位置。只有当运行时已经把该子代理表示为它自己的委派卡片时，子代理 To-do 才会出现。

持久化的 task/Fleet 账本仍然拥有生命周期状态。`update_plan` 不再被模型触达：`model_visible()` 返回 `false`（`crates/tui/src/tools/plan.rs:408-413`），因此它从 API 工具列表中过滤掉，绝不会出现在子代理面前。它只用于重放更早的转录。以前放在那里的策略现在放进响应体，生命周期状态放进 `todo_write`。

## Worktree 隔离

对于并行的编辑通道，用 `worktree: true` 发起子代理。Codewhale 为那个子代理创建一个全新的 git worktree 和分支，从隔离的检出中运行子代理，并在返回的会话投影和 worker 记录中报告得到的 workspace/分支。默认分支是 `codex/agent-<name>-<id>`，检出位于父仓库旁边、`.codewhale-worktrees/` 之下，因此父检出保持干净。

隔离不是写入权限。纯提示词的 worker 以只读开始。写入者还要声明 `write_authority: "workspace_write"` 或 `"worktree_write"`，以及至少一个规范化的仓库相对 `write_roots`、`exact_files` 或 `coordination_contracts` 值。活跃的重叠共享声明会在变更前失败；真正隔离的 worktree 可以并行进行。

可选字段：

- `worktree_branch`：要创建的确切分支。
- `worktree_base`：要从中开分支的 git ref；默认为 `HEAD`。
- `worktree_path`：确切的检出路径。相对路径留在默认的兄弟目录 `.codewhale-worktrees/` 根下。

不要组合 `cwd` 与 `worktree`；`cwd` 仍是针对父工作区内已经存在的目录的手动逃生舱。

## 委派简报

父代理应该传递一份紧凑的简报，而不是一段松散的文字。使用结构化的 `dependencies` 和 `acceptance` 数组承载有界的前提事实与可观察检查；把聚焦的目标放在 `prompt` 里。不要复制原始父代理推理或无界的转录。

```
QUESTION:
SCOPE:
ALREADY_KNOWN:
EFFORT: quick | medium | thorough
STOP_CONDITION:
OUTPUT: VERDICT, EVIDENCE, GAPS, NEXT
```

`scout` 简报默认为快速的只读调查（不写，但网络触达和有界验证面可用于真正的侦察）。约 3-5 次工具调用足以完成快速探索：定位、搜索、读取决定性代码行，然后返回。除非证据与之矛盾，否则不要重复 `ALREADY_KNOWN` 的工作。review 和 verifier 简报可以花更多调用，但应在拿到决定性证据后停止。builder 和修复型简报应在扩展示范围之前或反复失败之后设置检查点，而不是设置一个很小的调用上限。

好的委派提示词示例：

```text
QUESTION: PR #3124 是否在 provider 路由周围引入了发布风险行为？
SCOPE: PR #3124 的 diff、关联 issue、provider 路由测试、docs/PROVIDERS.md。
ALREADY_KNOWN: 分支是 hunter/0.8.62-glm-subagents；workspace 版本保持 0.8.61。
EFFORT: medium
STOP_CONDITION: 拿到一个 BLOCKER/MAJOR 问题或足够证明没有 MAJOR+ 问题的证据后立即返回。
OUTPUT: VERDICT、带 file:line 引用或 PR 引用的 EVIDENCE、GAPS、NEXT。
```

```text
QUESTION: 子代理提示词在哪里组装？
SCOPE: crates/tui/src/prompts*、crates/tui/src/tools/subagent/*。
ALREADY_KNOWN: 面向模型的启动器只有 `agent`；不要去找已移除的生命周期工具。
EFFORT: quick
STOP_CONDITION: 找到提示词源文件和包装委派文本的函数后停止。
OUTPUT: VERDICT、EVIDENCE、GAPS、NEXT。
```

```text
QUESTION: 聚焦的 prompt/subagent 测试过滤器是否有效，如果无效会失败什么？
SCOPE: cargo test -p codewhale-tui --bin codewhale-tui --locked prompt；需要时加 subagent 过滤器。
ALREADY_KNOWN: 不要修复失败；记录确切的命令、退出码和第一条相关断言。
EFFORT: medium
STOP_CONDITION: 一次干净的 PASS 或一条可复现的失败断言（带命令证据）后停止。
OUTPUT: VERDICT、EVIDENCE、GAPS、NEXT。
```

### 何时选择哪个角色

- **`worker`** —— 当任务是"做完这一整件事"，而不是"去看"、"设计"或"验证"。这是正确的默认；只有当姿态重要时才改用更具体的角色。
- **`scout`** —— 当父代理在决定下一步之前需要证据。scout 便宜又快；对独立区域并行开 2-3 个。他们应该先定位：确认项目根目录，在不熟悉的树中阅读相关 `AGENTS.md`/`README.md` 指南，只搜索可能的作用范围，返回 `path:line-range` 证据而不是一篇叙述式导游。要用的角色名是 `scout`。
- **`planner`** —— 当父代理有目标但没有可执行的分解。planner 写工件（`todo_write` 条目、响应体里的策略），但不执行它们。
- **`reviewer`** —— 当已经有一个变更，父代理想要它被评分。reviewer 不打补丁——他们在发现里描述修复方案，这样如果判定是"修它"，父代理可以派一个 builder。
- **`builder`** —— 当变更已经被明确指定、只需要落地。builder 保持严格的范围：最小改动，不做顺手重构，交回前跑一次快速验证。
- **`verifier`** —— 当父代理需要测试套件或其他验证上的权威通过/失败结论。verifier 不修失败；他们记录失败的断言 + 栈，把修复候选放在 RISKS 下。
- **`consultant`** —— 当操作者想在更便宜的执行继续之前得到一个高杠杆的第二意见。consultant 读足够的材料来支撑一条建议，但不能写，也不能运行 shell 命令。`oracle` 和 `advisor` 只在加载更老的请求或持久化记录时被接受；新的提示词、回执和 UI 使用 `consultant`。
- **`custom`** —— 只有当父代理需要显式约束工具集时。通过 legacy/internal 子代理记录上的 `allowed_tools` 字段传 allowlist；面向模型的 `agent` 工具刻意保持公共 schema 很小。

### 别名

模型可以用多种方式拼写每个角色：

| 规范名 | 别名 |
|---|---|
| `worker` | `general`、`default`、`general-purpose` |
| `scout` | `explore`、`explorer`、`exploration` |
| `planner` | `plan`、`planning`、`awaiter` |
| `reviewer` | `review`、`code-review`、`code_review` |
| `builder` | `implementer`、`implement`、`implementation` |
| `verifier` | `verify`、`verification`、`validator`、`tester` |
| `consultant` | `oracle`、`advisor`（仅兼容输入） |
| `custom` | （无；需要显式的 `allowed_tools` 数组） |

所有匹配都不区分大小写。未知值会产生一个列出可接受集合的类型化错误，因此模型可以在下一回合自我纠正。

## 并发上限

默认最多 **64** 个子代理并发运行（`DEFAULT_MAX_SUBAGENTS`），可通过 `~/.codewhale/config.toml` 中的 `[subagents].max_concurrent` 配置，硬上限为 **128**（`MAX_SUBAGENTS`）。会话默认接受一个有界的队列，最多 **1024** 个运行中加排队中的子代理（`MAX_SUBAGENT_ADMISSION`，`crates/tui/src/config/subagent_limits.rs:21`），因此一个回合可以请求大范围扇出，让管理器排空它，而不会产生无界群体。

默认情况下每个被接受的子代理都可以立即启动——没有人为的节流。如果想要更温和的扇出，降低 `[subagents].launch_concurrency`（一次启动多少个直接子代理）；超过该限制的子代理会为启动槽位**排队**，而不是爆发式启动。`launch_concurrency` 默认为解析后的 `max_subagents` 上限。（v0.8.61 之前的 `interactive_max_launch` 键仍作为弃用别名被接受；两个都设置时新键生效。）

高扇出 Workflow 可以用 `[subagents] max_admitted`（别名：`max_total`、`admission_limit`）调节那个有界群体。该总量上限同时计入**运行中**和**排队中**的代理，而 `launch_concurrency` 保持瞬时执行有界。已完成/失败/取消的记录会保留供检查，但不占用准入槽位。丢失了 `task_handle` 的代理（例如跨进程重启）也不计入上限。

Provider 配置档可以让一个配置对直接 API 路由保持激进，同时对订阅或聚合路由保持温和。`[subagents.providers.<provider>]` 下的每个键在省略时都从 `[subagents]` 继承。Provider 键接受规范名（如 `deepseek`、`zai`、`openrouter`）以及别名（如用于 Z.ai 的 `glm`）：

```toml
[subagents]
# 没有配置档的 provider 的全局回退。
max_concurrent = 20
launch_concurrency = 20
max_admitted = 200
max_depth = 6
# 调用不带预算时的每个子代理运行预算（角色默认：60/120 回合）。
default_max_steps = 120
default_wall_time_secs = 1800
token_budget = 100000

[subagents.providers.deepseek]
# 直连 API key，有余地扇出。
max_concurrent = 20
launch_concurrency = 20
max_admitted = 200

[subagents.providers.glm]
# Z.ai / GLM 订阅式路由：保持压力紧凑。
max_concurrent = 4
launch_concurrency = 3
max_admitted = 12
max_depth = 2
api_timeout_secs = 180
heartbeat_timeout_secs = 240

[subagents.providers.openrouter]
max_concurrent = 5
launch_concurrency = 3
max_admitted = 20

[subagents.providers.anthropic]
max_concurrent = 3
launch_concurrency = 2
max_admitted = 12
```

使用 `/config subagents status` 查看全局值和当前 provider 解析后的扇出、深度与超时配置。

## 对外公布的 agent 工具字段（v0.9.9）

面向模型的 `agent` 工具 schema 正好公布 **12 个字段**（#5324、#5123）：

`action`、`prompt`、`type`、`profile`、`name`、`agent_id`、`message`、`until`、`detached`、`worktree`、`write_roots`、`resume_from`

外加按 action 区分的 `dependentSchemas` 树（`start` 需要 `prompt`；`message`/`followup` 需要目标和 `message`；`peek`/`interrupt`/`cancel` 需要目标）。schema 变更是钉死的提示词前缀的一部分，所以升级会在每个会话中重新填充一次 provider KV 前缀（docs/CACHE.md；在 v0.9.9 边界接受）。

**解析接受但未公布（兼容）。** 以下输入已从公布的 schema 中移除，但仍保持解析接受并按原样生效，因此已保存的转录、ACP/MCP 客户端和 Fleet 配置照旧重放——`token_budget` 已经遵循的正是同一个契约：

- 预算：`max_steps`、`wall_time_secs`、`max_depth`（参见[子代理预算](#子代理预算步数墙钟时间)了解默认值现在来自何处）
- 路由：`model`、`model_strength`、`thinking`（`profile` 钉死路由和思维层级；没有它时子代理继承操作者模型）
- workspace/隔离：`workspace_policy`、`write_authority`、`fork_context`、`cwd`、`worktree_path`、`worktree_branch`、`worktree_base`
- 发起契约：`deliberate`、`dependencies`、`acceptance`、`expected_artifact`、`exact_files`、`coordination_contracts`
- 生命周期附加：`timeout_secs`（wait）、`reason`（interrupt）、`include_archived`（status）以及 `token_budget`

权限绝不会变宽：#5426/#5435 的遏制钳制不变——`write_authority` 移到角色/profile，委派只能收窄继承的权限。

## 子代理预算（步数、墙钟时间）

每个子代理的运行预算不再是调用级 schema 字段（#5324）。它们按顺序来自：

1. 调用上一个显式的、解析接受的 `max_steps` / `wall_time_secs`（重放兼容），
2. 操作者默认 `[subagents] default_max_steps` 和 `[subagents] default_wall_time_secs`，
3. Fleet 角色默认：读取为主的角色（scout/planner/reviewer/verifier/consultant）为 **60** 个模型回合，builder/worker/custom 为 **120**（`WorkerRuntimeProfile::default_max_steps`），墙钟默认 **1800 秒**。

步数值钳制到 2000 回合的硬上限；墙钟值钳制到 1..=86400 秒。

## Token 预算调节器

设置 `[subagents].token_budget`，给每个根 `agent` 运行一个聚合 token 上限，由该子代理和它的所有后代共享。没有配置预算时行为不变。

`token_budget` **不是**面向模型的 `agent` schema 上的字段，它的缺席是刻意的——`crates/tui/src/tools/subagent/tests.rs:4260-4263` 断言了这一点，理由是"ad-hoc 子代理应该继承宽松的运行时预算；暴露可选上限会招来意外的微观管理"。解析器仍然接受该键（加上 `tokenBudget`/`max_tokens` 别名，`mod.rs:10620`），因此自己构造调用的 Workflow 形态调用方可以限定预算，但模型绝不会被告知该字段存在。改为通过 `[subagents].token_budget` 配置它。自 v0.9.9 起，它是上面更宽泛的"解析接受但未公布"兼容列表中的一项。

provider 报告的输入和输出 token 会在每个子代理模型调用完成时汇入 worker 记录。持久化的 `usage` 对象显示 worker 自己的总数，外加共享作用域的聚合 `budget_spent_tokens` 和 `budget_remaining_tokens`。一旦共享作用域耗尽，进一步的后代发起会带着可操作的消息被拒绝，而不是向一个花光的池子里再开更多代理。

## 各角色模型（#3018）

子代理可以运行在与父代理不同的模型上。两个配置面喂同一个覆盖映射（冲突时 `[subagents.models]` 键生效，键不区分大小写）：

```toml
[subagents]
default_model  = "deepseek-v4-flash"   # 每个角色的回退
worker_model   = "deepseek-v4-pro"     # worker
scout_model    = "deepseek-v4-flash"   # scout
planner_model  = "deepseek-v4-flash"   # planner
reviewer_model = "deepseek-v4-pro"     # reviewer
custom_model   = "deepseek-v4-pro"     # custom

[subagents.models]
# 自由形式的角色 → 模型映射；agent 接受的任何角色别名都可以。
builder = "deepseek-v4-pro"
```

v0.9.x 便利键 `explorer_model`、`awaiter_model` 和 `review_model` 仍作为弃用别名被接受，这样现有配置文件不会损坏。

模型 id 可以是**活跃 provider 接受的任何模型**——验证是 provider 感知的，发生在发起时而不是加载时。在官方 DeepSeek API 上只接受 DeepSeek id；其他每个 provider 都把 id 透传给 provider API，由它说了算。一个非 DeepSeek 示例：

```toml
provider = "moonshot"
model = "kimi-k2.7-code"

[subagents]
worker_model = "kimi-k2.6"
```

模型 id 应用到子代理路由时以同样方式验证；官方 DeepSeek API 上的非法 id 会让发起带着可接受 id 列表失败，而不是一个晦涩的 provider 400。

在 `/model auto` 下，子代理路由同样是 provider 感知的：有已知大/便宜配对的 provider（DeepSeek，以及 NVIDIA NIM、OpenRouter、Novita、SiliconFlow、SGLang、vLLM 上的托管 DeepSeek 路由）在配对之间路由；没有已知便宜档的 provider（如 Ollama、Moonshot）跳过网络路由器，把子代理留在会话模型上。

## 各 profile 的 provider 路由（#3965）

`[subagents.models]` 在活跃 provider 内部更换子代理模型。要把子代理钉到不同的 provider，使用 Fleet/AgentProfile，并通过 `profile` 把它传给面向模型的 `agent` 工具。profile 显式的 `provider` + `model` 字段胜过父会话路由；省略 `provider` 保留现有的继承行为。

示例：让父会话留在 DeepSeek，但把一个格式化子代理跑在本地 LM Studio 的 OpenAI 兼容端点上：

```toml
# ~/.codewhale/config.toml 或 workspace 配置
provider = "deepseek"

[providers.deepseek]
api_key = "YOUR_DEEPSEEK_KEY"

[providers.lm-studio]
kind = "openai-compatible"
base_url = "http://127.0.0.1:1234/v1"
api_key = "lm-studio"
model = "qwen-2.5-7b"
```

```toml
# .codewhale/agents/local-formatter.toml
id = "local-formatter"
role_hint = "formatter"
provider = "lm-studio"
model = "qwen-2.5-7b"
reasoning_effort = "off"

[instructions]
text = "使用小而本地的编辑。让格式化改动保持机械性。"
```

然后调用 `agent(profile: "local-formatter", prompt: "...")`。进程内子代理为 `lm-studio` 构建一个客户端；Fleet worker 把 `--provider lm-studio` 转发给 `codewhale exec`，它解析同一个 `[providers.lm-studio]` 表。未知或未配置的 provider id 会让发起失败，而不是悄悄回退到父 provider。

## 单步 API 超时（#1806、#1808）

每个子代理步骤把它的 DeepSeek `create_message` 调用包在一个单步超时里，这样单个卡住的请求不会无限期卡住父代理的完成唤醒通道。默认是 `600` 秒。超时的尝试以指数退避重试（最多 5 次重试），然后步骤带着保留的检查点中断。合法超过该时长的长思考子代理，例如 `agent` 后面沉重的 plan 或 review 工作，可以在 `~/.codewhale/config.toml` 中延长超时：

```toml
[subagents]
api_timeout_secs = 900  # 15 分钟；钳制到 1..=3600
```

值被钳制到 `1..=3600`。`0` 和 `unset` 保持 `600` 秒默认。

## 陈旧 agent 心跳（#2614）

运行中的代理还跟踪 manager 可见的进度。如果子代理在心跳窗口内停止发出进度，manager 会自动取消它、释放它的子代理槽位，并通过返回的转录句柄和持久化的 worker 记录保留可检查的取消记录。默认是 5 分钟（解析为至少比 `api_timeout_secs` 高 30 秒，因此在 600 秒默认 API 超时下是 630 秒）：

```toml
[subagents]
heartbeat_timeout_secs = 300  # 钳制到 30..=3600
```

有效心跳至少保持在 `api_timeout_secs` 之上 30 秒，因此一个配置的长模型请求不会在自己的请求超时触发之前被取消。

## 生命周期

每个打开的会话产生一条记录，按以下顺序推进：

```
Pending → Running → (Completed | Failed(reason) | Cancelled | Interrupted(reason))
```

当 manager 检测到一个 `Running` 代理的 task 句柄消失时触发 `Interrupted`——通常是在加载了 workspace 从 `.codewhale/state/subagents.v1.json` 持久化状态的进程重启之后。父代理可以用同样的委派打开一个替代会话，或者把它当作终态。

### 会话边界（#405）

每个 `SubAgentManager` 实例在构造时给自己分配一个全新的 `session_boot_id`。每个新会话用该 id 给代理盖章；workspace 状态文件记录它用于重启恢复。

工作条/状态投影默认聚焦当前会话的代理。不再运行的先前会话代理被视为归档记录，这样模型不会把陈旧的工作误认为活跃工作。这只是一条*先前会话*规则：在当前会话中完成的代理在会话剩余时间内保留它们的工作条行（安静完成），它们的详情仍然可以从那些行打开。

从 #405 之前的持久化状态文件加载的记录（没有 `session_boot_id` 字段）被归类为先前会话，因为 manager 无法把它们匹配到当前启动。

## 运行回执、后续消息与接管

每个兼容子代理在 `.codewhale/state/subagents.v1.json` 中有一条持久化的 worker 记录。在那些通道直接由 fleet 账本支撑之前，该记录是子代理通道当前运行账本切片：它存储 `run_id`、目标、角色/模型、workspace/分支、生命周期事件、工件引用、后续目标、接管目标、用量来源和验证来源。

`agent` 返回一个会话投影，这些字段位于顶层和 `worker_record` 内部。正常的父代理契约不是轮询：继续工作，在子代理完成时消费完成事件。如果需要审计细节，用 `handle_read` 检查返回的 `transcript_handle`。

Legacy 后续消息投递仅为旧转录和内部恢复保留。如果一条消息被投递，worker 记录会存储一个有界预览和时间戳。新的面向模型的流程应该在子代理的委派不再合适时打开一个替代 `agent`。

工件是符号引用。用 `handle_read` 处理返回的 `transcript_handle` 获取转录详情，除非 `verification.status` 指向一个单独的关卡或回执，否则把 `result_summary` 当作子代理自报。`usage.status` 在 provider 用量报告之前是 `unknown`；之后切换到 `reported`，或者当配置的共享 token 预算没有剩余 token 时切换到 `budget_exhausted`。

## 输出契约

非 scout 子代理按此顺序以五个 Markdown 标题结尾：

```
### SUMMARY    一段；你做了什么、发生了什么
### EVIDENCE   path:line-range 引用和关键发现；每条一个要点
### CHANGES    修改过的文件，带一行描述；只读则为 "None."
### RISKS      可能出什么问题 / 父代理应该复核什么
### BLOCKERS   什么阻止了你；干净完成则为 "None."
```

它们是 `### HEADING` 行，不是 `HEADING:` 标签，而且 `EVIDENCE` 在 `CHANGES` 之前。这个五标题契约是 `crates/tui/src/prompts/text.rs` 中的 `SUBAGENT_OUTPUT_FORMAT`。`crates/tui/src/prompts.rs` 中的 `prompt_documents_structured_subagent_briefs` 断言每个标题都符合它。

Scout 是例外（#5189 F5）：它们只以 `### SUMMARY` 和 `### EVIDENCE` 结尾（`crates/tui/src/prompts/text.rs` 中的 `SUBAGENT_SCOUT_OUTPUT_FORMAT`）。`crates/tui/src/tools/subagent/mod.rs` 中的 `FleetRole::system_prompt` 为 `FleetRole::Scout` 注入 scout 契约，为所有其他角色注入五标题契约。一个子代理测试钉死 scout 包含 `## Output contract (scout)` 且不包含 `### BLOCKERS`。

父代理把 `EVIDENCE` 当作下一回合的工作集来读，所以 scout 和 reviewer 在这里要精确。

## 记忆与 `remember` 工具（#489）

当记忆启用时（`[memory] enabled = true` 或 `DEEPSEEK_MEMORY=on`），子代理共享父代理的原生记忆存储。它们可以通过 `remember` 工具追加持久化备注——方便 scout 发现值得跨会话携带的项目约定，或 verifier 学到"这个测试是 flaky"。

`remember` 接受 `global` 或 `workspace` 的 `scope`（`crates/tui/src/tools/remember.rs:79-108`），并通过 `NativeMemoryStore` 写入 `~/.codewhale/memory/global/MEMORY.md` 或 `~/.codewhale/memory/workspace/<id>/MEMORY.md`。写入不走标准的写审批流程。legacy 单文件 `memory.md` 路径在 v0.9.4 移除（remember.rs:165）；完整布局参见 `docs/MEMORY.md`。

## 实现说明

- 源码：`crates/tui/src/tools/subagent/mod.rs`。
- 持久化状态：`<workspace>/.codewhale/state/subagents.v1.json`。Schema 版本 `1`（向前兼容——新可选字段用 `#[serde(default)]`）。
- worker 记录按时间修剪：已完成/失败/取消/中断的记录在用于已结束代理的同一个保留窗口后逐出（默认 1 小时，`COMPLETED_AGENT_RETENTION`）。运行中/启动中/等待中的记录被保留。256 条记录的硬上限仍然作为安全边界存在（#4217）。
- `SubAgentRuntime::background_runtime()` 从 `child_runtime()` 开始，但把回合作用域的 child token 替换为全新的取消 token，因此父回合取消不会停止 detached 后台会话。
- `is_running` 检查忽略 `task_handle` 为 `None` 的代理；这避免把持久化但 detached 的记录计入并发上限（#509）。
- `SharedSubAgentManager` 是 `Arc<RwLock<...>>`——读路径使用读锁，因此 `/agents` 和侧边栏投影不会在多代理扇出期间阻塞主循环（#510）。
