# Agent Fleet

> 本文翻译自英文版 [FLEET.md](../FLEET.md)，与英文修订 `fc23323c4`（2026-08-17）同步。

Agent Fleet 是面向持久化多 worker 运行的本地优先控制平面。它**不是**一个独立的执行引擎：fleet worker 就是一次由 fleet 启动并持久跟踪的无头 `codewhale exec` 运行。关于子代理、`exec` 与 fleet 如何汇聚到同一个持久运行时，请参阅 [AGENT_RUNTIME.md](../AGENT_RUNTIME.md)。在产品语言里，用户仍然可以"打开一个子代理"；在架构语言里，持久的嵌套工作应当是一个带 role 的 fleet-backed worker。

只要工作场景需要重试、睡眠/重启后存活、远程执行、回执（receipt）或有账本（ledger）的审计轨迹，就应该使用 Fleet 而不是短命的 `agent` 扇出。初始 CLI 表面如下：

关于结合 Fleet 任务规范与 Workflow 编排的引导式端到端监控演练，请参阅 [Fleet + Workflow Tutorial](../FLEET_WORKFLOW_TUTORIAL.md)。

```sh
codewhale fleet init
codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet inspect <worker-id>
codewhale fleet logs <worker-id>
codewhale fleet artifacts <worker-id>
codewhale fleet interrupt <worker-id>
codewhale fleet restart <worker-id>
codewhale fleet resume <run-id>
codewhale fleet stop --all
```

`codewhale fleet resume <run-id>` 是重启恢复命令：它回放 ledger，对任何进行中的、其 worker 已停止心跳的 lease 进行对账（在任务预算内重试，否则按告警策略失败并升级），然后打印恢复后的状态。它不会启动任何新工作，并且是幂等的，因此在 manager 退出、笔记本休眠或运行时重启之后运行它都是安全的。

Fleet 状态存储在工作区下的 `.codewhale/fleet.jsonl`。worker 日志与 adapter 日志存储在 `.codewhale/fleet/` 与 `.codewhale/fleet-host/` 下。

### 交互式与持久状态

`/fleet status` 与 `codewhale fleet status` 是同一命令在两个表面（surface）上的体现。两者都通过一个共享的控制面契约读取工作区持久的 `.codewhale/fleet.jsonl` ledger，并报告相同的 verb id（`fleet.status`）、读 vs 写权限、持久化范围与回执。当工作区没有 ledger 时，它们会用带类型的理由（`no_fleet_ledger`）说明这一点，而不是渲染出看似"一切正常"的空状态——而且两者都不会把读取 ledger 作为副作用去创建它。

当前交互会话的子代理是**另一组**对象，现在它们有自己的名字：

- `/fleet workers`（或 `/subagents`，或 `n`）显示附着在当前 TUI 会话上的子代理。它不读取持久 ledger。
- `/fleet list|status|interrupt|resume` 与 `codewhale fleet list|status|interrupt|resume` 作用于持久 ledger。
- `codewhale fleet restart <worker-id>` 仅限 CLI：它重新获取任务的 lease，然后驱动 manager 循环直至完成。`/fleet restart` 不会默默做一个更小的动作——它会报告 `surface_not_supported` 并指名 CLI 命令。

在 v0.9.2 之前，`/fleet status` 显示的是会话子代理。该语义已经移除；`/fleet workers` 取代了它。

这背后的契约——描述符、可用性理由、精确身份目标、回执、带类型的未知项与边界——记录在 [`docs/COMMAND_CONTROL_PLANE.md`](../COMMAND_CONTROL_PLANE.md)。

## 编写 agent 配置（`/fleet setup`）

`/fleet setup`（也可以是 `/fleet setup edit` / `new`）打开一个 TUI 内向导，用于编写可复用的 agent 团队配置。裸 `/fleet` 以及 `roster`/`roles`/`profiles`/`party` 别名打开名册（已保存的配置）。`/fleet workers` 打开当前会话的 worker 视图；`/subagents` 是该视图的兼容快捷方式。要查看持久的运行历史，请使用上文描述的 `/fleet status` 或 shell 命令 `codewhale fleet status`——它们是同一条命令。

该向导是渐进式的：你每次只做一个聚焦的选择——先是 **role**，然后是 **model**（`inherit`，或来自 *任何已配置 provider* 的具体模型，不限于父会话当前正在使用的那一个），然后是 **配置存放在哪里**，最后是对完整姿态（route、thinking、权限、工具与 review 策略）的 **review**。每一步的头部都显示"Saves to: …"——要么是你还需要做出的选择，要么是你做出选择后解析出的确切文件。在 review 步骤激活保存控件之前，什么都不会写入。

**Destination** 步骤是一个聚焦的两选项列表（方向键移动，Enter 或 Space 选择；Tab 从不改变目的地）：

