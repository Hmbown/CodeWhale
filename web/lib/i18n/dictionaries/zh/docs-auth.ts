import type { DocsAuthDict } from "../types";

/**
 * Simplified-Chinese dictionary for `app/[locale]/docs/auth/page.tsx`.
 * Statements trace to docs/CONFIGURATION.md, docs/CODEWHALE_AGENT.md, docs/PROVIDERS.md.
 */
export const docsAuth: DocsAuthDict = {
  metaTitle: "账户与密钥 · Codewhale 文档",
  metaDescription: "提供商密钥与可选的 Codewhale 账户：各自如何设置、存放在哪里，以及哪些操作完全不需要账户。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "账户与密钥",
  overviewLead:
    "Codewhale 持有两种互不相关的凭证，把它们分开会更清楚。提供商密钥让模型能够回答。Codewhale 账户是可选的，它决定能否使用托管界面——云端 Agent 和聊天渠道。安装和运行本地 Runtime 既不需要账户，也不需要密钥。",
  credentials: [
    ["提供商密钥（BYOK）", "你自己的 DeepSeek、OpenAI、Anthropic、OpenRouter、本地运行时或任何其他路由的密钥。用 {authSet} 设置。模型回复需要它。"],
    ["Codewhale 账户", "由 {login} 发起的浏览器设备授权会话。可选。决定能否使用云端 Agent 与 Codewhale Agent 界面；本地从不需要。"],
  ],
  providerTitle: "提供商密钥",
  providerLead:
    "{authSet} 保存某一个提供商的密钥。进程级的 {apiKeyFlag} 仍然在单次运行中优先。{login} 不是提供商密钥命令：提供商凭证只能通过 {authSet} 配置。",
  accountTitle: "账户会话",
  accountLead: "{login} 与 {accountLogin} 是同一个浏览器设备授权流程。会话按所选的 {profile} 区分，旧的 {cloud} 写法仍然是别名。",
  accountCommands: [
    ["codewhale login", "通过浏览器设备授权流程登录。"],
    ["codewhale account status", "显示所选 profile 的会话。"],
    ["codewhale account logout", "移除该会话。"],
  ],
  storageTitle: "会话存放在哪里",
  storageLead:
    "账户会话优先使用操作系统的凭证管理器；在没有凭证管理器的环境（无头主机、SSH、容器）中自动回退到权限为 0600 的私有 Codewhale 密钥文件。旧的 {fileStoreEnv} 显式开关已弃用并被忽略。",
  vaultTitle: "账户自己的密钥库",
  vaultLead: "{keys} 管理已登录账户的自带密钥（BYOK）库，且永远不会显示密钥内容。",
  portableTitle: "迁移到另一台机器",
  portableLead:
    "{portable} 输出一个不含密钥的配置包：凭证和机器相关的键会被直接删除，而不是用打码占位符替代，因此这个文件可以安全携带，里面没有任何内容能以你的身份登录。",
  appTitle: "在网页上登录",
  appLead: "托管的 Codewhale 应用使用同一个账户。可以在那里登录或注册；本地 Runtime 没有它也照常工作。",
  appSignIn: "登录 →",
  appRegister: "注册 →",
  sourceNote: "源文档：docs/CONFIGURATION.md、docs/CODEWHALE_AGENT.md、docs/PROVIDERS.md · 修改时请同步更新 docs-map.ts。",
};
