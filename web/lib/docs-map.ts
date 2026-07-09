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
  /** Category for grouping in the sidebar. */
  category: "getting-started" | "core-concepts" | "reference" | "extending" | "operations";
}

export const DOC_TOPICS: DocTopic[] = [
  {
    id: "install",
    slug: "install",
    label: { en: "Install", zh: "安装" },
    description: {
      en: "npm, Cargo, Homebrew, Docker, Nix, Scoop, CNB mirror, and platform-specific notes.",
      zh: "npm、Cargo、Homebrew、Docker、Nix、Scoop、CNB 镜像及平台说明。",
    },
    repoSource: "docs/INSTALL.md",
    hasPage: false,
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
    hasPage: false,
    category: "getting-started",
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
    hasPage: false,
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
    hasPage: false,
    category: "reference",
  },
  {
    id: "constitution",
    slug: "constitution",
    label: { en: "Constitution", zh: "嵌套宪法" },
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
      en: "Plan, Agent, YOLO modes and orthogonal approval policies.",
      zh: "Plan、Agent、YOLO 三种模式与正交审批策略。",
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
      en: "Typed tool surface, tool lifecycle, and the curated tool catalog.",
      zh: "类型化工具集、工具生命周期和精选工具目录。",
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
    hasPage: false,
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
    hasPage: false,
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
    repoSource: "docs/rfcs/1364-hooks-lifecycle.md",
    hasPage: false,
    category: "extending",
  },
  {
    id: "sandbox",
    slug: "sandbox",
    label: { en: "Sandbox & Approval", zh: "沙箱与审批" },
    description: {
      en: "seatbelt (macOS), landlock (Linux), Windows containment, and approval policies.",
      zh: "seatbelt（macOS）、landlock（Linux）、Windows 隔离和审批策略。",
    },
    repoSource: "docs/SANDBOX.md",
    hasPage: false,
    category: "core-concepts",
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
    hasPage: false,
    category: "extending",
  },
  {
    id: "fleet",
    slug: "fleet",
    label: { en: "Fleet / Workflow", zh: "Fleet / Workflow" },
    description: {
      en: "Durable task execution, fleet management, and Workflow authoring.",
      zh: "持久任务执行、Fleet 管理和 Workflow 编写。",
    },
    repoSource: ["docs/FLEET.md", "docs/WORKFLOW_AUTHORING.md"],
    hasPage: false,
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
    hasPage: false,
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
export function getTopicsByCategory(): Map<string, DocTopic[]> {
  const map = new Map<string, DocTopic[]>();
  for (const t of DOC_TOPICS) {
    const group = map.get(t.category) ?? [];
    group.push(t);
    map.set(t.category, group);
  }
  return map;
}

/** Repo source base URL for generating direct links. */
export const REPO_DOCS_BASE = "https://github.com/Hmbown/CodeWhale/blob/main";
