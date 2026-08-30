import type { ChangelogDict } from "../types";

/**
 * Simplified-Chinese dictionary for `app/[locale]/changelog/page.tsx`.
 * Version numbers, tags, dates, and release-note text are content, not copy.
 */
export const changelog: ChangelogDict = {
  metaTitle: "更新日志 · Codewhale",
  metaDescription:
    "Codewhale 发布记录：最新已发布版本、尚未发布的源码候选版，以及每个版本的说明，均取自仓库中的 CHANGELOG.md。",
  kicker: "发布记录",
  title: "改了什么，在哪个版本。",
  lead:
    "页面顶部是两个事实：最新发布的版本，以及源码树当前声明的版本。下面的内容逐节来自仓库自己的 CHANGELOG.md。",
  publishedLabel: "最新已发布版本",
  publishedValue: "{tag} · 发布于 {date}",
  candidateLabel: "源码候选版",
  candidateValue: "{version} · 未发布",
  candidateMatches: "{version} · 与已发布版本一致",
  releasesLink: "GitHub Releases ↗",
  unreleasedHeading: "未发布",
  unreleasedNote: "自上一个标签以来合并到主分支的改动。它们属于源码候选版，不属于任何已发布的包。",
  compareLink: "在 GitHub 上比较 ↗",
  releasePageLink: "发布页 ↗",
  moreEntries: "显示 {shown} / {total} 条",
  fullNotes: "完整说明见 CHANGELOG.md ↗",
  releaseNotesLink: "{version} 完整说明 ↗",
  emptyTitle: "没有解析出发布说明",
  emptyBody: "构建时没有找到可解析的 CHANGELOG.md。GitHub 的发布列表仍然是权威记录。",
};
