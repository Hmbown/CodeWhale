import type { ChromeDict } from "../types";

/**
 * Simplified Chinese chrome. Extracted verbatim from the previous inline
 * `isZh` branches in components/nav.tsx, components/footer.tsx,
 * components/nav-links.tsx, components/theme-toggle.tsx,
 * components/ticker.tsx, and components/terminal-player.tsx — this was a
 * move, not a retranslation. Only genuinely new keys carry new prose.
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

  skipToContent: "跳到主要内容",


  navPrimaryAria: "主导航",
  navHomeAria: "Codewhale 首页",

  installCta: "安装 →",

  wordmarkSeal: "深",
  wordmarkTag: "任何模型 · 本地优先",

  issueLabel: "第 {date} 期",
  dateLocale: "zh-CN",

  starsAria: "GitHub 星标数",
  githubFallback: "GitHub",

  tickerLiveLabel: "实 时",
  tickerLiveTag: "LIVE",

  traceLabel: "推理痕迹",
  traceTabsAria: "会话片段",

  menuOpen: "打开菜单",
  menuClose: "关闭菜单",

  themeAuto: "自动",
  themeLight: "浅色",
  themeDark: "深色",
  themeAria: "文档主题：{mode}（点击切换）",
  themeTitle: "文档主题 · 自动 / 浅色 / 深色",

  footerTagline: "潜入数据与代码的深海——开源运行时、文档与社区入口。",
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
  footerCanonicalSource: "官方源码：",
  footerReleases: " · 发布：",
  footerReleasesLink: "GitHub 发布页",
  footerSecurity: "安全联系",

  switcherLabel: "语言",
  switcherSwitchTo: "切换到{label}",
  partialBadge: "(部分)",
};
