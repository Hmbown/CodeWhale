# Codewhale 产品遥测

> 本文翻译自英文版 [TELEMETRY.md](../TELEMETRY.md)，与英文修订 `fa50abf68`（2026-08-17）同步。

**0.9.6 的状态：匿名使用计数默认开启，可以立即关闭。** 首次交互式启动会汇总统计了哪些内容，链接到逐字段的精确 schema，并在原生启动弹窗中预选"保持开启"。用方向键或 Tab 选择，Enter 确认，`Y`/`N` 是直接快捷键。遥测在做出该选择之前不会启动。无头（headless）界面遵循相同的文档化默认值，不会假装展示过交互式提示。此前 0.9.4 opt-in 提示记录到的每一次拒绝，在升级后都保持关闭。

Codewhale 不会收集对话、代码、提示词、文件、文件名/仓库名/分支名、模型内容或凭据。它不发送任何按回合或按工具的时间线。它只发送下面这个封闭的聚合 schema：版本和平台类别、会话时长/结果、功能/错误计数器，以及一个每 90 天轮换一次的随机安装 id。

**现在有了一个真实的端点。** 已启用的会话会将其批次发送到第一方采集服务 `https://telemetry.codewhale.net/v1/telemetry`，这也是 `telemetry_endpoint` 的出厂默认值。该服务是什么、存储什么、结构上不可能存储什么，都在下面的"端点做什么"一节中说明。

**要想不向任何地方发送任何内容，请保持遥测关闭**（见"关闭遥测"）。想保持启用但不联系任何人，请设置 `telemetry_endpoint = ""`：此时批次会被追加到你本机的 `$CODEWHALE_HOME/telemetry/dryrun.jsonl`，与服务端本会收到的内容逐字节一致，而且永远不会构造任何 HTTP 客户端。这个文件就是你对照现实审计本文档的方式。

本文档就是 schema。它不是 schema 的摘要：`crates/telemetry` 中的一个测试会解析本文件的字段名，并断言与序列化器实际使用的结构体集合相等，因此文档里有而代码里没有——或代码里有而文档里没有——都会导致构建失败。

## 关闭遥测

有两个关闭开关，作用各不相同。两者都会彻底停止采集；其中只有一个会擦除任何东西。

```sh
codewhale config set telemetry false     # 选择退出：停止采集并擦除状态
CODEWHALE_TELEMETRY=0 codewhale          # 终止开关：停止采集，不擦除任何内容
codewhale --telemetry false              # 同样的终止开关，仅对单条命令生效
```

**配置文件中的 `telemetry = false` 就是选择退出。** 它是一个底线：`--telemetry true` 和 `CODEWHALE_TELEMETRY=1` 都会输给它，因为一个可能被包装脚本意外撤销的设置算不上设置。它会删除随机安装 id，截断每个已缓冲事件和每条 dry-run 记录，并写入一个 tombstone。追加、身份/状态写入和投递共享同一次擦除的排序锁，因此一旦选择退出返回，就不会有任何退出前写入或 POST 仍在飞行。如果擦除的任何部分失败，tombstone 依然存在，缓冲区无法再排空——擦除失败即失败关闭。只要该设置仍然生效，每一次后续运行都会重新断言同一个 tombstone，因此它能一直存活；重新开启遥测意味着在同一个位置写入 `telemetry = true`，而这也是清除它的方式。此前缓冲的任何内容都永远不会被发送。

**环境变量和 flag 是终止开关，不是选择退出。** 本次运行期间遥测关闭，不写入任何内容，不发送任何内容——磁盘上也什么都不触碰、不删除。这是刻意的：一个为某条命令设置 `CODEWHALE_TELEMETRY=0` 的 harness 或 agent，绝不能悄悄丢弃机器所有者的安装 id 和 dry-run 记录。如果你想要会擦除的那种，请使用配置文件。

