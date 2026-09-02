/**
 * docs-tasks.ts — the task-based index of codewhale.net documentation.
 *
 * `docs-map.ts` answers "what topics exist"; this registry answers "I am
 * trying to do X — where do I go". Every task points at a first-party route
 * (locale-relative, so `/${locale}${href}` always exists) and names the
 * topic it belongs to, so the hub can search tasks and topics together and
 * `docs-tasks.test.ts` can prove every target resolves.
 *
 * TRUTH CONTRACT: a task may only describe behaviour the target page (and
 * its repository source document) actually documents. Keep the verbs
 * concrete and keep the list short enough to read in one screen.
 *
 * Labels are `{ en, zh }` pairs like docs-map.ts; other locales fall back to
 * English through `pickText`.
 */
import type { LocalizedText } from "./content/vocabulary";
import { DOC_TOPICS, type DocTopic } from "./docs-map";

export interface DocTask {
  id: string;
  label: LocalizedText;
  description: LocalizedText;
  /** Locale-relative route, e.g. "/install" or "/docs/modes". */
  href: string;
  /** Owning docs-map topic id, for grouping and source attribution. */
  topicId: DocTopic["id"];
  /** Extra search words, both languages, lowercase not required. */
  keywords: LocalizedText;
}

export const DOC_TASKS: DocTask[] = [
  {
    id: "install",
    label: { en: "Install on macOS, Linux, or Windows", zh: "在 macOS、Linux 或 Windows 上安装" },
    description: {
      en: "One npm command, or Cargo, Homebrew, Docker, prebuilt archives, and China mirrors.",
      zh: "一条 npm 命令，或 Cargo、Homebrew、Docker、预编译包与中国镜像。",
    },
    href: "/install",
    topicId: "install",
    keywords: { en: "download setup brew cargo docker termux binary", zh: "下载 安装 二进制 镜像" },
  },
  {
    id: "first-session",
    label: { en: "Open a first session without a key", zh: "不用密钥打开第一个会话" },
    description: {
      en: "Start the runtime, look around in read-only Plan mode, then connect a provider.",
      zh: "启动 Runtime，在只读的 Plan 模式里看看，再连接提供商。",
    },
    href: "/docs/guide",
    topicId: "guide",
    keywords: { en: "getting started quickstart tutorial first run", zh: "入门 快速开始 教程 首次运行" },
  },
  {
    id: "connect-provider",
    label: { en: "Connect a provider key (BYOK)", zh: "连接提供商密钥（BYOK）" },
    description: {
      en: "DeepSeek, OpenAI, Anthropic, OpenRouter, or a local runtime — your key, your bill.",
      zh: "DeepSeek、OpenAI、Anthropic、OpenRouter 或本地运行时——你的密钥，你的账单。",
    },
    href: "/models",
    topicId: "providers",
    keywords: { en: "api key model switch ollama vllm sglang openai-compatible", zh: "密钥 模型 切换 本地模型" },
  },
  {
    id: "choose-mode",
    label: { en: "Choose Plan, Work, or Operate", zh: "选择 Plan、Work 或 Operate" },
    description: {
      en: "Read-only planning, execution, or Fleet orchestration — and the approval posture beside it.",
      zh: "只读规划、执行或 Fleet 编排——以及旁边独立的审批姿态。",
    },
    href: "/docs/modes",
    topicId: "modes",
    keywords: { en: "mode tab shift+tab ask auto-review full access", zh: "模式 审批 权限" },
  },
  {
    id: "approve-commands",
    label: { en: "Decide which commands may run", zh: "决定哪些命令可以执行" },
    description: {
      en: "Approval posture, the OS sandbox per platform, and why an approval is not a sandbox.",
      zh: "审批姿态、各平台的 OS 沙箱，以及为什么批准不等于沙箱。",
    },
    href: "/docs/sandbox",
    topicId: "sandbox",
    keywords: { en: "permission approval seatbelt bwrap policy", zh: "权限 审批 沙箱" },
  },
  {
    id: "sign-in",
    label: { en: "Sign in to a Codewhale account", zh: "登录 Codewhale 账户" },
    description: {
      en: "The optional account, where its session lives, and what needs no account at all.",
      zh: "可选的账户、会话存放位置，以及哪些操作完全不需要账户。",
    },
    href: "/docs/auth",
    topicId: "auth",
    keywords: { en: "login account keys vault profile logout register", zh: "登录 账户 密钥库 注册" },
  },
  {
    id: "cloud-computer",
    label: { en: "Dispatch a task to a cloud computer", zh: "把任务派发到云端计算机" },
    description: {
      en: "Propose, confirm, and track a Daytona cloud agent against an explicit forge.",
      zh: "向明确指定的代码托管平台提议、确认并跟踪一个 Daytona 云端 Agent。",
    },
    href: "/docs/computers",
    topicId: "computers",
    keywords: { en: "daytona dispatch cloud agent remote github cnb gitee", zh: "云端 派发 远程 计算机" },
  },
  {
    id: "parallel-agents",
    label: { en: "Run sub-agents in parallel", zh: "并行运行子 Agent" },
    description: {
      en: "Roles, context forking, worktree isolation, and the concurrency caps.",
      zh: "角色、上下文分叉、工作树隔离与并发上限。",
    },
    href: "/docs/subagents",
    topicId: "subagents",
    keywords: { en: "agent worker scout reviewer worktree concurrency", zh: "子代理 并行 角色 工作树" },
  },
  {
    id: "mcp-server",
    label: { en: "Connect an MCP server", zh: "连接一个 MCP 服务器" },
    description: {
      en: "Consume tools over stdio or HTTP/SSE, or expose Codewhale as a server.",
      zh: "通过 stdio 或 HTTP/SSE 使用工具，或把 Codewhale 暴露为服务器。",
    },
    href: "/docs/mcp",
    topicId: "mcp",
    keywords: { en: "model context protocol tools stdio sse", zh: "工具 协议 服务器" },
  },
  {
    id: "hooks",
    label: { en: "Run something before or after a tool", zh: "在工具执行前后运行自定义逻辑" },
    description: {
      en: "Lifecycle hooks for tool execution, mode changes, and session events.",
      zh: "工具执行、模式切换和会话事件的生命周期钩子。",
    },
    href: "/docs/hooks",
    topicId: "hooks",
    keywords: { en: "hook lifecycle pre post event", zh: "钩子 生命周期 事件" },
  },
  {
    id: "fleet",
    label: { en: "Write a Workflow for a Fleet", zh: "为 fleet 编写 Workflow" },
    description: {
      en: "Durable task execution, roster management, and Workflow authoring.",
      zh: "持久任务执行、成员管理和 Workflow 编写。",
    },
    href: "/docs/fleet",
    topicId: "fleet",
    keywords: { en: "fleet workflow lane operate durable", zh: "编排 持久 工作流" },
  },
  {
    id: "browser-client",
    label: { en: "Use the browser client", zh: "使用浏览器客户端" },
    description: {
      en: "Run the embedded web client on loopback with its one-time bootstrap.",
      zh: "在本机回环地址运行内置网页客户端，了解一次性引导。",
    },
    href: "/docs/web",
    topicId: "web",
    keywords: { en: "web ui localhost loopback", zh: "网页 客户端 本机" },
  },
  {
    id: "automate",
    label: { en: "Automate with the Runtime API", zh: "用运行时 API 做自动化" },
    description: {
      en: "The public HTTP API for integrations, bridges, and scripts.",
      zh: "用于集成、桥接和脚本的公开 HTTP API。",
    },
    href: "/docs/runtime-api",
    topicId: "runtime-api",
    keywords: { en: "http api exec acp integration", zh: "接口 集成 脚本" },
  },
  {
    id: "troubleshoot",
    label: { en: "Fix a failing install or session", zh: "修复失败的安装或会话" },
    description: {
      en: "Common incidents, diagnostics, the operations runbook, and Docker notes.",
      zh: "常见问题、诊断、运维手册和 Docker 说明。",
    },
    href: "/docs/troubleshooting",
    topicId: "troubleshooting",
    keywords: { en: "error crash doctor diagnose recover", zh: "错误 崩溃 诊断 恢复" },
  },
  {
    id: "trust",
    label: { en: "Understand what leaves your machine", zh: "了解哪些数据会离开你的机器" },
    description: {
      en: "The hosted-provider boundary, the sandbox, telemetry field by field, and how to turn it off.",
      zh: "托管提供商边界、沙箱、逐项说明的遥测，以及如何关闭。",
    },
    href: "/docs/trust",
    topicId: "trust",
    keywords: { en: "privacy security telemetry data vulnerability report", zh: "隐私 安全 遥测 数据 漏洞" },
  },
];

/** The owning topic for a task, or undefined if the registry drifted. */
export function taskTopic(task: DocTask): DocTopic | undefined {
  return DOC_TOPICS.find((t) => t.id === task.topicId);
}

/** Lowercase haystack across both languages, the route, and the topic. */
export function docTaskHaystack(task: DocTask): string {
  return [
    task.id,
    task.label.en,
    task.label.zh,
    task.description.en,
    task.description.zh,
    task.keywords.en,
    task.keywords.zh,
    task.href,
    task.topicId,
  ]
    .join(" ")
    .toLowerCase();
}