- **This project** 写入 `<workspace>/.codewhale/agents/<role>.toml`。它只作用于当前项目，并优先于 id 相同的 Personal 配置。当会话禁用了项目配置（`--no-project-config`）或工作区文件夹不可用时，该选项会以该理由显示为禁用；向导绝不会自行回退到 Personal。
- **Personal** 写入 `$CODEWHALE_HOME/agents/<role>.toml`，在本机所有项目中可用，除非某个项目有 id 相同的自有配置。

对于高亮选项，该步骤会显示确切文件、保存将新建文件还是**替换现有文件**，以及对名册的优先级影响。review 步骤会在"Saves to"下重复这些事实，并按效果命名最终动作——**Save to this project**、**Save as Personal profile** 或 **Replace …**。替换现有文件需要在保存控件上再按一次 Enter。Tab / Shift+Tab（或 ←/→）在保存控件、**Change destination** 与 **Back** 之间移动焦点；`s` 是返回 Destination 步骤的次要快捷键。从 `/fleet` 重新打开已保存的成员时，会从磁盘上的内容开始：它的 route、thinking 级别，以及它保存时的范围。Thinking（`inherit`、`off`、`low`、`medium`、`high`、`max` 或 `auto`）在 review 步骤用 `t` 调整。

配置范围（profile scope）控制角色定义在何处可复用；它不会扩大正在运行的操作的权限。要协调几个相邻的仓库，请从它们的共享父目录启动 Codewhale，使该父目录成为工作区。显式的受信任外部路径或 Full Access 仍可改变工具能触及的范围；worker 继承活动中的信任与权限姿态，绝不继承配置的存储范围。

选择具体模型会显式固定其 provider：保存的配置同时记录 `model` 与 `provider` 字段，因此它命名的 route 不依赖于配置稍后加载时碰巧处于活动状态的 provider。在 review 步骤按 **Enter**（"start"）会在同一屏内联预览确切的首个配置 TOML；在你保存之前什么都不会写入。`provider` 字段可以是内置 provider id（如 `openrouter`），也可以是配置在 `[providers.<name>]` 下的用户命名 OpenAI 兼容 provider（如 `lm-studio`）；启动路径保留该 id，并在 provider 未配置时 fail closed。

自 v0.9.9 schema 精简（#5324, #5123）以来，配置也是模型面向的 `agent` 工具选择 route 的方式：对外公布的表面不再携带 `model` 或 `thinking`——子任务要么以 `profile` 运行（精确使用其保存的 route 与 thinking 级别），要么继承操作者的 model。已移除的字段对已保存的 transcript、ACP/MCP 客户端与 Fleet 配置仍然保持可解析；对外公布的 12 字段列表与兼容列表见 docs/SUBAGENTS.md。

当配置了 provider 时，review 步骤还会在显式的保存前预览门控之后提供模型辅助起草：

- 按 **`m`** 让第一个已配置 model 起草配置。草稿到达时已被净化且有界——权限保持在 **fleet floor**（无 shell、无信任、需要 approval），无论模型提议什么。
- **起草不等于保存。** 精确渲染的 TOML 预览会内联显示在 review 步骤（而不是单独的滚动查看器），因此只有按 **`g`** 或 **Enter** 保存（或再按 `m` 重新起草）才会真正保存。保存会把配置写入预览中显示的项目或个人范围。

## 命名：Modes、Workflow 与 Fleet

这些名字描述不同的层次，而非互斥的系统。Plan 与 Act 是日常工作的模式。Operate 接受普通消息，并在与 Act 相同的 approval、sandbox、shell、ask-rule 与仓库保护之下保留父级的正常工具表面。它倾向于为独立、并行、隔离或长时间运行的工作使用后台 Fleet worker，但并不要求每个可执行步骤都配一个 worker。Workflow 是一个可选的编排叠加层，用于需要排序、门控、共享预算、回放或确定性汇入（fan-in）的工作。

简短的公开口径如下：

- **Fleet** = 谁来做这项工作：已配置的 workers、roles、models、hosts 与信任边界。
- **Workflow** = 工作按什么顺序执行：phases、gates、budgets、replay 与 fan-in。
- **Lane** = 一个正在运行的 Workflow 实例及其实时进度。
- **Runtime** = Lane 在何处、如何执行：本地或远程进程、provider route、sandbox 与 API 边界。

- **Workflow** 是可重复的计划与面向用户的编排叠加层：一个决定接下来运行哪些 phase 和 agent 的脚本/IR，把中间结果挡在主对话之外，并且可以检查或重跑。一次 Workflow 运行应该有可见的进度视图和清晰的活动头部状态，而不是像一个隐藏的后台任务。
- **Fleet** 是持久的子代理配置与执行基质：slots、profiles、per-slot models、tool posture、本地/SSH hosts、trust policy、leases、heartbeats、logs、receipts 与 status APIs。
- **High fan-out** 是 Workflow 运行的一种行为，而不是独立系统：当一个 phase 需要同时很多 worker 时，Workflow 会把它们作为 Fleet-backed 运行（持久 workers、receipts、目标再派发）派发，而不是复活仅提示词的子代理扇出。
- **Fan-in 是显式的：** 当用户需要一个合并结果时，由 owner 聚合、验证并综合 worker 回执。独立任务可以各自完成；派发绝不等于完成。