`CODEWHALE_TELEMETRY`（及其别名 `DEEPSEEK_TELEMETRY`）接受 `0`、`1`、`true`、`false`、`yes`、`no`、`on`、`off`、`enabled`、`disabled`。该列表无法读出的值也会解析为 off——终止开关里的拼写错误绝不能解析为"on"。

当任一开关已经设置时，首次运行提示根本不会显示：它绝不会问一个该环境会覆盖的问题，回答它也绝不会改写你自己写入的 `telemetry = false`。

仓库本地的 `.codewhale/config.toml` 不能设置 `telemetry` 或 `telemetry_endpoint`，工作区的 `.env` 同样不能设置这两者。别人的仓库无法打开你的遥测，也无法把它指向他们选定的主机。

## 数据存放位置与磁盘占用

所有内容都在 `$CODEWHALE_HOME/telemetry/`（`0700`）之下，每个文件都是 `0600`：

| 文件 | 作用 |
|---|---|
| `buffer.jsonl` | 待处理事件，每行一个 JSON 对象 |
| `buffer.jsonl.lock` | 兄弟排序锁，写入、投递、启动和擦除共享 |
| `dryrun.jsonl` | 端点配置为空时批次的去向 |
| `state.json` | 上次看到的应用版本和上次的 flush 尝试 |
| `install_id.json` | 随机安装 id 及其铸造时间 |
| `disabled` | tombstone；存在即表示不会追加或发送任何内容 |

`buffer.jsonl` 和 `dryrun.jsonl` 都是环形缓冲，上限为 512 条记录或 256 KiB，先到先截，最旧的被丢弃。因此整个目录有记录的占用上限为 **512 KiB 加几百字节元数据**。

安装 id 是一个随机的 v4 UUID。它绝不从你的主机名、MAC 地址、`machine-id`、主目录、用户名或可执行文件路径派生——派生的 id 就是设备指纹，重装后依然存在，并在你自己选择退出之后重新识别你。每当 `$CODEWHALE_HOME/telemetry/` 被清空时它都会重新生成（选择退出会自动清空），并且在任何情况下每 90 天轮换一次。

Codewhale 没有恢复出厂设置命令，因此本文档也不会声称有。

## 发送时机与发送去向

当持久选择退出或运行级终止开关生效时，不会发送任何内容。否则恰好只有一个 flush 点：关机期间的一次尝试，限时三秒。没有启动时 flush、会话中途 flush、按回合 flush 或按工具调用 flush。关机 flush 会在执行前立即从磁盘重新解析你的设置，因此从另一个终端写入的 `codewhale config set telemetry false` 会阻止一个已经在运行的会话的 flush。

一次 flush 就是对已解析端点的一次 **`POST`**——默认是 `https://telemetry.codewhale.net/v1/telemetry`。请求携带 `content-type: application/json` 头、`user-agent: codewhale-telemetry/<app_version>` 头，以及批次主体。仅此而已：没有 cookie（HTTP 客户端在构建时就没有可禁用的 cookie jar）、没有重定向（直接拒绝）、没有 `Authorization` 头、没有自定义头、没有查询字符串。响应主体被丢弃不读；只查看状态类别。

`https://` 是必需的。普通的 `http://` 只对回环主机接受，因此你可以把客户端指向自己的一台记录器并直接读取线上格式；没有任何环境变量能覆盖这一拒绝。客户端拒绝的端点会让本次运行的遥测关闭，而不是回退到其他目的地。

任何失败——DNS、连接、TLS、超时、非 2xx——都会丢弃该批次。没有重试、没有退避、没有重新排队。永久离线的机器每次 flush 点最多尝试一次，永远不会积压队列。

---

## 事件 schema

`SCHEMA_VERSION = 1`。除恰好三个有界字符串（`app_version`、`git_sha`、`panic_site`）外，每个字段都是整数、布尔值或**封闭枚举字符串**。这三个字符串各自都有成文规则和一个钉住该规则的测试。**该 schema 中没有自由格式字符串类型，也没有开放键映射。** 正是这一性质使红线 3 可强制执行，而不是停留在愿望层面。

