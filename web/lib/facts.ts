import { FACTS as BUILD_TIME_FACTS, type RepoFacts, type ProviderFact } from "./facts.generated";

const KV_KEY = "facts:current";

export type { RepoFacts, ProviderFact };
export const BUILD_FACTS = BUILD_TIME_FACTS;

interface KVNamespace {
  get(key: string): Promise<string | null>;
  put(key: string, value: string, opts?: { expirationTtl?: number }): Promise<void>;
}

async function getKv(): Promise<KVNamespace | undefined> {
  if (process.env.NEXT_PHASE === "phase-production-build") {
    return undefined;
  }

  try {
    const mod = await import("@opennextjs/cloudflare");
    const ctx = await mod.getCloudflareContext({ async: true });
    return (ctx.env as { CURATED_KV?: KVNamespace }).CURATED_KV;
  } catch {
    return undefined;
  }
}

/**
 * Resolved facts for the current request. Prefers a KV override (written by
 * the content-drift cron when it detects new repo state) over the build-time
 * snapshot. Always returns a valid RepoFacts — falls back to BUILD_FACTS on
 * any error.
 */
export async function getFacts(): Promise<RepoFacts> {
  try {
    const kv = await getKv();
    if (!kv) return BUILD_FACTS;
    const raw = await kv.get(KV_KEY);
    if (!raw) return BUILD_FACTS;
    const parsed = JSON.parse(raw) as RepoFacts;
    if (!parsed.version || !Array.isArray(parsed.providers)) return BUILD_FACTS;
    return parsed;
  } catch {
    return BUILD_FACTS;
  }
}