UI 指引：保持主 transcript 平静。一次 Workflow 运行应显示为紧凑的进度卡片加上工作条行（transcript 上方的条带，或侧栏），包含 phase 名、worker 数、回执，以及为子 worker 准备的嵌套缩进。鲸鱼标记（whale mark）应克制地用作活动头部/状态信号；避免为每个 worker 重复堆砌 emoji 行。

## Exact Fleets 与 Reasoning Router

Exact Fleet 会在 Workflow 启动前冻结每个 worker 的 provider、model、reasoning 策略与权限上限。把它保存为工作区或 `$CODEWHALE_HOME` 下的 `fleets/<name>.toml`。模型无法在运行时替换这些指派：

```toml
name = "release"
schema = "exact"
schema_revision = 1
reasoning_router = "luna-low"

[[members]]
id = "implementer"
role = "builder"
provider = "zai"
model = "glm-5.2"
reasoning = "auto"
permissions = "read_write"

[[members]]
id = "advice"
role = "consultant"
provider = "openai"
model = "gpt-5.6"
reasoning = "high"
permissions = "read_only"
```

可选的 Reasoning Router 是一个可复用服务，不是 Fleet 成员。把配置保存在任一搜索根下的 `routers/<name>.toml`，并让任意数量的 Fleet 引用它：

```toml
name = "luna-low"
schema = "reasoning_router"
schema_revision = 1
provider = "openai"
model = "gpt-5.6-luna"
call_reasrning = "lrw"
```

在运行时，它只能为已经冻结的 worker route 选择 reasoning 级别。它不能改变 worker、provider、model、role、tools 或 permissions。Router 调用本身被限制在 `off` 或 `low`；更贵的值会被拒绝。手动选择的 worker reasoning 级别不会产生 Router 调用。Route 与 reasoning 回执会指名 worker model，并在使用 Router 时给出 Router 的确切 provider/model，让操作者看到哪个 model 干了哪份活。如果相同的裸 Router 或 Fleet 名字在两个根里都存在，请用 `workspace/<name>` 或 `codewhale_home/<name>` 限定它，而不是依赖遮蔽（shadowing）。

每个成员的 `permissions` 预设是一个**上限**，绝不是授权：它会与会话的实时姿态取交集，因此只读会话里的 `read_write` 成员以只读方式运行。这个交集成为子任务真实的工具包——`permissions = "none"` 让它一个工具都没有，而没有网络工具的成员会失去每一个可触达的网络表面——`web.run`、`fetch_url`、`web_search`、`github`、`mcp*` 系列以及可触达的 `rlm` 动作——而不是仅仅在调用时被拒绝。唯一刻意的例外是规范 `Web` 家族的 `search`/`fetch` 动作：它们是只读成员有权使用的只读 web 表面（与普通 scout 对等），因此网络被禁的成员恰好保留 `Web {search, fetch}`。它在事实上保持只读，而不仅仅是名义上：按 URL 寻址的 `fetch` 在派发时被拒绝，表面上没有任何其他东西被授予。不能写入的成员会失去可变的文件工具，并且——当它保留了 `shell = "full"` 以便运行检查时——也会失去原始 shell，只保留它为之而存在的有界验证工具（`run_tests`、`run_verifiers`）：任意 shell 命令和 `write_file` 一样确定无疑地改变工作区，所以保留它会令 `write = false` 不成立。那个验证表面只在它的*默认*形式下有界，因此无界参数会随 shell 一起消失：被拒绝写入的成员可以运行内置 gate，但不能用显式 `commands` 数组运行 `run_verifiers`，也不能用原始 `args` 字符串运行 `run_tests`——这两者都会启动操作者提供的程序，是换了名字的原始 shell。

这些拒绝中有两项比工具名更窄，因为有两个工具触及了它们名字之外的地方。`rlm` 通过在进程*内部*、以它自己的名字调用 fetch 工具来加载 `url`，而 `rlm` 的 `eval` 动作针对活内核运行 Python——套接字和文件系统都触及。因此该家族**按动作**被拒绝，而不是整体拒绝：

