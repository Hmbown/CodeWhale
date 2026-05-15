/**
 * content-watch.ts — two daily watchers that catch site drift the mechanical
 * facts pipeline misses:
 *
 *   runLinkCheck     — pings every external URL referenced in the site copy,
 *                      writes a draft per broken link (4xx/5xx). Stores a
 *                      `linkcheck:last` summary so /admin can show last status.
 *
 *   runSemanticDrift — reads recent CHANGELOG / commits, asks deepseek-v4-flash
 *                      whether any specific claims on the site look out of
 *                      date, writes review-required drafts.
 *
 * Both surface as drafts in CURATED_KV under `draft:linkcheck:<...>` and
 * `draft:semantic-drift:<...>`, picked up by the existing /admin listing.
 */
import { agentChat, saveDraft, type AgentDraft, type DeepSeekEnv, VOICE_CONSTRAINTS } from "./community-agent";

interface KVNamespace {
  get(k: string): Promise<string | null>;
  put(k: string, v: string, o?: { expirationTtl?: number }): Promise<void>;
  list(o?: { prefix?: string; limit?: number }): Promise<{ keys: { name: string }[] }>;
  delete(k: string): Promise<void>;
}

interface WatchEnv {
  CURATED_KV?: KVNamespace;
  DEEPSEEK_API_KEY?: string;
  DEEPSEEK_BASE_URL?: string;
  DEEPSEEK_MODEL?: string;
  GITHUB_TOKEN?: string;
}

function dsEnv(env: WatchEnv): DeepSeekEnv {
  return {
    baseUrl: env.DEEPSEEK_BASE_URL ?? process.env.DEEPSEEK_BASE_URL,
    model: env.DEEPSEEK_MODEL ?? process.env.DEEPSEEK_MODEL,
  };
}

// --- Link checker ---

// Targets to probe daily. For registries that block bot HEAD/GET (npm, crates.io)
// we hit the public JSON API instead — same upstream, doesn't 403.
const LINK_TARGETS: { url: string; label: string }[] = [
  { url: "https://github.com/Hmbown/deepseek-tui", label: "Main repo" },
  { url: "https://github.com/Hmbown/deepseek-tui/issues", label: "Issues" },
  { url: "https://github.com/Hmbown/deepseek-tui/pulls", label: "Pull Requests" },
  { url: "https://github.com/Hmbown/deepseek-tui/discussions", label: "Discussions" },
  { url: "https://github.com/Hmbown/deepseek-tui/releases", label: "Releases" },
  { url: "https://github.com/Hmbown/deepseek-tui/blob/main/LICENSE", label: "License file" },
  { url: "https://github.com/Hmbown/deepseek-tui/blob/main/CODE_OF_CONDUCT.md", label: "Code of Conduct" },
  { url: "https://github.com/Hmbown/deepseek-tui/blob/main/SECURITY.md", label: "Security policy" },
  { url: "https://github.com/Hmbown/deepseek-tui/blob/main/CONTRIBUTING.md", label: "Contributing guide" },
  { url: "https://github.com/Hmbown/deepseek-tui/blob/main/.github/PULL_REQUEST_TEMPLATE.md", label: "PR template" },
  { url: "https://github.com/Hmbown/homebrew-deepseek-tui", label: "Homebrew tap" },
  { url: "https://buymeacoffee.com/hmbown", label: "Support link (BMC)" },
  { url: "https://registry.npmjs.org/deepseek-tui", label: "npm package (registry API)" },
  // crates.io intentionally not in this list — both their HTML and JSON API return 403 to
  // Cloudflare Workers, so the check produces false positives. The crate links on the site
  // still work for human users.
];

export interface LinkCheckResult {
  url: string;
  label: string;
  status: number | "error";
  ok: boolean;
  ms: number;
}

async function probe(target: { url: string; label: string }): Promise<LinkCheckResult> {
  const start = Date.now();
  try {
    // Use HEAD where possible; fall back to GET on 405/403 since some hosts
    // (e.g. Cloudflare-protected) reject HEAD.
    let r = await fetch(target.url, { method: "HEAD", redirect: "follow" });
    if (r.status === 405 || r.status === 403 || r.status === 404) {
      // Some sites return 404 to HEAD but 200 to GET (e.g. NPM)
      r = await fetch(target.url, { method: "GET", redirect: "follow" });
    }
    return { url: target.url, label: target.label, status: r.status, ok: r.ok, ms: Date.now() - start };
  } catch {
    return { url: target.url, label: target.label, status: "error", ok: false, ms: Date.now() - start };
  }
}

