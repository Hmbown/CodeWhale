import { fetchFeed, fetchRepoStats } from "@/lib/github";
import { curate } from "@/lib/deepseek";
import { putDispatchWithKv } from "@/lib/kv";
import {
  agentChat,
  TRIAGE_PROMPT,
  PR_REVIEW_PROMPT,
  STALE_PROMPT,
  DUPES_PROMPT,
  DIGEST_PROMPT,
  saveDraft,
  hasFreshDraft,
  logUsage,
  type AgentDraft,
  type DeepSeekEnv,
} from "@/lib/community-agent";

export interface AgentEnv {
  CURATED_KV?: {
    get(k: string): Promise<string | null>;
    put(k: string, v: string, o?: { expirationTtl?: number }): Promise<void>;
    list(o?: { prefix?: string; limit?: number }): Promise<{ keys: { name: string }[] }>;
    delete(key: string): Promise<void>;
  };
  DEEPSEEK_API_KEY?: string;
  DEEPSEEK_BASE_URL?: string;
  DEEPSEEK_MODEL?: string;
  GITHUB_TOKEN?: string;
  CRON_SECRET?: string;
  GITHUB_REPO?: string;
  MAINTAINER_TOKEN?: string;
  MAINTAINER_GITHUB_PAT?: string;
}

const CRON_STATUS_TTL = 60 * 60 * 24 * 14;

function dsEnv(env: AgentEnv): DeepSeekEnv {
  return {
    baseUrl: env.DEEPSEEK_BASE_URL ?? process.env.DEEPSEEK_BASE_URL,
    model: env.DEEPSEEK_MODEL ?? process.env.DEEPSEEK_MODEL,
  };
}

export async function runCurate(env: AgentEnv): Promise<Record<string, unknown>> {
  if (!env.DEEPSEEK_API_KEY) {
    return { skipped: true, reason: "DEEPSEEK_API_KEY not set" };
  }
  try {
    const [stats, feed] = await Promise.all([
      fetchRepoStats(env.GITHUB_TOKEN),
      fetchFeed(env.GITHUB_TOKEN, 30),
    ]);
    const dispatch = await curate(env.DEEPSEEK_API_KEY, stats, feed, dsEnv(env));
    await putDispatchWithKv(env.CURATED_KV, dispatch);
    await env.CURATED_KV?.put(
      "cron:curate:last",
      JSON.stringify({
        ok: true,
        generatedAt: dispatch.generatedAt,
        headline: dispatch.headline,
      }),
      { expirationTtl: CRON_STATUS_TTL }
    );
    return { ok: true, headline: dispatch.headline, stored: env.CURATED_KV ? "kv" : "memory" };
  } catch (e) {
    const error = String(e);
    await env.CURATED_KV?.put(
      "cron:curate:last",
      JSON.stringify({
        ok: false,
        generatedAt: new Date().toISOString(),
        error,
      }),
      { expirationTtl: CRON_STATUS_TTL }
    );
    return { ok: false, error };
  }
}

export async function runTriage(env: AgentEnv): Promise<Record<string, unknown>> {
  const repo = env.GITHUB_REPO ?? "Hmbown/deepseek-tui";
  try {
    const res = await fetch(
      `https://api.github.com/repos/${repo}/issues?state=open&sort=created&direction=desc&per_page=30`,
      {
        headers: {
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
          "User-Agent": "deepseek-tui-web",
          ...(env.GITHUB_TOKEN ? { Authorization: `Bearer ${env.GITHUB_TOKEN}` } : {}),
        },
      }
    );
    const issues = (await res.json()) as { number: number; title: string; body?: string; updated_at: string; html_url: string; pull_request?: unknown; labels: { name: string }[] }[];
    const newIssues = issues.filter((i) => !i.pull_request).slice(0, 10);

    let processed = 0;
    let skipped = 0;

    for (const issue of newIssues) {
      if (await hasFreshDraft(env.CURATED_KV, "issue", String(issue.number), issue.updated_at)) {
        skipped++;
        continue;
      }

      const payload = {
        number: issue.number,
        title: issue.title,
        body: (issue.body ?? "").slice(0, 3000),
        labels: issue.labels.map((l) => l.name),
        url: issue.html_url,
      };

      try {
        const { content, usage } = await agentChat(
          [{ role: "system", content: TRIAGE_PROMPT }, { role: "user", content: JSON.stringify(payload) }],
          env.DEEPSEEK_API_KEY!,
          true,
          dsEnv(env)
        );
        const parsed = JSON.parse(content) as { bodyEn: string; bodyZh: string };
        const draft: AgentDraft = {
          id: String(issue.number),
          type: "triage",
          targetNumber: issue.number,
          targetUrl: issue.html_url,
          bodyEn: parsed.bodyEn,
          bodyZh: parsed.bodyZh,
          generatedAt: new Date().toISOString(),
          posted: false,
        };
        await saveDraft(env.CURATED_KV, draft);
        await logUsage(env.CURATED_KV, usage.input, usage.output);
        processed++;
      } catch {
        skipped++;
      }
    }

    return { ok: true, processed, skipped };
  } catch (e) {
    return { ok: false, error: String(e) };
  }
}