### 批次信封——每次 POST 都会发送

```jsonc
{
  "schema_version": 1,
  "sent_at":     "2026-08-03T18:04:11Z",   // RFC3339 UTC，秒级精度
  "install_id":  "3f2a…",                  // uuid v4，每 90 天轮换
  "app_version": "0.9.4",
  "git_sha":     null,                     // 仅发布 CI 构建为非 null
  "surface":     "tui",
  "os":          "macos",
  "arch":        "aarch64",
  "libc":        "none",
  "tty":         true,
  "events":      [ … ]
}
```

| 字段 | 类型 | 来源锚点 | 规则 |
|---|---|---|---|
| `schema_version` | `u32` | `crates/telemetry/src/event.rs` 中的常量 | 任何字段新增/删除/改型时递增。绝不复用。由 golden snapshot 测试钉住。 |
| tsent_att | RFC3339 | tchrono::Utc::now()t | 秒级精度。仅按**批次**——事件本身完全不携带时间戳。 |
| `install_id` | uuid v4 | `crates/telemetry/src/envelope.rs` | 随机、绝不派生，每 90 天轮换。见上文"数据存放位置"。 |
| `app_version` | string | `env!("CARGO_PKG_VERSION")`，即 `crates/telemetry/src/lib.rs:112` 处 | 必须匹配 `^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$`。 |
| `git_sha` | string \| null | `option_env!("CODEWHALE_RELEASE_BUILD_SHA")`——一个**新的** rustc-env | 前 12 个十六进制字符。仅当 `codewhale_build_support::release_build_sha` 在构建环境中看到 `DEEPSEEK_BUILD_SHA` 或 `GITHUB_SHA` 时才发送，即仅对发布 CI 构建。对所有本地构建的二进制无条件为 `null`，且不做任何形式的运行时查找。**绝不**是 `CODEWHALE_BUILD_COMMIT`——那会回退到 `git_commit`，是构建者的私有 HEAD。**绝不**是 `Thread.git_sha`（`crates/state/src/lib.rs:93`）——那是用户工作区的提交，是一条红线，只隔一个名字。 |
| `surface` | enum | 在每次子命令分发时显式设置 | `tui \| exec \| cli \| app-server \| mcp-server \| serve`。**不能从可执行文件推导**：`codewhale-tui` 至少服务五个 surface，且 app-server 在 `codewhale` *进程内* 运行（`crates/cli/src/lib.rs:4225`），因此 `current_exe()` 会把每次 app-server 会话都报成 CLI。`desktop` 被省略——不存在桌面 surface。哪些 surface 能发送由选择退出策略决定，而非由 surface 决定：见下文"哪些 surface 会发送"。 |
| `os` | enum | `std::env::consts::OS`，即 `crates/cli/src/update.rs:41` 处 | allowlist：`linux \| macos \| windows \| freebsd \| android \| other`。 |
| `arch` | enum | `std::env::consts::ARCH` | `x86_64 \| aarch64 \| other`。 |
| `libc` | enum | `cfg!(target_env)`——**编译期** | `gnu \| musl \| none`。运行时检测会读取发行版厂商字符串；编译期免费且不泄露任何内容。 |
| `tty` | bool | `std::io::IsTerminal`，即 `crates/telemetry/src/envelope.rs:196` 处 | `stdin().is_terminal() && stdout().is_terminal()`。 |
| `events` | array | 被排空的缓冲区 | 每个元素都只能是下面四种事件之一，没有其他。每批次上限为 200 个事件或 64 KiB；超出任一上限的批次会把剩余部分留到下一次 flush。 |

