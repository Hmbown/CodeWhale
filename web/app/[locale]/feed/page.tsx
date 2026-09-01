import Link from "next/link";
import { Seal } from "@/components/seal";
import { FeedCard } from "@/components/feed-card";
import { FeedRetry } from "@/components/feed-retry";
import { EmptyState, ErrorState, UnavailableState } from "@/components/surface-state";
import { loadFeed, type FeedLoadStatus } from "@/lib/github";
import { getEnv } from "@/lib/kv";
import { getStates } from "@/lib/i18n/dictionaries";
import { buildPageMetadata } from "@/lib/page-meta";
import type { FeedItem } from "@/lib/types";

export const revalidate = 600;

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";
  return buildPageMetadata({
    path: "/feed",
    locale,
    title: isZh ? "动态 · Codewhale" : "Activity · Codewhale",
    description: isZh
      ? "来自 Hmbown/CodeWhale GitHub 仓库的议题、合并请求和发布的实时动态。"
      : "Live feed of issues, pull requests, and releases mirrored from the Hmbown/CodeWhale GitHub repo.",
  });
}

export default async function FeedPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const isZh = locale === "zh";

  const env = await getEnv();
  let feed: FeedItem[] = [];
  // Four honest answers for an empty column: GitHub answered and had nothing
  // (`ok` → empty), GitHub was not asked or refused (`skipped` / `unavailable`
  // → not loaded, retry), or the fetch itself threw (`failed` → error, retry).
  // Each column answers for itself: one refused endpoint must not tell the
  // other column that its source did not answer.
  let issuesStatus: FeedLoadStatus | "failed" = "ok";
  let pullsStatus: FeedLoadStatus | "failed" = "ok";
  try {
    const load = await loadFeed(env.GITHUB_TOKEN, 50);
    feed = load.items;
    issuesStatus = load.issuesStatus;
    pullsStatus = load.pullsStatus;
  } catch (e) {
    issuesStatus = "failed";
    pullsStatus = "failed";
    console.error("feed fetch failed", e);
  }

  const issues = feed.filter((f) => f.kind === "issue");
  const pulls = feed.filter((f) => f.kind === "pull");
  const eyebrow = isZh ? "动态" : "Activity";
  const states = getStates(locale);
  // FeedRetry first busts this route's ISR entry, because a bare server
  // re-render would serve the same cached `skipped`/`unavailable` record for
  // up to ten minutes.
  const retry = <FeedRetry label={states.retry} />;
  const columnState = (status: FeedLoadStatus | "failed") =>
    status === "failed" ? (
      <ErrorState locale={locale} compact action={retry} />
    ) : status === "ok" ? (
      <EmptyState locale={locale} compact />
    ) : (
      <UnavailableState locale={locale} compact action={retry} />
    );

  return (
    <>
      {isZh ? (
        <>
          <section className="site-container section">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="动" />
              <div className="eyebrow">{eyebrow}</div>
            </div>
            <h1 className="font-display tracking-crisp">
              动态 <span className="font-cjk text-indigo text-5xl ml-2">Activity</span>
            </h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-[1.9] tracking-wide">
              来自{" "}
              <Link href="https://github.com/Hmbown/CodeWhale" className="body-link">Hmbown/CodeWhale</Link>
              {" "}的议题与合并请求实时镜像。每十分钟刷新一次。点击任意条目跳转至 GitHub。
            </p>
          </section>

          <section className="site-container pb-16 grid lg:grid-cols-12 gap-10">
            <div className="lg:col-span-6">
              <div className="hairline-t hairline-b hairline-l hairline-r bg-paper">
                <div className="bg-indigo text-paper px-4 py-3 flex items-baseline justify-between">
                  <h2 className="font-cjk text-base tracking-wider">合并请求 · Pull Requests</h2>
                  <span className="font-mono text-[0.7rem] uppercase tabular tracking-widest">{pulls.length} 条</span>
                </div>
                <div className="px-4">
                  {pulls.length > 0 ? (
                    pulls.map((p) => <FeedCard key={p.url} item={p} />)
                  ) : (
                    <div className="py-4">{columnState(pullsStatus)}</div>
                  )}
                </div>
              </div>
            </div>

            <div className="lg:col-span-6">
              <div className="hairline-t hairline-b hairline-l hairline-r bg-paper">
                <div className="bg-paper-deep text-ink px-4 py-3 flex items-baseline justify-between">
                  <h2 className="font-cjk text-base tracking-wider">议题 · Issues</h2>
                  <span className="font-mono text-[0.7rem] uppercase tabular tracking-widest">{issues.length} 条</span>
                </div>
                <div className="px-4">
                  {issues.length > 0 ? (
                    issues.map((i) => <FeedCard key={i.url} item={i} />)
                  ) : (
                    <div className="py-4">{columnState(issuesStatus)}</div>
                  )}
                </div>
              </div>
            </div>
          </section>

          <section className="bg-paper-deep hairline-t hairline-b">
            <div className="site-container py-10 grid md:grid-cols-3 gap-6 text-center">
              <Link href="https://github.com/Hmbown/CodeWhale/issues/new/choose" className="hairline-t hairline-b hairline-l hairline-r bg-paper p-6 hover:bg-indigo hover:text-paper transition-colors">
                <div className="font-display text-xl mb-1">提交议题</div>
                <div className="font-cjk text-sm text-ink-mute">Open an issue</div>
              </Link>
              <Link href="https://github.com/Hmbown/CodeWhale/compare" className="hairline-t hairline-b hairline-l hairline-r bg-paper p-6 hover:bg-indigo hover:text-paper transition-colors">
                <div className="font-display text-xl mb-1">提交合并请求</div>
                <div className="font-cjk text-sm text-ink-mute">Open a PR</div>
              </Link>
              <Link href="https://github.com/Hmbown/CodeWhale/discussions/new" className="hairline-t hairline-b hairline-l hairline-r bg-paper p-6 hover:bg-indigo hover:text-paper transition-colors">
                <div className="font-display text-xl mb-1">发起讨论</div>
                <div className="font-cjk text-sm text-ink-mute">Start a discussion</div>
              </Link>
            </div>
          </section>
        </>
      ) : (
        <>
          <section className="site-container section">
            <div className="flex items-baseline gap-4 mb-3">
              <Seal char="动" />
              <div className="eyebrow">{eyebrow}</div>
            </div>
            <h1 className="font-display tracking-crisp">
              Activity <span className="font-cjk text-indigo text-5xl ml-2">动态</span>
            </h1>
            <p className="mt-5 max-w-3xl text-ink-soft text-lg leading-relaxed">
              A live mirror of issues and pull requests from{" "}
              <Link href="https://github.com/Hmbown/CodeWhale" className="body-link">Hmbown/CodeWhale</Link>.
              Refreshed every ten minutes. Click any item to jump to GitHub.
            </p>
          </section>

          <section className="site-container pb-16 grid lg:grid-cols-12 gap-10">
            <div className="lg:col-span-6">
              <div className="hairline-t hairline-b hairline-l hairline-r bg-paper">
                <div className="bg-indigo text-paper px-4 py-3 flex items-baseline justify-between">
                  <h2 className="font-cjk text-base tracking-wider">合并 · Pull Requests</h2>
                  <span className="font-mono text-[0.7rem] uppercase tabular tracking-widest">{pulls.length} shown</span>
                </div>
                <div className="px-4">
                  {pulls.length > 0 ? (
                    pulls.map((p) => <FeedCard key={p.url} item={p} />)
                  ) : (
                    <div className="py-4">{columnState(pullsStatus)}</div>
                  )}
                </div>
              </div>
            </div>

            <div className="lg:col-span-6">
              <div className="hairline-t hairline-b hairline-l hairline-r bg-paper">
                <div className="bg-paper-deep text-ink px-4 py-3 flex items-baseline justify-between">
                  <h2 className="font-cjk text-base tracking-wider">议题 · Issues</h2>
                  <span className="font-mono text-[0.7rem] uppercase tabular tracking-widest">{issues.length} shown</span>
                </div>
                <div className="px-4">
                  {issues.length > 0 ? (
                    issues.map((i) => <FeedCard key={i.url} item={i} />)
                  ) : (
                    <div className="py-4">{columnState(issuesStatus)}</div>
                  )}
                </div>
              </div>
            </div>
          </section>

          <section className="bg-paper-deep hairline-t hairline-b">
            <div className="site-container py-10 grid md:grid-cols-3 gap-6 text-center">
              <Link href="https://github.com/Hmbown/CodeWhale/issues/new/choose" className="hairline-t hairline-b hairline-l hairline-r bg-paper p-6 hover:bg-indigo hover:text-paper transition-colors">
                <div className="font-display text-xl mb-1">Open an issue</div>
                <div className="font-cjk text-sm text-ink-mute">提交议题</div>
              </Link>
              <Link href="https://github.com/Hmbown/CodeWhale/compare" className="hairline-t hairline-b hairline-l hairline-r bg-paper p-6 hover:bg-indigo hover:text-paper transition-colors">
                <div className="font-display text-xl mb-1">Open a PR</div>
                <div className="font-cjk text-sm text-ink-mute">提交合并</div>
              </Link>
              <Link href="https://github.com/Hmbown/CodeWhale/discussions/new" className="hairline-t hairline-b hairline-l hairline-r bg-paper p-6 hover:bg-indigo hover:text-paper transition-colors">
                <div className="font-display text-xl mb-1">Start a discussion</div>
                <div className="font-cjk text-sm text-ink-mute">发起讨论</div>
              </Link>
            </div>
          </section>
        </>
      )}
    </>
  );
}