export async function runPrReview(env: AgentEnv): Promise<Record<string, unknown>> {
  const repo = env.GITHUB_REPO ?? "Hmbown/deepseek-tui";
  try {
    const res = await fetch(
      `https://api.github.com/repos/${repo}/pulls?state=open&sort=created&direction=desc&per_page=20`,
      {
        headers: {
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
          "User-Agent": "deepseek-tui-web",
          ...(env.GITHUB_TOKEN ? { Authorization: `Bearer ${env.GITHUB_TOKEN}` } : {}),
        },
      }
    );
    const prs = (await res.json()) as { number: number; title: string; body?: string; updated_at: string; html_url: string; changed_files?: number; additions?: number; deletions?: number; user: { login: string } }[];

    let processed = 0;
    let skipped = 0;

    for (const pr of prs.slice(0, 10)) {
      if (await hasFreshDraft(env.CURATED_KV, "pr", String(pr.number), pr.updated_at)) {
        skipped++;
        continue;
      }

      // Fetch diff stats if not included
      let diffStats = { changed_files: pr.changed_files ?? 0, additions: pr.additions ?? 0, deletions: pr.deletions ?? 0 };
      if (!pr.changed_files) {
        try {
          const diffRes = await fetch(`https://api.github.com/repos/${repo}/pulls/${pr.number}`, {
            headers: {
              Accept: "application/vnd.github+json",
              "X-GitHub-Api-Version": "2022-11-28",
              "User-Agent": "deepseek-tui-web",
              ...(env.GITHUB_TOKEN ? { Authorization: `Bearer ${env.GITHUB_TOKEN}` } : {}),
            },
          });
          const diffData = (await diffRes.json()) as { changed_files?: number; additions?: number; deletions?: number };
          diffStats = { changed_files: diffData.changed_files ?? 0, additions: diffData.additions ?? 0, deletions: diffData.deletions ?? 0 };
        } catch { /* use defaults */ }
      }

      const payload = {
        number: pr.number,
        title: pr.title,
        body: (pr.body ?? "").slice(0, 3000),
        author: pr.user.login,
        url: pr.html_url,
        ...diffStats,
      };

      try {
        const { content, usage } = await agentChat(
          [{ role: "system", content: PR_REVIEW_PROMPT }, { role: "user", content: JSON.stringify(payload) }],
          env.DEEPSEEK_API_KEY!,
          true,
          dsEnv(env)
        );
        const parsed = JSON.parse(content) as { bodyEn: string; bodyZh: string };
        const draft: AgentDraft = {
          id: String(pr.number),
          type: "pr-review",
          targetNumber: pr.number,
          targetUrl: pr.html_url,
          bodyEn: parsed.bodyEn,
          bodyZh: parsed.bodyZh,
          generatedAt: new Date().toISOString(),
          posted: false,
        };
        await saveDraft(env.CURATED_KV, draft);
        await logUsage(env.CURATED_KV, usage.input, usage.output);
        processed++;
      } catch {
        skipped++;
      }
    }

    return { ok: true, processed, skipped };
  } catch (e) {
    return { ok: false, error: String(e) };
  }
}

