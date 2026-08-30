import type { DocsShellDict } from "../types";

/**
 * Simplified-Chinese dictionary for the docs shell: portal hero, hub
 * metadata, task/topic search, sidebar and breadcrumb chrome, the
 * release-truth band, and the contextual help band on every docs page.
 */
export const docsShell: DocsShellDict = {
  metaTitle: "文档 · Codewhale",
  metaDescription:
    "Codewhale 文档：安装、使用指南、配置、提供商、账户与密钥、云端计算机、安全与信任、核心概念、工具、MCP、技能、沙箱、运行时 API、排障。",
  portalMark: "Codewhale 文档",
  heroTitle: "查找准确的使用说明。",
  heroLead:
    "从一个任务或一个主题开始。每一页都说明它取自仓库中的哪份文档、描述的是哪个版本。",
  installCta: "安装 Codewhale",
  sourceDocsCta: "浏览源文档 ↗",

  releaseLabel: "版本事实",
  releasePublished: "最新发布 {tag} · {date}",
  releaseCandidate: "本站文档描述的是尚未发布的 {version} 源码候选版。",
  releaseMatches: "本站文档描述的是已发布的 {tag}。",
  releaseChangelog: "更新日志 →",

  searchLabel: "搜索文档",
  searchPlaceholder: "按任务或主题搜索…（按 / 快速聚焦）",
  searchClear: "清除",
  searchMatches: "{matched} / {total} 条匹配 “{query}”",
  searchNoMatches: "没有条目匹配 “{query}”",
  tasksHeading: "按任务",
  tasksLead: "从你想完成的事情开始。",
  topicsHeading: "按主题",
  webGuideTag: "网页",
  sourceDocTag: "源文档",
  emptyTitle: "没有匹配的条目",
  emptyBody: "换一个关键词试试——中英文都可以搜索——或浏览 GitHub 上的完整文档目录。",
  emptyCta: "GitHub 文档目录 ↗",
  indexNote:
    "“网页”条目提供站内指南；“源文档”条目直接打开 GitHub 仓库中的完整参考资料。任务来自 docs-tasks.ts，主题来自 docs-map.ts，两份注册表都在仓库中维护。",

  sidebarHeading: "文档目录",
  sidebarAria: "文档目录",
  breadcrumbAria: "面包屑导航",
  breadcrumbHome: "首页",
  breadcrumbDocs: "文档",

  helpTitle: "这一页还不够？",
  helpLead:
    "每份指南都取自仓库中的一份文档。如果它写错了或缺了什么，最快的办法是在维护者能看到的地方说出来。",
  helpSource: "来源：{name}",
  helpTroubleshooting: "排障",
  helpFaq: "常见问题",
  helpDiscord: "到 Discord 提问 ↗",
  helpIssue: "报告文档问题 ↗",
};