- **No network tool** 移除 `rlm_open` 与 `rlm_eval`，无论是旧别名拼写还是 `rlm {action: ...}` 拼写。这刻意比它所保护的能力更窄：`rlm_open` 从*输入字段*（`file_path`、`content`、`url`、`session_object`）而不是从动作名选择来源，而动作-策略接缝解析的是名字而不是字段形状——它无法在工具运行前证明来源是本地的。与其留下一个 URL 形状的漏洞，网络被禁的成员会完全失去 RLM 加载，**包括纯粹本地的 `file_path` 形式**，只保留有界的元数据动作（`session_objects`、`configure`、`close`）。
- **No write** 只移除 `rlm_eval`。把大型本地文件加载进内核并读取是分析，不是变更，因此家族其余部分保留。

在这些名字之外，只要任何工具被交给一个携带 URL 的字段（`url`、`urls`、`endpoint`、`target`、…）并持有 `http`/`https`/`ws`/`wss`/`ftp` 地址，网络被禁的成员就会在调用时被拒绝。出现在文件*内容*或搜索模式里的 URL 是数据而非目的地，不受影响。

成员的 `role` 在该 role 已经适合上限内部时选择 worker 姿态（与系统提示）——`reviewer`、`verifier`、`consultant`、`planner`——而像 `auditor` 这样的领域特定 role 会回退到上限允许的最窄姿态。内置 role 默认值只扣留该 role 本意要扣留的东西：只读 role 从不写工作区，每个 role 保留网络读取，`planner` 可以运行只读 shell 探测，而 `builder`/`worker`/`custom` 把父级的有效写入/网络/shell 姿态继承为它们的上限（见 `docs/SUBAGENTS.md` 中的 role 表）。会话的权限姿态——Ask、Auto-Review、Full Access——随后以与门控父级完全相同的方式门控每个 worker 调用（`docs/MODES.md`，"Children"）。任务不能覆盖其中任何一项：`model`、`thinking`、`subagent_type`、`allowed_tools` 与 `write_authority` 在 exact Fleet 上会被拒绝，而不是被静默忽略。

Reasoning 回执记录被请求的级别*以及*实际向 provider 请求的级别。只要 route 无法表达被请求的级别，这两者就会不同——CodeWhale 的 route 归一化器在大多数 route 上会把请求的 `low` 发送为 `high`，而 Z.AI 的 GLM route 只能表达 thinking 开/关——因此回执报告真实请求，而不是被选中的标签。调用实际携带的值由该 route 自己的归一化器拼写，而不是由级别标签决定：OpenAI Codex route 被请求 `xhigh` 而不是 `max`，并且根本无法被请求 `off`。

回执还把成员的**语义角色**与它的**权限姿态**分开。`member_role` 是操作者命名的、gate 所依据的东西；`posture_role`（仅当二者不同时才出现）是收敛后的上限所允许的内置工具表面——因此名为 `auditor` 的成员在 `scout` 姿态下运行时会显示为 `auditor` 而按 `scout` 执行，这两个事实互不替代。

Workflow 启动时对任何可以在本地判定的事情 fail closed：无法解析的 provider 或 model、缺失的凭据、无法为成员 route 构建的客户端，或没有可用 Reasoning Router 的 `auto` 成员。按任务验证——那些 spawn 边界本来就会拒绝的东西，特别是没有声明 `write_roots`/`exact_files`/`coordination_contracts` 的可写成员——会在调用 Router 之前检查，因此无效任务永远不会消耗一次路由请求。如果 spawn 在 Router 决策*之后*失败，回执仍会被记录：token 已经花了，任何跨 provider 披露也已经发生。

## Manager 拥有的 Workflow fan-in

当并行工作必须返回一个合并答案时，使用 manager 拥有的 Workflow，而不是扁平的 `agent` 扇出：

1. **指定一个 manager**（操作者或 workflow 编排器）。
2. 通过 `workflow`（`task()`、`parallel()`、`pipeline()`、`phase()`）或一个拥有这些子任务的单一 manager 会话**扇出**子任务。
3. **等待**子任务回执或完成事件。
4. 在把承载结论的主张当作事实之前，**聚合并验证**它们。
5. **综合**出一个操作者可以依赖的结果。

裸 `agent` 扇出只适合独立的、发射后不管（fire-and-forget）的工作，这类工作不需要单一的 fan-in 结果。如果结果必须合并、比较或验证，请经由 `workflow` 路由，让 manager 拥有 fan-in。

## Workflow on Fleet

预期的高能力路径是 agent 编写的。当主 agent 判定一项任务需要的持久协调超过逐轮子代理调用时，它会起草一份 Workflow 脚本/IR，按活动权限模式呈现运行计划，运行时把它编译成带类型的 Fleet 工作。

Fleet 仍然是子代理配置表面。它拥有 slot 数量、role profiles、已保存的 route 固定或继承、tool posture、启动并发与 ledger。Workflow 只拥有编排计划：branch、sequence、loop、expand、review 与 reduce 决策。Workflow 脚本绝不能直接获得 shell、文件系统、网络、provider 秘密、取消或 TUI 权限；workers 作为 `codewhale exec` 进程执行真实工作。

