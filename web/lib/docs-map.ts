/**
 * docs-map.ts — canonical documentation registry for codewhale.net.
 *
 * Maps every first-class documentation topic area to its repo source file(s)
 * and website route. This is the single source of truth for the docs hub
 * sidebar, breadcrumbs, and drift/parity checks.
 *
 * EXTENSION PATH FOR NEW LOCALES:
 *   Labels are keyed by locale. Add a new locale column and update the page
 *   components that consume this map. The topic IDs, slugs, and repo sources
 *   are locale-agnostic.
 */

export interface DocTopic {
  /** Stable identifier used in routes and anchors. */
  id: string;
  /** URL slug for the docs sub-route (e.g. "install"). */
  slug: string;
  /** Label per locale. */
  label: { en: string; zh: string };
  /** Short description per locale. */
  description: { en: string; zh: string };
  /** Repo source file(s) — the canonical markdown doc in the repo. */
  repoSource: string | string[];
  /** Whether this topic has a dedicated website page (vs. linking out). */
  hasPage: boolean;
  /** Locale-relative website path when the page lives outside `/docs/<slug>`. */
  sitePath?: string;
  /** Category for grouping in the sidebar. */
  category: "getting-started" | "core-concepts" | "reference" | "extending" | "operations";
}

/** Sidebar and breadcrumb labels for each docs-map category. */
export const DOC_CATEGORY_LABELS: Record<DocTopic["category"], { en: string; zh: string }> = {
  "getting-started": { en: "Getting started", zh: "入门" },
  "core-concepts": { en: "Core concepts", zh: "核心概念" },
  reference: { en: "Reference", zh: "参考" },
  extending: { en: "Extending", zh: "扩展" },
  operations: { en: "Operations", zh: "运维" },
};

