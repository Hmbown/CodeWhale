import Link from "next/link";
import { fetchFeed, fetchRepoStats } from "@/lib/github";
import { getDispatch, getEnv } from "@/lib/kv";
import { getFacts } from "@/lib/facts";
import { buildPageMetadata } from "@/lib/page-meta";
import { Seal } from "@/components/seal";
import { Ticker } from "@/components/ticker";
import { StatGrid } from "@/components/stat-grid";
import { RELEASE_CONTRIBUTORS, RELEASE_HELPERS } from "@/lib/release-credits";
import type { CuratedDispatch, FeedItem, RepoStats } from "@/lib/types";

export const revalidate = 1800;

const FALLBACK_STATS: RepoStats = {
  stars: 0,
  forks: 0,
  openIssues: 0,
  openPulls: 0,
  contributors: 141,
  fetchedAt: new Date().toISOString(),
};

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/community",
    locale,
    title: isZh ? "社区 · CodeWhale" : "Community · CodeWhale",
    description: isZh
      ? "CodeWhale 的社区一角：实时仓库动态、每周摘要、路线图、贡献指南与发布致谢，集中在一处。"
      : "The community side of CodeWhale in one place: live repo activity, the weekly digest, the roadmap, how to contribute, and release credits.",
  });
}

export default async function CommunityPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  const p = (path: string) => (isZh ? `/zh${path}` : path);

  const env = await getEnv();
  const facts = await getFacts();

  let stats: RepoStats = FALLBACK_STATS;
  let feed: FeedItem[] = [];
  let dispatch: CuratedDispatch | null = null;

  try {
    [stats, feed] = await Promise.all([
      fetchRepoStats(env.GITHUB_TOKEN),
      fetchFeed(env.GITHUB_TOKEN, 12),
    ]);
  } catch (e) {
    console.error("github fetch failed", e);
  }

  try {
    dispatch = await getDispatch();
  } catch {
    /* dispatch stays null; the section falls back to a link */
  }

  const highlights =
    dispatch && isZh && dispatch.highlightsZh ? dispatch.highlightsZh : dispatch?.highlights ?? [];

  const hubs = isZh
    ? [
        { t: "活动动态", d: "议题与合并请求的实时镜像，每十分钟刷新。看此刻在发生什么。", cta: "查看动态 →", href: p("/feed"), tag: "实时" },
        { t: "社区摘要", d: "由维护者审核的每周更新存档，双语撰写。慢一点，但更成体系。", cta: "阅读摘要 →", href: p("/digest"), tag: "每周" },
        { t: "路线图", d: "已确认、正在评估、以及已排除的功能——公开分类，没有暗箱。", cta: "查看路线图 →", href: p("/roadmap"), tag: "规划" },
        { t: "参与贡献", d: "如何选题、开分支、匹配本地检查、提交合并请求。维护者会亲自看每一条。", cta: "开始贡献 →", href: p("/contribute"), tag: "上手" },
      ]
    : [
        { t: "Activity feed", d: "A live mirror of issues and pull requests, refreshed every ten minutes. See what's happening right now.", cta: "Open the feed →", href: p("/feed"), tag: "live" },
        { t: "Community digest", d: "A maintainer-approved archive of weekly updates, written in both languages. Slower, more structured.", cta: "Read the digest →", href: p("/digest"), tag: "weekly" },
        { t: "Roadmap", d: "What's confirmed, what's being weighed, and what's been ruled out — triaged in the open, no black box.", cta: "See the roadmap →", href: p("/roadmap"), tag: "planning" },
        { t: "Contribute", d: "How to pick a thread, branch, match the local checks, and open a PR. The maintainer reads every one.", cta: "Start contributing →", href: p("/contribute"), tag: "get started" },
      ];

  return (
    <>
      {/* HEADER — community framed as an addition, not the headline */}
      <section className="mx-auto max-w-[1400px] px-6 pt-12 pb-8">
        <div className="flex items-baseline gap-4 mb-3">
          <Seal char="众" />
          <div className="eyebrow">{isZh ? "社区 · Community" : "Community · 社区"}</div>
        </div>
        <h1 className="font-display tracking-crisp">
          {isZh ? (
            <>
              有人守护 <span className="font-cjk text-indigo text-5xl ml-2">不止于发布</span>
            </>
          ) : (
            <>
              Stewarded, <span className="font-cjk text-indigo text-5xl ml-2">not just shipped</span>
            </>
          )}
        </h1>
        <p className={`mt-5 max-w-3xl text-ink-soft text-lg ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh
            ? "CodeWhale 的核心是那部嵌套的法典和它的执行框架。社区是重要的补充，而不是标题——一个人维护，但由许多人塑造。这一页把散落各处的社区线索集中起来：此刻的仓库动态、每周摘要、公开的路线图、上手贡献的路径，以及每个版本背后的致谢。"
            : "CodeWhale's core is the nested constitution and the harness that enforces it. The community is an important addition, not the headline — maintained by one person, shaped by many. This page gathers the community threads that live in different corners: what's moving in the repo right now, the weekly digest, the roadmap in the open, the path to contributing, and the credits behind every release."}
        </p>
      </section>

      {/* live repo activity — the same ticker as the homepage, composed here */}
      <Ticker items={feed} />

      {/* HUB CARDS — links to the existing community pages */}
      <section className="mx-auto max-w-[1400px] px-6 py-12">
        <div className="flex items-baseline gap-4 mb-6 hairline-b pb-4">
          <Seal char="聚" />
          <h2 className="font-display">{isZh ? "从哪里开始" : "Where to start"}</h2>
        </div>
        <div className="grid md:grid-cols-2 gap-0 col-rule hairline-t hairline-b">
          {hubs.map((h) => (
            <Link key={h.t} href={h.href} className="block p-6 hover:bg-paper-deep transition-colors">
              <div className="flex items-baseline justify-between gap-3 mb-2">
                <h3 className="font-display text-xl">{h.t}</h3>
                <span className="font-mono text-[0.62rem] uppercase tracking-widest text-indigo shrink-0">{h.tag}</span>
              </div>
              <p className={`text-sm text-ink-soft mb-4 ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
                {h.d}
              </p>
              <span className="font-mono text-[0.7rem] uppercase tracking-widest text-indigo">{h.cta}</span>
            </Link>
          ))}
        </div>
      </section>

      {/* the numbers — same StatGrid as the homepage */}
      <StatGrid stats={stats} />

      {/* TODAY'S DISPATCH — composed from the same cron-curated source */}
      <section className="bg-paper-deep hairline-t hairline-b">
        <div className="mx-auto max-w-[1400px] px-6 py-14">
          <div className="flex items-baseline gap-4 mb-8 hairline-b pb-4">
            <Seal char="讯" />
            <h2 className="font-display">{isZh ? "今日要闻" : "Today's dispatch"}</h2>
          </div>
          {dispatch ? (
            <article className="grid lg:grid-cols-12 gap-x-10 gap-y-6 max-w-[1100px]">
              <h3 className="lg:col-span-12 font-display text-2xl sm:text-3xl leading-tight">
                {isZh && dispatch.headlineZh ? dispatch.headlineZh : dispatch.headline}
              </h3>
              <p className={`lg:col-span-7 text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
                {isZh && dispatch.summaryZh ? dispatch.summaryZh : dispatch.summary}
              </p>
              <ul className="lg:col-span-5 space-y-3">
                {highlights.slice(0, 3).map((h, i) => (
                  <li key={i} className="flex items-baseline gap-3">
                    <span className="font-mono text-[0.66rem] text-indigo uppercase tracking-widest w-16 shrink-0">{h.tag}</span>
                    <div>
                      <Link href={h.href} className="body-link font-display text-base leading-snug">
                        {h.title}
                      </Link>
                      <p className={`text-sm text-ink-soft mt-0.5 ${isZh ? "leading-[1.8]" : ""}`}>{h.blurb}</p>
                    </div>
                  </li>
                ))}
              </ul>
            </article>
          ) : (
            <p className={`text-ink-soft max-w-2xl ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
              {isZh ? (
                <>
                  今日要闻由 DeepSeek V4-Flash 每六小时重新生成。最新一期在首页顶部；完整的每周存档见{" "}
                  <Link href={p("/digest")} className="body-link">社区摘要</Link>。
                </>
              ) : (
                <>
                  The dispatch is regenerated by DeepSeek V4-Flash every six hours. The latest one sits near the top of the
                  homepage; the full weekly archive lives in the{" "}
                  <Link href={p("/digest")} className="body-link">community digest</Link>.
                </>
              )}
            </p>
          )}
        </div>
      </section>

      {/* RELEASE CREDITS — the people behind the current release */}
      <section className="mx-auto max-w-[1400px] px-6 py-14">
        <div className="flex items-baseline gap-4 mb-5 hairline-b pb-4">
          <Seal char="谢" />
          <div>
            <div className="eyebrow mb-2">{isZh ? `v${facts.version} 致谢` : `v${facts.version} credits`}</div>
            <h2 className="font-display text-3xl">{isZh ? "每个补丁和报告都算数" : "Every patch and report counts"}</h2>
          </div>
        </div>
        <div className="grid lg:grid-cols-12 gap-10">
          <div className="lg:col-span-5">
            <p className={`text-ink-soft ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
              {isZh
                ? "这一版合并和吸收了来自社区的大量工作。完整条目在 CHANGELOG 中；这里保留最新发布的公开致谢入口。没有 CLA，没有赞助商优先通道，议题公开分类，版本从 main 分支发布。"
                : "This release merged and harvested a large community tranche. The full notes live in the changelog; this keeps the latest public credit surface easy to find. No CLA, no sponsor lockouts, issues triaged in the open, releases cut from main."}
            </p>
            <div className="flex flex-wrap gap-x-5 gap-y-2 mt-4">
              <Link href="https://github.com/Hmbown/CodeWhale/blob/main/docs/CONTRIBUTORS.md" className="font-mono text-xs uppercase tracking-wider text-indigo hover:underline">
                {isZh ? "完整贡献者名单 →" : "Full contributor list →"}
              </Link>
              <Link href="https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md" className="font-mono text-xs uppercase tracking-wider text-indigo hover:underline">
                CHANGELOG →
              </Link>
              <Link href={p("/contribute")} className="font-mono text-xs uppercase tracking-wider text-indigo hover:underline">
                {isZh ? "参与贡献 →" : "Contribute →"}
              </Link>
            </div>
          </div>
          <div className="lg:col-span-7 grid gap-6">
            <div>
              <div className="eyebrow mb-3">{isZh ? "已合并 / 已吸收贡献" : "Merged and harvested contributions"}</div>
              <div className="flex flex-wrap gap-2">
                {RELEASE_CONTRIBUTORS.map((handle) => (
                  <Link
                    key={handle}
                    href={`https://github.com/${handle.slice(1)}`}
                    className="font-mono text-xs px-2 py-1 hairline-t hairline-b hairline-l hairline-r text-ink-soft hover:text-indigo hover:bg-paper-deep"
                  >
                    {handle}
                  </Link>
                ))}
              </div>
            </div>
            <div>
              <div className="eyebrow mb-3">{isZh ? "报告、复现和验证" : "Reports, repros, and verification"}</div>
              <div className="flex flex-wrap gap-2">
                {RELEASE_HELPERS.map((handle) => (
                  <Link
                    key={handle}
                    href={`https://github.com/${handle.slice(1)}`}
                    className="font-mono text-xs px-2 py-1 hairline-t hairline-b hairline-l hairline-r text-ink-soft hover:text-indigo hover:bg-paper-deep"
                  >
                    {handle}
                  </Link>
                ))}
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* JOIN IN — one clear ask, matching the homepage closer */}
      <section className="bg-ink text-paper">
        <div className="mx-auto max-w-[1400px] px-6 py-16 grid lg:grid-cols-12 gap-10 items-center">
          <div className="lg:col-span-8">
            <div className="eyebrow text-paper-deep/70 mb-3">{isZh ? "参与其中" : "Join in"}</div>
            <h2 className="font-display text-paper text-3xl sm:text-4xl leading-tight">
              {isZh ? "最有用的贡献，往往是一句「这里用不了」。" : "The most useful contribution is often “this doesn't work for me.”"}
            </h2>
            <p className={`mt-5 text-paper-deep/80 max-w-2xl ${isZh ? "leading-[1.9]" : "leading-relaxed"}`}>
              {isZh
                ? "遇到 bug、想要一个没列出来的模型、或者对某个决定有异议——直接开 issue 或 PR。所有反馈都是礼物。"
                : "Hit a bug, want a model that isn't listed, or disagree with a decision — open an issue or a PR. All feedback is a gift."}
            </p>
            <div className="mt-6 flex flex-wrap items-center gap-3">
              <Link href="https://github.com/Hmbown/CodeWhale/issues/new" className="px-4 py-2 bg-indigo text-paper font-mono text-sm hover:bg-indigo-deep transition-colors">
                {isZh ? "开个 issue →" : "Open an issue →"}
              </Link>
              <Link href={p("/contribute")} className="px-4 py-2 hairline-t hairline-b hairline-l hairline-r border-white/20 text-paper font-mono text-sm hover:bg-white/10 transition-colors">
                {isZh ? "参与贡献 →" : "Contribute →"}
              </Link>
              <Link href="https://github.com/Hmbown/CodeWhale/discussions" className="px-4 py-2 font-mono text-sm text-paper-deep/80 hover:text-paper transition-colors">
                {isZh ? "讨论区 →" : "Discussions →"}
              </Link>
            </div>
          </div>
          <div className="lg:col-span-4 font-mono text-sm text-paper-deep/80 space-y-2">
            <div className="flex justify-between hairline-b border-white/15 pb-2">
              <span className="uppercase tracking-widest text-[0.66rem] text-paper-deep/60">{isZh ? "版本" : "version"}</span>
              <span className="tabular text-paper">{facts.version ?? "v0.8.x"}</span>
            </div>
            <div className="flex justify-between hairline-b border-white/15 pb-2">
              <span className="uppercase tracking-widest text-[0.66rem] text-paper-deep/60">{isZh ? "提供商" : "providers"}</span>
              <span className="tabular text-paper">{facts.providers.length}</span>
            </div>
            <div className="flex justify-between hairline-b border-white/15 pb-2">
              <span className="uppercase tracking-widest text-[0.66rem] text-paper-deep/60">{isZh ? "许可证" : "license"}</span>
              <span className="text-paper">{facts.license ?? "MIT"}</span>
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
