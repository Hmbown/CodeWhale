/**
 * facts-drift.ts — runtime version of scripts/derive-facts.mjs.
 *
 * Fetches source-of-truth files from raw.githubusercontent.com on a schedule,
 * re-derives the same RepoFacts shape, compares to the value cached in KV (or
 * to the build-time fallback on first run), and if anything changed writes
 * the new facts to CURATED_KV under "facts:current". Pages prefer the KV
 * value over the build-time `FACTS` constant via `getFacts()`.
 *
 * Mechanical drift (provider added, sandbox backend renamed, version bumped)
 * fixes itself within one cron tick — no redeploy. Semantic drift (a new
 * feature should be advertised on the homepage) is still left to humans.
 */
import type { RepoFacts, ProviderFact } from "./facts.generated";
import { FACTS as BUILD_FACTS } from "./facts.generated";

const RAW_BASE = "https://raw.githubusercontent.com/Hmbown/CodeWhale/main";
const KV_KEY = "facts:current";
const LOG_KEY = "facts:drift-log";

interface KVNamespace {
  get(k: string): Promise<string | null>;
  put(k: string, v: string, o?: { expirationTtl?: number }): Promise<void>;
}

async function fetchText(path: string, ghToken?: string): Promise<string | null> {
  const headers: Record<string, string> = {
    "User-Agent": "codewhale-web-drift",
  };
  if (ghToken) headers["Authorization"] = `Bearer ${ghToken}`;
  try {
    const r = await fetch(`${RAW_BASE}/${path}`, { headers });
    if (!r.ok) return null;
    return await r.text();
  } catch {
    return null;
  }
}