**`os_major` 不会被收集。** 读取它需要两个平台上的不安全 FFI 外加第三个平台上的文件解析器，而这正是那个以"小到足以审计"为全部价值的 crate——而 `os`、`arch`、`libc` 是免费的，并且能回答平台问题。如果存储的数据将来显示 OS 版本切分正是分诊所缺的，可以重新考虑；光凭直觉不是那样的证据。

### 哪些 surface 会发送

除非机器有持久选择退出或本次运行有终止开关，否则 surface 默认发送。提示只在 TTY 上渲染。因此：

- **`tui`** ——先进入原生 TUI，把披露内容显示为启动弹窗，在第一次交互选择之前保持未启动状态，然后遵循该选择。
- **`exec`、`cli`、`app-server`、`mcp-server`、`serve`** ——在新主目录上遵循文档化默认值，在任何主目录上遵循所有持久/运行级选择退出。
- **Fleet worker 在任何 surface 上都不会发送**，这是构造层面的保证（`crates/tui/src/fleet/host.rs:1386-1388`）。

### 事件：install_or_upgrade

当 `state.json` 的 `last_version` 与 `app_version` 不同时发送一次。

```jsonc
{ "event": "install_or_upgrade", "kind": "upgrade", "previous_version": "0.9.3" }
```

| 字段 | 类型 | 来源 | 规则 |
|---|---|---|---|
| `kind` | enum | 派生 | `install`（无先前记录）\| `upgrade` \| `downgrade`。 |
| `previous_version` | string \| null | **仅** `$CODEWHALE_HOME/telemetry/state.json` | 与 `app_version` 相同的正则。绝不从会话历史或配置 mtime 派生——那些文件有不同的隐私契约。 |

### 事件：session_start

```jsonc
{ "event": "session_start", "source": "interactive" }
```

`source` 是 `SessionSource`（`crates/state/src/lib.rs:34-41`），由 `session_source_to_str`（`:1909-1917`）字符串化：`interactive | resume | fork | api | unknown`。

### 事件：session_end

主力事件。会话积累的一切都在这一个事件里发出，只发一次。

```jsonc
{
  "event": "session_end",
  "duration_bucket": "1m_10m",
  "exit_class": "clean",
  "cold_start_bucket": "250_1000",
  "providers": ["deepseek", "custom"],
  "counters": { "turns": 14, "tool_calls": 61, "fleet_dispatch": 0, "workflow_run": 0,
                "subagent_spawn": 2, "mcp_server_connected": 0, "memory_search": 0,
                "approval_modal_shown": 0, "approval_auto_allowed": 0,
                "command_palette_open": 3 },
  "errors":   { "auth_preflight_failed": 0, "provider_http_4xx": 0, "provider_http_5xx": 1,
                "tool_denied_by_policy": 0, "tool_timeout": 0, "network_error": 0 },
  "turn_wall": { "lt_5s": 9, "5_30s": 4, "30_120s": 1, "gte_120s": 0 }
}
```

**`counters` 和 `errors` 是 `#[derive(Serialize)]` 的具名 `u32` 字段结构体，不是映射。** 每个字段都会被序列化，包括零值。键集合由编译器封闭：新增计数器需要编辑 `crates/telemetry/src/event.rs`，文档匹配测试就在那里。

**`duration_bucket`** ——来自 `app.session_started_at`（`crates/tui/src/tui/app.rs:1889`）的 `chrono` 差值。半开区间，单位秒：`lt_1m`（`d < 60`）、`1m_10m`（`60 ≤ d < 600`）、`10m_60m`（`600 ≤ d < 3600`）、`gt_60m`（`d ≥ 3600`）。

