import Link from "next/link";
import { getFacts } from "@/lib/facts";
import { buildPageMetadata } from "@/lib/page-meta";
import { RELEASE_CONTRIBUTORS, RELEASE_HELPERS } from "@/lib/release-credits";

export const revalidate = 300;

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/community",
    locale,
    title: isZh ? "社区 · Codewhale" : "Community · Codewhale",
    description: isZh
      ? "了解 Codewhale 的国际开源社区，提交 issue、发送 pull request、改进翻译并查看版本贡献者。"
      : "Meet Codewhale's international open-source community, file issues, send pull requests, improve translations, and see release contributors.",
  });
}

export default async function CommunityPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const p = (path: string) => `/${locale}${path}`;
  const facts = await getFacts();
  const sourceIsPublished = facts.latestPublishedRelease?.version === facts.version;

  const contributionPaths = isZh
    ? [
        {
          title: "报告问题",
          description: "报告 bug、兼容性问题或不清楚的行为，并附上系统信息、复现步骤和可以安全分享的日志。",
          cta: "提交 issue →",
          href: "https://github.com/Hmbown/CodeWhale/issues/new/choose",
        },
        {
          title: "改进代码或测试",
          description: "选择一个范围清晰的问题，提交尽可能小的有效补丁，并添加能够证明行为变化的回归测试。",
          cta: "查看开放 issues →",
          href: "https://github.com/Hmbown/CodeWhale/issues",
        },
        {
          title: "改进文档或翻译",
          description: "修正不准确的说明、补充实际示例，或帮助完成新的语言包，让文档在不同地区都自然准确。",
          cta: "查看本地化指南 ↗",
          href: "https://github.com/Hmbown/CodeWhale/blob/main/docs/LOCALIZATION.md",
        },
        {
          title: "复现并审查现有工作",
          description: "在你的平台和提供商上验证 issue 或 pull request，然后分享你运行的命令、结果和剩余问题。",
          cta: "查看 pull requests →",
          href: "https://github.com/Hmbown/CodeWhale/pulls",
        },
      ]
    : [
        {
          title: "Report a problem",
          description: "File a bug, compatibility problem, or unclear behavior with system details, reproduction steps, and any logs you can share safely.",
          cta: "File an issue →",
          href: "https://github.com/Hmbown/CodeWhale/issues/new/choose",
        },
        {
          title: "Improve code or tests",
          description: "Choose one well-bounded problem, make the smallest useful patch, and add a regression test that proves the changed behavior.",
          cta: "Browse open issues →",
          href: "https://github.com/Hmbown/CodeWhale/issues",
        },
        {
          title: "Improve documentation or translations",
          description: "Correct inaccurate guidance, add a practical example, or help complete a language pack so the project reads naturally in more regions.",
          cta: "Open the localization guide ↗",
          href: "https://github.com/Hmbown/CodeWhale/blob/main/docs/LOCALIZATION.md",
        },
        {
          title: "Reproduce and review existing work",
          description: "Verify an issue or pull request with your platform and provider, then share the commands you ran, the result, and any remaining problem.",
          cta: "Browse pull requests →",
          href: "https://github.com/Hmbown/CodeWhale/pulls",
        },
      ];

  const activityLinks = isZh
    ? [
        { title: "仓库动态", description: "最近的 issues 与 pull requests。", href: p("/feed") },
        { title: "社区摘要", description: "经过维护者审核的每周项目记录。", href: p("/digest") },
        { title: "公开路线图", description: "已发布、正在进行、考虑中和明确不做的工作。", href: p("/roadmap") },
      ]
    : [
        { title: "Repository activity", description: "Recent issues and pull requests.", href: p("/feed") },
        { title: "Community digest", description: "The maintainer-reviewed weekly project record.", href: p("/digest") },
        { title: "Public roadmap", description: "Shipped, underway, considered, and ruled-out work.", href: p("/roadmap") },
      ];

  return (
    <>
      <section className="hero">
        <div className="portal-current" aria-hidden="true" />
        <div className="portal-container community-welcome-inner">
          <div className="eyebrow">{isZh ? "国际开源社区" : "International open-source community"}</div>
          <h1>{isZh ? "与世界各地的贡献者一起构建 Codewhale。" : "Build Codewhale with contributors around the world."}</h1>
          <p>
            {isZh
              ? "Codewhale 的运行时、文档、测试和翻译由不同国家、语言、平台和技术背景的贡献者共同改进。第一次参与不需要从大功能开始；清楚的 bug 报告、准确的文档修正或带测试的小补丁都是重要的项目工作。"
              : "Codewhale's runtime, documentation, tests, and translations improve through contributors across countries, languages, platforms, and technical backgrounds. A first contribution does not need to be a large feature; a clear bug report, an accurate documentation correction, or a small tested patch is valuable project work."}
          </p>
          <div className="portal-actions">
            <Link href="https://github.com/Hmbown/CodeWhale/issues/new/choose" className="portal-button portal-button-primary">
              {isZh ? "提交 issue" : "File an issue"}
            </Link>
            <Link href="https://github.com/Hmbown/CodeWhale/pulls" className="portal-button portal-button-secondary">
              {isZh ? "查看 pull requests" : "Browse pull requests"}
            </Link>
            <Link href={p("/contribute")} className="portal-button portal-button-secondary">
              {isZh ? "阅读贡献指南" : "Read the contribution guide"}
            </Link>
          </div>
        </div>
      </section>

      <section className="portal-section">
        <div className="portal-container portal-section-grid">
          <div className="portal-section-copy">
            <span>{isZh ? "参与方式" : "Ways to contribute"}</span>
            <h2>{isZh ? "从一个具体、可验证的改进开始。" : "Start with one concrete, verifiable improvement."}</h2>
            <p>
              {isZh
                ? "问题报告、代码、测试、文档、翻译和审查都会推进项目。请选择最适合你当前经验和时间的一种方式。"
                : "Issue reports, code, tests, documentation, translations, and review all move the project forward. Choose the path that fits your experience and available time."}
            </p>
          </div>
          <div className="contribute-path-grid">
            {contributionPaths.map((path) => (
              <article key={path.title}>
                <h3>{path.title}</h3>
                <p>{path.description}</p>
                <Link href={path.href}>{path.cta}</Link>
              </article>
            ))}
          </div>
        </div>
      </section>

      <section className="portal-section portal-section-muted">
        <div className="portal-container portal-section-grid">
          <div className="portal-section-copy">
            <span>{isZh ? "公开项目记录" : "Public project record"}</span>
            <h2>{isZh ? "从提案到发布，项目工作保持公开。" : "Follow the repository work from proposal to release."}</h2>
            <p>
              {isZh
                ? "动态页面汇总最近的仓库活动，社区摘要保留每周存档，路线图区分已发布的能力与仍在讨论的方向。"
                : "The activity feed collects recent repository work, the community digest keeps the weekly archive of repository activity, and the roadmap separates shipped capabilities from work that is still being discussed."}
            </p>
          </div>
          <div className="portal-topic-list">
            {activityLinks.map((item) => (
              <Link key={item.href} href={item.href}>
                <strong>{item.title}</strong>
                <span>{item.description}</span>
                <span aria-hidden="true">→</span>
              </Link>
            ))}
          </div>
        </div>
      </section>

      <section className="portal-section community-credit-section">
        <div className="portal-container portal-section-grid">
          <div className="portal-section-copy">
            <span>
              {sourceIsPublished
                ? isZh
                  ? `v${facts.version} 版本致谢`
                  : `v${facts.version} release credit`
                : isZh
                  ? `v${facts.version} 候选版致谢`
                  : `v${facts.version} candidate credit`}
            </span>
            <h2>{isZh ? "贡献者署名是版本记录的一部分。" : "Contributor credit is part of the release record."}</h2>
            <p>
              {isZh
                ? `${sourceIsPublished ? "这一版本" : "这一候选版"}包含社区提交的代码、测试、复现和验证。即使维护者需要调整补丁后再合入，原始贡献者的署名也会保留在提交、更新日志和贡献者名单中。`
                : `This ${sourceIsPublished ? "release" : "candidate"} includes code, tests, reproductions, and verification from the community. When a maintainer needs to adapt a patch before it lands, the original contributor remains credited in the commit, changelog, and contributor record.`}
            </p>
            <div className="community-record-links">
              <Link href="https://github.com/Hmbown/CodeWhale/blob/main/docs/CONTRIBUTORS.md">
                {isZh ? "完整贡献者名单 ↗" : "Full contributor record ↗"}
              </Link>
              <Link href="https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md">CHANGELOG ↗</Link>
            </div>
          </div>
          <div className="community-credit-groups">
            <section>
              <h3>{isZh ? "已合并或吸收的贡献" : "Merged or adapted contributions"}</h3>
              <div className="community-credit-list">
                {RELEASE_CONTRIBUTORS.map((handle) => (
                  <Link key={handle} href={`https://github.com/${handle.slice(1)}`}>{handle}</Link>
                ))}
              </div>
            </section>
            {RELEASE_HELPERS.length > 0 ? (
              <section>
                <h3>{isZh ? "报告、复现与验证" : "Reports, reproductions, and verification"}</h3>
                <div className="community-credit-list">
                  {RELEASE_HELPERS.map((handle) => (
                    <Link key={handle} href={`https://github.com/${handle.slice(1)}`}>{handle}</Link>
                  ))}
                </div>
              </section>
            ) : null}
          </div>
        </div>
      </section>
    </>
  );
}