export async function runStale(env: AgentEnv): Promise<Record<string, unknown>> {
  const repo = env.GITHUB_REPO ?? "Hmbown/deepseek-tui";
  const thirtyDaysAgo = new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString().slice(0, 10);
  try {
    const res = await fetch(
      `https://api.github.com/search/issues?q=${encodeURIComponent(`repo:${repo} is:issue is:open updated:<${thirtyDaysAgo}`)}&sort=updated&per_page=20`,
      {
        headers: {
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
          "User-Agent": "deepseek-tui-web",
          ...(env.GITHUB_TOKEN ? { Authorization: `Bearer ${env.GITHUB_TOKEN}` } : {}),
        },
      }
    );
    const data = (await res.json()) as { items?: { number: number; title: string; body?: string; updated_at: string; html_url: string }[] };
    const issues = data.items ?? [];

    let processed = 0;
    let skipped = 0;

    for (const issue of issues.slice(0, 10)) {
      if (await hasFreshDraft(env.CURATED_KV, "stale", String(issue.number), issue.updated_at)) {
        skipped++;
        continue;
      }

      const payload = {
        number: issue.number,
        title: issue.title,
        body: (issue.body ?? "").slice(0, 2000),
        url: issue.html_url,
        lastUpdated: issue.updated_at,
      };

      try {
        const { content, usage } = await agentChat(
          [{ role: "system", content: STALE_PROMPT }, { role: "user", content: JSON.stringify(payload) }],
          env.DEEPSEEK_API_KEY!,
          true,
          dsEnv(env)
        );
        const parsed = JSON.parse(content) as { bodyEn: string; bodyZh: string };
        const draft: AgentDraft = {
          id: String(issue.number),
          type: "stale",
          targetNumber: issue.number,
          targetUrl: issue.html_url,
          bodyEn: parsed.bodyEn,
          bodyZh: parsed.bodyZh,
          generatedAt: new Date().toISOString(),
          posted: false,
        };
        await saveDraft(env.CURATED_KV, draft);
        await logUsage(env.CURATED_KV, usage.input, usage.output);
        processed++;
      } catch {
        skipped++;
      }
    }

    return { ok: true, processed, skipped };
  } catch (e) {
    return { ok: false, error: String(e) };
  }
}

export async function runDupes(env: AgentEnv): Promise<Record<string, unknown>> {
  const repo = env.GITHUB_REPO ?? "Hmbown/deepseek-tui";
  try {
    const res = await fetch(
      `https://api.github.com/repos/${repo}/issues?state=open&per_page=100`,
      {
        headers: {
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
          "User-Agent": "deepseek-tui-web",
          ...(env.GITHUB_TOKEN ? { Authorization: `Bearer ${env.GITHUB_TOKEN}` } : {}),
        },
      }
    );
    const issues = (await res.json()) as { number: number; title: string; body?: string; updated_at: string; html_url: string; pull_request?: unknown }[];
    const openIssues = issues
      .filter((i) => !i.pull_request)
      .map((i) => ({
        number: i.number,
        title: i.title,
        body: (i.body ?? "").slice(0, 500),
        url: i.html_url,
      }));

    if (openIssues.length < 3) {
      return { ok: true, skipped: true, reason: "too few issues to compare" };
    }

    const { content, usage } = await agentChat(
      [{ role: "system", content: DUPES_PROMPT }, { role: "user", content: JSON.stringify({ issues: openIssues }) }],
      env.DEEPSEEK_API_KEY!,
      true,
      dsEnv(env)
    );

    const parsed = JSON.parse(content) as { suggestions?: { targetNumber: number; duplicateNumber: number; reason: string; bodyEn: string; bodyZh: string }[] };
    const suggestions = parsed.suggestions ?? [];

    let processed = 0;
    for (const s of suggestions) {
      const draft: AgentDraft = {
        id: String(s.duplicateNumber),
        type: "dupes",
        targetNumber: s.duplicateNumber,
        bodyEn: s.bodyEn,
        bodyZh: s.bodyZh,
        generatedAt: new Date().toISOString(),
        posted: false,
      };
      await saveDraft(env.CURATED_KV, draft);
      processed++;
    }

    await logUsage(env.CURATED_KV, usage.input, usage.output);
    return { ok: true, processed };
  } catch (e) {
    return { ok: false, error: String(e) };
  }
}

