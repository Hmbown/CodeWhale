import type { DocsShellDict } from "../types";

/**
 * Simplified-Chinese dictionary for the docs shell.
 * Copy moved verbatim from the former `isZh` branches in
 * `app/[locale]/docs/layout.tsx` and `app/[locale]/docs/page.tsx`.
 */
export const docsShell: DocsShellDict = {
  metaTitle: "文档 · Codewhale",
  metaDescription:
    "Codewhale 文档：安装、使用指南、配置、提供商、核心概念、工具、MCP、技能、沙箱、运行时 API、排障。",
  portalMark: "Codewhale 文档",
  heroTitle: "查找准确的使用说明。",
  heroLead:
    "从新手指引和安装开始，或直接查看名词、模式、权限、工具、提供商、Pod、钩子、MCP 与运行时 API。每页都链接到仓库中的源文档。",
  installCta: "安装 Codewhale",
  sourceDocsCta: "浏览源文档 ↗",
};
