# 模式与权限姿态

> 本文翻译自英文版 [MODES.md](../MODES.md)，与英文修订 `1a9600e7c`（2026-08-19）同步。

Codewhale 有三个相关概念:

- **TUI 模式**:你当前处于哪种可见交互(Plan/Work/Operate)。
- **权限姿态(permission posture)**:UI 在执行工具前主动询问的激进程度。
- **工作流叠加(workflow overlay)**:可选的长时间运行编排，当任务需要许多协调的 worker 时，可以在任何 TUI 模式之上运行。

模型选择是独立的。`--model auto` 和 `/model auto` 把每一轮路由到具体的模型与思考级别；它们不是 TUI 模式，也不属于 `Tab` 循环。

工作流也独立于模式本身。它是可重复工作流和 Fleet worker 的可见的有序编排层。高扇出通过持久的 Fleet-backed worker 路由，而不是纯提示词的子智能体扇出。活动的模式仍然控制权限；工作流控制一个大型任务是否被规划成带自有进度视图的可恢复工作流。

## TUI 模式

按 `Tab` 补全 composer 菜单，或在 composer 为空时循环切换可见模式:**Plan → Work → Operate → Plan**。`Tab` 从不发送或排队 composer 文本；用 `Enter` 发送或排队。按 `Shift+Tab` 循环切换权限姿态(Ask → Auto-Review → Full Access)。按 `Ctrl+T` 循环切换推理强度。运行 `/mode` 打开模式选择器，或直接用 `/mode work`、`/mode plan`、`/mode operate` 切换。

- **Plan**:设计优先的提示方式。稳定的原语名称保持熟悉，但运行时集中拒绝文件修改和 shell 执行。只读检查与策略允许的研究(包括延迟的 Web 搜索/抓取)仍然可用。
- **Work**(内部为 `agent`):普通的多步执行。第一回合的小工具箱是 `read`、`write`、`edit`、`bash`、`agent` 和 `todo_write`;审批、沙箱、仓库法和托管策略仍然决定什么可以执行。
- **Operate**:多任务指挥姿态。它与 Work 拥有相同的原语身份和执行权限。父会话是 **operator**:派发后台 worker 是独立或并行工作的默认方式。小而紧密耦合的任务在父会话中处理；可分离的流用后台 `agent` worker,当顺序、阶段、门、共享预算或确定性汇入重要时使用 Workflow。**派发不等于完成** — 有写权限的子智能体必须返回真实的验证证据。

`Act` 和 `/mode act` 仍然是 Work 的兼容别名。保存的设置仍然规范化为内部值 `agent`。

### 按模式划分的工具可用性

| 工具族 | Plan | Work | Operate |
|:---|:---:|:---:|:---:|
| `read` 与策略允许的延迟研究工具 | 是 | 是 | 是 |
| `write` 和 `edit` | 可见名称；执行被拒绝 | 受审批与策略门控 | 与 Work 相同 |
| `bash` | 可见名称；执行被拒绝 | 受审批与策略门控 | 与 Work 相同；并行或隔离有帮助时优先委托 |
| `agent` | 可以，受子智能体深度权限约束 | 可以，受子智能体深度权限约束 | 可以，受子智能体深度权限约束 |
| 延迟的原生、MCP 与插件工具 | 策略允许时可通过 `tool_search` 发现 | 相同 | 相同 |
| 付费或外部服务工具 | 遵循权限姿态 | 遵循权限姿态 | 遵循权限姿态 |
| 工作区根目录之外的访问 | 仅显式可信路径 | 仅通过可信路径或信任模式 | 与 Work 相同的可信路径/信任策略;Fleet profile 从不扩大它 |

Operate 改变的是调度重点，而不是权限。它既不增加特定于模式的工具拒绝，也不绕过活动的审批、沙箱、shell、询问规则、仓库法或托管策略边界。Plan 仍然是 shell 与可写工具的特定于模式的执行边界；这种权限差异不需要不同的原语词汇。

### Operate 循环(一屏)