**`exit_class`** —— `clean | signal | panic | error`。**来自显式的 `AtomicU8`，绝不来自退出码。** `RunTerminationReason::Canceled` 映射到退出码 130（`crates/tui/src/core/runtime_contract/termination.rs:53`），与信号任务使用同一个值（`crates/tui/src/lib.rs:682`，128+SIGINT），因此基于退出码的推导会把每次 Esc 取消的回合都报成信号。这个 atomic 由 panic hook（`crates/tui/src/lib.rs:1582`）、信号任务（`:678-696`，在 `std::process::exit` 之前）以及干净路径上的 `RunTerminationReason::is_success()`（`crates/tui/src/core/runtime_contract/termination.rs:44-46`）设置——其他情况为 `error`。**不要**使用 `exec_failure_exit_code`（`crates/tui/src/lib.rs:10432`）：它只知道 `{75, 1}`，会把需要审批的退出码（3）报成普通失败。

**`cold_start_bucket`** ——来自 `startup_trace::elapsed_ms()`，它直接读取 `PROCESS_START`，并且独立于启动摘要的缓冲区清空（`crates/tui/src/startup_trace.rs:39-45`）。边界：`lt_250`、`250_1000`、`1000_3000`、`gte_3000`。非 TUI surface 上缺席。

**`providers`** ——`ProviderKind::as_str()`（`crates/config/src/provider_kind.rs:295`，来自封闭枚举的 `&'static str`；`Custom` 产生字面量 `"custom"`）的排序、去重数组。**API 按值接收 `ProviderKind`，绝不接收 `&str`。** 不要调用 `ProviderKind::parse` 或 `parse_config_identity`（`:300`、`:330`）——那些用于配置表解析。**不要读取** `provider_identity_for_persistence()`（`crates/tui/src/tui/app.rs:5049`）、`provider_id_for_persistence()`（`:5058`）、`ExecStreamMeta.provider_id`（`crates/tui/src/lib.rs:10220`）或 `PlannedTurnRoute.effective_provider_label`（`crates/tui/src/turn_route_plan.rs:189-193`）——当路由是 Custom 时，这四个都会返回客户自己的 `[providers.<name>]` 表键。这是该功能最可能的泄露点：它离天然接缝只差一个字段，而且 `/status` 已经会打印它（`crates/tui/src/commands/groups/config/status.rs:24-28`）。**任何 provider 都绝不发送 model id**——`crates/tui/src/safe_label.rs:11-15` 记录了一个事实：model id 可以是路径、URL 或本身就是凭据的部署 id。

**`counters`** ——封闭字段集。每次递增都发生在**调用点**，绝不在条件进入的处理器内部：

| 字段 | 来源锚点 |
|---|---|
| `turns` | `crates/tui/src/tui/ui/event_loop.rs:1856`——`execute_turn_end_observer_hook` 的*调用者*。绝不在其内部：该函数的第一条语句是 `if !app.hooks.has_hooks_for_event(HookEvent::TurnEnd) { return Ok(()); }`（`crates/tui/src/tui/ui.rs:1035`），而自然的未来优化会把该检查提升到调用点，从而悄悄把所有没有 hooks 的用户的计数器归零。 |
| `tool_calls` | `crates/tui/src/core/engine/tool_execution.rs:495`——与 surface 无关，exec 和 CLI 也会触发 |
| `fleet_dispatch` | `crates/tui/src/fleet/manager.rs:374`——单一漏斗（`create_queued_run_with_descriptor`），`create_run` 和 `create_queued_run` 都落入其中；在任一调用方计数都会使普通的 `fleet run` 被重复计数。 |
| `workflow_run` | 从 `parse_workflow_action`（`crates/tui/src/tools/workflow.rs:752-765`）返回的 **`WorkflowAction` 变体判别值**计数，绝不从 `input["action"]` 计数。`:775-779` 处的 JSON Schema 是发布*给模型*的——是声明，不是守卫；真正的解析还接受 `spawn\|wait\|list\|inspect\|stop\|abort`，其 `:761-763` 处的拒绝分支会原样嵌入模型字符串。 |
| `subagent_spawn` | `crates/tui/src/tui/ui/apply.rs:32` |
| `mcp_server_connected` | `crates/tui/src/mcp.rs:4254-4261` 快照中 `.connected` 的计数；绝不统计 `name`、`command_or_url` 或 `error`——服务器名是用户自选的，往往是内部基础设施 |
| `memory_search` | `crates/tui/src/tools/native_memory.rs:60-61` 处的工具名，在 tool_execution 瓶颈点计数 |
| `approval_modal_shown` | `crates/tui/src/tui/ui/event_loop.rs:2372`（`Event::ApprovalRequired` 的消费者，`crates/tui/src/core/events.rs:444`） |
| `approval_auto_allowed` | `crates/tui/src/core/engine.rs:5714`。只计数。绝不统计 `matched_rule`、`reason()`、命令或 argv——`auto_allow` 模式是用户编写的命令字符串（`crates/tui/src/command_safety.rs:35/309`） |
| `command_palette_open` | `crates/tui/src/tui/ui/event_loop.rs:3941` 和 `crates/tui/src/tui/mouse_ui.rs:1346` |