async function fetchListing(dir: string, ghToken?: string): Promise<string[] | null> {
  // Use GitHub Contents API to list a directory.
  const url = `https://api.github.com/repos/Hmbown/CodeWhale/contents/${dir}?ref=main`;
  const headers: Record<string, string> = {
    "Accept": "application/vnd.github+json",
    "User-Agent": "codewhale-web-drift",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  if (ghToken) headers["Authorization"] = `Bearer ${ghToken}`;
  try {
    const r = await fetch(url, { headers });
    if (!r.ok) return null;
    const arr = (await r.json()) as { name: string; type: string }[];
    return arr.filter((e) => e.type === "file").map((e) => e.name);
  } catch {
    return null;
  }
}

function deriveVersion(cargo: string): string | null {
  const m = cargo.match(/^version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}

function deriveCrates(cargo: string): string[] {
  const block = cargo.match(/members\s*=\s*\[([\s\S]*?)\]/);
  if (!block) return [];
  return [...block[1].matchAll(/"crates\/([^"]+)"/g)].map((m) => m[1]).sort();
}

function deriveProvidersFromConfig(cfg: string): ProviderFact[] {
  const enumBlock = cfg.match(/pub enum ApiProvider \{([\s\S]*?)\}/);
  if (!enumBlock) return [];
  const variants = [...enumBlock[1].matchAll(/^\s*(\w+)\s*,\s*$/gm)].map((m) => m[1]);
  // Match what the published CLI binary's `--provider` flag accepts
  // (ProviderArg in crates/cli/src/lib.rs). DeepseekCN exists in the
  // legacy tui ApiProvider enum but is not wired through ProviderKind,
  // so the binary rejects it — keep it out of the docs. Issue #1104.
  const labelMap: Record<string, ProviderFact> = {
    Deepseek: { id: "deepseek", label: "DeepSeek", env: "DEEPSEEK_API_KEY" },
    DeepseekAnthropic: { id: "deepseek-anthropic", label: "DeepSeek Anthropic", env: "DEEPSEEK_API_KEY / ANTHROPIC_API_KEY" },
    NvidiaNim: { id: "nvidia-nim", label: "NVIDIA NIM", env: "NVIDIA_API_KEY / NVIDIA_NIM_API_KEY" },
    Openai: { id: "openai", label: "OpenAI-compatible", env: "OPENAI_API_KEY" },
    Atlascloud: { id: "atlascloud", label: "AtlasCloud", env: "ATLASCLOUD_API_KEY" },
    WanjieArk: { id: "wanjie-ark", label: "Wanjie Ark", env: "WANJIE_ARK_API_KEY / WANJIE_API_KEY / WANJIE_MAAS_API_KEY" },
    Volcengine: { id: "volcengine", label: "Volcengine Ark", env: "VOLCENGINE_API_KEY / VOLCENGINE_ARK_API_KEY / ARK_API_KEY" },
    Openrouter: { id: "openrouter", label: "OpenRouter", env: "OPENROUTER_API_KEY" },
    XiaomiMimo: { id: "xiaomi-mimo", label: "Xiaomi MiMo", env: "XIAOMI_MIMO_TOKEN_PLAN_API_KEY / MIMO_TOKEN_PLAN_API_KEY / XIAOMI_MIMO_API_KEY / XIAOMI_API_KEY / MIMO_API_KEY" },
    Novita: { id: "novita", label: "Novita AI", env: "NOVITA_API_KEY" },
    Fireworks: { id: "fireworks", label: "Fireworks AI", env: "FIREWORKS_API_KEY" },
    Siliconflow: { id: "siliconflow", label: "SiliconFlow", env: "SILICONFLOW_API_KEY" },
    SiliconflowCn: { id: "siliconflow-CN", label: "SiliconFlow CN", env: "SILICONFLOW_API_KEY" },
    Arcee: { id: "arcee", label: "Arcee AI", env: "ARCEE_API_KEY" },
    Moonshot: { id: "moonshot", label: "Moonshot/Kimi", env: "MOONSHOT_API_KEY / KIMI_API_KEY" },
    Sglang: { id: "sglang", label: "SGLang", env: "SGLANG_API_KEY" },
    Vllm: { id: "vllm", label: "vLLM", env: "VLLM_API_KEY" },
    Ollama: { id: "ollama", label: "Ollama", env: "OLLAMA_API_KEY" },
    Huggingface: { id: "huggingface", label: "Hugging Face", env: "HUGGINGFACE_API_KEY / HF_TOKEN" },
    Deepinfra: { id: "deepinfra", label: "DeepInfra", env: "DEEPINFRA_API_KEY / DEEPINFRA_TOKEN" },
    Together: { id: "together", label: "Together AI", env: "TOGETHER_API_KEY" },
    Qianfan: { id: "qianfan", label: "Baidu Qianfan", env: "QIANFAN_API_KEY / BAIDU_QIANFAN_API_KEY" },
    OpenaiCodex: { id: "openai-codex", label: "OpenAI Codex", env: "ChatGPT/Codex OAuth via `codex login` (OPENAI_CODEX_ACCESS_TOKEN / CODEX_ACCESS_TOKEN override)" },
    Anthropic: { id: "anthropic", label: "Anthropic", env: "ANTHROPIC_API_KEY" },
    Zai: { id: "zai", label: "Z.ai", env: "ZAI_API_KEY / Z_AI_API_KEY" },
    Stepfun: { id: "stepfun", label: "StepFun", env: "STEPFUN_API_KEY / STEP_API_KEY" },
    Minimax: { id: "minimax", label: "MiniMax", env: "MINIMAX_API_KEY" },
    Openmodel: { id: "openmodel", label: "OpenModel", env: "OPENMODEL_API_KEY" },
    Sakana: { id: "sakana", label: "Sakana AI", env: "FUGU_API_KEY / SAKANA_API_KEY" },
    LongCat: { id: "longcat", label: "LongCat", env: "LONGCAT_API_KEY" },
    Meta: { id: "meta", label: "Meta Model API", env: "META_MODEL_API_KEY / MODEL_API_KEY" },
    Xai: { id: "xai", label: "xAI", env: "XAI_API_KEY" },
  };
  // Log loudly on unmapped variants so a new provider can never be silently
  // dropped from the drift-derived facts again. DeepseekCN (#1104) and the
  // dynamic Custom meta-provider (#1519, user-defined endpoints) are the
  // deliberate exclusions.
  const EXCLUDED = new Set(["DeepseekCN", "Custom"]);
  const unmapped = variants.filter((v) => !EXCLUDED.has(v) && !labelMap[v]);
  if (unmapped.length > 0) {
    console.warn(
      `[facts-drift] ApiProvider variants missing from labelMap: ${unmapped.join(", ")}. ` +
        "Add them to labelMap here AND PROVIDER_LABEL_MAP in web/scripts/facts-lib.mjs (or to EXCLUDED if intentionally hidden).",
    );
  }
  return variants.map((v) => labelMap[v]).filter(Boolean);
}

function deriveDefaultModel(cfg: string): string | null {
  // Match the const *definition* (`= "..."`); the definition moved to
  // config/models.rs in the #3311 split, so callers pass config.rs + models.rs.
  const m = cfg.match(/DEFAULT_TEXT_MODEL\s*(?::\s*&str\s*)?=\s*"([^"]+)"/);
  return m ? m[1] : null;
}

function deriveSandboxBackends(files: string[]): string[] {
  const map: Record<string, string> = {
    seatbelt: "seatbelt (macOS)",
    landlock: "landlock (Linux)",
  };
  return files
    .map((f) => f.replace(/\.rs$/, ""))
    .filter((n) => map[n])
    .sort()
    .map((n) => map[n]);
}

async function fetchLatestRelease(ghToken?: string): Promise<string | null> {
  const headers: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "User-Agent": "codewhale-web-drift",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  if (ghToken) headers["Authorization"] = `Bearer ${ghToken}`;
  try {
    const r = await fetch("https://api.github.com/repos/Hmbown/CodeWhale/releases/latest", { headers });
    if (!r.ok) return null;
    const j = (await r.json()) as { tag_name?: string };
    return j.tag_name ?? null;
  } catch {
    return null;
  }
}

function deriveLicense(licText: string): string | null {
  const first = licText.split(/\r?\n/).find((l) => l.trim().length > 0);
  if (!first) return null;
  if (/^MIT License/i.test(first)) return "MIT";
  if (/Apache.*2\.0/i.test(first)) return "Apache-2.0";
  return first.trim();
}

export async function deriveFactsFromRemote(ghToken?: string): Promise<RepoFacts | null> {
  const [cargo, configRs, configModels, sandboxFiles, npmPkg, licText, toolFiles, latestRelease] = await Promise.all([
    fetchText("Cargo.toml", ghToken),
    fetchText("crates/tui/src/config.rs", ghToken),
    fetchText("crates/tui/src/config/models.rs", ghToken),
    fetchListing("crates/tui/src/sandbox", ghToken),
    fetchText("npm/codewhale/package.json", ghToken),
    fetchText("LICENSE", ghToken),
    fetchListing("crates/tui/src/tools", ghToken),
    fetchLatestRelease(ghToken),
  ]);

  void toolFiles; // unused now; build-time value is canonical
  if (!cargo || !configRs) return null;

  const facts: RepoFacts = {
    generatedAt: new Date().toISOString(),
    version: deriveVersion(cargo),
    crates: deriveCrates(cargo),
    sandboxBackends: sandboxFiles ? deriveSandboxBackends(sandboxFiles) : BUILD_FACTS.sandboxBackends,
    providers: deriveProvidersFromConfig(configRs),
    defaultModel: deriveDefaultModel(`${configRs}\n${configModels ?? ""}`),
    nodeEngines: (() => {
      try { return npmPkg ? JSON.parse(npmPkg).engines?.node ?? null : null; } catch { return null; }
    })(),
    // Tool count: build-time uses ToolSpec impl regex; fetching every tool file at runtime is too
    // expensive, and the file count would be a different (less accurate) number. Preserve the
    // build-time value through KV instead of approximating.
    toolCount: BUILD_FACTS.toolCount,
    license: licText ? deriveLicense(licText) : BUILD_FACTS.license,
    latestRelease,
  };

  if (!facts.version || facts.crates.length === 0 || facts.providers.length === 0) {
    return null;
  }
  return facts;
}

interface DriftDiff {
  field: keyof RepoFacts;
  before: unknown;
  after: unknown;
}

function diff(a: RepoFacts, b: RepoFacts): DriftDiff[] {
  const fields: (keyof RepoFacts)[] = ["version", "crates", "sandboxBackends", "providers", "defaultModel", "nodeEngines", "toolCount", "license", "latestRelease"];
  const out: DriftDiff[] = [];
  for (const f of fields) {
    const av = JSON.stringify(a[f]);
    const bv = JSON.stringify(b[f]);
    if (av !== bv) out.push({ field: f, before: a[f], after: b[f] });
  }
  return out;
}

export interface FactsDriftResult {
  ok: boolean;
  changed?: boolean;
  diffs?: DriftDiff[];
  reason?: string;
}

export async function runFactsDrift(env: { CURATED_KV?: KVNamespace; GITHUB_TOKEN?: string }): Promise<FactsDriftResult> {
  if (!env.CURATED_KV) return { ok: false, reason: "CURATED_KV not bound" };

  const remote = await deriveFactsFromRemote(env.GITHUB_TOKEN);
  if (!remote) return { ok: false, reason: "remote derivation failed" };

  const cachedRaw = await env.CURATED_KV.get(KV_KEY);
  const cached: RepoFacts = cachedRaw ? JSON.parse(cachedRaw) : BUILD_FACTS;

  const diffs = diff(cached, remote);
  if (diffs.length === 0) {
    return { ok: true, changed: false };
  }

  // Write new facts. No TTL — they live until next drift overwrites them.
  await env.CURATED_KV.put(KV_KEY, JSON.stringify(remote));

  // Append to drift log (last 20 entries).
  try {
    const logRaw = await env.CURATED_KV.get(LOG_KEY);
    const log = logRaw ? (JSON.parse(logRaw) as Array<{ at: string; diffs: DriftDiff[] }>) : [];
    log.unshift({ at: remote.generatedAt, diffs });
    await env.CURATED_KV.put(LOG_KEY, JSON.stringify(log.slice(0, 20)));
  } catch {
    /* non-fatal */
  }

  return { ok: true, changed: true, diffs };
}
