/**
 * facts-drift.ts — runtime version of scripts/derive-facts.mjs.
 *
 * Fetches source-of-truth files from raw.githubusercontent.com on a schedule,
 * re-derives the same RepoFacts shape, compares to the value cached in KV (or
 * to the build-time fallback on first run), and if anything changed writes
 * the new facts to CURATED_KV under "facts:current". `getFacts()` accepts the
 * KV value only when its exact source provenance is at least as new as the
 * deployed build; published-release metadata is resolved separately.
 *
 * Mechanical drift (provider added, sandbox backend renamed, version bumped)
 * fixes itself within one cron tick — no redeploy. Semantic drift (a new
 * feature should be advertised on the homepage) is still left to humans.
 */
import type {
  PublishedReleaseFact,
  RepoFacts,
  ProviderFact,
} from "./facts.generated";
import { FACTS as BUILD_FACTS } from "./facts.generated";

const RAW_ROOT = "https://raw.githubusercontent.com/Hmbown/CodeWhale";
const KV_KEY = "facts:current";
const LOG_KEY = "facts:drift-log";

interface KVNamespace {
  get(k: string): Promise<string | null>;
  put(k: string, v: string, o?: { expirationTtl?: number }): Promise<void>;
}

interface SourceMarker {
  revision: string;
  committedAt: string;
}

async function fetchText(
  path: string,
  revision: string,
  ghToken?: string,
): Promise<string | null> {
  const headers: Record<string, string> = {
    "User-Agent": "codewhale-web-drift",
  };
  if (ghToken) headers["Authorization"] = `Bearer ${ghToken}`;
  try {
    const r = await fetch(`${RAW_ROOT}/${revision}/${path}`, { headers });
    if (!r.ok) return null;
    return await r.text();
  } catch {
    return null;
  }
}

