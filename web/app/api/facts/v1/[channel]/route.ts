import { getEnv } from "@/lib/kv";
import { isValidChannel, resolveCloudFacts, type CloudFactsResult } from "@/lib/cloud-facts";

/**
 * Public, credential-free cloud facts envelope for one channel (facts/v1).
 *
 * GET  /api/facts/v1/<channel>  → signed envelope JSON (strong ETag, CDN-cacheable)
 * HEAD /api/facts/v1/<channel>  → headers only
 *
 * The envelope is verified server-side before it is served; clients verify it
 * again against the keys pinned in the binary. No cookies, no Vary, no query
 * parameters: the response is identical for every caller so any CDN in front
 * (Cloudflare today, Vercel if the host moves) can cache it.
 */
export const dynamic = "force-dynamic";
export const revalidate = 0;

const CACHE_CONTROL = "public, max-age=300, s-maxage=300, stale-while-revalidate=3600, stale-if-error=604800";

function baseHeaders(): Record<string, string> {
  return {
    "Content-Type": "application/json; charset=utf-8",
    "Access-Control-Allow-Origin": "*",
    "X-Content-Type-Options": "nosniff",
  };
}

function errorResponse(status: number, body: Record<string, unknown>, extra: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { ...baseHeaders(), "Cache-Control": "no-store", ...extra },
  });
}

function etagMatches(ifNoneMatch: string | null, etag: string): boolean {
  if (!ifNoneMatch) return false;
  return ifNoneMatch
    .split(",")
    .map((v) => v.trim().replace(/^W\//, ""))
    .some((v) => v === etag || v === "*");
}

export function responseFor(result: CloudFactsResult, req: Request, channel: string, method: "GET" | "HEAD"): Response {
  switch (result.kind) {
    case "none":
      return errorResponse(404, { error: "no-facts", channel }, { "Cache-Control": "public, max-age=60" });
    case "sha-mismatch":
      return errorResponse(502, { error: "facts-digest-mismatch", channel, factsVersion: result.factsVersion });
    case "unverifiable":
      return errorResponse(503, { error: "facts-unverifiable", reason: result.reason, channel, factsVersion: result.factsVersion }, { "Retry-After": "600" });
    case "unavailable":
      return errorResponse(503, { error: "facts-unavailable", channel }, { "Retry-After": "600" });
    case "ok": {
      const headers: Record<string, string> = {
        ...baseHeaders(),
        "Cache-Control": CACHE_CONTROL,
        ETag: result.etag,
        "X-Facts-Channel": result.envelope.channel,
        "X-Facts-Version": String(result.envelope.facts_version),
        "X-Facts-Source": result.source,
        "X-Facts-Verified": result.verified,
      };
      if (etagMatches(req.headers.get("if-none-match"), result.etag)) {
        return new Response(null, { status: 304, headers });
      }
      headers["Content-Length"] = String(new TextEncoder().encode(result.body).length);
      return new Response(method === "HEAD" ? null : result.body, { status: 200, headers });
    }
  }
}

async function handle(req: Request, ctx: { params: Promise<{ channel: string }> }, method: "GET" | "HEAD"): Promise<Response> {
  const { channel } = await ctx.params;
  if (!isValidChannel(channel)) {
    return errorResponse(404, { error: "no-facts" }, { "Cache-Control": "public, max-age=60" });
  }
  const env = await getEnv();
  const result = await resolveCloudFacts(channel, env);
  return responseFor(result, req, channel, method);
}

export async function GET(req: Request, ctx: { params: Promise<{ channel: string }> }) {
  return handle(req, ctx, "GET");
}

export async function HEAD(req: Request, ctx: { params: Promise<{ channel: string }> }) {
  return handle(req, ctx, "HEAD");
}