```text
User message
  → small / chat / one-file?  → parent does it (Work-equivalent tools)
  → real / multi-stream work? → goal (if needed) → dispatch background workers
       → each write child: implement → VERDICT PASS/FAIL with evidence
       → ordered / gated fan-in? → Workflow (operate_* starters)
       → high-stakes ambiguous? → best-of-n (N worktrees + reviewer; apply on PASS)
  → parent synthesizes receipts; stays free for the next ask
```

生命周期声明保持精确：已派发 ≠ 已定案 ≠ 已验证。

`allow_shell` 控制 `bash` 是否可以执行；它不重命名工具，也不让模式成为审批权威。持久任务与自动化保持保守的省略字段默认值，并且只有当其设置显式授予时才获得 shell 权限。有状态的终端/后台控制是专门的延迟工具，而不是小型前台 `bash` schema 上的字段。Full Access 改变权限姿态，而硬性安全与仓库策略持有仍然具有权威。

具备行动能力的模式可以通过 `tool_search` 发现延迟的 `rlm` 工具族；它的 `open`、`eval`、`configure` 和 `close` 动作拥有持久的 RLM 会话。遗留的拆分 `rlm_*` 拼写仍然是仅回放的别名。在 RLM Python REPL 内部,`sub_query_batch` 扇出 1-16 个固定到 `deepseek-v4-flash` 的廉价并行子调用。

快速的 `deepseek-v4-flash` / 关闭思考路径在产品语言中叫 Fin。Fin 是路由、摘要、廉价子调用和协调工作的接缝；它不改变审批行为。

编排三件套是空白提示时的第一个选择(按 `/`,然后 Enter):`/auto` 打开 Auto-Review 让智能体直接工作,`/goal` 跨回合保持一个目标,`/workflow` 运行可重复的有序或扇出工作流。当热键栏启用时，它们也位于空闲欢迎页、空闲页脚和前三个 Hotbar 槽位。

`/goal <objective>` 设置一个带可选 token 预算的会话目标，并让活动的目标作为 Work 上下文保持可见。当直接请求描述一个需要多于一回合才能完成的可验证最终状态时("直到测试通过"、"让 X 端到端工作"),智能体也可以自己创建目标；然后它显示一行回执，你可以 `/goal pause` 或 `/goal clear` 它。裸 `/goal` 显示进度(状态、已用时间、延续次数，以及没有回合运行时如何继续);没有目标且还没有对话时，打印用法。`/goal pause` 停止目标延续而不改变目标,`/goal resume` 恢复并把目标送回回合中,`/goal complete` 标记完成,`/goal blocked` 标记受阻,`/goal clear` 移除它。目标状态不改变活动的 TUI 模式、权限姿态或模型路由。这与只控制模型与思考选择的 `--model auto` 仍然不同。

工作流建立在同样的分离之上：目标可以让智能体继续工作，而 Workflow 为大型扇出提供可重复的工作流/进度表面。在 UI 中,Workflow 运行应该作为主屏幕上的覆盖层显示，而不是作为 Plan、Work 和 Operate 旁边的另一个模式。

App-server 客户端可以用 `thread/goal/set` 持久化线程范围的目标，用 `thread/goal/get` 读取，用 `thread/goal/clear` 清除。该持久化记录携带 `active`、`paused`、`blocked`、`usage_limited`、`budget_limited` 或 `complete` 状态，加上为需要线程恢复语义的客户端准备的 token/时间记账字段。

## 模式持久化

交互式选择模式也会设置新会话启动时的模式。Tab/Shift+Tab 循环、`Alt+A` / `Alt+P` / `Alt+Y` 快捷键、热键栏的 Plan/Work/Operate 动作和 `/mode` 都会把 `default_mode` 写入 `~/.codewhale/settings.toml`,所以切换到 Operate 会在重启后保留。写入发生在事件循环之外；如果失败,TUI 会在警告 toast 中说明，而不是在下次启动时静默回退。

模式、思考级别和模型选择器共享一个串行化的写入器，所以最后的选择就是磁盘上的选择 — 一阵 Tab 按键不会最终持久化恰好最后完成的那个写入 — 模式写入也永远不会回滚 `default_model` 等无关的键。

有两条路径故意**不**重写启动默认值：恢复已保存的会话(它会重新安装该会话所在的模式),以及因回合正在运行而被拒绝的模式更改。遗留的 `yolo` 入口点安装 Work 加 Full Access,它持久化的是 `agent` — `yolo` 是权限别名，绝不是启动模式。

