import type { DocsEnterpriseDict } from "../types";

/** 中文对照见 `en/docs-enterprise.ts`。 */
export const docsEnterprise: DocsEnterpriseDict = {
  metaTitle: "企业审阅 · Codewhale 文档",
  metaDescription:
    "运营与安全审阅材料：本地运行时、自带密钥、第一方遥测、崩溃转储、沙箱与隔离网络控制。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "企业审阅",
  overviewLead:
    "Codewhale 是开源的编码 Agent，运行在你安装它的机器上。模型由你提供。审批策略、沙箱和遥测都是本地控制。app.codewhale.net 上的托管账户是可选的。本页只描述运行时里已经存在的行为——不是合规证书。",
  facts: [
    [
      "运行在你安装它的地方",
      "托管、网关和本地模型都经过同一套本地运行时、工具和权限栈。",
    ],
    [
      "除非你主动加入，凭据留在本地",
      "提供商密钥用 codewhale auth set 配置。可选的账户登录是另一套浏览器设备流。",
    ],
    [
      "没有第三方分析 SDK",
      "运行时里没有 PostHog、Sentry 或会话回放供应商。匿名用量统计是第一方的，并且可以关闭。",
    ],
    [
      "不编造认证",
      "本页不声称 SOC 2、ISO、SSO 或托管 SLA。请审阅源文档和代码。",
    ],
  ],
  telemetryTitle: "遥测与崩溃报告",
  telemetryLead:
    "启用时，会话把封闭的聚合 schema 发往 telemetry.codewhale.net。对话、代码、提示、文件、模型 id 和凭据永远不会被采集。panic 事件只携带允许列表内的 crates/… 位置——从不发送 panic 消息。崩溃转储留在磁盘上的 $CODEWHALE_HOME/crashes。",
  controlsTitle: "切断开关",
  controls: [
    [
      "codewhale config set telemetry false",
      "持久退出。停止采集并擦除本地遥测状态。",
    ],
    [
      "CODEWHALE_TELEMETRY=0",
      "单次运行的切断开关。不采集；不擦除磁盘状态。",
    ],
    [
      "telemetry_endpoint = \"\"",
      "保持启用但不联网。批次追加到本地 dry-run 文件。",
    ],
    [
      "仓库配置无法指向它",
      "一次检出不能打开遥测，也不能把它指向别的主机。",
    ],
  ],
  credentialsTitle: "凭据与自带密钥",
  credentialsLead:
    "托管提供商使用你的密钥。本地 vLLM、SGLang 和 Ollama 通常不需要密钥。codewhale account login 是可选的设备流登录；account keys list|set|remove 管理该保险库且不打印密钥。可移植配置导出会丢弃凭据，而不是就地打码。",
  policyTitle: "授权与沙箱",
  policyLead:
    "工具调用依次经过配置、模式、钩子、类型化权限、自动审阅、仓库法、人工审批，然后才是操作系统沙箱。后面的层仍可拦住或阻止。macOS 在探测成功时使用 Seatbelt；Linux bubblewrap 需显式启用；Windows 目前报告没有 OS 命令沙箱。项目覆盖层可以收紧策略，不能放宽。",
  airgapTitle: "隔离网络与受管桌面",
  airgapLead:
    "在隔离网络、公司代理或镜像托管的桌面上，持久写入 telemetry = false，并设置 [update] check_for_updates = false。CODEWHALE_HOME 会把崩溃转储、遥测文件和其余产品状态隔离到你选择的路径。",
  sessionsTitle: "并发会话",
  sessionsLead:
    "运行时线程存储是单所有者。第二个使用默认存储（$CODEWHALE_HOME/tasks/runtime）的 Codewhale 会在启动时失败。在 0.9.12 按会话划分存储（#5630）落地之前，把 CODEWHALE_RUNTIME_DIR 设到每个会话自己的路径。不要跨进程共享同一个存储，也不要去掉所有者锁——该存储不是多写安全的。codewhale web --tailscale 把回环界面发布到你的 Tailscale tailnet，不会打开局域网绑定或 Funnel。",
  claimsTitle: "本页不声明的内容",
  claimsLead:
    "没有 SOC 2、ISO 27001、FedRAMP 或 HIPAA 认证。这个开源运行时没有企业 SSO / SAML / SCIM 面。不要求使用托管应用。漏洞报告走 SECURITY.md，不要开公开 issue。",
  sourceNote: "来源文档：docs/ENTERPRISE.md · 更新时请同步修改 docs-map.ts。",
};
