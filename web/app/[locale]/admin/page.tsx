import type { Metadata } from "next";
import { cookies } from "next/headers";
import { ConnectionBanner } from "@/components/connection-banner";
import { EmptyState, ErrorState } from "@/components/surface-state";
import { getAgentEnv, listDrafts, validateSession, type AgentDraft } from "@/lib/community-agent";
import { AdminClient } from "./admin-client";

export const dynamic = "force-dynamic";

// Maintainer-only surface: keep it out of search indexes (robots.ts also
// disallows /*/admin).
export const metadata: Metadata = {
  robots: { index: false, follow: false },
};

const TYPE_LABELS: Record<string, { en: string; zh: string }> = {
  triage: { en: "Issue Triage", zh: "议题分类" },
  "pr-review": { en: "PR Review", zh: "PR 审阅" },
  stale: { en: "Stale Nudge", zh: "过期提醒" },
  dupes: { en: "Duplicate", zh: "重复检测" },
  digest: { en: "Weekly Digest", zh: "每周摘要" },
  linkcheck: { en: "Broken Link", zh: "失效链接" },
  "semantic-drift": { en: "Content Drift", zh: "内容漂移" },
};

function LoginForm({ locale, error }: { locale: string; error: boolean }) {
  const isZh = locale === "zh";
  return (
    <div className="mx-auto max-w-md px-6 py-20">
      <ConnectionBanner locale={locale} />
      <h1 className="font-display text-3xl mb-6 mt-6">
        {isZh ? "维护者登录" : "Maintainer login"}
      </h1>
      <form method="POST" action={`/api/admin/login?locale=${locale}`} autoComplete="off" className="space-y-4">
        <input type="hidden" name="locale" value={locale} />
        <label className="block">
          <span className="eyebrow block mb-2">{isZh ? "令牌" : "Token"}</span>
          <input
            type="password"
            name="token"
            required
            autoFocus
            autoComplete="off"
            spellCheck={false}
            className="w-full px-3 py-2 hairline-t hairline-b hairline-l hairline-r bg-paper font-mono text-sm focus:outline-none focus:border-indigo"
          />
        </label>
        <button
          type="submit"
          className="w-full px-5 py-3 bg-indigo text-paper font-mono text-sm uppercase tracking-wider hover:bg-indigo-deep transition-colors"
        >
          {isZh ? "登录 →" : "Sign in →"}
        </button>
        {error && (
          <p className="text-sm text-indigo font-mono">
            {isZh ? "令牌错误。" : "Invalid token."}
          </p>
        )}
      </form>
    </div>
  );
}

export default async function AdminPage({
  params,
  searchParams,
}: {
  params: Promise<{ locale: string }>;
  searchParams: Promise<{ err?: string }>;
}) {
  const { locale } = await params;
  const { err } = await searchParams;
  const isZh = locale === "zh";

  const env = await getAgentEnv();

  if (!env.MAINTAINER_TOKEN) {
    return (
      <div className="route-state">
        <ConnectionBanner locale={locale} />
        <div className="mt-6" />
        <ErrorState
          locale={locale}
          title={isZh ? "未配置" : "Not configured"}
          titleAs="h1"
          body={
            isZh
              ? "MAINTAINER_TOKEN 未设置。请在部署前配置此环境变量。"
              : "MAINTAINER_TOKEN is not set. Configure this secret before deployment."
          }
        />
      </div>
    );
  }

  const cookieStore = await cookies();
  const sid = cookieStore.get("mt_sid")?.value;
  const authed = await validateSession(env.CURATED_KV, sid);

  if (!authed) {
    return <LoginForm locale={locale} error={err === "1"} />;
  }

  let drafts: AgentDraft[] = [];
  try {
    drafts = await listDrafts(env.CURATED_KV);
  } catch (e) {
    console.error("failed to list drafts", e);
  }

  const pending = drafts.filter((d) => !d.posted);
  const posted = drafts.filter((d) => d.posted);

  return (
    <section className="mx-auto max-w-[1400px] px-6 pt-4 pb-20">
      {/* The signed-in shell: typed offline / reconnect state with a real
          server probe, so a paused action is never mistaken for a posted one. */}
      <ConnectionBanner locale={locale} />
      <div className="flex items-baseline justify-between mb-8 mt-8 hairline-b pb-4">
        <div>
          <h1 className="font-display tracking-crisp text-3xl">
            {isZh ? "社区助理草稿" : "Community Assistant Drafts"}
          </h1>
          <p className="mt-2 text-sm text-ink-mute font-mono">
            {pending.length} pending · {posted.length} posted
          </p>
        </div>
        <form method="POST" action={`/api/admin/logout?locale=${locale}`}>
          <button
            type="submit"
            className="font-mono text-xs text-ink-mute hover:text-indigo uppercase tracking-wider"
          >
            {isZh ? "退出 →" : "Sign out →"}
          </button>
        </form>
      </div>

      {pending.length === 0 && posted.length === 0 && (
        <EmptyState
          locale={locale}
          title={isZh ? "暂无草稿" : "No drafts yet"}
          body={
            isZh
              ? "草稿将在 cron 运行后出现。可在 wrangler.jsonc 中配置触发时间。"
              : "Drafts will appear here after cron runs. Configure triggers in wrangler.jsonc."
          }
        />
      )}

      <AdminClient
        drafts={pending}
        posted={posted}
        isZh={isZh}
        typeLabels={TYPE_LABELS}
      />
    </section>
  );
}