重新选择你已经在的模式不是 no-op。恢复会话后，活动模式和 `default_mode` 经常不一致，所以再次选择活动模式就是让它持久化的方式;Codewhale 会给出"已保存为启动默认值"的回执，而不是报告"已经在该模式"。

回合运行时，对活动路由的每次更改都会被拒绝 — 模式、模型、思考级别和 provider — 无论你使用哪个表面。现在这包括斜杠表面(`/mode`、`/model`、`/config <key> <value>`、`/config preset`),它们在回合中也可达。先按 Esc 中断。仅重启的 `default_mode` 键豁免，因为它不触及正在运行的回合。

Codewhale 在跨进程的锁下写入 `settings.toml`,并原子替换文件，所以同一主目录上的第二个 Codewhale 实例不会丢失你的选择，也不会读到写了一半的文件。退出时，排队写入在终端恢复前被刷新；任何失败的东西都会在退出时打印出来，而不是随备用屏幕一起消失。

## 兼容性说明

- 带有 `default_mode = "normal"` 的旧设置文件仍然作为 `agent` 加载；保存会重写规范化值。

## Esc 键行为

`Esc` 是一个取消栈，不是模式开关。

- 先关闭斜杠菜单或瞬态 UI。
- 如果回合正在运行，取消活动的请求。
- 如果 composer 为空，丢弃排队的草稿。
- 如果存在文本，清除当前输入。
- 否则不执行任何操作。

## 权限姿态

权限姿态控制工具审批，以及回合是否可以为缺失的用户决定而暂停。它是完整[授权顺序](../AUTHORIZATION_ORDER.md)的一层，不是工具准入、仓库法或沙箱执行的绕过。用 `Shift+Tab` 循环它，或在运行时编辑它:

```text
/config
# 把 approval_mode 行编辑为: suggest | auto | never
```

遗留说明:`/set approval_mode ...` 已被 `/config` 取代。

- `suggest`(**Ask**,默认):工具审批可能打断,Codewhale 在未解决的用户选择会实质性改变权限、成本、范围或结果时询问。
- `auto`(**Auto-Review**):完全自主的姿态。它从不打开用户问题；模型从上下文中解决歧义，选择安全可逆的解释，或报告它无法安全继续。工具安全持有与用户问题保持分离。两层决定审批。**确定性下限**(配置的阻止规则加内置安全下限)允许被证明安全的调用，并硬阻止发布类动作和破坏性的后台/无头工作；它从不被模型审查。回退持有 — 确定性引擎无法证明安全的调用 — 升级到一次性**模型守护者**(v0.9.8),它返回风险、允许/拒绝和理由。守护者把精确的持有调用和确定性观察放在单独的 JSON 字段中；对话历史、技能指令、附件和展开的模型上下文被排除在外。它不推断用户意图，也不计算通用的用户意图分数。高或严重风险即使模型说允许也不能自动运行。它没有工具，不记住规则，并且对过大的精确调用宁可拒绝也不截断。恰好发出一个审查请求；不完整或格式错误的输出、超时、取消或 provider 失败都失败关闭。无头适配器使用仅确定性层级。显式要求人参与的仓库法持有在 Auto-Review 中作为硬阻止，而不是打开隐藏的审批模态。