export const DOC_TOPICS: DocTopic[] = [
  {
    id: "install",
    slug: "install",
    label: { en: "Install", zh: "安装" },
    description: {
      en: "npm, Cargo, Homebrew, Docker, prebuilt binaries, CNB mirror, and where config lives.",
      zh: "npm、Cargo、Homebrew、Docker、预编译二进制、CNB 镜像，以及配置文件位置。",
    },
    repoSource: "docs/INSTALL.md",
    hasPage: true,
    sitePath: "install",
    category: "getting-started",
  },
  {
    id: "guide",
    slug: "guide",
    label: { en: "User Guide", zh: "使用指南" },
    description: {
      en: "First run, sessions, commands, keyboard shortcuts, and everyday workflows.",
      zh: "首次运行、会话、命令、快捷键和日常使用流程。",
    },
    repoSource: ["docs/GUIDE.md", "docs/KEYBINDINGS.md"],
    hasPage: true,
    category: "getting-started",
  },
  {
    id: "vocabulary",
    slug: "vocabulary",
    label: { en: "Vocabulary", zh: "产品名词" },
    description: {
      en: "The exact product nouns — Fleet, Workflow, Lane, Runtime; Plan / Work / Operate; Advisor; and explicit route provenance — plus measurement principles.",
      zh: "确切的产品名词——Fleet、Workflow、Lane、Runtime；Plan / Work / Operate；Advisor；明确的路由来源——以及测量原则。",
    },
    repoSource: ["docs/FLEET.md", "docs/MODES.md", "docs/public-surface-facts.json"],
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "configuration",
    slug: "configuration",
    label: { en: "Configuration", zh: "配置" },
    description: {
      en: "config.toml reference, environment variables, project overrides, and legacy paths.",
      zh: "config.toml 参考、环境变量、项目覆盖和旧版路径。",
    },
    repoSource: ["docs/CONFIGURATION.md", "docs/LEGACY_PATHS.md"],
    hasPage: true,
    category: "getting-started",
  },
  {
    id: "auth",
    slug: "auth",
    label: { en: "Account & Keys", zh: "账户与密钥" },
    description: {
      en: "Provider keys versus the optional Codewhale account: how each is set, where each is stored, and what needs no account.",
      zh: "提供商密钥与可选的 Codewhale 账户：各自如何设置、存放在哪里，以及哪些操作不需要账户。",
    },
    repoSource: ["docs/CONFIGURATION.md", "docs/CODEWHALE_AGENT.md"],
    hasPage: true,
    category: "getting-started",
  },
  {
    id: "providers",
    slug: "providers",
    label: { en: "Providers & Models", zh: "提供商与模型" },
    description: {
      en: "Supported providers, model switching, local runtimes (vLLM, Ollama, SGLang), and Model Lab.",
      zh: "支持的提供商、模型切换、本地运行时（vLLM、Ollama、SGLang）和模型实验室。",
    },
    repoSource: ["docs/PROVIDERS.md", "docs/MODEL_LAB.md"],
    hasPage: true,
    sitePath: "models",
    category: "reference",
  },
  {
    id: "constitution",
    slug: "constitution",
    label: { en: "Constitution", zh: "嵌套宪章" },
    description: {
      en: "Agent identity, authority hierarchy, evidence rules, and the nested law system.",
      zh: "Agent 自我模型、权威层次、证据规则和嵌套法律系统。",
    },
    repoSource: "docs/ARCHITECTURE.md",
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "modes",
    slug: "modes",
    label: { en: "Modes", zh: "模式" },
    description: {
      en: "Plan, Work, Operate modes and orthogonal permission posture.",
      zh: "Plan、Work、Operate 三种模式与正交权限姿态。",
    },
    repoSource: "docs/MODES.md",
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "tools",
    slug: "tools",
    label: { en: "Tools", zh: "工具" },
    description: {
      en: "Canonical action tools, deferred discovery, and replay compatibility.",
      zh: "小型核心工具、按需搜索、会话缓存与精确回放兼容边界。",
    },
    repoSource: ["docs/TOOL_SURFACE.md", "docs/RUNTIME_SIMPLIFICATION_DESIGN.md"],
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "work",
    slug: "work",
    label: { en: "Work Surface", zh: "工作面板" },
    description: {
      en: "The single To-do list, how the model sees it through its own tool results, and how work state flows to the sidebar, relay, and sub-agents.",
      zh: "唯一的 To-do 列表、模型如何通过自己的工具结果看到它，以及工作状态如何流向侧栏、relay 和子 Agent。",
    },
    repoSource: ["docs/TOOL_SURFACE.md", "docs/TOOL_LIFECYCLE.md"],
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "subagents",
    slug: "subagents",
    label: { en: "Sub-Agents", zh: "子 Agent" },
    description: {
      en: "Parallel execution, role types, transcript handles, and nesting.",
      zh: "并行执行、角色类型、transcript 句柄和嵌套。",
    },
    repoSource: "docs/SUBAGENTS.md",
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "mcp",
    slug: "mcp",
    label: { en: "MCP", zh: "MCP" },
    description: {
      en: "Model Context Protocol — consuming and exposing tools via stdio and HTTP/SSE.",
      zh: "Model Context Protocol — 通过 stdio 和 HTTP/SSE 消费和暴露工具。",
    },
    repoSource: "docs/MCP.md",
    hasPage: true,
    category: "extending",
  },
  {
    id: "hooks",
    slug: "hooks",
    label: { en: "Hooks", zh: "钩子" },
    description: {
      en: "Lifecycle hooks for pre/post tool execution, mode changes, and session events.",
      zh: "工具执行前后、模式切换和会话事件的生命周期钩子。",
    },
    repoSource: ["docs/rfcs/1364-hooks-lifecycle.md", "docs/CONFIGURATION.md"],
    hasPage: true,
    category: "extending",
  },
  {
    id: "skills",
    slug: "skills",
    label: { en: "Skills", zh: "技能" },
    description: {
      en: "Install, discover, trust, and load reusable instruction packages.",
      zh: "安装、发现、信任并加载可复用的指令包。",
    },
    repoSource: "docs/SKILLS.md",
    hasPage: false,
    category: "extending",
  },
  {
    id: "plugins",
    slug: "plugins",
    label: { en: "Plugins", zh: "插件" },
    description: {
      en: "Plugin discovery, installation, bundles, trust boundaries, and runtime lifecycle.",
      zh: "插件发现、安装、Bundle、信任边界与运行时生命周期。",
    },
    repoSource: ["docs/PLUGINS.md", "docs/PLUGIN_BUNDLES.md"],
    hasPage: false,
    category: "extending",
  },
  {
    id: "sandbox",
    slug: "sandbox",
    label: { en: "Sandbox & Approval", zh: "沙箱与审批" },
    description: {
      en: "Available Seatbelt (macOS), opt-in bubblewrap (Linux), platform gaps, and approval policies.",
      zh: "可用的 Seatbelt（macOS）、显式启用的 bubblewrap（Linux）、平台缺口和审批策略。",
    },
    repoSource: "docs/SANDBOX.md",
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "trust",
    slug: "trust",
    label: { en: "Security & Trust", zh: "安全与信任" },
    description: {
      en: "What stays local, what a hosted provider receives, approvals versus the OS sandbox, telemetry field by field, and where to report a vulnerability.",
      zh: "哪些留在本地、托管提供商收到什么、审批与 OS 沙箱的区别、逐项说明的遥测，以及在哪里报告漏洞。",
    },
    repoSource: ["docs/SANDBOX.md", "docs/AUTHORIZATION_ORDER.md", "docs/TELEMETRY.md"],
    hasPage: true,
    category: "reference",
  },
  {
    id: "runtime-api",
    slug: "runtime-api",
    label: { en: "Runtime API", zh: "运行时 API" },
    description: {
      en: "Public HTTP API for integrations, bridges, and automation.",
      zh: "用于集成、桥接和自动化的公开 HTTP API。",
    },
    repoSource: "docs/RUNTIME_API.md",
    hasPage: true,
    category: "extending",
  },
  {
    id: "web",
    slug: "web",
    label: { en: "Browser Client", zh: "浏览器客户端" },
    description: {
      en: "Run the embedded browser client on loopback, with its one-time bootstrap and session boundaries.",
      zh: "仅在本机回环地址运行内置浏览器客户端，了解一次性引导与会话边界。",
    },
    repoSource: "docs/WEB.md",
    hasPage: true,
    category: "extending",
  },
  // Fleet is the canonical customer noun; `/docs/pod` remains a
  // permanent compatibility redirect in app/[locale]/docs/pod/page.tsx.
  {
    id: "computers",
    slug: "computers",
    label: { en: "Cloud Computers", zh: "云端计算机" },
    description: {
      en: "Propose, confirm, and track a Daytona cloud agent against an explicit forge — fail-closed credentials and what is not built yet.",
      zh: "向明确指定的代码托管平台提议、确认并跟踪 Daytona 云端 Agent——凭证缺失即拒绝，以及尚未实现的部分。",
    },
    repoSource: ["docs/DAYTONA_CLOUD_DISPATCH.md", "docs/CODEWHALE_AGENT.md"],
    hasPage: true,
    category: "operations",
  },
  {
    id: "fleet",
    slug: "fleet",
    label: { en: "Fleet / Workflow", zh: "Fleet / Workflow" },
    description: {
      en: "Durable task execution, fleet roster management, and Workflow authoring.",
      zh: "持久任务执行、fleet 花名册管理和 Workflow 编写。",
    },
    repoSource: ["docs/FLEET.md", "docs/WORKFLOW_AUTHORING.md"],
    hasPage: true,
    category: "operations",
  },
  {
    id: "troubleshooting",
    slug: "troubleshooting",
    label: { en: "Troubleshooting", zh: "排障" },
    description: {
      en: "Common issues, diagnostics, operations runbook, and Docker notes.",
      zh: "常见问题、诊断、运维手册和 Docker 说明。",
    },
    repoSource: ["docs/OPERATIONS_RUNBOOK.md", "docs/DOCKER.md"],
    hasPage: true,
    category: "operations",
  },
  {
    id: "contribution",
    slug: "contribution",
    label: { en: "Contribution", zh: "贡献" },
    description: {
      en: "Contributing guide, agent ethos, contributor credits, and release process.",
      zh: "贡献指南、Agent 伦理、贡献者致谢和发布流程。",
    },
    repoSource: [
      "CONTRIBUTING.md",
      "docs/AGENT_ETHOS.md",
      "docs/CONTRIBUTORS.md",
      "docs/RELEASE_CHECKLIST.md",
    ],
    hasPage: false,
    category: "operations",
  },
];