async function fetchSourceMarker(ghToken?: string): Promise<SourceMarker | null> {
  const headers: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "User-Agent": "codewhale-web-drift",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  if (ghToken) headers.Authorization = `Bearer ${ghToken}`;
  try {
    const response = await fetch(
      "https://api.github.com/repos/Hmbown/CodeWhale/commits/main",
      { headers },
    );
    if (!response.ok) return null;
    const json = (await response.json()) as {
      sha?: string;
      commit?: { committer?: { date?: string } };
    };
    const revision = json.sha;
    const committedAt = json.commit?.committer?.date;
    if (
      !revision ||
      !/^[0-9a-f]{40}$/i.test(revision) ||
      !committedAt ||
      !Number.isFinite(Date.parse(committedAt))
    ) {
      return null;
    }
    return { revision, committedAt };
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
    Orcarouter: { id: "orcarouter", label: "OrcaRouter", env: "ORCAROUTER_API_KEY" },
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
    OllamaCloud: { id: "ollama-cloud", label: "Ollama Cloud", env: "OLLAMA_CLOUD_API_KEY / OLLAMA_API_KEY" },
    Huggingface: { id: "huggingface", label: "Hugging Face", env: "HUGGINGFACE_API_KEY / HF_TOKEN" },
    Deepinfra: { id: "deepinfra", label: "DeepInfra", env: "DEEPINFRA_API_KEY / DEEPINFRA_TOKEN" },
    Together: { id: "together", label: "Together AI", env: "TOGETHER_API_KEY" },
    Qianfan: { id: "qianfan", label: "Baidu Qianfan", env: "QIANFAN_API_KEY / BAIDU_QIANFAN_API_KEY" },
    OpenaiCodex: { id: "openai-codex", label: "OpenAI Codex", env: "ChatGPT/Codex OAuth via `codex login` (OPENAI_CODEX_ACCESS_TOKEN / CODEX_ACCESS_TOKEN override)" },
    OpencodeGo: { id: "opencode-go", label: "OpenCode Go", env: "OPENCODE_GO_API_KEY" },
    OpencodeZen: { id: "opencode-zen", label: "OpenCode Zen", env: "OPENCODE_ZEN_API_KEY / OPENCODE_API_KEY" },
    Anthropic: { id: "anthropic", label: "Anthropic", env: "ANTHROPIC_API_KEY" },
    Zai: { id: "zai", label: "Z.ai", env: "ZAI_API_KEY / Z_AI_API_KEY" },
    Stepfun: { id: "stepfun", label: "StepFun", env: "STEPFUN_API_KEY / STEP_API_KEY" },
    Minimax: { id: "minimax", label: "MiniMax", env: "MINIMAX_API_KEY" },
    MinimaxAnthropic: { id: "minimax-anthropic", label: "MiniMax (Anthropic-compatible)", env: "MINIMAX_API_KEY" },
    Openmodel: { id: "openmodel", label: "OpenModel", env: "OPENMODEL_API_KEY" },
    Sakana: { id: "sakana", label: "Sakana AI", env: "FUGU_API_KEY / SAKANA_API_KEY" },
    LongCat: { id: "longcat", label: "Meituan LongCat", env: "LONGCAT_API_KEY" },
    Meta: { id: "meta", label: "Meta Model API", env: "META_MODEL_API_KEY / MODEL_API_KEY" },
    Telecomjs: { id: "telecomjs", label: "TelecomJS TokenHub", env: "TELECOMJS_API_KEY" },
    Xai: { id: "xai", label: "xAI", env: "XAI_API_KEY" },
    Mistral: { id: "mistral", label: "Mistral AI", env: "MISTRAL_API_KEY" },
    Google: { id: "google", label: "Google Gemini", env: "GOOGLE_API_KEY / GEMINI_API_KEY" },
    Edenai: { id: "edenai", label: "Eden AI", env: "EDENAI_API_KEY" },
    Concentrate: { id: "concentrate", label: "Concentrate", env: "CONCENTRATE_API_KEY" },
    ModelstudioTokenPlan: { id: "modelstudio-token-plan", label: "Model Studio Token Plan", env: "MODELSTUDIO_API_KEY" },
    ModelstudioTokenPlanAnthropic: { id: "modelstudio-token-plan-anthropic", label: "Model Studio Token Plan (Anthropic-compatible)", env: "MODELSTUDIO_API_KEY" },
    ModelstudioCodingPlan: { id: "modelstudio-coding-plan", label: "Model Studio Coding Plan", env: "MODELSTUDIO_API_KEY" },
    ModelstudioCodingPlanAnthropic: { id: "modelstudio-coding-plan-anthropic", label: "Model Studio Coding Plan (Anthropic-compatible)", env: "MODELSTUDIO_API_KEY" },
  };
  // Log loudly on unmapped variants so a new provider can never be silently
  // dropped from the drift-derived facts again. DeepseekCN (#1104), the
  // dynamic Custom meta-provider (#1519, user-defined endpoints), and
  // Antigravity (off the website 44 until the cloud-code wire is a
  // first-class advertised outbound route) are the deliberate exclusions.
  const EXCLUDED = new Set(["DeepseekCN", "Custom", "Antigravity"]);
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

function deriveSandboxBackends(source: string): string[] {
  const marker = source.match(
    /pub const PUBLIC_SANDBOX_BACKENDS\s*:\s*&\[&str\]\s*=\s*&\[([\s\S]*?)\];/,
  );
  if (!marker) return [];
  return [...marker[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

async function fetchLatestPublishedRelease(
  ghToken?: string,
): Promise<PublishedReleaseFact | null> {
  const headers: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "User-Agent": "codewhale-web-drift",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  if (ghToken) headers["Authorization"] = `Bearer ${ghToken}`;
  try {
    const r = await fetch("https://api.github.com/repos/Hmbown/CodeWhale/releases/latest", { headers });
    if (!r.ok) return null;
    const j = (await r.json()) as {
      tag_name?: string;
      published_at?: string;
      html_url?: string;
    };
    if (
      !j.tag_name ||
      !/^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(j.tag_name) ||
      !j.published_at ||
      !Number.isFinite(Date.parse(j.published_at)) ||
      !j.html_url
    ) {
      return null;
    }
    return {
      tag: j.tag_name,
      version: j.tag_name.slice(1),
      publishedAt: j.published_at,
      url: j.html_url,
    };
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

function deriveToolCountFromGeneratedFacts(source: string): number | null {
  const match = source.match(
    /export\s+const\s+FACTS(?:\s*:\s*RepoFacts)?\s*=\s*(\{[\s\S]*\})\s*;?\s*$/,
  );
  if (!match) return null;

  try {
    const parsed = JSON.parse(match[1]) as { toolCount?: unknown };
    const toolCount = parsed.toolCount;
    return typeof toolCount === "number" && Number.isSafeInteger(toolCount) && toolCount >= 0
      ? toolCount
      : null;
  } catch {
    return null;
  }
}

export async function deriveFactsFromRemote(ghToken?: string): Promise<RepoFacts | null> {
  const source = await fetchSourceMarker(ghToken);
  if (!source) return null;

  const [cargo, configRs, configModels, sandboxSource, npmPkg, licText, generatedFacts, latestPublishedRelease] = await Promise.all([
    fetchText("Cargo.toml", source.revision, ghToken),
    fetchText("crates/tui/src/config.rs", source.revision, ghToken),
    fetchText("crates/tui/src/config/models.rs", source.revision, ghToken),
    fetchText("crates/tui/src/sandbox/mod.rs", source.revision, ghToken),
    fetchText("npm/codewhale/package.json", source.revision, ghToken),
    fetchText("LICENSE", source.revision, ghToken),
    fetchText("web/lib/facts.generated.ts", source.revision, ghToken),
    fetchLatestPublishedRelease(ghToken),
  ]);

  if (!cargo || !configRs) return null;
  const toolCount = generatedFacts
    ? deriveToolCountFromGeneratedFacts(generatedFacts)
    : null;
  // Never attach current-main provenance to a build-time tool count. The
  // checked-in generated snapshot is guarded by the exact revision's CI drift
  // check, so an absent or malformed value makes the whole derivation fail.
  if (toolCount === null) return null;

  const facts: RepoFacts = {
    generatedAt: new Date().toISOString(),
    sourceRevision: source.revision,
    sourceCommittedAt: source.committedAt,
    version: deriveVersion(cargo),
    crates: deriveCrates(cargo),
    sandboxBackends: sandboxSource
      ? deriveSandboxBackends(sandboxSource)
      : BUILD_FACTS.sandboxBackends,
    providers: deriveProvidersFromConfig(configRs),
    defaultModel: deriveDefaultModel(`${configRs}\n${configModels ?? ""}`),
    nodeEngines: (() => {
      try { return npmPkg ? JSON.parse(npmPkg).engines?.node ?? null : null; } catch { return null; }
    })(),
    toolCount,
    license: licText ? deriveLicense(licText) : BUILD_FACTS.license,
    latestPublishedRelease:
      latestPublishedRelease ?? BUILD_FACTS.latestPublishedRelease,
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
  const fields: (keyof RepoFacts)[] = [
    "sourceRevision",
    "sourceCommittedAt",
    "version",
    "crates",
    "sandboxBackends",
    "providers",
    "defaultModel",
    "nodeEngines",
    "toolCount",
    "license",
    "latestPublishedRelease",
  ];
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
  let cached: RepoFacts = BUILD_FACTS;
  if (cachedRaw) {
    try {
      const parsed = JSON.parse(cachedRaw) as unknown;
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        cached = parsed as RepoFacts;
      }
    } catch {
      // A truncated or legacy cache is replaced by the newly derived snapshot.
    }
  }

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
