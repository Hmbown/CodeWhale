export type WikiStatus = "live" | "experimental" | "mixed";

export type WikiPage = {
  id: string;
  title: string;
  file: string;
  slug: string;
  status: WikiStatus;
  summary: string;
  cn: string;
};

export const WIKI_PAGES: WikiPage[] = [
  {
    id: "01",
    title: "Overview",
    file: "01-overview.md",
    slug: "01-overview",
    status: "live",
    summary: "Product shape, workspace crates, entry points, and architectural vocabulary.",
    cn: "产品形态、工作区 crate、入口点与架构词汇。",
  },
  {
    id: "02",
    title: "Crate Reference",
    file: "02-crate-reference.md",
    slug: "02-crate-reference",
    status: "live",
    summary: "Workspace crate map, dependencies, major modules, and source landmarks.",
    cn: "工作区 crate 地图、依赖关系、主要模块与源码坐标。",
  },
  {
    id: "03",
    title: "Agent System",
    file: "03-agent-system.md",
    slug: "03-agent-system",
    status: "live",
    summary: "Sub-agent lifecycle, recursion depth, forked context, mailbox, and TUI cards.",
    cn: "子 Agent 生命周期、递归深度、上下文继承、邮箱与 TUI 卡片。",
  },
  {
    id: "04",
    title: "Tool System",
    file: "04-tool-system.md",
    slug: "04-tool-system",
    status: "live",
    summary: "Core tool schema, registry, dispatch path, and concurrency model.",
    cn: "核心工具 schema、注册表、派发路径与并发模型。",
  },
  {
    id: "05",
    title: "RLM System",
    file: "05-rlm-system.md",
    slug: "05-rlm-system",
    status: "live",
    summary: "Recursive LM sessions, Python helpers, batch fanout, and large-output handles.",
    cn: "递归 LM 会话、Python 辅助函数、批量 fanout 与大输出句柄。",
  },
  {
    id: "06",
    title: "Whaleflow",
    file: "06-whaleflow.md",
    slug: "06-whaleflow",
    status: "experimental",
    summary: "Workflow IR, Starlark/JS authoring, replay, and model policy. Defined, not wired.",
    cn: "Workflow IR、Starlark/JS authoring、replay 与模型策略。已定义，尚未接入运行时。",
  },
  {
    id: "07",
    title: "Configuration",
    file: "07-configuration.md",
    slug: "07-configuration",
    status: "live",
    summary: "config.toml, providers, modes, sandbox, hooks, memory, and feature gates.",
    cn: "config.toml、provider、模式、沙箱、hook、记忆与 feature gate。",
  },
  {
    id: "08",
    title: "Web Layer",
    file: "08-web-layer.md",
    slug: "08-web-layer",
    status: "live",
    summary: "Next site, Session OS shell, runtime API bridge, and npm packages.",
    cn: "Next 网站、Session OS shell、runtime API 桥接与 npm 包。",
  },
  {
    id: "09",
    title: "Operations",
    file: "09-operations.md",
    slug: "09-operations",
    status: "live",
    summary: "Install paths, release automation, CI, benchmarks, and deployment scripts.",
    cn: "安装路径、发布自动化、CI、benchmark 与部署脚本。",
  },
  {
    id: "10",
    title: "Constitution",
    file: "10-constitution.md",
    slug: "10-constitution",
    status: "live",
    summary: "Behavior hierarchy, evidence rules, authority tiers, and prompt composition.",
    cn: "行为层级、证据规则、权威顺位与 prompt 组合。",
  },
  {
    id: "11",
    title: "Skills System",
    file: "11-skills-system.md",
    slug: "11-skills-system",
    status: "live",
    summary: "Progressive disclosure, SKILL.md format, installation, and extension boundaries.",
    cn: "渐进式披露、SKILL.md 格式、安装与扩展边界。",
  },
  {
    id: "12",
    title: "Fleet System",
    file: "12-fleet-system.md",
    slug: "12-fleet-system",
    status: "experimental",
    summary: "Fleet protocol, task specs, worker specs, security model, and receipts. Not stable.",
    cn: "Fleet 协议、任务规格、worker 规格、安全模型与 receipt。尚未稳定。",
  },
  {
    id: "13",
    title: "Additional Tools",
    file: "13-additional-tools.md",
    slug: "13-additional-tools",
    status: "live",
    summary: "Supplemental tool families including finance, speech, validators, and project tools.",
    cn: "补充工具族，包括 finance、speech、validator 与 project 工具。",
  },
  {
    id: "14",
    title: "Systems Internals",
    file: "14-systems-internals.md",
    slug: "14-systems-internals",
    status: "mixed",
    summary: "Workrooms, REPL sandbox, snapshots, compaction, and purge internals.",
    cn: "Workroom、REPL sandbox、snapshot、compaction 与 purge 内部机制。",
  },
];

export function wikiHref(locale: string, page: WikiPage): string {
  return `/${locale}/wiki/${page.slug}`;
}

export function getWikiPage(slugOrFile: string): WikiPage | undefined {
  const clean = slugOrFile.replace(/\.md$/, "");
  return WIKI_PAGES.find((page) => page.slug === clean || page.file === slugOrFile);
}

export function wikiStatusLabel(status: WikiStatus, isZh: boolean): string {
  if (status === "experimental") return isZh ? "实验性" : "Experimental";
  if (status === "mixed") return isZh ? "混合" : "Mixed";
  return isZh ? "已上线" : "Live";
}

export function wikiStatusClass(status: WikiStatus): string {
  if (status === "experimental") return "text-ochre";
  if (status === "mixed") return "text-cobalt";
  return "text-jade";
}