/** Convenience lookup. */
export function getTopic(id: string): DocTopic | undefined {
  return DOC_TOPICS.find((t) => t.id === id);
}

/** Group topics by category for sidebar rendering. */
export function getTopicsByCategory(): Map<DocTopic["category"], DocTopic[]> {
  const map = new Map<DocTopic["category"], DocTopic[]>();
  for (const t of DOC_TOPICS) {
    const group = map.get(t.category) ?? [];
    group.push(t);
    map.set(t.category, group);
  }
  return map;
}

/** Resolve a topic to its on-site route or canonical repository document. */
export function docTopicHref(topic: DocTopic, locale: string): string {
  if (topic.sitePath) return `/${locale}/${topic.sitePath}`;
  if (topic.hasPage) return `/${locale}/docs/${topic.slug}`;
  const source = Array.isArray(topic.repoSource) ? topic.repoSource[0] : topic.repoSource;
  return `${REPO_DOCS_BASE}/${source}`;
}

/** Whether following a topic leaves codewhale.net for the source document. */
export function docTopicIsExternal(topic: DocTopic): boolean {
  return !topic.hasPage;
}

/** Repo source base URL for generating direct links. */
export const REPO_DOCS_BASE = "https://github.com/Hmbown/CodeWhale/blob/main";
