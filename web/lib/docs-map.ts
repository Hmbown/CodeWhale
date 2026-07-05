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
  /**
   * Optional zh-Hans intro paragraph, shown on the rendered zh page above the
   * untranslated-body note. Plain text summary — not a translation of the doc.
   */
  zhIntro?: string;
  /** Repo source file(s) — the canonical markdown doc in the repo. */
  repoSource: string | string[];
  /** Whether this topic has a dedicated website page (vs. linking out). */
  hasPage: boolean;
  /** Category for grouping in the sidebar. */
  category:
    | "getting-started"
    | "workflows"
    | "core-concepts"
    | "reference"
    | "extending"
    | "operations";
}

/**
 * Slugs whose pages are hand-authored TSX under app/[locale]/docs/<slug>/.
 * Every other topic with `hasPage: true` is rendered from its repo Markdown
 * source(s) by the dynamic route app/[locale]/docs/[slug]/page.tsx.
 */
export const CUSTOM_DOC_PAGES = ["constitution", "modes", "tools"] as const;

export const DOC_TOPICS: DocTopic[] = [
  {
    id: "install",
    slug: "install",
    label: { en: "Install", zh: "安装" },
    description: {
      en: "npm, Cargo, Homebrew, Docker, Nix, Scoop, CNB mirror, and platform-specific notes.",
      zh: "npm、Cargo、Homebrew、Docker、Nix、Scoop、CNB 镜像及平台说明。",
    },
    zhIntro:
      "CodeWhale 支持 npm、Cargo、Homebrew、Docker、Nix、Scoop 等多种安装方式，并为无法稳定访问 GitHub 的用户提供 CNB 镜像。本页涵盖各平台的安装步骤、校验和验证与常见问题排查。",
    repoSource: "docs/INSTALL.md",
    hasPage: true,
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
    zhIntro:
      "本指南带你从首次运行走到日常工作流：会话管理、斜杠命令、键盘快捷键、模式切换与常用技巧，是新用户的推荐起点。",
    repoSource: ["docs/GUIDE.md", "docs/KEYBINDINGS.md"],
    hasPage: true,
    category: "getting-started",
  },
  {
    id: "restore",
    slug: "restore",
    label: { en: "Rollback & Restore", zh: "回滚与恢复" },
    description: {
      en: "Turn-level snapshots, /restore, and recovering from a bad turn without losing the conversation.",
      zh: "回合级快照、/restore 命令，以及在不丢失对话的前提下从错误回合恢复。",
    },
    zhIntro:
      "CodeWhale 会自动为工作区拍摄回合级快照——存放在独立的 side git 仓库中，绝不触碰你自己的 .git。用 /restore 回滚文件而保留对话，用 /undo 撤销最近一次修改。",
    repoSource: "docs/RESTORE.md",
    hasPage: true,
    category: "getting-started",
  },
  {
    id: "adding-context",
    slug: "adding-context",
    label: { en: "Adding Context", zh: "添加上下文" },
    description: {
      en: "Every input surface — @ file mentions, paste, /attach, vision, MCP resources, web tools — plus prompt assembly order and compaction.",
      zh: "全部输入方式 — @ 文件提及、粘贴、/attach、视觉、MCP 资源、网络工具 — 以及提示词组装顺序与压缩。",
    },
    repoSource: "docs/ADDING_CONTEXT.md",
    hasPage: true,
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
    zhIntro:
      "CodeWhale 通过 config.toml 配置：提供商与模型、环境变量优先级、项目级覆盖，以及每个运行时开关的完整参考。",
    repoSource: ["docs/CONFIGURATION.md", "docs/LEGACY_PATHS.md"],
    hasPage: true,
    category: "getting-started",
  },
  {
    id: "workflows",
    slug: "workflows",
    label: { en: "Common Workflows", zh: "常用工作流" },
    description: {
      en: "Short, copy-pasteable recipes: first real task, fixing tests, PR review, Fleet runs, local models, rollback.",
      zh: "简短可复制的实用配方：第一个真实任务、修测试、PR 审查、Fleet 运行、本地模型、回滚。",
    },
    repoSource: [
      "docs/workflows/first-real-task.md",
      "docs/workflows/investigate-before-editing.md",
      "docs/workflows/fix-failing-tests.md",
      "docs/workflows/review-a-pr.md",
      "docs/workflows/use-plan-agent-yolo.md",
      "docs/workflows/rollback-with-restore.md",
      "docs/workflows/fleet-run.md",
      "docs/workflows/local-models.md",
    ],
    hasPage: true,
    category: "workflows",
  },
  {
    id: "providers",
    slug: "providers",
    label: { en: "Providers & Models", zh: "提供商与模型" },
    description: {
      en: "Supported providers, model switching, local runtimes (vLLM, Ollama, SGLang), and Model Lab.",
      zh: "支持的提供商、模型切换、本地运行时（vLLM、Ollama、SGLang）和模型实验室。",
    },
    zhIntro:
      "CodeWhale 支持多家模型提供商（DeepSeek 为一等公民），可在会话中随时切换模型，也支持 vLLM、Ollama、SGLang 等本地运行时以及 OpenAI 兼容网关。",
    repoSource: ["docs/PROVIDERS.md", "docs/MODEL_LAB.md"],
    hasPage: true,
    category: "reference",
  },
  {
    id: "cost",
    slug: "cost",
    label: { en: "Cost & Route Honesty", zh: "成本与路由诚实" },
    description: {
      en: "The five explicit cost states, why CodeWhale never invents a price, /cost /tokens /cache, and single-authority route resolution.",
      zh: "五种明确的成本状态、CodeWhale 为何绝不虚构价格、/cost /tokens /cache，以及单一权威的路由解析。",
    },
    repoSource: "docs/COST.md",
    hasPage: true,
    category: "reference",
  },
  {
    id: "directory-structure",
    slug: "directory-structure",
    label: { en: "Directory Structure", zh: "目录结构" },
    description: {
      en: "What lives in $CODEWHALE_HOME (~/.codewhale) and repo-local .codewhale/: config, constitution, sessions, skills, Fleet state.",
      zh: "$CODEWHALE_HOME（~/.codewhale）与仓库本地 .codewhale/ 的内容：配置、宪法、会话、技能、Fleet 状态。",
    },
    repoSource: "docs/DIRECTORY_STRUCTURE.md",
    hasPage: true,
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
    repoSource: ["docs/CONSTITUTION.md", "docs/ARCHITECTURE.md"],
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
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "sessions",
    slug: "sessions",
    label: { en: "Sessions & Persistence", zh: "会话与持久化" },
    description: {
      en: "Session save/resume (--resume, --fresh), what survives restarts, and the Fleet vs. WhaleFlow vs. goal loop vs. sub-agents mental model.",
      zh: "会话保存与恢复（--resume、--fresh）、重启后保留的状态，以及 Fleet、WhaleFlow、目标循环与子 Agent 的心智模型。",
    },
    repoSource: "docs/SESSIONS.md",
    hasPage: true,
    category: "core-concepts",
  },
  {
    id: "extend",
    slug: "extend",
    label: { en: "Extend CodeWhale", zh: "扩展 CodeWhale" },
    description: {
      en: "Decision guide: AGENTS.md, constitution layers, skills, MCP, hooks, sub-agents, Fleet, WhaleFlow, Runtime API, ACP, bridges.",
      zh: "决策指南：AGENTS.md、宪法层、技能、MCP、钩子、子 Agent、Fleet、WhaleFlow、运行时 API、ACP、桥接。",
    },
    repoSource: "docs/EXTENDING.md",
    hasPage: true,
    category: "extending",
  },
  {
    id: "mcp",
    slug: "mcp",
    label: { en: "MCP", zh: "MCP" },
    description: {
      en: "Model Context Protocol — consuming and exposing tools via stdio and HTTP/SSE.",
      zh: "Model Context Protocol — 通过 stdio 和 HTTP/SSE 消费和暴露工具。",
    },
    zhIntro:
      "CodeWhale 既可以作为 MCP 客户端接入外部工具服务器（stdio 与 HTTP/SSE），也可以通过 codewhale mcp-server 将自身暴露为 MCP 服务器，供其他智能体或应用调用。",
    repoSource: "docs/MCP.md",
    hasPage: true,
    category: "extending",
  },
  {
    id: "skills",
    slug: "skills",
    label: { en: "Skills", zh: "技能" },
    description: {
      en: "Skill loading, invocation design, and the community skill ecosystem.",
      zh: "技能加载、调用设计和社区技能生态。",
    },
    repoSource: ["docs/SKILL_INVOCATION_DESIGN.md"],
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
    hasPage: true,
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
    hasPage: true,
    category: "extending",
  },
  {
    id: "fleet",
    slug: "fleet",
    label: { en: "Fleet / WhaleFlow", zh: "Fleet / WhaleFlow" },
    description: {
      en: "Durable task execution, fleet management, and WhaleFlow authoring.",
      zh: "持久任务执行、Fleet 管理和 WhaleFlow 编写。",
    },
    repoSource: ["docs/FLEET.md", "docs/WHALEFLOW_AUTHORING.md"],
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
