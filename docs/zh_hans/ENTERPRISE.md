# 企业审阅

这是在公司环境中运行 Codewhale 的运营与安全审阅材料。它只描述运行时里已经存在的行为，不是合规证书、托管服务 SLA，也不表示必须使用托管控制面。

Codewhale 是开源的编码 Agent，运行在你安装它的机器上。模型由你提供。审批策略、沙箱和遥测都是本地控制。`app.codewhale.net` 上的托管账户是可选的。

## 审阅者应视为既定事实的内容

- **二进制运行在你安装它的地方。** 托管、网关和本地模型都经过同一套本地运行时、工具和权限栈。
- **除非你主动登录账户，凭据留在本机。** 提供商密钥用 `codewhale auth set --provider <provider>` 配置。可选的 Codewhale 账户（`codewhale account login`）是另一套浏览器设备流，可以保存经过脱敏的 BYOK 保险库。
- **本文不编造认证。** 这里没有 SOC 2、ISO 或 SSO 声明。请审阅所链接的源文档和代码。

## 永远不会离开本机的数据

产品遥测 schema 是封闭的，并由本仓库的测试锁定。Codewhale 不收集对话、代码、提示、文件、文件/仓库/分支名、模型内容、模型 id 或凭据。它不发送按轮次或按工具的时间线。

字段级合同见 [TELEMETRY.md](../TELEMETRY.md)。那里的公开红线列表是权威；本页不会放宽它。

## 遥测与崩溃报告

Codewhale **不**嵌入 PostHog、Sentry 或任何第三方分析 / 崩溃 SDK。在遥测开启时，匿名用量计数和 panic *位置* 发往第一方接收端 `https://telemetry.codewhale.net/v1/telemetry`。接收端源码在本仓库 [`telemetry-ingest/`](../../telemetry-ingest/)。

| 控制 | 效果 |
|---|---|
| `codewhale config set telemetry false` | 持久退出。停止采集并擦除本地遥测状态。 |
| `CODEWHALE_TELEMETRY=0` | 单次运行的切断开关。不采集；不擦除磁盘状态。 |
| `codewhale --telemetry false` | 同一切断开关，仅对一条命令生效。 |
| `telemetry_endpoint = ""` | 保持启用但不联网。批次追加到 `$CODEWHALE_HOME/telemetry/dryrun.jsonl`。 |

仓库内的 `.codewhale/config.toml` 不能设置 `telemetry` 或 `telemetry_endpoint`。工作区 `.env` 也不能。别人的检出不能打开你的遥测，也不能把它指向别的主机。

**IT / MDM 下限。** 在用户配置里持久写入 `telemetry = false`，或在受管环境里设置 `CODEWHALE_TELEMETRY=0`。文件设置是下限：`--telemetry true` 和 `CODEWHALE_TELEMETRY=1` 都赢不了它。

**崩溃留在本地。** Panic 转储和致命信号标记写到 `$CODEWHALE_HOME/crashes`（否则是 `~/.codewhale/crashes`）。若遥测已武装，进程还可以发送只携带允许列表内 `crates/…` 源位置的 `panic` 事件——从不发送 panic 消息。Fleet worker 从不发送遥测。

首次交互启动会在终端就绪后显示一条本地化、非阻塞的说明。在该说明真正画出之前，遥测保持未武装。`/settings` 仍是日常开关。

## 凭据与账户登录

- **提供商 BYOK。** `codewhale auth set --provider <provider>` 把密钥写入用户配置。托管提供商使用你的凭据；本地 vLLM、SGLang 和 Ollama 通常不需要密钥。
- **可选账户。** `codewhale account login` 对 `app.codewhale.net` 启动 Codewhale 浏览器设备流。`codewhale account keys list|set|remove` 管理该账户的 BYOK 保险库，不打印密钥值。
- **密钥存储。** 账户会话优先使用操作系统凭据管理器；在无头主机、SSH 和容器上自动回退到权限为 `0600` 的 Codewhale 私钥文件。
- **可移植配置。** `codewhale config export --portable` 写出不含密钥的包。凭据和机器相关键会被丢弃，而不是就地打码。

## 授权、模式与沙箱

工具调用不是单一的允许/拒绝位。交互引擎按顺序评估配置、模式、钩子、类型化 `permissions.toml` 规则、自动审阅、仓库法、人工审批，然后才是执行沙箱。后面的层仍可拦住或阻止一次调用。见 [AUTHORIZATION_ORDER.md](../AUTHORIZATION_ORDER.md)。

运营者决定 Agent 在不问你的情况下能做多少：

- **模式。** Plan 只读。Work 和 Operate 改变推进方式，并不替换权限栈。
- **权限姿态。** Ask、Auto-Review 和 Full Access。Full Access 不是沙箱授权。
- **操作系统命令沙箱。** macOS 在探测成功时使用 Seatbelt。Linux bubblewrap 需显式启用（`prefer_bwrap = true`）。Windows 目前报告没有 OS 命令沙箱。审批策略和感知工作区的文件工具仍然生效。见 [SANDBOX.md](../SANDBOX.md)。
- **项目覆盖层** 可以收紧 `approval_policy`、`sandbox_mode` 或 shell 可用性，但不能放宽它们。

## 审计面

- `~/.codewhale/audit.log` 记录解析后的**键名**，从不记录值。
- `/config audit` 显示 TUI 可以现场改动的文档化键。
- 工具审计事件（`tool.repo_law_decision`、`tool.auto_review`）携带标签，不携带提示文本或密钥值。
- Fleet 账本记录保存 `slack` 或 `webhook` 这类审计标签，不保存消息正文。

这些是本地日志，不是托管 SIEM 集成。

## 隔离网络与受管桌面

```toml
telemetry = false

[update]
check_for_updates = false
```

```sh
CODEWHALE_TELEMETRY=0 codewhale
```

启动时的更新检查从不阻塞一轮对话，离线时静默失败。在隔离网络、公司代理或镜像托管的桌面上应关闭它。从 [INSTALL.md](../INSTALL.md) 中经过校验的渠道安装——npm、Cargo、Homebrew、Docker，或带校验和的 `install.sh` 二进制。

`CODEWHALE_HOME` 会把全部产品状态（包括崩溃转储和遥测文件）隔离到你选择的路径。

## Fleet 与远程控制

- Fleet worker 不发送产品遥测。
- Fleet 的 `security_policy` 和 worker 的 `trust_level` 是派发运行的权限信封。见 [FLEET.md](../FLEET.md)。
- `/rc` 可在一次性浏览器批准后，把当前会话租给 `app.codewhale.net`。终端仍是可读的安全面；中断仍然有效。断开连接会保持本地输入锁定，直到租约过期，这样两个控制器不会竞争。

## 本文不声明的内容

- 没有 SOC 2、ISO 27001、FedRAMP 或 HIPAA 认证。
- 这个开源运行时没有企业 SSO / SAML / SCIM 面。
- 不承诺必须使用托管应用，也不承诺它能替代本地策略。
- 没有第三方分析、会话回放或崩溃报告供应商。

漏洞报告走 [SECURITY.md](../../SECURITY.md)，不要开公开 issue。
