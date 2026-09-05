/**
 * vocabulary.ts — shared, locale-aware product vocabulary for codewhale.net.
 *
 * This module is the single source of truth for the exact product nouns a new
 * user meets on the site: the Fleet/Workflow/Lane/Runtime execution nouns,
 * the Plan/Work/Operate + Ask/Auto-Review/Full Access control vocabulary, the
 * public Advisor role, and the fields that make route provenance legible.
 *
 * TRUTH CONTRACT:
 *   - `short.en` for every product term MUST equal the verbatim definition in
 *     docs/public-surface-facts.json → product.terminology, which is itself
 *     pinned verbatim against docs/FLEET.md by public-surface-contract.test.ts.
 *   - Mode and posture names MUST equal matrix.control.modes /
 *     matrix.control.permissionPostures (pinned against docs/MODES.md).
 *   - No marketing adjectives. Each description states behavior and boundary.
 *
 * EXTENSION PATH FOR NEW LOCALES (localization lane):
 *   Every user-facing string is a `{ en, zh }` pair. Add the new locale key to
 *   each pair (and widen the `LocalizedText` type) — the consuming components
 *   and tests pick it up without structural changes. The tests assert key
 *   parity across locales, so a missing translation fails deterministically.
 */

export interface LocalizedText {
  en: string;
  zh: string;
}

export interface ProductTerm {
  /** The exact product noun. Never translate the noun itself. */
  term: "Fleet" | "Workflow" | "Lane" | "Runtime";
  /** One-line definition; `en` is verbatim from docs/public-surface-facts.json. */
  short: LocalizedText;
  /** One-sentence elaboration used on docs pages. */
  long: LocalizedText;
}

export const PRODUCT_TERMS: ProductTerm[] = [
  {
    term: "Fleet",
    short: {
      en: "the user's model inventory: who is in the roster and which member is selected",
      zh: "用户的模型清单：花名册中有哪些成员，以及选中了哪一位",
    },
    long: {
      en: "Fleet records member IDs and names, semantic roles, provider/model identities, and roster state.",
      zh: "Fleet 记录成员 ID 和名称、语义角色、提供商/模型身份以及花名册状态。",
    },
  },
  {
    term: "Workflow",
    short: { en: "what order the work follows", zh: "工作按什么顺序进行" },
    long: {
      en: "What order the work follows: phases, gates, budgets, replay, and fan-in.",
      zh: "工作按什么顺序进行：阶段、门禁、预算、回放和汇总。",
    },
  },
  {
    term: "Lane",
    short: { en: "one running Workflow instance", zh: "一个正在运行的 Workflow 实例" },
    long: {
      en: "One running Workflow instance and its live progress.",
      zh: "一个正在运行的 Workflow 实例及其实时进度。",
    },
  },
  {
    term: "Runtime",
    short: {
      en: "where, how, and with what authority selected work executes",
      zh: "选定工作在哪里、如何以及以何种权限执行",
    },
    long: {
      en: "Runtime owns the local or remote process, provider route, project/workspace trust, filesystem, network, secrets, approvals, sandbox, tools, and API boundary.",
      zh: "Runtime 负责本地或远程进程、提供商路由、项目/工作区信任、文件系统、网络、密钥、审批、沙箱、工具和 API 边界。",
    },
  },
];

export interface ControlTerm {
  /** The exact control noun. Never translate the noun itself. */
  term: string;
  kind: "mode" | "permission-posture";
  /** Behavioral description aligned with docs/MODES.md. */
  description: LocalizedText;
}

/** TUI modes — cycle with Tab when the composer is empty (docs/MODES.md). */
export const CONTROL_MODES: ControlTerm[] = [
  {
    term: "Plan",
    kind: "mode",
    description: {
      en: "Design-first and read-only: the primitive names stay stable while mutation and shell execution are centrally refused.",
      zh: "设计优先且只读：基础工具名称保持稳定，但修改与 shell 执行会被集中拒绝。",
    },
  },
  {
    term: "Work",
    kind: "mode",
    description: {
      en: "The default execution mode: multi-step tool use under the active permission posture, sandbox, and repository rules.",
      zh: "新会话的默认工作模式：多步骤工具调用，每次 shell 调用都有审批提示把关。",
    },
  },
  {
    term: "Operate",
    kind: "mode",
    description: {
      en: "Multitask conductor under the same permission posture, sandbox, and safety rules as Work; background worker dispatch is the default for separable work.",
      zh: "在与 Work 相同的权限姿态、沙箱和安全规则下进行多任务调度；可分离工作默认派发给后台 worker。",
    },
  },
];