export async function runDigest(env: AgentEnv): Promise<Record<string, unknown>> {
  const repo = env.GITHUB_REPO ?? "Hmbown/deepseek-tui";
  const weekAgo = new Date(Date.now() - 7 * 24 * 60 * 60 * 1000).toISOString();

  try {
    const [issuesRes, pullsRes, stats] = await Promise.all([
      fetch(
        `https://api.github.com/repos/${repo}/issues?state=all&since=${weekAgo}&per_page=50&sort=updated&direction=desc`,
        {
          headers: {
            Accept: "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "deepseek-tui-web",
            ...(env.GITHUB_TOKEN ? { Authorization: `Bearer ${env.GITHUB_TOKEN}` } : {}),
          },
        }
      ),
      fetch(
        `https://api.github.com/repos/${repo}/pulls?state=all&sort=updated&direction=desc&per_page=50`,
        {
          headers: {
            Accept: "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "deepseek-tui-web",
            ...(env.GITHUB_TOKEN ? { Authorization: `Bearer ${env.GITHUB_TOKEN}` } : {}),
          },
        }
      ),
      fetchRepoStats(env.GITHUB_TOKEN),
    ]);

    const issues = (await issuesRes.json()) as { number: number; title: string; state: string; pull_request?: unknown; created_at: string; user: { login: string } }[];
    const pulls = (await pullsRes.json()) as { number: number; title: string; state: string; merged_at?: string; created_at: string; user: { login: string } }[];

    const weekIssues = issues.filter((i) => !i.pull_request && new Date(i.created_at) > new Date(weekAgo));
    const weekPRs = pulls.filter((p) => new Date(p.created_at) > new Date(weekAgo));
    const mergedPRs = pulls.filter((p) => p.merged_at && new Date(p.merged_at) > new Date(weekAgo));

    const contributors = new Set([
      ...weekIssues.map((i) => i.user.login),
      ...weekPRs.map((p) => p.user.login),
    ]);

    const payload = {
      period: `${weekAgo.slice(0, 10)} — ${new Date().toISOString().slice(0, 10)}`,
      stats: { stars: stats.stars, forks: stats.forks },
      newIssues: weekIssues.map((i) => ({ number: i.number, title: i.title, author: i.user.login })),
      newPRs: weekPRs.map((p) => ({ number: p.number, title: p.title, author: p.user.login })),
      mergedPRs: mergedPRs.map((p) => ({ number: p.number, title: p.title })),
      contributors: [...contributors],
    };

    const { content, usage } = await agentChat(
      [{ role: "system", content: DIGEST_PROMPT }, { role: "user", content: JSON.stringify(payload) }],
      env.DEEPSEEK_API_KEY!,
      true,
      dsEnv(env)
    );

    const parsed = JSON.parse(content) as { titleEn: string; titleZh: string; summaryEn: string; summaryZh: string; sections: { heading: string; items: string[] }[] };

    // Compute week ID
    const now = new Date();
    const startOfYear = new Date(now.getFullYear(), 0, 1);
    const weekNum = Math.ceil(((now.getTime() - startOfYear.getTime()) / 86400000 + startOfYear.getDay() + 1) / 7);
    const weekId = `${now.getFullYear()}-W${String(weekNum).padStart(2, "0")}`;

    const draft: AgentDraft = {
      id: weekId,
      type: "digest",
      bodyEn: `# ${parsed.titleEn}\n\n${parsed.summaryEn}\n\n${parsed.sections.map((s) => `## ${s.heading}\n${s.items.map((i) => `- ${i}`).join("\n")}`).join("\n\n")}`,
      bodyZh: `# ${parsed.titleZh}\n\n${parsed.summaryZh}\n\n${parsed.sections.map((s) => `## ${s.heading}\n${s.items.map((i) => `- ${i}`).join("\n")}`).join("\n\n")}`,
      generatedAt: new Date().toISOString(),
      posted: false,
    };

    await saveDraft(env.CURATED_KV, draft);

    // Also save the structured digest for the weekly page
    await env.CURATED_KV?.put(
      `digest:weekly-${weekId}`,
      JSON.stringify({ ...parsed, weekId, generatedAt: draft.generatedAt }),
      { expirationTtl: 60 * 60 * 24 * 90 }
    );

    await logUsage(env.CURATED_KV, usage.input, usage.output);
    return { ok: true, weekId };
  } catch (e) {
    return { ok: false, error: String(e) };
  }
}
