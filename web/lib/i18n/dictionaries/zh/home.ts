import type { HomeDict } from "../types";

/**
 * Simplified Chinese home copy. Extracted verbatim from the previous inline
 * `isZh` branches in app/[locale]/page.tsx (including the WORKFLOW and
 * SURFACES `zh:` tuples) — a move, not a retranslation. Only genuinely new
 * keys (install metadata, "See how it decides", receipt column, seals)
 * carry new prose.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — 潜入数据与代码的深海，让你不必亲自下潜",
  metaDescription:
    "数据与代码如海。Codewhale 是给你杠杆的终端智能体——读取、修改、验证，让普通人也能用 LLM 把东西做出来。运行在你自己的机器上；Rust 编写，MIT 许可。",

  kicker: "开源 · 任意模型 · 运行在你的终端",
  heroTitleA: "潜入深海，",
  heroTitleB: "你不必亲自下潜。",
  heroIntro:
    "{brand} 把大模型的杠杆交给普通人：在你的终端里读取仓库、修改文件、运行检查、留下收据。不必已经是程序员，也能把东西做出来——运行在你自己的机器上，模型只是可替换的组件。",
  install: "安装",
  docs: "文档",
  copy: "复制",
  copied: "已复制 ✓",

  installEyebrow: "一行安装",
  installRequirement: "需要 Node 18+，无需 Rust 工具链",
  installOtherWays: "其他方式 →",

  latestRelease: "最新发布 {tag}",
  releaseUnavailable: "发布状态暂不可用",
  currentSource: "当前源码",
  sourceCandidate: "源码候选版",
  providerRoutes: "{count} 个提供商路由",
  publishedRelease: "已发布版本",
  figcaptionSourceCandidate: "源码候选版",

  shotSession: "当前会话",
  screenshotAlt: "Codewhale 当前终端会话，显示 Operate 模式、鲸鱼、输入区和状态栏",
  figcaption: "当前 Codewhale 会话 · Operate 模式 · Ask 权限姿态",

  proofHeading: "终端原生的水下壳。模型与提供商中立。本地优先。",
  proofBody:
    "连接你已有的托管、网关或本地模型。Codewhale 在你的机器上运行；模型是可选择的组件，不是产品本身。Plan / Act / Operate 与明确的审批边界，让深潜也保持可控。",

  sealDecides: "法",
  decidesEyebrow: "看它如何裁决",
  decidesHeading: "推理里看得见的规则",
  decidesLede:
    "摘自真实会话的忠实片段——嵌套宪法可以在模型的推理里被直接观察到，而不是落地页上的一句宣称。",

  sealWorkflow: "行",
  workflowHeading: "从任务到经过验证的改动。",
  workflow: [
    ["检查", "读取仓库、项目说明与任务。"],
    ["执行", "在明确的审批边界内修改文件。"],
    ["验证", "运行检查并核对结果。"],
    ["报告", "留下简洁、可追溯的工作收据。"],
  ],
  receiptAria: "工作流程示例",
  receiptInspect: "仓库与项目说明",
  receiptAct: "在所选权限姿态下修改",
  receiptReport: "检查通过 · 收据已保存",

  sealStart: "起",
  startHeading: "第一次使用？四步走完。",
  startLede:
    "安装 → 无需密钥的首次会话 → 连接提供商 → 第一个 Fleet Workflow。名词含义见产品名词页。",
  startGuideLink: "阅读新手指引 →",
  startVocabularyLink: "查看产品名词 →",

  sealBoundaries: "界",
  boundariesHeadingA: "你的模型。",
  boundariesHeadingB: "你的边界。",
  boundariesBody:
    "显式选择模型、工作模式与权限姿态。Codewhale 不会把未知成本显示成零，也不会把预览功能说成已发布产品。",
  hostedGatewayLocal: "托管、网关与本地模型",
  planActOperateDesc: "从只读规划到自主执行",
  askAutoReviewDesc: "为任务选择权限姿态",
  tuiExecWebDesc: "交互式与无头运行时界面",

  sealSurfaces: "面",
  surfacesHeading: "在工作发生的地方使用运行时。",
  surfaces: [
    ["TUI", "交互式终端工作"],
    ["codewhale exec", "脚本与 CI"],
    ["Web 客户端", "仅限本机回环的浏览器客户端"],
    ["运行时 API + MCP", "本地集成"],
    ["Fleet", "持久化多智能体工作"],
  ],
  runtimeLink: "查看运行时界面与稳定性说明 →",

  installBandHeading: "从一条命令开始。",
  binaries: "预编译包",
  chinaMirrors: "中国镜像",
  installGuideLink: "阅读安装指南 →",

  sealCommunity: "众",
  communityHeading: "公开构建",
  communityBody:
    "Codewhale 采用 MIT 许可证，由来自不同时区、语言和技术背景的贡献者共同塑造。",
  communityLinksAria: "社区链接",
  contribute: "参与贡献",
};