默认的 Workflow-to-Fleet 验证刻意有界：

- 每次 Workflow 运行最多 1,000 个 worker agent；
- 同时最多 16 个存活 worker agent；更大的群体在宿主的每次运行并发门控上排队（阻塞），直到一个存活槽释放，然后经 Fleet 路由；
- 最多 8 层递归 Fleet ring 作为选择加入的上限（默认用户配置：3）；
- 只允许有界循环（必须 `max_iterations`）；
- 只允许有界动态扩展（必须 `max_children` 加一个模板）。

这些是群体上限，不是要求一次全部启动。1,000 agent 的 Workflow 仍应流经已配置的 Fleet worker 池。
推荐的模型布局，例如 DeepSeek Pro 编排器搭配第一层 Flash workers、更外层更便宜的 workers，只是预设。每个 slot 都可以继承活动 model 或携带显式 model 覆盖。继承是字面的：你在 `/model` 中选择的 model 就是 **operator**（`/fleet roster` 中固定的第一行），任何任务规范与名册配置都没有固定 model 的 worker 都会运行在该会话 model 上。任务级 `model` 与配置 `model` 覆盖仍然优先；route 回执记录哪个来源生效（`task.model`、`agent_profile.model` 或 `run.model`）。

设置 UI 应把它渲染为一个可展开的网格：一个编排器加上少量可见的子代理槽，Right/Enter 下钻到某个槽的下一层递归 ring，而不是试图一次显示整棵树。

## Task Spec

`codewhale fleet run` 接受 JSON 或 TOML。一个最小 JSON 规范：

```json
{
  "name": "local smoke",
  "tasks": [
    {
      "id": "lint",
      "name": "Lint",
      "instructions": "Run the lint check and report failures.",
      "expected_artifacts": ["log"]
    }
  ]
}
```

workers 是可选的。如果省略，Codewhale 会创建本地 worker 槽，最多 `--max-workers` 个。

任务规范在 Rust 中带类型，并保持验证数据与 worker transcript 分离。一个任务可以声明：

- `id`、`name`、`description`、`objective` 与 `instructions`
- `worker` role、tool profile、tools 与必需 capabilities
- `workspace` 根、必需文件、可写路径与环境 allowlist
- `input_files`、额外的 `context`、`budget`、`timeout_seconds` 与 `retry_policy`
- `expected_artifacts`、`scorer`、`tags` 与自由格式 `metadata`

workers 在 `.codewhale/fleet/` 下写有界的 artifact 文件，ledger 只记录 artifact 引用：kind、path、checksum、MIME type 与 size。回执记录 `pass`、`fail`、`partial`、`skip` 或 `timeout`；失败回执还可能把来源标记为 `transport`、`task` 或 `verifier`。`codewhale fleet status` 单独呈现这些失败来源计数。

确定性的内置 scorer 是 `exit_code`、`file_exists`、`regex_match` 与 `json_path`。规范还可以声明 `command`、`code_whale_verifier_prompt` 或 `manual`；这些会记录部分回执，直到显式的 verifier 通过完成。

### 使用 Role 预设

任务可以引用 role 名，fleet manager 会从 role 注册表填入默认值。内置 role（`smoke-runner`、`reviewer`、`builder`、`read-only`）始终可用；你也可以在 `[fleet.roles]` 里定义自己的。

```json
{
  "name": "smoke check",
  "tasks": [
    {
      "id": "lint",
      "name": "Lint check",
      "instructions": "Run lint and report failures.",
      "worker": { "role": "smoke-runner" },
      "expected_artifacts": ["log"]
    }
  ]
}
```

任务继承该 role 的 tool profile、budget 与 timeout。你可以在任务规范中覆盖任何字段：

```json
{
  "id": "deep-review",
  "name": "Deep review",
  "instructions": "Review the entire crate for soundness issues.",
  "worker": {
    "role": "reviewer",
    "tools": ["cargo", "rg", "git"],
    "capabilities": ["rust"]
  },
  "input_files": ["crates/**/*.rs"],
  "budget": { "max_tokens": 32000 },
  "expected_artifacts": ["log", "report"],
  "scorer": { "kind": "regex_match", "path": ".codewhale/fleet/report.md", "pattern": "finding|all clear" }
}
```

### 多任务运行示例

一次 fleet 运行可以并行派发几个独立任务：

