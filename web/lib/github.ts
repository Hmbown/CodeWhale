import type { FeedItem, RepoStats } from "./types";

const REPO = process.env.GITHUB_REPO ?? "Hmbown/deepseek-tui";
const GH = "https://api.github.com";
const MIN_KNOWN_CONTRIBUTORS = 91;

function headers(token?: string): HeadersInit {
  const h: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "deepseek-tui-web",
  };
  if (token) h.Authorization = `Bearer ${token}`;
  return h;
}

export async function fetchRepoStats(token?: string): Promise<RepoStats> {
  const [repoRes, contribRes, releaseRes] = await Promise.all([
    fetch(`${GH}/repos/${REPO}`, { headers: headers(token), next: { revalidate: 1800 } }),
    fetch(`${GH}/repos/${REPO}/contributors?per_page=1&anon=true`, {
      headers: headers(token),
      next: { revalidate: 3600 },
    }),
    fetch(`${GH}/repos/${REPO}/releases/latest`, { headers: headers(token), next: { revalidate: 3600 } }),
  ]);

  const repo = (await repoRes.json()) as {
    stargazers_count: number;
    forks_count: number;
    open_issues_count: number;
  };

  const contributors = await contributorCount(contribRes);

  // Open PRs: cheapest path is the search API.
  const prRes = await fetch(
    `${GH}/search/issues?q=${encodeURIComponent(`repo:${REPO} is:pr is:open`)}&per_page=1`,
    { headers: headers(token), next: { revalidate: 1800 } }
  );
  const prJson = (await prRes.json()) as { total_count?: number };
  const openPulls = prJson.total_count ?? 0;
  const openIssues = Math.max(0, repo.open_issues_count - openPulls);

  let latestRelease: RepoStats["latestRelease"];
  if (releaseRes.ok) {
    const r = (await releaseRes.json()) as { tag_name: string; published_at: string; html_url: string };
    latestRelease = { tag: r.tag_name, publishedAt: r.published_at, url: r.html_url };
  }

  return {
    stars: repo.stargazers_count,
    forks: repo.forks_count,
    openIssues,
    openPulls,
    contributors,
    latestRelease,
    fetchedAt: new Date().toISOString(),
  };
}

async function contributorCount(res: Response): Promise<number> {
  if (!res.ok) return MIN_KNOWN_CONTRIBUTORS;

  const fromLink = lastPageFromLink(res.headers.get("link"));
  if (fromLink) return Math.max(fromLink, MIN_KNOWN_CONTRIBUTORS);

  const body = await res.json().catch(() => null);
  if (Array.isArray(body)) return Math.max(body.length, MIN_KNOWN_CONTRIBUTORS);

  return MIN_KNOWN_CONTRIBUTORS;
}

function lastPageFromLink(link: string | null): number | undefined {
  if (!link) return undefined;

  for (const part of link.split(",")) {
    const [rawUrl, rawRel] = part.split(";").map((segment) => segment.trim());
    if (rawRel !== 'rel="last"') continue;

    const match = rawUrl.match(/^<(.+)>$/);
    if (!match) continue;

    const page = new URL(match[1]).searchParams.get("page");
    const parsed = page ? Number.parseInt(page, 10) : NaN;
    if (Number.isFinite(parsed) && parsed > 0) return parsed;
  }

  return undefined;
}

interface RawIssue {
  number: number;
  title: string;
  html_url: string;
  state: "open" | "closed";
  user: { login: string; avatar_url: string };
  created_at: string;
  updated_at: string;
  comments: number;
  labels: { name: string; color: string }[];
  pull_request?: unknown;
  draft?: boolean;
  body?: string | null;
}

export async function fetchFeed(token?: string, limit = 30): Promise<FeedItem[]> {
  const [issuesRes, pullsRes] = await Promise.all([
    fetch(
      `${GH}/repos/${REPO}/issues?state=all&per_page=${limit}&sort=updated&direction=desc`,
      { headers: headers(token), next: { revalidate: 600 } }
    ),
    fetch(
      `${GH}/repos/${REPO}/pulls?state=all&per_page=${limit}&sort=updated&direction=desc`,
      { headers: headers(token), next: { revalidate: 600 } }
    ),
  ]);

  const issues = (await issuesRes.json()) as RawIssue[];
  const pulls = (await pullsRes.json()) as (RawIssue & { merged_at?: string | null })[];

  const items: FeedItem[] = [];

  for (const it of issues) {
    if (it.pull_request) continue; // GH issues endpoint returns PRs too
    items.push({
      kind: "issue",
      number: it.number,
      title: it.title,
      url: it.html_url,
      state: it.state,
      author: it.user.login,
      authorAvatar: it.user.avatar_url,
      createdAt: it.created_at,
      updatedAt: it.updated_at,
      comments: it.comments,
      labels: it.labels?.map((l) => ({ name: l.name, color: l.color })) ?? [],
      body: it.body ?? undefined,
    });
  }

  for (const pr of pulls) {
    let state: FeedItem["state"] = pr.state;
    if (pr.merged_at) state = "merged";
    else if (pr.draft) state = "draft";
    items.push({
      kind: "pull",
      number: pr.number,
      title: pr.title,
      url: pr.html_url,
      state,
      author: pr.user.login,
      authorAvatar: pr.user.avatar_url,
      createdAt: pr.created_at,
      updatedAt: pr.updated_at,
      comments: pr.comments,
      labels: pr.labels?.map((l) => ({ name: l.name, color: l.color })) ?? [],
      body: pr.body ?? undefined,
    });
  }

  return items
    .sort((a, b) => +new Date(b.updatedAt) - +new Date(a.updatedAt))
    .slice(0, limit);
}

export function relativeTime(iso: string): string {
  const diff = Date.now() - +new Date(iso);
  const mins = Math.round(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h`;
  const days = Math.round(hrs / 24);
  if (days < 30) return `${days}d`;
  const months = Math.round(days / 30);
  if (months < 12) return `${months}mo`;
  return `${Math.round(months / 12)}y`;
}