**`errors`** ——封闭字段集。每个值都是**变体判别值**，绝不是 `err.to_string()`：

| 字段 | 来源锚点 |
|---|---|
| `auth_preflight_failed` | `CredentialReadiness`（`crates/workflow/src/fleet_preflight.rs:37-58`）/ `ProviderAuthClass`（`crates/tui/src/provider_readiness.rs:32`）的判别值。只取判别值——`Missing { detail }` 携带自由文本 |
| `provider_http_4xx` | `status.as_u16() / 100 == 4`，在 `crates/tui/src/client/chat.rs:595` 和 `:673` 处、**在** `bail!` **之前**捕获。每字段一行，因为文档匹配测试会逐字段读取此表 |
| `provider_http_5xx` | `status.as_u16() / 100 == 5`，同样的捕获点 |
| `tool_denied_by_policy` | `crates/tui/src/core/engine/tool_execution.rs:512-531` 处 8 变体匹配的 `permission_denied` 分支 |
| `tool_timeout` | 同一匹配的 `timeout` 分支 |
| `network_error` | `retry_reason_label_and_human()` 的 `&'static str` 一半，`crates/tui/src/client.rs:2659` |

为什么只要判别值：`ToolError::PathEscape` 的 `Display` *就是*一个绝对路径（`crates/tools/src/lib.rs:61`）；`fim.rs:48-50` 的 `Display` *就是*模型发出的字面源码片段；`secrets/src/lib.rs:50` 的 `Display` 携带密钥库的绝对路径；每个 `LlmError` 变体都原样携带 provider 的原始 HTTP 主体（`crates/tui/src/llm_client/mod.rs:327`），而内容过滤器的 400 通常会回显提示词。

**`turn_wall`** ——按会话的计数直方图，绝不是按回合的事件。`lt_5s`、`5_30s`、`30_120s`、`gte_120s`。来源 `crates/tui/src/tui/ui/event_loop.rs:1857`，那里已经手握 `duration`。

### 事件：panic

由 panic hook **同步**追加，因为 `session_end` 可能永远写不出来。

```jsonc
{ "event": "panic", "site": "crates/tui/src/lib.rs:1582:5" }
```

`site` 来自 `panic_info.location()`（`crates/tui/src/lib.rs:1597-1600`）或 `Location::caller()`（`crates/tui/src/utils.rs:523`）。**allowlist 缩减，不是可选的：** 仅当 `file()` 以 `crates/` 开头时才原样发送；否则发送字面量 `"<dep>"`。必须匹配 `^crates/[A-Za-z0-9_/.-]+\.rs:\d+:\d+$` 或 `^<dep>$`。本仓库没有 `--remap-path-prefix`（没有 `.cargo/config.toml`；`Cargo.toml:69-74` 只设置 `lto`/`strip`/`codegen-units`），因此注册表依赖内部的 panic 会得到 `/Users/<builder>/.cargo/registry/src/…/ratatui-0.29.0/src/…`——**构建机器的用户名**，从每个用户的二进制里发送出去。