```json
{
  "name": "CI gate",
  "tasks": [
    {
      "id": "check",
      "name": "Compile check",
      "instructions": "Run cargo check --workspace and report errors.",
      "worker": { "role": "builder" },
      "expected_artifacts": ["log"],
      "scorer": { "kind": "exit_code" }
    },
    {
      "id": "clippy",
      "name": "Clippy lint",
      "instructions": "Run cargo clippy --workspace and report warnings.",
      "worker": { "role": "reviewer", "tools": ["cargo", "cargo-clippy"] },
      "expected_artifacts": ["log"],
      "scorer": { "kind": "exit_code" }
    },
    {
      "id": "security",
      "name": "Secret audit",
      "instructions": "Search for plaintext secrets and report any matches.",
      "worker": { "role": "read-only", "tools": ["rg"] },
      "input_files": ["crates/**/*.rs"],
      "expected_artifacts": ["log", "report"],
      "retry_policy": { "max_attempts": 1 }
    }
  ]
}
```

## 告警

Fleet 告警默认关闭。调用方必须先提供已启用的告警配置，才会发送任何东西。Route 匹配带类型的 fleet 事件类别，而不是日志字符串：

- `stale`
- `restart_exhausted`
- `needs_human`
- `budget_exceeded`
- `verifier_failed`
- `run_completed`

Adapter 配置存储环境变量名，而不是秘密值。发送时代码从环境或未来的 secrets provider 解析这些名字。Ledger 记录只存储审计标签，如 `slack`、`webhook` 或 `pagerduty`；持久化在 ledger 中的任务规范会脱敏 webhook URL 与路由键。

示例告警配置形状：

```json
{
  "enabled": true,
  "dry_run": true,
  "routes": [
    {
      "events": ["stale", "restart_exhausted", "verifier_failed"],
      "adapter": "ops-slack"
    },
    {
      "events": ["restart_exhausted"],
      "adapter": "pager"
    }
  ],
  "adapters": {
    "ops-slack": {
      "kind": "slack",
      "webhook_env": "CODEWHALE_FLEET_SLACK_WEBHOOK",
      "channel": "#codewhale-fleet"
    },
    "pager": {
      "kind": "pager_duty",
      "routing_key_env": "CODEWHALE_FLEET_PAGERDUTY_ROUTING_KEY",
      "severity": "critical"
    }
  }
}
```

使用 dry-run 检查脱敏后的 adapter payload 而不发送：

```sh
codewhale fleet alert-dry-run \
  --event stale \
  --run-id fleet-demo \
  --worker-id fleet-demo-local-1 \
  --task-id release-triage \
  --reason "worker heartbeat stale since 2026-06-13T02:00:00Z" \
  --adapter slack
```

payload 包含 run id、worker id、task id、status、简短 reason，以及诸如 `codewhale fleet status` 与 `codewhale fleet inspect <worker-id>` 的安全检查命令。端点、webhook 秘密与 PagerDuty 路由键显示为 `<redacted:env:...>`。

## 状态表面

`codewhale fleet status` 显示 queued、running、completed、partial、failed、restarted、escalated、cancelled、stale 以及 verifier/transport 失败来源的紧凑计数。`inspect` 显示 worker 状态以及当前任务 objective、role、host、heartbeat、最新事件、artifact 引用、最新错误与告警状态。`logs` 打印有界日志 artifact 内容，`artifacts` 列出 artifact 引用而不内嵌大型 payload。

Runtime API 在现有运行时认证中间件背后暴露同样的 ledger-backed 投影：

```text
GET  /v1/fleet/runs
GET  /v1/fleet/runs/{run_id}
GET  /v1/fleet/runs/{run_id}/workers
GET  /v1/fleet/workers/{worker_id}
POST /v1/fleet/workers/{worker_id}/interrupt
POST /v1/fleet/workers/{worker_id}/restart
POST /v1/fleet/runs/{run_id}/stop
```

动作端点调用与 CLI 相同的 manager 控件，并把它们的决策记录在 fleet ledger 中。

## Manager-Agent 运行手册

Manager agent 应把 Fleet 操作当作带类型的、有 ledger 的控制面工作。从 `codewhale fleet status` 开始，然后用 `codewhale fleet inspect <worker-id>`、`logs` 与 `artifacts` 检查一次运行或一个 worker。只有当带类型的 CLI/API 表面无法提供所需证据时，才直接读取 `.codewhale/fleet.jsonl`、宿主日志或远程文件。

在采取行动前先对 worker 分类：

- `transient failure`（瞬时失败）：心跳过期、宿主超时、传输被中断、可重试的 provider/网络错误，或一个在不改动任务的情况下合理可能恢复的 adapter 状态。
- `task failure`（任务失败）：worker 完成了，但产生了错误结果、领域失败、缺少必需 artifact，或显式的任务级错误。
- `verifier failure`（verifier 失败）：worker 结果存在，但 scorer/verifier 失败、超时，或与回执不一致。
- `needs-human`：缺少权限、秘密请求、破坏性操作、反复的 restart 耗尽、含糊的产品决策，或 manager 无法从带类型 artifact 解决的冲突证据。

选择一个带类型的动作：

