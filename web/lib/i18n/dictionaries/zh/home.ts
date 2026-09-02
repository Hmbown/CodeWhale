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
    "{brand} 是一个跑在终端里的开源编程智能体。给它一个模型和一个任务。它会读你的代码、改文件、跑检查，活干完了或需要你拿主意时就停下来。模型随便用，也可以给每个角色各配一个。",
  install: "安装",
  docs: "文档",
  copy: "复制",
  copied: "已复制 ✓",

  installEyebrow: "一行安装",
  installRequirement: "需要 Node 18+，无需 Rust 工具链",
  installOtherWays: "其他方式 →",

  latestRelease: "最新发布 {tag}",
  releaseUnavailable: "发布状态暂不可用",
  currentSource: "源码",
  sourceCandidate: "未发布",
  providerRoutes: "{count} 个提供商",
  publishedRelease: "已发布",
  figcaptionSourceCandidate: "未发布",

  shotSession: "会话",
  screenshotAlt: "Codewhale 终端会话，Operate 模式：鲸鱼、输入区与状态栏",
  figcaption: "Codewhale 会话 · Operate 模式 · 权限：Ask",

  proofHeading: "终端里的编程智能体。任意模型。本机运行。",
  proofBody:
    "用你手头已有的模型——托管、网关或本地都行。选一个模式：Plan、Work 或 Operate。再选它不问你就能做多少：Ask、Auto-Review 或 Full Access。",

  sealDecides: "法",
  decidesEyebrow: "它如何决策",
  decidesHeading: "推理过程，原话呈现",
  decidesLede:
    "会话摘录。每一段都写明模型依据了哪条项目规则，以及接着做了什么。",

  sealWorkflow: "行",
  workflowHeading: "从任务到验证过的改动。",
  workflow: [
    ["检查", "读取仓库、项目说明与任务。"],
    ["执行", "修改文件，你要求先问的地方会先问。"],
    ["验证", "运行检查，核对结果。"],
    ["报告", "说明改了什么、通过了什么。"],
  ],
  receiptAria: "运行摘要示例",
  receiptInspect: "仓库与项目说明",
  receiptAct: "在你设定的权限内修改文件",
  receiptReport: "检查通过 · 摘要已保存",

  sealStart: "起",
  startHeading: "第一次用？四步。",
  startLede:
    "安装 → 首次会话，无需密钥 → 接入提供商 → 配置 fleet。",
  startGuideLink: "阅读新手指引 →",
  startVocabularyLink: "查名词 →",

  sealBoundaries: "界",
  boundariesHeadingA: "你的模型。",
  boundariesHeadingB: "你的边界。",
  boundariesBody:
    "模型、模式、以及它不问你能做多少，都由你来选。你不改，提供商和模型就不会变。预览功能会标注预览。",
  hostedGatewayLocal: "托管、网关与本地模型",
  planActOperateDesc: "从只读规划到自主执行",
  askAutoReviewDesc: "它在问你之前能做多少",
  tuiExecWebDesc: "交互式或脚本化",

  sealSurfaces: "面",
  surfacesHeading: "活在哪里干，就在哪里用。",
  surfaces: [
    ["TUI", "交互式终端工作"],
    ["codewhale exec", "脚本与 CI"],
    ["Web 客户端", "浏览器客户端，仅限本机"],
    ["运行时 API + MCP", "本地集成"],
    ["fleet", "多个智能体协作一件事"],
  ],
  runtimeLink: "运行时界面与稳定程度 →",

  installBandHeading: "从一条命令开始。",
  binaries: "预编译包",
  chinaMirrors: "中国镜像",
  installGuideLink: "阅读安装指南 →",

  sealCommunity: "众",
  communityHeading: "公开构建",
  communityBody:
    "MIT 许可。贡献者的工作覆盖运行时、提供商、平台、文档与测试。",
  communityLinksAria: "社区链接",
  contribute: "参与贡献",
};