export async function runLinkCheck(env: WatchEnv): Promise<{ ok: boolean; checked: number; broken: number; results?: LinkCheckResult[] }> {
  if (!env.CURATED_KV) return { ok: false, checked: 0, broken: 0 };

  const results = await Promise.all(LINK_TARGETS.map(probe));
  const broken = results.filter((r) => !r.ok);

  await env.CURATED_KV.put("linkcheck:last", JSON.stringify({
    at: new Date().toISOString(),
    checked: results.length,
    broken: broken.length,
    results,
  }), { expirationTtl: 60 * 60 * 24 * 14 });

  // Write drafts ONLY for new breakages — dedup by URL on the open-draft list.
  for (const b of broken) {
    const id = b.url.replace(/[^a-z0-9]+/gi, "-").slice(0, 80);
    const key = `draft:linkcheck:${id}`;
    const existing = await env.CURATED_KV.get(key);
    if (existing) continue; // already flagged; don't churn

    const draft: AgentDraft = {
      id,
      type: "triage", // reuse existing draft type so /admin renders it
      targetUrl: b.url,
      bodyEn: `**Broken link** (auto-detected by daily watch cron)\n\n- Label: **${b.label}**\n- URL: ${b.url}\n- HTTP status: ${b.status}\n- Latency: ${b.ms}ms\n\nThis URL is referenced in deepseek-tui.com copy. Update the source page or fix the destination.\n\n— drafted by community assistant, pending maintainer review`,
      bodyZh: `**链接失效**（每日巡检自动发现）\n\n- 名称：**${b.label}**\n- 地址：${b.url}\n- HTTP 状态：${b.status}\n- 延迟：${b.ms}ms\n\n该地址被 deepseek-tui.com 文案引用，请更新源页面或修复目标。\n\n— 由社区助理草拟，待维护者审阅`,
      generatedAt: new Date().toISOString(),
      posted: false,
    };
    await saveDraft(env.CURATED_KV, draft);
  }

  return { ok: true, checked: results.length, broken: broken.length, results: broken };
}

// --- Semantic drift ---

const SEMANTIC_DRIFT_PROMPT = `You are reviewing copy on a community website (deepseek-tui.com) for the open-source deepseek-tui project.

Given:
1. The CHANGELOG entries below (most recent first)
2. The current homepage and docs page text below
3. Recent commit messages

Identify any factual claims on the site that are CONTRADICTED by recent changes. Be conservative — only flag claims you can directly tie to a CHANGELOG line or commit. Don't speculate.

Return ONLY this JSON shape (no prose, no markdown fences):
{
  "drifts": [
    {
      "page": "homepage" | "docs" | "install" | "contribute" | "roadmap",
      "claim": "exact text on the site that is now inaccurate",
      "evidence": "the CHANGELOG line or commit hash that contradicts it",
      "suggested_replacement": "what the site should say instead"
    }
  ]
}

If nothing is drifted, return { "drifts": [] }.

${VOICE_CONSTRAINTS}`;

function startsWithAsciiCI(input: string, index: number, needle: string): boolean {
  if (index + needle.length > input.length) return false;
  return input.slice(index, index + needle.length).toLowerCase() === needle;
}

function isWhitespace(c: string | undefined): boolean {
  return c === " " || c === "\n" || c === "\r" || c === "\t" || c === "\f";
}

function tagNameBoundary(input: string, index: number): boolean {
  const c = input[index];
  return c === undefined || c === ">" || c === "/" || isWhitespace(c);
}

function findClosingRawTextTag(input: string, from: number, tagName: "script" | "style"): number {
  const closePrefix = `</${tagName}`;
  for (let i = from; i < input.length; i += 1) {
    if (startsWithAsciiCI(input, i, closePrefix) && tagNameBoundary(input, i + closePrefix.length)) {
      const close = input.indexOf(">", i + closePrefix.length);
      return close === -1 ? input.length : close + 1;
    }
  }
  return input.length;
}

function collapseWhitespace(input: string): string {
  let out = "";
  let pendingSpace = false;
  for (const c of input) {
    if (isWhitespace(c)) {
      pendingSpace = out.length > 0;
      continue;
    }
    if (pendingSpace) out += " ";
    out += c;
    pendingSpace = false;
  }
  return out.trim();
}

function stripHtmlForPrompt(input: string): string {
  let out = "";
  for (let i = 0; i < input.length;) {
    if (input[i] !== "<") {
      out += input[i];
      i += 1;
      continue;
    }

    if (startsWithAsciiCI(input, i, "<script") && tagNameBoundary(input, i + "<script".length)) {
      out += " ";
      const openEnd = input.indexOf(">", i + 1);
      i = openEnd === -1 ? input.length : findClosingRawTextTag(input, openEnd + 1, "script");
      continue;
    }
    if (startsWithAsciiCI(input, i, "<style") && tagNameBoundary(input, i + "<style".length)) {
      out += " ";
      const openEnd = input.indexOf(">", i + 1);
      i = openEnd === -1 ? input.length : findClosingRawTextTag(input, openEnd + 1, "style");
      continue;
    }

    out += " ";
    const tagEnd = input.indexOf(">", i + 1);
    i = tagEnd === -1 ? input.length : tagEnd + 1;
  }
  return collapseWhitespace(out).slice(0, 8000);
}