- 仅当失败是瞬时的、重试预算还有剩余、任务幂等或可安全重试、且不涉及权限或秘密边界时，才重启 worker：`codewhale fleet restart <worker-id>`。
- 仅当当前任务继续下去不安全或操作者明确要求取消时，才中断或停止：`codewhale fleet interrupt <worker-id>` 或 `codewhale fleet stop --all`。
- 默认不要重启纯粹的任务失败；保留 artifact 并把回执交给任务 owner，除非任务规范说明重试可以产生新证据。
- 对于 verifier 失败，先检查 scorer 输入与 artifact 引用。如果无法通过带类型的 fleet 动作修正 verifier，升级给人工 review。
- 对于 `needs-human`，起草升级内容而不是直接发送，除非告警配置明确授权发送。

安全的 Slack 或 PagerDuty 草稿：

```text
Codewhale fleet needs attention
Run: <run-id>
Worker: <worker-id>
Task: <task-id or unknown>
Classification: <transient failure | task failure | verifier failure | needs-human>
Reason: <one sentence, no secrets>
Latest typed evidence: codewhale fleet inspect <worker-id>; codewhale fleet artifacts <worker-id>
Safe log excerpt: <3 lines max or "see artifact <ref>">
Requested decision: <restart approval | verifier review | task owner review | permission decision>
```

运行后总结应包括 run id、已检查的 workers、分类、已采取或已起草的带类型动作、预期 ledger 影响、已审查的 artifact 引用与下一个 owner。保持总结有界；链接 artifact 引用而不是复制完整日志或 transcript。

捆绑的 `fleet-manager` skill 为 manager agent 镜像了这本运行手册。它是第一方系统 skill，在系统 skill 安装或刷新后应能通过常规 skill 注册表发现。

## 宿主 Adapter

宿主 adapter 边界支持本地子进程与显式 SSH workers。Adapters 暴露相同的操作：start、read status、read bounded logs、interrupt、restart、stop 与 cleanup。

本地 worker 作为 stdin 关闭、stdout/stderr 写入有界 fleet 宿主日志的子进程运行。它们只继承一个小的安全基础环境，如 `PATH` 与显式 allowlist 的变量。

SSH worker 通过系统 `ssh` 客户端以 `BatchMode=yes` 与有界连接超时运行。远程环境变量通过 OpenSSH `SendEnv` 发送；值不会嵌入本地 ssh argv 或 fleet 日志。

示例 SSH worker 规范：

```json
{
  "id": "builder-1",
  "name": "Builder 1",
  "host": {
    "kind": "ssh",
    "host": "builder.example.com",
    "user": "codewhale",
    "port": 22,
    "identity": "~/.ssh/codewhale_fleet",
    "working_directory": "/srv/codewhale/work",
    "env_allowlist": ["CODEWHALE_PROFILE"],
    "codewhale_binary": "/usr/local/bin/codewhale"
  },
  "capabilities": ["local", "linux", "tests"],
  "max_concurrent_tasks": 1
}
```

默认值刻意保守：

- 不启用托管控平面或云供给；
- SSH 要求显式的 host、working directory 与 Codewhale 二进制路径；
- 类似秘密的环境名，如 `TOKEN`、`SECRET`、`PASSWORD`、`API_KEY` 与 `PRIVATE_KEY`，会被 adapter allowlist 拒绝；
- 秘密应留在 Codewhale 配置 provider 或远程宿主配置中，而不是任务说明、argv 或 fleet 日志里。

## 安全与信任边界

Agent Fleet 强制一个把 worker 分成四个层级的信任级别模型。信任级别决定 worker 能访问什么（秘密、网络、工作区写入），以及它在被授予这些特权之前必须如何证明身份。

### 信任级别

| 级别 | 访问 | 需要 |
|-------|--------|----------|
| `sandbox` | 无网络、无秘密，只写入 `.codewhale/fleet/` | 无——新 worker 的默认 |
| `local` | 工作区读取、受门控的写入、已配置的秘密 | 本地进程（相同 uid） |
| `remote-verified` | 网络访问、有界能力授权、已配置的秘密 | SSH 宿主密钥验证或等效证明 |
| `operator` | 全部秘密的完全访问、不受限写入、任何动作 | 操作者拥有的机器 |

默认信任级别是 `sandbox`。操作者必须通过安全策略显式提升 SSH 或容器 worker 的信任。

### 安全策略

fleet 运行可以携带可选的 `security_policy` 块，定义默认信任级别、workers 可以解析哪些秘密、授予什么能力，以及最大信任级别的上限：

```json
{
  "security_policy": {
    "default_trust_level": "sandbox",
    "allowed_secrets": [
      {"key": "GH_TOKEN", "source": "env"},
      {"key": "CODEWHALE_API_KEY", "source": "keyring"}
    ],
    "capability_grants": [
      {
        "capability": "network",
        "scope": "github.com",
        "reason": "PR review needs GitHub API access"
      }
    ],
    "max_trust_level": "remote_verified",
    "require_identity_verification": true
  }
}
```