**panic 消息绝不发送。** `crates/tui/src/lib.rs:1590-1596` 处的 hook 从 payload 构建 `msg`；遥测绝不能读取它。切片（slicing）panic 会嵌入正在被切片的整个字符串，而本代码库在几十处切片用户和模型文本。

### 端点做什么——发布门槛，而非脚注

本节曾是配置任何非回环端点的门槛。端点现在默认已配置，因此这是对已存在服务的描述，而不是对可能存在的服务的承诺。

**它是什么。** `https://telemetry.codewhale.net/v1/telemetry` ——一个名为 `codewhale-telemetry-ingest` 的 Cloudflare Worker，其完整源码就在本仓库的 [`telemetry-ingest/`](../../telemetry-ingest/) 中。它是唯一的组件；路径上没有队列、没有代理、没有其他服务。它只写：Worker 中没有任何东西能回读已存储的内容，查询通过 Cloudflare 的 SQL API、以所有者的 token 带外进行。主机名刻意自描述，因此任何检查自己网络流量的人光看名字就能知道它是什么。

**它存储什么。** 本文档中的一切，仅此而已，存放在 Workers Analytics Engine——每个事件一行。`telemetry-ingest/src/schema.ts` 中的校验器是一个**封闭**字段集：批次任意位置的未知键都会以 `400` 拒绝整个批次。未来某个客户端 bug 开始附带路径、提示词或 provider 表名时，会被服务器拒绝，而不是被悄悄存储。`telemetry-ingest/test/schema-doc.test.ts` 会从*本文件*中解析出字段名和枚举拼写，并断言与校验器的集合相等；`telemetry-ingest/test/ingest.test.ts` 会发布 Rust 客户端自己钉住的 golden 批次，并断言它被逐字节接受——因此本文档、客户端和端点不会在没有红色测试的情况下彼此漂移。

批次在**采集时剥离 IP**。不存储、不记录 IP，也不与 `install_id` 关联——这是结构性的，而不是任何人都能翻转的设置。Analytics Engine 的一行恰好是 `_sample_interval`、`blob1`–`blob20`、`dataset`、`double1`–`double20`、`index1` 和 `timestamp`。这些列中的每一列都由 Worker 自己的 `writeDataPoint` 调用写入；没有隐式列，因此**没有 IP、国家或地理列**——即使代码想放也没有任何槽位可以占据。而且代码不可能想：

- handler 恰好读取**两个**请求头——`content-type` 和 `content-length`。其他任何东西，永远不会。
- 它从不触碰请求的 `cf` 属性，因此国家、colo、城市、地区、ASN、时区、坐标永远不在范围内。
- 构建每一存储行的函数根本看不到请求；它的输入类型是校验过的批次主体。
- 什么都不记日志。`invocation_logs` 在 `wrangler.jsonc` 中是关闭的——Cloudflare 将这类日志描述为"在调用上下文中携带 Cloudflare 可用的信息增强"，而这正是本服务不会保留的那类自动按请求记录——源码中也没有任何 `console.*` 调用。
- 限流以校验过主体中的 `install_id` 为键，绝不基于网络地址。按 IP 键控的限流器意味着这个 Worker 要处理 IP。它是更弱的限流器，也是正确的取舍。
- `telemetry-ingest/test/no-ip.test.ts` 将已发布的源码作为文本读取，如果出现任何地址或地理名称、读取的头集合增长到两个以上、新增 `console.*` 调用，或构造带主体的 `Response`，构建就会失败。

**保留期：三个月。** 这是 Analytics Engine 的固定窗口，不可配置，因此它是上限而非策略——没有任何设置能让它更长。

**客户端与存储之间没有第三方分析处理器。** 端点或运行时二进制中没有广告 SDK、分析 SDK、会话回放。

