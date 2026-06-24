/**
 * roadmap-feed.ts — fetch the live roadmap from GitHub.
 *
 *   "Shipped"    ← last 8 published Releases on Hmbown/CodeWhale
 *   "Underway"   ← open issues with label `roadmap:underway`, or the next
 *                  open version milestone after the latest release
 *   "Considered" ← open issues with label `roadmap:considered`, or queued
 *                  version milestones after the current one
 *   "Ruled out"  ← issues (open or closed) with label `roadmap:ruled-out`
 *
 * Cached in CURATED_KV under `roadmap:feed` with a 30-minute TTL so the
 * roadmap page renders fast and the GH rate limit never matters.
 *
 * Categories that come back empty fall through to the page's static items —
 * the maintainer can adopt label-driven roadmap incrementally.
 */
const REPO = process.env.GITHUB_REPO ?? "Hmbown/CodeWhale";
const KV_KEY = "roadmap:feed:v3";
const KV_TTL = 60 * 30;

export interface RoadmapItem {
  title: string;
  note: string;
  href?: string;
  number?: number;
}

export interface RoadmapFeed {
  generatedAt: string;
  shipped: RoadmapItem[];
  underway: RoadmapItem[];
  considered: RoadmapItem[];
  ruledOut: RoadmapItem[];
}

interface KVNamespace {
  get(k: string): Promise<string | null>;
  put(k: string, v: string, o?: { expirationTtl?: number }): Promise<void>;
}

async function gh<T>(url: string, ghToken?: string): Promise<T | null> {
  const headers: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "User-Agent": "codewhale-web-roadmap",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  if (ghToken) headers["Authorization"] = `Bearer ${ghToken}`;
  try {
    const r = await fetch(url, { headers });
    if (!r.ok) return null;
    return (await r.json()) as T;
  } catch {
    return null;
  }
}

interface GhRelease { tag_name: string; name: string | null; body: string | null; html_url: string; prerelease: boolean; draft: boolean }
interface GhIssue { number: number; title: string; html_url: string; body: string | null; state: string; pull_request?: unknown }
interface GhMilestone {
  number: number;
  title: string;
  description: string | null;
  html_url: string;
  open_issues: number;
  closed_issues: number;
  state: string;
}

const FALLBACK_SHIPPED: RoadmapItem[] = [
  {
    title: "CodeWhale v0.8.64",
    note: "Security/release hardening, provider-wait polish, delegated-server cleanup, ACP history repair, and dependency harvests",
    href: "https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.64",
  },
];

function withPinnedShipped(items: RoadmapItem[]): RoadmapItem[] {
  // Safety net only: the static fallback entry must never sit ahead of live
  // releases — use it solely when the live list is empty.
  return items.length > 0 ? items : FALLBACK_SHIPPED;
}

function stripMilestoneScheduleText(note: string): string {
  return note
    .replace(
      /(\d+ open \/ \d+ closed);\s+(?:milestone target \d{4}-\d{2}-\d{2}|no fixed due date)\.\s+/g,
      "$1. ",
    )
    .replace(/\s+/g, " ")
    .trim();
}

function normalizeItems(items: RoadmapItem[] | undefined): RoadmapItem[] {
  return (items ?? []).map((item) => ({
    ...item,
    note: stripMilestoneScheduleText(item.note),
  }));
}

function normalizeRoadmapFeed(feed: RoadmapFeed): RoadmapFeed {
  return {
    ...feed,
    shipped: withPinnedShipped(normalizeItems(feed.shipped)),
    underway: normalizeItems(feed.underway),
    considered: normalizeItems(feed.considered),
    ruledOut: normalizeItems(feed.ruledOut),
  };
}

function summarizeReleaseBody(body: string | null): string {
  if (!body) return "";
  // First non-empty line, stripped of markdown headers / bullets / links
  const lines = body.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
  const candidate = lines.find((l) => !l.startsWith("#") && !l.startsWith("---") && l.length > 8);
  if (!candidate) return "";
  // Strip bullets, trailing emoji, links, and cap length
  const stripped = candidate.replace(/^[*\-•]\s+/, "").replace(/\[([^\]]+)\]\([^)]+\)/g, "$1").trim();
  return stripped.length > 140 ? stripped.slice(0, 137) + "…" : stripped;
}

