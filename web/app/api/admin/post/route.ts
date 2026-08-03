import { NextResponse } from "next/server";
import {
  deleteDraft,
  getAgentEnv,
  getDraft,
  parseDraftKey,
  validateSession,
  type CommunityAgentEnv,
} from "@/lib/community-agent";

export const dynamic = "force-dynamic";

async function checkAuth(req: Request, env: CommunityAgentEnv): Promise<{ ok: boolean; status?: number; error?: string }> {
  if (!env.MAINTAINER_TOKEN) {
    return { ok: false, status: 503, error: "MAINTAINER_TOKEN not configured" };
  }

  const cookieHeader = req.headers.get("cookie") ?? "";
  let sid: string | undefined;
  for (const c of cookieHeader.split(";")) {
    const [name, ...rest] = c.trim().split("=");
    if (name === "mt_sid") {
      sid = rest.join("=");
      break;
    }
  }

  if (!sid || !(await validateSession(env.CURATED_KV, sid))) {
    return { ok: false, status: 401, error: "unauthorized" };
  }
  return { ok: true };
}

const ALLOWED_ACTIONS = new Set(["post", "discard"]);
const ALLOWED_ORIGINS = new Set(["https://codewhale.net", "https://www.codewhale.net"]);
const MAX_BODY_BYTES = 65_536;

export async function POST(req: Request) {
  const env = await getAgentEnv();

  const origin = req.headers.get("origin");
  if (origin && !ALLOWED_ORIGINS.has(origin)) {
    return NextResponse.json({ error: "forbidden origin" }, { status: 403 });
  }

  const auth = await checkAuth(req, env);
  if (!auth.ok) {
    return NextResponse.json(
      { error: auth.error ?? "unauthorized" },
      { status: auth.status ?? 401, headers: { "Cache-Control": "no-store" } }
    );
  }

  const contentLength = Number(req.headers.get("content-length") ?? "0");
  if (contentLength > MAX_BODY_BYTES) {
    return NextResponse.json({ error: "payload too large" }, { status: 413 });
  }

  const body = await req.json() as { action: string; draftKey: string; editedBody?: string; lang?: "en" | "zh" };
  const { action, draftKey, editedBody, lang } = body;

  if (!ALLOWED_ACTIONS.has(action)) {
    return NextResponse.json({ error: "unknown action" }, { status: 400 });
  }
  if (typeof draftKey !== "string" || !draftKey || draftKey.length > 256) {
    return NextResponse.json({ error: "missing or invalid draftKey" }, { status: 400 });
  }
  if (!parseDraftKey(draftKey)) {
    return NextResponse.json({ error: "invalid draftKey namespace" }, { status: 400 });
  }
  if (editedBody !== undefined && (typeof editedBody !== "string" || editedBody.length > MAX_BODY_BYTES)) {
    return NextResponse.json({ error: "editedBody too long" }, { status: 413 });
  }
  if (lang !== undefined && lang !== "en" && lang !== "zh") {
    return NextResponse.json({ error: "invalid lang" }, { status: 400 });
  }

  const draft = await getDraft(env.CURATED_KV, draftKey);
  if (!draft) {
    return NextResponse.json({ error: "draft not found" }, { status: 404 });
  }

  if (action === "discard") {
    await deleteDraft(env.CURATED_KV, draftKey);
    return NextResponse.json({ ok: true, action: "discarded" });
  }

  if (action === "post") {
    if (!env.MAINTAINER_GITHUB_PAT) {
      return NextResponse.json({ error: "MAINTAINER_GITHUB_PAT not configured" }, { status: 500 });
    }

    const commentBody = editedBody ?? (lang === "zh" ? draft.bodyZh : draft.bodyEn);

    if (draft.type === "digest") {
      const digestBody = commentBody;
      const firstLine = digestBody.split("\n")[0].replace(/^#+\s*/, "").trim();
      const title = firstLine || `Weekly Digest ${draft.id}`;

      const digestRepo = env.GITHUB_REPO ?? "Hmbown/CodeWhale";
      const issuesUrl = `https://api.github.com/repos/${digestRepo}/issues`;

      const digestRes = await fetch(issuesUrl, {
        method: "POST",
        headers: {
          Accept: "application/vnd.github+json",
          Authorization: `token ${env.MAINTAINER_GITHUB_PAT}`,
          "X-GitHub-Api-Version": "2022-11-28",
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ title, body: digestBody, labels: ["digest"] }),
      });

      if (!digestRes.ok) {
        const text = await digestRes.text();
        return NextResponse.json({ error: `GitHub ${digestRes.status}: ${text}` }, { status: 502 });
      }

      const issue = await digestRes.json() as { number: number; html_url: string };

      draft.posted = true;
      draft.targetNumber = issue.number;
      draft.targetUrl = issue.html_url;
      await env.CURATED_KV?.put(draftKey, JSON.stringify(draft), { expirationTtl: 60 * 60 * 24 * 7 });

      return NextResponse.json({ ok: true, action: "posted", number: issue.number, url: issue.html_url });
    }

    if (!draft.targetNumber) {
      return NextResponse.json({ error: "no target number" }, { status: 400 });
    }

    const repo = env.GITHUB_REPO ?? "Hmbown/CodeWhale";
    const commentUrl = `https://api.github.com/repos/${repo}/issues/${draft.targetNumber}/comments`;

    const ghRes = await fetch(commentUrl, {
      method: "POST",
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${env.MAINTAINER_GITHUB_PAT}`,
        "X-GitHub-Api-Version": "2022-11-28",
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ body: commentBody }),
    });

    if (!ghRes.ok) {
      const text = await ghRes.text();
      return NextResponse.json({ error: `GitHub ${ghRes.status}: ${text}` }, { status: 502 });
    }

    // Mark as posted
    draft.posted = true;
    await env.CURATED_KV?.put(draftKey, JSON.stringify(draft), { expirationTtl: 60 * 60 * 24 * 7 });

    return NextResponse.json({ ok: true, action: "posted" });
  }

  // ALLOWED_ACTIONS guard above means this is unreachable.
  return NextResponse.json({ error: "unknown action" }, { status: 400 });
}
