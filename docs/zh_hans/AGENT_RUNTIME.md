# CodeWhale Agent 运行时 —— 一个持久化底座，多种熟悉的启动方式

> 本文翻译自英文版 [AGENT_RUNTIME.md](../AGENT_RUNTIME.md)，与英文修订 `fa50abf68`（2026-08-17）同步。

本文说明子代理（sub-agent）、无头 `exec` 路径和 Agent Fleet 之间的关系。之所以写这篇文档，是因为它们一度漂移成了两套并行的 "worker" 体系；修复方案是让 **fleet 支撑的 worker 运行**成为持久化的原语。"子代理"作为嵌套角色的产品用语仍然有用，但它绝不能暗示一个生命周期语义更弱的独立执行底座。本文也回答了 #2972 中悬而未决的方向问题（"与 Claude Code 收敛多少才是对的？"）。

## 核心思想

恰好有**一个**东西在运行脱离式的代理工作：一个包裹在持久化 worker 生命周期中的**无头代理运行时**。它是一个模型循环，带有完整（受策略约束）的工具面，并且可以通过同一生命周期进一步委派子工作。其他一切只是启动那一个运行时的不同方式，或者观察它的不同方式。

```
                         ┌───────────────────────────────┐
                         │     headless agent runtime     │
                         │  (full tools + can sub-spawn)  │
                         └───────────────────────────────┘
                            ▲             ▲             ▲
            launches │              │              │ launches
                     │              │              │
        ┌────────────┴───┐  ┌───────┴────────┐  ┌──┴───────────────────┐
        │   TUI turn     │  │ `codewhale     │  │   Agent Fleet         │
        │  (interactive, │  │   exec`        │  │  (durable: ledger,    │
        │   in-process)  │  │  (headless CLI,│  │   scheduler, SSH,     │
        │                │  │   anyone/any-  │  │   alerts) — launches   │
        │                │  │   time)        │  │   `codewhale exec`     │
        └────────────────┘  └────────────────┘  │   per worker          │
                                                 └───────────────────────┘
```

- **子代理**是*嵌套任务*面向用户的名称，带有角色（`explore`、`review`、`implementer`、`verifier` 等）。它应该由与 fleet 相同的 worker 运行生命周期支撑。`agent` 是面向模型的启动器，而不是第二个运行时。
- **`codewhale exec`** 是无头前门：任何人在任何时间都可以使用（CI、脚本、另一个代理），拥有完整工具，输出 `stream-json` 事件流，并且可以派生子代理。它就是*那个*带 CLI 的运行时。
- **fleet worker** *就是*一次由 fleet 启动并持久化跟踪的 `codewhale exec` 运行——本地以子进程方式运行，远程以 `ssh host … codewhale exec …` 方式运行。fleet 不会重新实现执行；它只在那一个运行时*之上*增加**编排**（持久化账本、调度/租约/重试、主机传输、告警升级）。

所以 "fleet 与子代理" 并不是两个类别。它们是**同一次无头运行**：Fleet 是持久化控制面，而子代理是嵌套 worker 的角色/UX 用语。

## 切换规则

如果脱离的 `agent` 子级可能因为一次性 provider 超时而失败且没有重试，而等效的 fleet worker 会重试并保留账本证据，那么切换就是不完整的。应将其视为 CodeWhale 运行时缺口，而不是正常的 "子代理行为"。

兼容性 `agent` 运行时现在会在将 worker 标记为中断之前，以退避方式重试瞬时 provider 头、流和超时失败；当重试耗尽时会保留一个检查点并返回延续句柄。剩余的收敛工作是让该生命周期在进程重启、远程执行和完整 fleet 账本调度下保持持久化。

目标规则是：

- 持久化或长期运行的工作走 fleet worker 生命周期；
- `agent` 应该入队或观察一个由 fleet 支撑的 worker 运行，而不是自己拥有独立生命周期；
- 进程内子级只允许作为小的兼容性/延迟优化，而且它们必须暴露与 fleet 路径相同的终态、重试语义、回执和检查句柄。

在产品语言里说 "打开一个子代理" 没问题。在架构语言里，这意味着 "以这个角色启动一个嵌套 fleet worker"。

## 为什么是这种形态（以及为什么它能修复卡顿）

动因问题：派生大量进程内子代理会让 TUI 卡顿，因为每个子级都会克隆一个重型运行时并重建整个工具注册表，*而且* TUI 会为每个子级渲染一整张卡片/转录。

调研 Claude Code、Codex 和 Kimi 后发现，让编排器在高扇出下保持轻量的**不是**进程边界——三者都在进程内运行子代理。而是**隔离 + 紧凑的事件流**：

- 子级的转录**绝不**回流到父级——父级得到的是结果摘要和一条小的生命周期事件流；
- UI 渲染的是**计数**（`2 running / 3 done`），而不是每个 worker 一个子会话；
- 每个 worker 的工具面直接根据**角色/能力配置文件**构建，而不是"先构建全部再过滤"。

因此 "无头" 意味着*执行形态不像 UI*——它**并不**意味着能力更少。无头 worker 保留完整工具集，并且可以派生子代理。

当工作还需要**持久化**（在 TUI 关闭、笔记本休眠后仍存活）或**远程**（SSH）时，fleet 会在进程外以 `codewhale exec` 运行 worker。重型构造因此完全位于另一个进程中，编排器无论扇出多大都保持流畅，而且运行能在重启后存活——这正是 #3154 的天级自治目标。

## 单一递归轴

worker 在 `spawn_depth = 0` 运行，并且可以在满足 `spawn_depth + 1 ≤ max_spawn_depth` 时派生子级，因此预算 `N` 提供 `N` 层嵌套委派。子代理和 fleet worker 共享**一条**轴，来源是 `codewhale_config`：

- `DEFAULT_SPAWN_DEPTH = 3` —— 独立子代理和 fleet worker 的默认预算（因此它们不会漂移成"两个移动靶"）；
- `MAX_SPAWN_DEPTH_CEILING = 8` —— 可选上限，每个配置值（fleet 的 `max_spawn_depth`、`agent` 的 `max_depth`）都会被钳制到该值。

注意解析器和对外公布的 schema 对 `agent` 的 `max_depth` 看法不一致：解析器钳制到 8（`tools/subagent/mod.rs:10601-10617`），而展示给模型的 JSON schema 声明 `"maximum": 3`（`mod.rs:6845-6848`）。因此模型无法请求运行时愿意兑现的深度。这里作为代码差异跟踪，而不是文档差异。

根 worker 即使在预算为 0 时也会运行；预算约束的是*子级*委派。默认预算至少提供三层嵌套。

## 事件词汇

fleet 账本持久化的是 worker 自身的事件流，而不是另一套模拟的分类法。`codewhale exec --output-format stream-json` 会发出 `{"type": "content" | "tool_use" | "tool_result" | "sandbox_denied" | "workflow_event" | "session_capture" | "turn_usage" | "metadata" | "done" | "error"}` 行，它们映射到 fleet 账本的 `FleetWorkerEventPayload`（`RunningTool`、`WorkflowEvent`、`Running`、`Completed`、`Failed` 等）。`workflow_event` 在 Workflow 飞行期间携带类型化的 run/phase/task/gate 回执，并作为类型化的 `WorkflowEvent` 保留在 Fleet 账本中；外层 worker 仍然拥有终态 `done` 或 `error`。一套词汇，两个表面。

`turn_usage` 是每次模型调用的用量回执，当 provider 为该调用报告了用量时，每个模型请求（turn 步骤）发出一次：

```json
{"type": "turn_usage", "schema": "codewhale.exec-stream", "schema_version": 1,
 "turn": 1, "input_tokens": 1200, "output_tokens": 180,
 "reasoning_tokens": 90, "prompt_cache_hit_tokens": 900,
 "prompt_cache_miss_tokens": 300, "prompt_cache_write_tokens": 0,
 "reasoning_replay_tokens": 40, "duration_ms": 1834}
```

- `turn` 是 exec 运行内模型调用的从 1 开始的索引；`input_tokens`、`output_tokens` 和 `duration_ms` 始终存在。
- 可选 token 字段在 provider 未报告时会**省略**——绝不作为 null 发出，也绝不用零回填。字段名镜像终态 `metadata` 回执：`prompt_cache_hit_tokens` 是 provider 的缓存读取计数（Anthropic 的 `cache_read_input_tokens`），`prompt_cache_write_tokens` 是缓存创建计数（`cache_creation_input_tokens`）。`reasoning_tokens` 只出现在报告它的 provider 路径上（OpenAI 兼容的 `completion_tokens_details` / Responses 的 `output_tokens_details`；Anthropic 不报告思维 token 计数）。`reasoning_replay_tokens` 是 DeepSeek V4 交错思维重放的客户端侧估算。
- 当 provider 对某次调用完全没有报告用量时，该次调用的整个事件会被跳过。延迟/收敛分析应该累加 `turn_usage` 事件，而不是从墙钟时间推断每步 token；终态 `metadata` 回执仍然携带累计总数。

## 与 Claude Code 的收敛（#2972）

CodeWhale 应该在**形态**上与 Claude Code 收敛，而不是在品牌上：

- **采纳**：带真实 CLI/SDK 前门的无头运行时；作为返回摘要（而非转录）的隔离运行的子代理；紧凑的、事件驱动的扇出投影；能力/角色工具配置文件；技能生态（#2743）；结构化运行回执。
- **保持独特**：CodeWhale 品牌和一流的 DeepSeek/GLM/MiniMax/多 provider 支持；本地优先的 **Agent Fleet**（持久化、支持 SSH 的编排）作为 CodeWhale 在共享运行时之上的自有层；Workflow 作为编排覆盖层。
- **不要**按表面分叉执行语义。TUI、`agent`、`exec`、Runtime API 和 fleet 都必须驱动*同一个*运行时并观察*同一条*事件流——那里的分歧正是产生"两个移动靶"的原因，本文档的存在就是为了防止它。

任何新代理表面的试金石是：*它启动并观察那一个运行时，还是发明了第二个？*只有前者是允许的。

## v0.9.0 之后还剩下什么

2026-08-17 根据对较老的 0.9 时代文档的全面审计刷新。那些计划是证据，不是第二个真相来源。v0.9.0 整合了水下 shell、消息优先的 Operate、权限姿态、接线的 Workflow 引擎和持久化运行日志、Lane CLI/运行时、带 `operate_ready` 的设置、宪法再平衡，以及 ProviderLake/Models.dev。剩余工作属于后续版本：

1. **品牌重塑完成** —— `deepseek`/`deepseek-tui` 二进制垫片和垫片发布资源已在 v0.9.0 移除；剩余义务是 Homebrew `codewhale` 配方发布（`docs/REBRAND.md`）。
2. **Operate 作为价值流** —— 水下 shell 之上的控制面板表面（WIP、队列年龄、瓶颈）；阶段历史（#4039）；Workrooms Phase 2（#3209/#3210）作为收件箱底座；回执对账。
3. **流量控制** —— 真正的 WIP 限制和可见队列（#4015、#4016），与已发布的 16 并发/1k 运行访问模型（#4292）协调一致。
4. **Fleet/Workflow 收敛残留** —— 实时 tmux/verifier-gate 自用，收尾 #4175/#4177/#4178/#4179；Fleet 消费规范的 AgentProfiles；Conductor/topology（#4010、#4012）作为延伸目标。
5. **TTC 设计实现**（设计文档在 `codewhale-ops`）—— 已批准，v0.9.0 之后不再受阻。
6. **HarnessProfile 完成** —— 状态/UX 显示通道（`docs/rfcs/HARNESS_PROFILE_CUTLINE.md`）。
7. **文件分解，已落地** —— v0.9.0 时代的违规文件被拆分：`main.rs` 现在是薄桩，`ui.rs` 已被分解为 `crates/tui/src/tui/` 下聚焦的模块（今天约 3.9k 行；`docs/rfcs/FILE_DECOMPOSITION_0_9_0.md` 中的数字是 0.9.0 时代的快照）。剩余工作是 `POST_0_9_1_SEAMS.md` 中跟踪的"核心之上的薄 TUI"北极星。

由其自身文档明确推迟的：外部工作流内存（仅边界）、自动 harness 演化、托管 workrooms、`constitution_modules`（需要签署）、权限配置文件（#3211，需要设计），以及计划上限探测（需要产品决策）。

## 外部 harness 的公共启动契约（#4641）

外部评估 harness（例如未来的 Verifiers v1 内置 harness）通过启动公共 `codewhale exec` 前门、指向它自己拥有的拦截端点来嵌入 CodeWhale。CodeWhale 只拥有它的**启动契约**；harness 拥有拦截、轨迹、模型调用计时、token 核算、重试、发布限制和运行时编排。不要向 CodeWhale 添加 harness 运行时、轨迹解析器或回执 schema。

可复现的无头启动只使用现有的通用表面：

- 一个显式的临时配置，指明路由和凭据**环境变量**，绝不指明秘密本身：

  ```toml
  provider = "openai"

  [providers.openai]
  base_url = ""            # harness 会填入它的拦截端点
  model = ""               # harness 会填入目标模型
  api_key_env = "VF_CODEWHALE_API_KEY"
  ```

- `CODEWHALE_HOME` 设置为全新的每运行目录；
- `CODEWHALE_SECRET_BACKEND=file`;`
- `CODEWHALE_MCP_CONFIG` 指向一个生成的每运行 MCP JSON 文件，其中只包含 harness 提供的任务服务器（`{"mcpServers":{"task-tools":{"url":""}}}`；`mcpServers` 别名和基于 URL 的 Streamable HTTP / SSE 传输已经存在）；
- `CODEWHALE_MEMORY=false` 和 `CODEWHALE_TELEMETRY=false`。匿名用量计数默认开启，所以每个封闭的 harness 都会显式设置运行级 kill 开关。它也保护调用者复用的 home，其普通会话会把聚合计数发送到实时端点（`https://telemetry.codewhale.net/v1/telemetry`，已发布的默认值），而不是发送到本地文件。这是一个硬地板——环境中显式的 "off" 胜过 `--telemetry true` 和配置里的 `telemetry = true`。如果 harness 想让已启用的 home 继续本地缓冲而不联系任何东西，则改为设置 `CODEWHALE_TELEMETRY_ENDPOINT=`（空）。参见 [`docs/TELEMETRY.md`](../TELEMETRY.md)；
- `CODEWHALE_ALLOW_INSECURE_HTTP=1` **仅当** harness 提供受信任的 `http://` 拦截端点时设置（容器/隧道端点不总是回环）；
- 当调用者提供 `--append-system-prompt` 和 `--disallowed-tools` 时使用它们。

拦截秘密留在子环境中（通过路由的 `api_key_env` 解析）；它绝不会被写入 argv、路由配置、日志、`stream-json` 流或任何生成的文件。

确切的参数顺序是：

```sh
codewhale \
  --config .vf-codewhale/config.toml \
  --workspace . \
  --no-project-config \
  --skip-onboarding \
  exec \
  --auto \
  --sandbox danger-full-access \
  --output-format stream-json \
  -- "<task prompt>"
```

`--no-project-config` 必须出现在子命令**之前**（和 `--skip-onboarding` 一样）。公共分发器会解析它并把它转发到 TUI 子命令之前；`Exec` 随后跳过工作区特定的 `[workspace]`/`[projects]` 用户配置覆盖层，使配置表面只依赖显式的 `--config`。`crates/tui/tests/integration/verifiers_harness_contract.rs` 是这个契约的无 provider 验收锁。

### 未来的上游清单（不在本文范围内——不要执行）

把 CodeWhale 实际添加为内置 harness 的工作位于外部 Verifiers 仓库；它需要的、带校验和清单的公开不可变 CodeWhale GitHub Releases 自 v0.9.1 起就已存在（当前发布线是 v0.9.9）。该上游变更预计仅限于一个新的 `verifiers/v1/harnesses/codewhale/` 包及其测试矩阵和文档注册，其中 `CodewhaleHarnessConfig` 锁定目标发布，`setup()` 下载并校验已发布的归档，`launch()` 写入上面的临时路由/MCP 文件并调用 `runtime.run_program(...)`。

本契约工作明确**不**执行的保留事项：打标签、发布或创建 CodeWhale 发布；打开或提交上游 Verifiers PR；运行其需要凭据的 E2E 矩阵；或在确切的已发布归档于该上游运行时运行之前宣称运行时/架构支持。