function summarizeIssueBody(body: string | null): string {
  if (!body) return "";
  // Issue bodies are often very long; take the first non-empty paragraph (up to ~140 chars)
  const para = body.split(/\r?\n\r?\n/).map((p) => p.trim()).find((p) => p.length > 0) ?? "";
  const stripped = para
    .replace(/^[#>*\-\s]+/, "")
    .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  return stripped.length > 140 ? stripped.slice(0, 137) + "…" : stripped;
}

async function fetchByLabel(label: string, ghToken?: string, state: "open" | "closed" | "all" = "open"): Promise<RoadmapItem[]> {
  const url = `https://api.github.com/repos/${REPO}/issues?state=${state}&labels=${encodeURIComponent(label)}&per_page=10&sort=updated`;
  const issues = await gh<GhIssue[]>(url, ghToken);
  if (!issues) return [];
  return issues
    .filter((i) => !i.pull_request) // skip PRs
    .map((i) => ({
      title: i.title,
      note: summarizeIssueBody(i.body) || `Issue #${i.number}`,
      href: i.html_url,
      number: i.number,
    }));
}

function parseVersion(value: string): [number, number, number] | null {
  const m = value.trim().match(/^v?(\d+)\.(\d+)\.(\d+)$/);
  if (!m) return null;
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

function compareVersions(a: [number, number, number], b: [number, number, number]): number {
  return a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
}

function latestReleaseVersion(releases: GhRelease[] | null): [number, number, number] | null {
  if (!releases) return null;
  return releases
    .filter((r) => !r.draft)
    .map((r) => parseVersion(r.tag_name))
    .filter((v): v is [number, number, number] => Boolean(v))
    .sort((a, b) => compareVersions(b, a))[0] ?? null;
}

const MILESTONE_NOTES: Record<string, string> = {
  "v0.8.65": "Provider/model/offering cleanup: split provider facts from model facts, resolve switches through ReadyRouteCandidate, add live catalogs, dashboards, and provider-scoped pricing/usage.",
  "v0.8.66": "Token/cache/context discipline plus Hotbar keys: MVP command surface, default bindings, focus/modal key-dispatch coverage, approval semantics, terminal visual QA, and regression fixtures.",
  "v0.8.67": "Guided setup hub: provider/model setup, trust and sandbox choices, tools/MCP/skills, remote/mobile/chat bridge setup, persistence, and release QA.",
  "v0.8.68": "TUI reliability and rendering polish, Hotbar bindable sources, slash-command/workbench refactors, and user-reported terminal edge cases.",
  "v0.8.69": "Website, docs, distribution-name migration, community credit surfaces, and selected platform/search/proxy backlog cleanup.",
  "v0.8.70": "Display/reasoning-output reliability backlog: stuck turns, truncated output, thinking-block rendering, and terminal inspection affordances.",
  "v0.8.71": "Legacy follow-up and dead-code inventory: delete, wire, or explicitly track remaining migration, sandbox, release, i18n, profile-switching, and connector issues.",
  "v0.8.72": "Memory/fork/cache-maximal carryover: typed memory, fork UX, and active-file context behavior.",
  "v0.8.73": "Configurable keymap and Hotbar key carryover.",
  "v0.9.0": "Multiplayer workrooms and integration track: chat-native rooms, presence, threaded/shareable agent work, WhaleFlow execution, always-on agent state, typed memory, and stabilization gates.",
};

function milestoneItem(m: GhMilestone): RoadmapItem {
  const note = MILESTONE_NOTES[m.title] || summarizeIssueBody(m.description);
  const count = `${m.open_issues} open / ${m.closed_issues} closed`;
  return {
    title: m.title,
    note: `${count}. ${note}`,
    href: m.html_url,
    number: m.number,
  };
}

function cleanupItem(milestones: GhMilestone[]): RoadmapItem | null {
  if (milestones.length === 0) return null;
  const open = milestones.reduce((sum, m) => sum + m.open_issues, 0);
  const closed = milestones.reduce((sum, m) => sum + m.closed_issues, 0);
  const titles = milestones.map((m) => m.title).join(", ");
  return {
    title: "Current shipped-milestone cleanup",
    note: `${open} open / ${closed} closed across ${titles}. Triage, close, move, or cut follow-up fixes for the already-shipped milestone backlog while the AM/PM release train continues.`,
    href: `https://github.com/${REPO}/issues?q=is%3Aissue+is%3Aopen`,
  };
}

async function fetchMilestoneRoadmap(releases: GhRelease[] | null, ghToken?: string): Promise<Pick<RoadmapFeed, "underway" | "considered"> | null> {
  const latest = latestReleaseVersion(releases);
  if (!latest) return null;

  const milestones = await gh<GhMilestone[]>(
    `https://api.github.com/repos/${REPO}/milestones?state=open&per_page=100`,
    ghToken,
  );
  if (!milestones) return null;

  const versionMilestones = milestones
    .filter((m) => m.state === "open" && m.open_issues > 0)
    .map((m) => ({ milestone: m, version: parseVersion(m.title) }))
    .filter((m): m is { milestone: GhMilestone; version: [number, number, number] } => Boolean(m.version))
    .sort((a, b) => compareVersions(a.version, b.version));

  const cleanup = cleanupItem(
    versionMilestones
      .filter(({ version }) => compareVersions(version, latest) <= 0)
      .map(({ milestone }) => milestone),
  );
  const patchTrain = versionMilestones
    .filter(({ version }) => compareVersions(version, latest) > 0)
    .filter(({ version }) => version[0] === latest[0] && version[1] === latest[1]);
  const majorTracks = versionMilestones
    .filter(({ version }) => compareVersions(version, latest) > 0)
    .filter(({ version }) => version[0] > latest[0] || version[1] > latest[1]);

  const underway = [
    ...(cleanup ? [cleanup] : []),
    ...patchTrain.slice(0, 4).map(({ milestone }) => milestoneItem(milestone)),
  ];
  const considered = [
    ...patchTrain.slice(4).map(({ milestone }) => milestoneItem(milestone)),
    ...majorTracks.slice(0, 1).map(({ milestone }) => milestoneItem(milestone)),
  ];

  return { underway, considered };
}

export async function fetchRoadmap(ghToken?: string): Promise<RoadmapFeed> {
  const [releases, underway, considered, ruledOut] = await Promise.all([
    gh<GhRelease[]>(`https://api.github.com/repos/${REPO}/releases?per_page=8`, ghToken),
    fetchByLabel("roadmap:underway", ghToken, "open"),
    fetchByLabel("roadmap:considered", ghToken, "open"),
    fetchByLabel("roadmap:ruled-out", ghToken, "all"),
  ]);

  const shipped: RoadmapItem[] = releases
    ? releases
    .filter((r) => !r.draft)
    .map((r) => ({
      title: r.name?.trim() || r.tag_name,
      note: summarizeReleaseBody(r.body) || r.tag_name,
      href: r.html_url,
    }))
    : FALLBACK_SHIPPED;

  const milestoneFallback = (underway.length === 0 || considered.length === 0)
    ? await fetchMilestoneRoadmap(releases, ghToken)
    : null;

  return normalizeRoadmapFeed({
    generatedAt: new Date().toISOString(),
    shipped: withPinnedShipped(shipped),
    underway: underway.length > 0 ? underway : milestoneFallback?.underway ?? [],
    considered: considered.length > 0 ? considered : milestoneFallback?.considered ?? [],
    ruledOut,
  });
}

export async function getCachedRoadmap(kv: KVNamespace | undefined, ghToken: string | undefined): Promise<RoadmapFeed | null> {
  try {
    if (kv) {
      const cached = await kv.get(KV_KEY);
      if (cached) {
        const parsed = JSON.parse(cached) as RoadmapFeed;
        return normalizeRoadmapFeed(parsed);
      }
    }
    const fresh = await fetchRoadmap(ghToken);
    if (kv) {
      await kv.put(KV_KEY, JSON.stringify(fresh), { expirationTtl: KV_TTL });
    }
    return fresh;
  } catch {
    return null;
  }
}
