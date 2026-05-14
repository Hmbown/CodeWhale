import type { CuratedDispatch, FeedItem, RepoStats } from "./types";

const FALLBACK_BASE = "https://api.deepseek.com";
const FALLBACK_MODEL = "deepseek-v4-flash";

interface ChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

interface ChatResponse {
  choices: { message: { content: string } }[];
}

export interface DeepSeekEnv {
  baseUrl?: string;
  model?: string;
}

export async function chat(
  messages: ChatMessage[],
  apiKey: string,
  jsonMode = false,
  dsEnv?: DeepSeekEnv
): Promise<string> {
  const base = dsEnv?.baseUrl ?? process.env.DEEPSEEK_BASE_URL ?? FALLBACK_BASE;
  const model = dsEnv?.model ?? process.env.DEEPSEEK_MODEL ?? FALLBACK_MODEL;
  const res = await fetch(`${base}/v1/chat/completions`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({
      model,
      messages,
      temperature: 0.4,
      max_tokens: 4096,
      reasoning_effort: "high",
      ...(jsonMode ? { response_format: { type: "json_object" } } : {}),
    }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`DeepSeek ${res.status}: ${text}`);
  }
  const data = (await res.json()) as ChatResponse;
  return data.choices[0]?.message?.content ?? "";
}

const SYSTEM_PROMPT = `You are the editor of "今日要闻 / Today's Dispatch", a daily-ish digest for the deepseek-tui open source project.

You receive: repo stats and a list of recently updated issues, PRs, and releases.
Output a single JSON object — no prose around it — matching this exact shape:

{
  "headline": "string — English editorial headline (max ~70 chars)",
  "summary": "string — 2-3 English sentences, calm factual editorial voice",
  "highlights": [
    { "title": "string", "href": "string", "tag": "shipped|merged|opened|discussion|release", "blurb": "one sentence, max ~120 chars" }
  ],
  "movers": [
    { "number": 123, "title": "string", "href": "string", "reason": "one short clause" }
  ],
  "headlineZh": "string — Chinese (zh-CN) editorial headline, rewritten natively, not translated",
  "summaryZh": "string — 2-3 Chinese sentences, native zh-CN prose",
  "highlightsZh": [
    { "title": "string — zh-CN native", "href": "string", "tag": "shipped|merged|opened|discussion|release", "blurb": "zh-CN one sentence" }
  ],
  "moversZh": [
    { "number": 123, "title": "string — zh-CN", "href": "string", "reason": "zh-CN clause" }
  ]
}

Rules:
- Pick 3-5 highlights and 3-5 movers from the actual provided items. Never invent.
- Prefer items with discussion, merged PRs, recent releases, or labelled "good first issue".
- Tone: like a small-paper editor — measured, specific, never breathless.
- Never use words like "exciting", "amazing", "powerful", "revolutionary" (or Chinese equivalents like 令人兴奋, 强大无比, 革命性).
- href must be the html_url provided.
- The zh-CN fields must be native Chinese prose — not a direct translation of the English fields. Write them as a Chinese-speaking maintainer would.
- zh-CN uses full-width punctuation in CJK sentences (。，、).`;

export async function curate(
  apiKey: string,
  stats: RepoStats,
  feed: FeedItem[],
  dsEnv?: DeepSeekEnv
): Promise<CuratedDispatch> {
  const trimmedFeed = feed.slice(0, 25).map((f) => ({
    kind: f.kind,
    number: f.number,
    title: f.title,
    state: f.state,
    href: f.url,
    author: f.author,
    updated: f.updatedAt,
    comments: f.comments,
    labels: f.labels.map((l) => l.name),
  }));

  const userPayload = {
    repo: "Hmbown/deepseek-tui",
    stats: {
      stars: stats.stars,
      forks: stats.forks,
      open_issues: stats.openIssues,
      open_pulls: stats.openPulls,
      latest_release: stats.latestRelease?.tag,
    },
    recent: trimmedFeed,
  };

  const raw = await chat(
    [
      { role: "system", content: SYSTEM_PROMPT },
      { role: "user", content: JSON.stringify(userPayload, null, 2) },
    ],
    apiKey,
    true,
    dsEnv
  );

  const parsed = JSON.parse(raw) as Omit<CuratedDispatch, "generatedAt">;
  return { ...sanitizeDispatch(parsed), generatedAt: new Date().toISOString() };
}

const SAFE_HREF_RE = /^https:\/\/(?:github\.com|api\.github\.com|deepseek-tui\.com|crates\.io|www\.npmjs\.com|docs\.rs)\//;
const FALLBACK_HREF = "https://github.com/Hmbown/deepseek-tui";

function safeHref(u: unknown): string {
  return typeof u === "string" && SAFE_HREF_RE.test(u) ? u : FALLBACK_HREF;
}

function sanitizeDispatch(d: Omit<CuratedDispatch, "generatedAt">): Omit<CuratedDispatch, "generatedAt"> {
  return {
    ...d,
    highlights: (d.highlights ?? []).map((h) => ({ ...h, href: safeHref(h.href) })),
    movers: (d.movers ?? []).map((m) => ({ ...m, href: safeHref(m.href) })),
    highlightsZh: (d.highlightsZh ?? []).map((h) => ({ ...h, href: safeHref(h.href) })),
    moversZh: (d.moversZh ?? []).map((m) => ({ ...m, href: safeHref(m.href) })),
  };
}