/** Permission postures — cycle with Shift+Tab unless a non-Config modal owns input. */
export const PERMISSION_POSTURES: ControlTerm[] = [
  {
    term: "Ask",
    kind: "permission-posture",
    description: {
      en: "The default: Codewhale asks when an unresolved choice materially changes authority, cost, scope, or outcome.",
      zh: "默认值：当一个未决选择会实质改变权限、成本、范围或结果时，Codewhale 会询问。",
    },
  },
  {
    term: "Auto-Review",
    kind: "permission-posture",
    description: {
      en: "Fully autonomous: never opens a user question; resolves ambiguity to a safe reversible interpretation or reports that it cannot proceed safely.",
      zh: "完全自主：从不弹出用户提问；把歧义消解为安全可逆的解释，或明确报告无法安全继续。",
    },
  },
  {
    term: "Full Access",
    kind: "permission-posture",
    description: {
      en: "Ordinary tool calls skip approval prompts; non-bypassable safety, repository-law, and managed-policy holds still fail closed.",
      zh: "普通工具调用不再显示审批提示；不可绕过的安全、仓库法则和托管策略拦截仍然会失败关闭。",
    },
  },
];

/**
 * Route identity vocabulary. Requested and effective reasoning are separate:
 * an adaptive request is not itself evidence of the tier a provider used.
 * Routing source is provenance, not a provider or model substitute. Unknown
 * effective values stay explicitly unknown.
 */
export const ROUTE_IDENTITY: { term: string; description: LocalizedText }[] = [
  {
    term: "Provider",
    description: {
      en: "Who serves inference — a hosted API, a gateway, or a loopback local runtime (Ollama, vLLM, SGLang). A configured provider is never inferred from a model name.",
      zh: "谁提供推理——托管 API、网关或本机回环本地运行时（Ollama、vLLM、SGLang）。绝不会根据模型名称推断已配置的 provider。",
    },
  },
  {
    term: "Model",
    description: {
      en: "The exact model on that provider. Codewhale treats models as selectable components; no provider or model is privileged over another.",
      zh: "该提供商上的具体模型。Codewhale 把模型当作可选组件；任何提供商或模型都不享有特权。",
    },
  },
  {
    term: "Requested reasoning",
    description: {
      en: "The policy requested for the frozen route: inherit, off, low, medium, high, max, or auto. Auto permits adaptive reasoning; it never permits a silent provider or model switch.",
      zh: "为冻结路由请求的策略：inherit、off、low、medium、high、max 或 auto。Auto 允许自适应思考，但绝不允许静默切换 provider 或模型。",
    },
  },
  {
    term: "Effective reasoning",
    description: {
      en: "The tier actually applied for the run when the runtime or provider can establish it. If it cannot be established, the value is unavailable — never copied from the request or invented.",
      zh: "运行时或 provider 能够确认时，显示该次运行实际采用的档位；无法确认时标为暂不可用，绝不从请求值复制或臆造。",
    },
  },
  {
    term: "Routing source",
    description: {
      en: "Why this configured route was selected, such as an explicit member profile or inherited session setting. Missing provenance stays unavailable rather than being guessed.",
      zh: "说明为何选择这条已配置路由，例如显式成员档案或继承的会话设置。缺失的来源保持暂不可用，绝不猜测。",
    },
  },
];

/** Public advisory role vocabulary; legacy spellings are input compatibility. */
export const ADVISORY_ROLE = {
  term: "Advisor",
  description: {
    en: "The public read-only advisory fleet role. The historical consultant and oracle spellings remain compatibility aliases for saved configuration and replay only; new product surfaces say Advisor.",
    zh: "面向用户的只读 fleet 咨询角色。历史拼写 consultant 与 oracle 仅作为已保存配置和回放的兼容别名保留；新的产品界面统一使用 Advisor。",
  },
} as const;

/**
 * Measurement truth — what the site may claim about benchmark-style numbers.
 * These are policy statements, not results: the site publishes no leaderboard,
 * and any future number must carry its exact route identity and harness.
 */
export const MEASUREMENT_PRINCIPLES: LocalizedText[] = [
  {
    en: "Provider token and cache usage is shown locally when the provider reports it; unknown usage stays unknown and is never displayed as zero.",
    zh: "当提供商上报时，token 与缓存用量会在本地显示；未知用量保持未知，绝不显示为零。",
  },
  {
    en: "Costs, progress, capabilities, and delivery state are shown only when a source establishes them. Unavailable values remain unavailable rather than becoming zero or success.",
    zh: "成本、进度、能力和交付状态仅在有来源能够确认时显示。暂不可用的值保持暂不可用，绝不会变成零或成功。",
  },
  {
    en: "This site publishes no benchmark leaderboard. Any number Codewhale ever publishes must name its exact provider, model, requested and effective reasoning, and measurement harness alongside the result.",
    zh: "本站不发布基准排行榜。Codewhale 今后发布任何数字时，都必须同时给出确切的提供商、模型、请求与实际思考档位和测量工具链。",
  },
];
