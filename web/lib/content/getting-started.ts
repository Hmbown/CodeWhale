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
 *     API key (recommended working-agreement setup); model replies require a provider —
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
      en: "One npm command installs the terminal runtime. Cargo, prebuilt archives, Docker, Nix, and China mirrors also work. All of them install published releases.",
      zh: "一条 npm 命令即可安装终端运行时。Cargo、预编译包、Docker、Nix 和中国镜像也可以。它们安装的都是已发布版本。",
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
      en: "Starts without any API key. A short setup, then the full interface. Look around in Plan mode, which is read-only. Model replies need a provider — that's the next step.",
      zh: "无需任何 API 密钥即可启动。简短设置后进入完整界面。先在只读的 Plan 模式里看看。模型回复需要提供商——那是下一步。",
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
      en: "Use a hosted API key, a gateway, or a local runtime with no key (Ollama, vLLM, SGLang). You pick the provider and the model. A model name never switches the provider for you.",
      zh: "用托管 API 密钥、网关，或不需要密钥的本地运行时（Ollama、vLLM、SGLang）。提供商和模型都由你来选。模型名不会替你切换提供商。",
    },
    commands: ["codewhale auth set --provider deepseek"],
    link: {
      href: "/models",
      label: { en: "Providers and models", zh: "提供商与模型" },
    },
  },
  {
    id: "fleet-workflow",
    title: { en: "Set up your ideal fleet", zh: "配置你的理想 fleet" },
    body: {
      en: "Add each provider you use with one auth set (local runtimes need none). Then run /fleet setup. It goes one member at a time — a semantic role, a model from any configured provider, a thinking tier, then an exact identity/route review — and saves the roster profile for this repo or for every repo on this machine. Runtime permissions stay separate. A single task needs none of this.",
      zh: "每个要用的提供商执行一次 auth set（本地运行时不需要）。然后运行 /fleet setup。它一次配置一个成员——语义角色、任意已配置提供商的模型、思考档位，再核对准确的身份与路由——并保存为花名册档案，可只用于本仓库或本机所有仓库。Runtime 权限始终独立。单个任务不需要这些。",
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
      en: "Run your own commands before and after tool calls, at turn end, and on session events, with per-project trust rules.",
      zh: "借助项目级信任规则，响应生命周期事件——工具调用前后、回合结束、会话事件。",
    },
  },
  {
    href: "/docs/modes",
    label: { en: "Modes and permissions", zh: "模式与权限" },
    note: {
      en: "Plan / Work / Operate and Ask / Auto-Review / Full Access: what each one allows.",
      zh: "Plan / Work / Operate 与 Ask / Auto-Review / Full Access：各自允许做什么。",
    },
  },
  {
    href: "/docs",
    label: { en: "Documentation hub", zh: "文档中心" },
    note: {
      en: "Every topic, searchable. Each page links to its source document in the repository.",
      zh: "所有主题均可搜索。每页都链接到仓库中的源文档。",
    },
  },
];
