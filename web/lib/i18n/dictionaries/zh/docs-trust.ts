import type { DocsTrustDict } from "../types";

/**
 * Simplified-Chinese dictionary for `app/[locale]/docs/trust/page.tsx`.
 * Statements trace to docs/SANDBOX.md, docs/AUTHORIZATION_ORDER.md,
 * docs/TELEMETRY.md, and docs/public-surface-facts.json.
 */
export const docsTrust: DocsTrustDict = {
  metaTitle: "安全与信任 · Codewhale 文档",
  metaDescription:
    "哪些内容留在你的机器上、托管提供商会收到什么、审批与 OS 沙箱有何区别、遥测发送什么以及如何关闭，以及在哪里报告安全漏洞。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "安全与信任",
  overviewLead:
    "这一页说明 Codewhale 如何处理你的代码、命令和数据——按实际实现来写，而不是按意图来写。每一条都取自仓库中的文档，事实门禁会在每次构建时核对。",
  boundaryTitle: "你的数据去了哪里",
  boundaries: [
    ["本地 Runtime", "Runtime、工作区状态和审计日志都留在你的机器上。"],
    ["托管提供商", "你选择的托管提供商会收到本轮推理所需的上下文。中间没有强制经过 Codewhale 的中继。"],
    ["本地推理", "回环地址的本地模型路由（vLLM、Ollama、SGLang）可以让推理完全留在本机。"],
    ["账户", "本地 Runtime 不需要账户。"],
    ["Plan 模式", "Plan 是只读的。"],
  ],
  approvalTitle: "审批不是沙箱",
  approvalLead:
    "审批姿态——Ask、Auto-Review、Full Access——决定一条被提议的命令在执行前是否先展示给你。某一层的批准从来不是全局放行：后面的层仍然可以要求审查或阻止调用，而且批准也不等于操作系统级的沙箱授权。完整的模型工具调用流水线有九个有序的层，从有效配置、钩子、类型化权限规则、仓库法则、人工审批，直到执行沙箱。",
  sandboxTitle: "各平台的 OS 沙箱",
  sandboxLead: "这里只描述已接入命令执行路径的行为，Codewhale 会报告它实际选用的机制。",
  sandboxes: [
    ["macOS — Seatbelt", "sandbox-exec 的运行时探测成功时自动启用。报告为 {seatbelt}。"],
    ["Linux — bubblewrap", "需显式启用：{preferBwrap} 且 /usr/bin/bwrap 可执行。报告为 {bwrap}。"],
    ["没有 bwrap 的 Linux", "默认没有 OS 包装层。报告为 {none}。"],
    ["Windows", "当前实现没有 OS 包装层。报告为 {none}。"],
    ["外部服务", "{opensandbox} 会把执行路由到兼容 OpenSandbox 的服务。"],
  ],
  sandboxNote:
    "仓库中还有一个 seccomp 模块和一份未来的 Windows 辅助程序契约。两者都没有接入子命令启动，所以 Codewhale 不会宣传它们：只存在于源码中的沙箱代码不能证明某条命令受到了限制。",
  telemetryTitle: "遥测，精确到每一项",
  telemetryLead: "匿名使用计数默认开启，由一次不阻塞的首次运行提示告知，可以立即并永久关闭。",
  telemetry: [
    ["永远不收集", "对话、代码、prompt、文件、文件/仓库/分支名、模型内容、凭据，以及任何逐轮或逐工具的时间线。"],
    ["会发送", "版本与平台类别、会话时长与结果、功能与错误计数、封闭枚举，以及一个每 90 天轮换的随机安装 id。"],
    ["端点", "{endpoint}——第一方 Cloudflare Worker，源码在仓库的 telemetry-ingest/ 目录下。"],
    ["存储", "没有 IP、国家或地理位置列——这是结构上的，不是一个设置。不写日志。保留期固定为三个月。"],
    ["自己审计", "设置 {dryRun}：批次会追加写入你机器上的 {dryRunFile}，与服务器本应收到的内容逐字节一致，且不会构造任何 HTTP 客户端。"],
    ["关闭", "{configOff} 或 {envOff}。"],
  ],
  auditTitle: "本地审计日志",
  auditLead: "敏感事件——凭证、审批和提权事件——会尽力追加写入 {auditLog}。写入失败会被记录而不是隐藏。提供商的 token 与缓存用量在可用时会在本地显示。",
  reportTitle: "报告安全漏洞",
  reportLead: "安全报告请通过邮件发给维护者，而不是提交公开 issue。请附上页眉中的版本号，如有复现步骤也请一并提供。",
  reportCta: "给维护者发邮件",
  sourceNote: "源文档：docs/SANDBOX.md、docs/AUTHORIZATION_ORDER.md、docs/TELEMETRY.md、docs/public-surface-facts.json · 修改时请同步更新 docs-map.ts。",
};
