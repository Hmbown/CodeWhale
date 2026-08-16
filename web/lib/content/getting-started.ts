/**
 * getting-started.ts — the canonical new-user path for codewhale.net.
 *
 * Four steps, in order: install → first offline session → provider connection
 * → fleet setup. Both the homepage band and the /docs/guide page
 * render from this module, so the path reads identically everywhere.
 *
 * TRUTH CONTRACT:
 *   - Step copy must match documented behavior in docs/GUIDE.md, docs/MODES.md,
 *     docs/PROVIDERS.md, and docs/FLEET.md. The runtime launches without any
 *     API key (constitution-first setup); model replies require a provider —
 *     hosted key or a keyless loopback route. Do not imply otherwise.
 *   - `href` values are locale-relative (no locale prefix); consumers render
 *     `/${locale}${href}` and the tests assert every target route exists.
 *
 * EXTENSION PATH FOR NEW LOCALES: add the locale key to each `{ en, zh }`
 * pair; commands stay locale-agnostic shell.
 */

import type { LocalizedText } from "./vocabulary";

export interface GuideStep {
  id: "install" | "first-session" | "connect-provider" | "fleet-workflow";
  title: LocalizedText;
  body: LocalizedText;
  /** Locale-agnostic shell commands shown for the step (may be empty). */
  commands: string[];
  /** Deeper-reading link; href is locale-relative. */
  link: { href: string; label: LocalizedText };
}

export const GETTING_STARTED_STEPS: GuideStep[] = [
  {
    id: "install",
    title: { en: "Install Codewhale", zh: "安装 Codewhale" },
    body: {
      en: "One npm command installs the dispatcher and terminal runtime. Cargo, archives, Docker, Nix, and China mirrors are documented alternatives — published releases only.",
      zh: "一条 npm 命令即可安装调度器和终端运行时。Cargo、预编译包、Docker、Nix 和中国镜像是有文档的备选渠道——只提供已发布版本。",
    },
    commands: ["npm install -g codewhale", "codewhale doctor"],
    link: {
      href: "/install",
      label: { en: "Full install guide", zh: "完整安装指南" },
    },
  },
  {
    id: "first-session",
    title: { en: "Open a first session — no key needed", zh: "打开第一个会话——无需密钥" },
    body: {
      en: "Launches without any API key: short constitution-first setup, then the full interface. Explore in Plan mode — always read-only. Model replies need a provider; that's the next step.",
      zh: "无需任何 API 密钥即可启动：简短的宪章优先设置，然后进入完整界面。在 Plan 模式中探索——始终只读。模型回复需要提供商；这正是下一步。",
    },
    commands: ["codewhale"],
    link: {
      href: "/docs/vocabulary",
      label: { en: "Learn the product nouns first", zh: "先了解产品名词" },
    },
  },
  {
    id: "connect-provider",
    title: { en: "Connect a provider", zh: "连接提供商" },
    body: {
      en: "Pick a supported route — hosted key, gateway, or keyless local runtime (Ollama, vLLM, SGLang). Provider and model stay explicit; reasoning and routing provenance stay separate, and unavailable values stay unavailable.",
      zh: "任选受支持的路由——托管密钥、网关，或 Ollama、vLLM、SGLang 等免密钥本地运行时。Provider 与模型始终明确；思考档位与路由来源分开记录，暂不可用的值保持暂不可用。",
    },
    commands: ["codewhale auth set --provider deepseek"],
    link: {
      href: "/models",
      label: { en: "Providers and models", zh: "提供商与模型" },
    },
  },
  {
    id: "fleet-workflow",
    title: { en: "Set up your ideal fleet", zh: "配置你的理想 Fleet" },
    body: {
      en: "Add every provider you use — one auth set per hosted route; keyless local runtimes need nothing — then author the team: /fleet setup walks one role at a time (a model from any configured provider, a thinking tier, permissions) and saves a reusable profile for this repo or every repo on this machine. Fleet state lives in the workspace ledger; ordinary single tasks need none of this.",
      zh: "把你用的每个提供商都接进来——每条托管路由执行一次 auth set，免密钥本地运行时无需配置——然后编写团队：/fleet setup 一次聚焦一个角色（可选任意已配置提供商的模型、思考档位与权限），保存为可复用档案，可仅用于本仓库或本机所有仓库。Fleet 状态保存在工作区台账中；普通单一任务不需要这些。",
    },
    commands: ["/fleet setup", "codewhale fleet status"],
    link: {
      href: "/docs/fleet",
      label: { en: "Fleet and Workflow docs", zh: "Fleet 与 Workflow 文档" },
    },
  },
];

/**
 * Where to go after the path — discovery links rendered at the end of the
 * /docs/guide page. Hooks are first-class here on purpose: they are the
 * supported extension point a new user should find without digging.
 */
export const GUIDE_NEXT_LINKS: { href: string; label: LocalizedText; note: LocalizedText }[] = [
  {
    href: "/docs/hooks",
    label: { en: "Hooks", zh: "钩子" },
    note: {
      en: "React to lifecycle events — before and after tool calls, on turn end, on session events — with project-local trust rules.",
      zh: "借助项目级信任规则，响应生命周期事件——工具调用前后、回合结束、会话事件。",
    },
  },
  {
    href: "/docs/modes",
    label: { en: "Modes and permission postures", zh: "模式与权限姿态" },
    note: {
      en: "Plan / Work / Operate and Ask / Auto-Review / Full Access, exactly as the runtime enforces them.",
      zh: "Plan / Work / Operate 与 Ask / Auto-Review / Full Access，与运行时实际执行的一致。",
    },
  },
  {
    href: "/docs",
    label: { en: "Documentation hub", zh: "文档中心" },
    note: {
      en: "Every topic, searchable, each page citing its source document in the repository.",
      zh: "所有主题均可搜索，每个页面都注明仓库中的源文档。",
    },
  },
];