当运行没有显式 `security_policy` 时，workers 继承保守默认值：`sandbox` 信任、无秘密、无能力授权、无身份验证要求。

### 秘密引用

秘密从不以明文存储在任务规范、告警配置或 worker 定义中。相反，每个秘密都是一个 `FleetSecretRef`——一个键名加一个可选来源提示，告诉 fleet manager 在哪里解析该值：

```json
{"key": "GH_TOKEN", "source": "env"}
```

支持的来源：
- `"env"` — 从进程环境变量解析
- `"keyring"` — 从操作系统 keyring 解析（macOS Keychain、Windows 凭据管理器、Linux Secret Service）
- `"file"` — 从 `~/.codewhale/secrets/` 解析
- 缺省 — 按默认顺序尝试所有来源（先 store，再 env）

Secret 引用在日志与 ledger 条目中一律脱敏：`<secret:env.GH_TOKEN>`。

### Worker 认证

workers 用四种方法之一向 fleet manager 认证：

- **None** — 共享相同 uid 的本地 worker（默认）
- **SSH key** — 可选的宿主密钥指纹固定与 known-hosts 验证。`host_key_fingerprint` 字段（SHA256:...）固定预期服务器密钥，防止首次连接时的 MITM 攻击。
- **Token** — 从 `FleetSecretRef` 解析的 bearer token，适用于 fleet 代理后的远程 worker。
- **mTLS** — 带客户端证书与秘密支撑私钥的相互 TLS。

SSH workers 在生产中应始终设置 `host_key_fingerprint`：

```json
{
  "id": "builder-1",
  "name": "Builder 1",
  "trust_level": "remote_verified",
  "host": {
    "kind": "ssh",
    "host": "builder.example.com",
    "user": "codewhale",
    "port": 22,
    "identity": "~/.ssh/codewhale_fleet",
    "host_key_fingerprint": "SHA256:aLGqZo1M6c...",
    "known_hosts": "~/.ssh/known_hosts",
    "working_directory": "/srv/codewhale/work",
    "env_allowlist": ["CODEWHALE_PROFILE"],
    "codewhale_binary": "/usr/local/bin/codewhale"
  },
  "capabilities": ["local", "linux", "tests"],
  "max_concurrent_tasks": 1
}
```

### 告警渠道秘密

告警渠道（Slack、通用 webhook、PagerDuty）使用 `FleetAlertEndpoint` 而不是原始 URL。webhook URL 可以内联提供用于非敏感端点，或作为秘密引用：

```json
{
  "kind": "slack",
  "webhook": {
    "url_ref": {"key": "CODEWHALE_FLEET_SLACK_WEBHOOK", "source": "env"},
    "secret_ref": {"key": "CODEWHALE_FLEET_SLACK_SIGNING_SECRET", "source": "keyring"}
  }
}
```

`secret_ref` 字段为 webhook payload 签名提供可选 HMAC 秘密，从不明文存储。

### 配置文件

`config.toml` 中的 `[fleet]` 表设置全局信任策略默认值：

```toml
[fleet]
default_trust_level = "sandbox"
require_identity_verification = true
max_trust_level = "operator"

[fleet.exec]
# 递归深度与独立子代理共享同一条轴线——fleet worker
# 就是无头子代理。0 会阻止子 agent（根 worker 仍会运行）；
# 3 是默认值；显式配置会被夹到共享安全上限。
max_spawn_depth = 3
```

这些默认值适用于没有携带自己 `security_policy` 的 fleet 运行。每次运行的策略总是覆盖配置默认值。

### 能力授权

能力授权是加法式、有范围限定的权限，授权特定动作。默认情况下，workers 得不到任何授权（最小权限）。常见授权：

- `"network"` 配合 scope `"github.com"` — 允许对 GitHub 的出站 HTTP
- `"git-push"` — 允许向 remotes 执行 `git push`
- `"provider-secrets"` — 允许访问 provider API 密钥
- `"release"` — 允许发布相关操作（打标签、发布）
- `"workspace-write"` 配合 scope `"crates/tui/**"` — 允许在某个路径内写入

### 环境净化

宿主 adapter 层在 worker 启动时强制环境净化：

- 默认只把 `HOME`、`PATH` 与平台特定变量（`SYSTEMROOT`、`COMSPEC`）注入 worker 进程
- 环境 allowlist 拒绝任何包含 `SECRET`、`TOKEN`、`PASSWORD`、`PASSWD`、`API_KEY`、`CREDENTIAL` 或 `PRIVATE_KEY` 的键
- SSH workers 只通过 OpenSSH `SendEnv` 发送显式 allowlist 的变量
- 秘密值从不嵌入 worker argv、任务说明或 fleet 日志——只出现秘密引用，而且它们总是被脱敏