**每个响应都是带空主体的裸状态码**——`204` 接受、`400` schema 违规、`404`/`405` 路径或方法错误、`413` 过大、`415` 内容类型错误、`429` 被限流、`500` 内部错误。端点无法回显它收到或持有的内容，而且由于客户端会在任何非 2xx 时丢弃批次，拒绝对你而言在构造上就是不可见的，服务器错误也永远不会表现为客户端可见的失败。

`install_id` 每 90 天在客户端轮换（`install_id.json` 中的 `rotated_at`），因此没有任何单一标识符跨越很长的历史。这牺牲了纵向准确性，文档也如实说明：**任何由 `install_id` 推导出的计数都不是用户数。** 它是某个窗口内不同机器安装数的下界，并且会在一次轮换中少算一个回访用户。

**关闭它会删除本地保留的内容，而不是已经发送的内容。** `codewhale config set telemetry false` 会擦除你机器上的安装 id、缓冲区和 dry-run 记录，并停止后续一切。端点已经接受的行只由现在已消失的轮换随机 id 键控；它们随三个月窗口一起过期。没有删除 API，本文档也不会声称有。

### 所有者回读的内容——观察到的活跃安装数

从这些数据推导出的唯一产品指标是**观察到的活跃安装数（observed active installs）**：在一个 UTC 日内产生 `session_start` 事件的不同轮换匿名安装 id 的数量。这就是全部定义。它不是人数、不是账户数、也不是安装总数——id 按安装存在，每 90 天轮换，选择退出时被删除，因此任何由它推导出的数字都不可能是这些。

所有者的确切命令已签入仓库，因此例行查询是可审阅的代码，而不是从聊天里粘贴的 SQL：

```sh
cd telemetry-ingest
CF_ACCOUNT_ID=... CF_API_TOKEN=... npm run report:active-installs
```

它打印每日序列、完整 UTC 日上的 7 天趋势、最新摄入事件的时效性，以及——连同数字，在文本和 `--json` 输出中都有的——覆盖率注意事项：比遥测功能更老的客户端、选择退出的安装、不发送的环境（终止开关、fleet worker、离线关机、被丢弃的 flush）都是不可见的，因此每个计数都是下界；又因为 id 会轮换，周与周的比较不是留存指标。报告的读取路径本身也经过测试：安装 id 只出现在聚合内部，不选择任何 payload 列，措辞也绝不会漂移成把结果称为用户。（`npm run report:dau` 仍是同一报告的兼容别名。）

### 绝不收集的内容——公开红线清单

提示词；补全；工具参数；diff；补丁；文件内容；文件名；绝对或相对路径；git remote；仓库名；分支名；工作区提交 SHA；记忆条目；聊天历史；API 密钥、token、cookie 或 `Authorization` 头（包括任何断言密钥存在的布尔值）；任何种类的 model id；自定义 provider 表名；MCP 服务器名、命令或 URL；审批规则文本；错误消息主体；panic 消息文本；按事件的时间戳；按键；剪贴板；截图；麦克风；摄像头；位置；以及任何第三方广告或分析 SDK——运行时二进制中没有，也不得添加。

给实现者的两个具名陷阱。`crates/state/src/lib.rs` 在线程表上持久化 `git_sha`、`git_branch`、`git_origin_url`、`cwd` 和 `path`（`:93, :399, :653`）：一个接受 `Thread` 或 `ThreadMeta` 并 `derive(Serialize)` 的 payload 构建器一行就违反契约。**绝不在现有状态类型上派生 `Serialize`**——从零开始、用显式字段构建每个遥测结构体。而 `crates/core/src/lib.rs:1389-1398` 是整棵树中 `telemetry` 一词与 `prompt`、`base_url`、`has_api_key` 同处一个 JSON 对象的唯一位置。它是某人会复制的那一个对象。不要复制。

---