export async function runSemanticDrift(env: WatchEnv): Promise<{ ok: boolean; drafted: number; reason?: string }> {
  if (!env.CURATED_KV || !env.DEEPSEEK_API_KEY) {
    return { ok: false, drafted: 0, reason: "missing CURATED_KV or DEEPSEEK_API_KEY" };
  }

  const ghHeaders: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "User-Agent": "deepseek-tui-web-semantic-drift",
  };
  if (env.GITHUB_TOKEN) ghHeaders["Authorization"] = `Bearer ${env.GITHUB_TOKEN}`;

  // Fetch CHANGELOG (truncated), recent commits, and live homepage HTML.
  const [changelog, commits, homepageHtml, docsHtml] = await Promise.all([
    fetch("https://raw.githubusercontent.com/Hmbown/deepseek-tui/main/CHANGELOG.md", { headers: ghHeaders }).then((r) => r.ok ? r.text() : "").catch(() => ""),
    fetch("https://api.github.com/repos/Hmbown/deepseek-tui/commits?per_page=30", { headers: ghHeaders }).then((r) => r.ok ? r.json() as Promise<{ commit: { message: string }; sha: string }[]> : []).catch(() => []),
    fetch("https://deepseek-tui.com/en", { headers: { "User-Agent": "deepseek-tui-watch" } }).then((r) => r.ok ? r.text() : "").catch(() => ""),
    fetch("https://deepseek-tui.com/en/docs", { headers: { "User-Agent": "deepseek-tui-watch" } }).then((r) => r.ok ? r.text() : "").catch(() => ""),
  ]);

  if (!changelog && (!commits || commits.length === 0)) {
    return { ok: false, drafted: 0, reason: "no changelog or commits available" };
  }

  const homepageText = stripHtmlForPrompt(homepageHtml);
  const docsText = stripHtmlForPrompt(docsHtml);
  const changelogHead = changelog.slice(0, 4000);
  const commitMsgs = commits.slice(0, 30).map((c) => `- ${c.sha.slice(0, 7)}: ${c.commit.message.split("\n")[0]}`).join("\n");

  const userMessage = `## Recent CHANGELOG entries
${changelogHead || "(no CHANGELOG.md fetched)"}

## Last 30 commits
${commitMsgs || "(no commits fetched)"}

## Homepage text (HTML stripped)
${homepageText}

## Docs page text (HTML stripped)
${docsText}`;

  let response: { content: string; usage: { input: number; output: number } };
  try {
    response = await agentChat(
      [
        { role: "system", content: SEMANTIC_DRIFT_PROMPT },
        { role: "user", content: userMessage },
      ],
      env.DEEPSEEK_API_KEY,
      true,
      dsEnv(env),
    );
  } catch (e) {
    return { ok: false, drafted: 0, reason: `LLM call failed: ${e}` };
  }

  // Extract JSON (jsonMode usually returns clean JSON, but defend against fences)
  let parsed: { drifts?: { page: string; claim: string; evidence: string; suggested_replacement: string }[] };
  try {
    const trimmed = response.content.replace(/^```(?:json)?\s*/i, "").replace(/\s*```\s*$/i, "").trim();
    parsed = JSON.parse(trimmed);
  } catch {
    return { ok: false, drafted: 0, reason: "LLM returned non-JSON" };
  }

  const drifts = parsed.drifts ?? [];
  let drafted = 0;
  for (const d of drifts) {
    const id = `${d.page}-${d.claim.slice(0, 40).replace(/[^a-z0-9]+/gi, "-").toLowerCase()}`.slice(0, 80);
    const key = `draft:semantic-drift:${id}`;
    const existing = await env.CURATED_KV.get(key);
    if (existing) continue;

    const body = `Page: **${d.page}**\n\nClaim that may be drifted:\n> ${d.claim}\n\nEvidence:\n> ${d.evidence}\n\nSuggested replacement:\n> ${d.suggested_replacement}\n\n— drafted by community assistant, pending maintainer review`;
    const draft: AgentDraft = {
      id,
      type: "triage",
      targetUrl: `https://deepseek-tui.com/en/${d.page === "homepage" ? "" : d.page}`,
      bodyEn: body,
      bodyZh: body,
      generatedAt: new Date().toISOString(),
      posted: false,
    };
    await saveDraft(env.CURATED_KV, draft);
    drafted++;
  }

  return { ok: true, drafted };
}
