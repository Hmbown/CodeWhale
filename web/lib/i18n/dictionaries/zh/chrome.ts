import type { ChromeDict } from "../types";

/**
 * Simplified Chinese chrome — a native rewrite mirroring the current
 * English direction (the wordmark tag is "any model, on your machine";
 * the retired "local-first" framing is gone). Nav pairs each Chinese
 * primary label with a short English secondary — the inverse of the
 * English edition's Han companion labels.
 */
export const chrome: ChromeDict = {
  navDocs: "文档",
  navStart: "指引",
  navInstall: "安装",
  navFaq: "常见问题",
  navCommunity: "社区",
  navContribute: "贡献",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "跳转到主要内容",


  navPrimaryAria: "主导航",
  navHomeAria: "Codewhale 首页",

  installCta: "安装 →",

  authSignIn: "登录",
  authRegister: "注册",
  authGroupAria: "账户",

  wordmarkSeal: "深",
  wordmarkTag: "任意模型，本机运行",

  issueLabel: "第 {date} 期",
  dateLocale: "zh-CN",

  starsAria: "GitHub 星标数",
  githubFallback: "GitHub",

  tickerLiveLabel: "实 时",
  tickerLiveTag: "LIVE",
  tickerMerged: "已合并",
  tickerOpened: "已开启",
  tickerClosed: "已关闭",
  tickerReleased: "已发布",
  tickerFirstContribution: "首次贡献",
  tickerBy: "作者 {handle}",
  tickerAria: "仓库近期动态",

  traceLabel: "推理痕迹",
  traceTabsAria: "会话片段",

  menuOpen: "打开菜单",
  menuClose: "关闭菜单",

  themeAuto: "自动",
  themeLight: "浅色",
  themeDark: "深色",
  themeAria: "文档主题：{mode}（点击切换）",
  themeTitle: "文档主题 · 自动 / 浅色 / 深色",

  footerTagline: "Codewhale 潜入深海，你不必亲自下潜——开源运行时的文档、源码与社区。",
  footerProduct: "产品",
  footerProject: "项目",
  footerDocs: "文档",
  footerGuide: "新手指引",
  footerInstall: "安装",
  footerModels: "模型",
  footerRuntime: "运行时",
  footerFaq: "常见问题",
  footerIssues: "议题",
  footerContribute: "参与贡献",
  footerLicense: "MIT 许可证",
  footerPricing: "价格",
  footerTerms: "服务条款",
  footerPrivacy: "隐私政策",
  footerChangelog: "更新日志",
  footerCanonicalSource: "官方源码：",
  footerReleases: " · 发布：",
  footerReleasesLink: "GitHub 发布页",
  footerSecurity: "安全",

  switcherLabel: "语言",
  switcherSwitchTo: "切换到 {label}",
  partialBadge: "(部分)",
};