LLM 审查器最接近 OpenAI Codex 的实验性 Auto-Review,提交为 [`6fc6b9d6d2580d62622fc9884b5f5707f6505a5e`](https://github.com/openai/codex/tree/6fc6b9d6d2580d62622fc9884b5f5707f6505a5e)。Codex 的 [guardian 入口点](https://github.com/openai/codex/blob/6fc6b9d6d2580d62622fc9884b5f5707f6505a5e/codex-rs/core/src/guardian/mod.rs)重建对话上下文并运行专门的审查会话。Codewhale 有意只采用精确动作的结构化决策、90 秒截止时间和失败关闭的结果。它不复制 Codex 的转录重建、用户授权分数、审查器工具、重试、持久审查会话或拒绝账本。

Kimi Code 在提交 [`1414d4602898f406e540b23342cb18db23ff9efc`](https://github.com/MoonshotAI/kimi-code/tree/1414d4602898f406e540b23342cb18db23ff9efc) 也没有 LLM 审查器。它的有序[权限策略](https://github.com/MoonshotAI/kimi-code/blob/1414d4602898f406e540b23342cb18db23ff9efc/packages/agent-core-v2/src/agent/permissionPolicy/permissionPolicyService.ts)先应用显式拒绝规则，然后它的 [Auto 策略](https://github.com/MoonshotAI/kimi-code/blob/1414d4602898f406e540b23342cb18db23ff9efc/packages/agent-core-v2/src/agent/permissionPolicy/policies/auto-mode-approve.ts)直接返回 `approve`。Codewhale 借用 Kimi 的无提问自主 UX,而不是那条一刀切的批准规则。

沙箱与升级基线基于 DeepSeek Harness `0.1.0-rc.5`,提交为 [`47f943859bef60e4160492346772ded9b24f765a`](https://github.com/deepseek-ai/deepseek-harness/tree/47f943859bef60e4160492346772ded9b24f765a):它的[sandbox 契约](https://github.com/deepseek-ai/deepseek-harness/blob/47f943859bef60e4160492346772ded9b24f765a/docs/subsystems/sandbox.md)为每次调用定义 `read-only`、`workspace-write` 和 `danger-full-access` 边界，并禁止静默的无约束回退；它的[approval 契约](https://github.com/deepseek-ai/deepseek-harness/blob/47f943859bef60e4160492346772ded9b24f765a/docs/subsystems/approval.md)只授予 `allowed-once`,并在拒绝、取消或应答者不可用时失败关闭；它的[sandbox 结果契约](https://github.com/deepseek-ai/deepseek-harness/blob/47f943859bef60e4160492346772ded9b24f765a/packages/shell/bash-sandbox/README.md)告诉模型用最窄的更宽模式加一句理由，恰好重试一次被拒绝的命令。DeepSeek Harness 不向该路径添加 LLM 审查器。Codewhale 的自主姿态只添加上面描述的单个无状态守护者请求；确定性硬阻止仍然不可绕过。
- `bypass`(**Full Access**):普通工具调用不显示审批提示，而有意的用户问题仍然可用。不可绕过的注册持有自动批准，而不是打开矛盾的模态。仓库法和托管策略持有作为硬阻止失败关闭，而不是用审批模态与 Full Access 矛盾。
- `never`:阻止任何不被视为安全/只读的工具；有意的用户问题仍然可用。

有效姿态及其提问纪律从与门控工具相同的运行时权威投射到每一回合。因此模式/姿态更改对下一回合可见。不可信运行时生成的输入在元数据构建前被收窄，不能凭空发明审批权限。显式的 Full Access 子智能体交接保留父会话的常驻姿态，所以普通子智能体工作不会再次开始提示。

### 子智能体(sub-agents 与 Fleet worker)

子智能体忠实继承会话姿态，而不是一个裸露的 auto-approve 位:

- **Auto-Review**:worker 的持有调用经过相同的确定性策略(被证明安全的调用运行；发布类与破坏性后台工作被硬阻止),对于它无法证明安全的持有，使用子智能体自己的会话客户端走同一次性的模型守护者。从不为了子智能体打开提示；守护者不可用时拒绝，失败关闭。
- **Ask**:角色可以委托的调用会运行。当宿主是交互式 TUI 时，持有调用作为审批提示在父会话的 UI 中提出(`agent:<id>:approval:<n>`);worker 可见地等待(`waiting for user`),人的回答被路由回它，无论父回合是空闲还是自己也在等待审批。无法提示的宿主拒绝并说明原因。
- **Full Access**:普通调用运行；破坏性的分离工作仍然失败关闭，因为子智能体是后台 worker。

角色姿态和执行边界在此门之前和之后都被检查，绝不扩大。人没有在提示处做出的每个决定都被写入审计日志和子智能体的转录，作为一行说明(`Auto-Review allowed 'bash' (low risk, model guardian): …`),在 worker 被聚焦时可见。

## 小屏幕状态行为

终端高度受限时，状态区先压缩，让 header/chat/composer/footer 保持可见:

- 加载与排队的状态行按可用高度分配预算。
- 完整预览放不下时，排队预览折叠成紧凑摘要。
- `/queue` 工作流仍然可用；紧凑状态只影响渲染密度。

## 工作区边界与信任模式

默认情况下，文件工具被限制在 `--workspace` 目录。启用信任模式以允许访问工作区之外的文件:

```text
/trust on
```

裸 `/trust`(像 `/trust status`)只*报告*当前设置 — 它不启用任何东西。用 `/trust off` 再次限制访问。

Full Access 自动启用信任模式。

## MCP 行为

MCP 工具以 `mcp_<server>_<tool>` 暴露，使用与内置工具相同的审批流程。策略允许时，只读 MCP 辅助工具可以在 Ask 和 Auto-Review 中自动运行；可能有副作用的 MCP 工具需要审批。Full Access 不绕过硬策略持有。

参见 [MCP.md](MCP.md)。

## 相关 CLI 标志

运行 `codewhale --help` 获取规范列表。常见标志:

- `-p, --prompt <TEXT>`:一次性提示模式(打印并退出)
- `codewhale exec --auto --output-format stream-json <PROMPT>`:运行工具支持的非交互式智能体，为 harness 和后端包装器每行发出一个 JSON 对象。退出码:`0` 成功,`1` 真正的任务/智能体失败,`75`(`EX_TEMPFAIL`)当回合因可重试的基础设施失败结束(所有会话内重试之后的 provider/transport `network`/`timeout`),让 harness 能把可重试的 infra 退出与任务失败区分开；终端流 `metadata` 事件的 `error_category` 携带相同的分类
- `codewhale exec --resume <ID|PREFIX> <PROMPT>` / `--session-id <ID|PREFIX>`:非交互式继续一个已保存的会话
- `codewhale exec --continue <PROMPT>`:非交互式继续此工作区最近的已保存会话
- `codewhale fork <ID|PREFIX>` / `codewhale fork --last`:把已保存的会话复制到新的兄弟会话；分叉的会话保留附加的父会话元数据，并在会话列表中显示该谱系
- `--model <MODEL>`:使用 `codewhale` facade 时，向 TUI 转发 DeepSeek 模型覆盖
- `--workspace <DIR>`:文件工具的工作区根目录
- `-r, --resume <ID|PREFIX|latest>`:恢复一个已保存的会话
- `-c, --continue`:恢复此工作区最近的会话
- `--max-subagents <N>`:钳制在 `1..=128`
- `--mouse-capture` / `--no-mouse-capture`:选择启用或退出内部鼠标滚动、转录选择、右键上下文动作和转录滚动条拖动。鼠标捕获在非 Windows 终端和 Windows Terminal/ConEmu/Cmder 上默认启用，因此拖动选择只复制转录文本，从段落中移除视觉换行列断行，并保持在转录窗格内；拖动时按住 Shift,或使用 `--no-mouse-capture` 进行原始终端选择。它在遗留 Windows 控制台(没有 `WT_SESSION` / `ConEmuPID` 的 CMD)和 JetBrains JediTerm 内部默认关闭 — PyCharm/IDEA/CLion 等 — 这些地方终端宣称支持鼠标，但把 SGR 鼠标事件作为原始文本转发(#878、#898)。在默认关闭的任何地方用 `--mouse-capture` 选择启用。原始终端选择可能越过右侧边栏并包含视觉换行，因为选择由终端而不是 TUI 拥有。
- `--profile <NAME>`:选择配置 profile
- `--config <PATH>`:配置文件路径
- `-v, --verbose`:详细日志

## 分支与回滚

Codewhale 有三条相关但有意的独立恢复路径:

- `codewhale fork <ID>` 从现有已保存对话创建新的已保存会话，并记录源会话 id。这是在不覆盖原始会话的情况下探索不同答案路径的安全方式。
- Esc-Esc 回溯把实时转录倒回到之前的用户提示，并把该提示恢复到 composer 中供编辑。
- `/restore` 和 `revert_turn` 工具从 side-git 快照恢复工作区文件。`/restore list [N]` 在选择回滚点前列出更多快照选项。它们不重写对话历史。

Pi 风格的文件内树浏览器是一个更大的 UI/数据模型项目。v0.8.40 交付有界的 fork/backtrack 原语和显式谱系元数据。
