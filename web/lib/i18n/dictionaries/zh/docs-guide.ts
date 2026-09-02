import type { DocsGuideDict } from "../types";

/**
 * Simplified-Chinese dictionary for the docs "Getting started" page.
 * Copy moved verbatim from the former `isZh` branches in
 * `app/[locale]/docs/guide/page.tsx`.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "新手指引 · Codewhale 文档",
  metaDescription:
    "从安装到配置理想 fleet 的完整路径：安装、无需密钥的首次会话、连接提供商、设置 fleet。",
  bodyClassName: "text-ink-soft leading-[1.9] tracking-wide",
  overviewTitle: "新手指引",
  overviewLead:
    "从一条安装命令到配置好你的 fleet，四步走完。",
  sessionTitle: "看一次真实会话",
  sessionLead:
    "这里将放一段真实会话的录像。目前还没有录制，所以什么也不显示。",
  nextTitle: "接下来",
  sourceNote:
    "来源文档：docs/GUIDE.md, docs/KEYBINDINGS.md · 步骤文案来自 web/lib/content/getting-started.ts；更新时请同步修改 docs-map.ts。",
};
