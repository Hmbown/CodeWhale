import type { HomeDict } from "../types";

/**
 * Simplified Chinese home copy — a native rewrite mirroring the current
 * English direction (the brand dives so you don't have to; bring your own
 * model; runs on your machine), not a translation of it. The hero leans on
 * the classical 「一入侯门深似海」 allusion per community feedback — clever
 * in a way machine translation never is. The seal* glyphs are shared
 * editorial marks; the keys exist so a locale can override them without
 * touching the page.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — 一入码门深似海，它替你潜。",
  metaDescription:
    "Codewhale 潜入深海，你不必亲自下潜——开源的终端编程智能体。模型自带，跑在你自己的机器上。Rust 编写，MIT 许可。",

  kicker: "开源 · 自带模型 · 运行在你的终端",
  heroTitleA: "一入码门深似海，",
  heroTitleB: "Codewhale 替你潜。",
  heroIntro:
    "{brand} 是一个跑在终端里的开源编程智能体。给它一个模型、一个任务——它会读你的代码、改文件、自己跑检查，活干完了、或者需要你拿主意的时候，就停下来。模型随便带，也可以混着用：给每个角色各配一个模型。",
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
  screenshotAlt: "Codewhale 当前终端会话，可见 Operate 模式、鲸鱼、输入区与状态栏",
  figcaption: "当前 Codewhale 会话 · Operate 模式 · Ask 权限姿态",

  proofHeading: "水下终端壳。任意模型。本机运行。",
  proofBody:
    "接入你已经在用的模型——托管、网关或本地都行。Plan / Work / Operate 加上明确的权限姿态，每一次深潜都在你的掌控之中。",

  sealDecides: "法",
  decidesEyebrow: "看它如何决策",
  decidesHeading: "推理痕迹里看得见的法则",
  decidesLede:
    "真实会话摘录——分级排序的项目法则就写在模型的推理里，不只是落地页上的一句口号。",

  sealWorkflow: "行",
  workflowHeading: "从任务到验证过的改动。",
  workflow: [
    ["检查", "读取仓库、项目说明与任务。"],
    ["执行", "在明确的审批边界内修改文件。"],
    ["验证", "运行检查并核对结果。"],
    ["报告", "留下一份简洁、可查的工作收据。"],
  ],
  receiptAria: "工作收据示例",
  receiptInspect: "仓库与项目说明",
  receiptAct: "在所选权限姿态下修改",
  receiptReport: "检查通过 · 收据已保存",

  sealStart: "起",
  startHeading: "第一次用？四步走完。",
  startLede:
    "安装 → 免密钥的首次会话 → 接入提供商 → 配置你的 Fleet。名词解释见产品名词页。",
  startGuideLink: "阅读新手指引 →",
  startVocabularyLink: "查看产品名词 →",

  sealBoundaries: "界",
  boundariesHeadingA: "你的模型。",
  boundariesHeadingB: "你的边界。",
  boundariesBody:
    "模型、工作模式、权限姿态，都由你显式选择。未知的成本就标未知，预览中的功能就标注预览，不含糊。",
  hostedGatewayLocal: "托管、网关与本地模型",
  planActOperateDesc: "从只读规划到自主执行",
  askAutoReviewDesc: "为手头的活选择权限姿态",
  tuiExecWebDesc: "交互式与无头运行时界面",

  sealSurfaces: "面",
  surfacesHeading: "活在哪里干，运行时就在哪里用。",
  surfaces: [
    ["TUI", "交互式终端工作"],
    ["codewhale exec", "脚本与 CI"],
    ["Web 客户端", "仅监听本机回环的浏览器客户端"],
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
    "MIT 许可，由横跨运行时、提供商、平台、文档与测试的贡献者们共同塑造。",
  communityLinksAria: "社区链接",
  contribute: "参与贡献",
};
