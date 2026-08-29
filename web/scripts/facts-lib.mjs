/**
 * facts-lib.mjs — shared derivation logic for website fact generation and
 * drift checking. Imported by both derive-facts.mjs (prebuild) and
 * check-facts.mjs (CI gate).
 *
 * Sources of truth:
 *   - <repo>/Cargo.toml                         → version, workspace crates
 *   - <repo>/crates/tui/src/sandbox/mod.rs      → enforced sandbox markers
 *   - <repo>/crates/tui/src/config.rs           → provider list (ApiProvider enum), DEFAULT_TEXT_MODEL
 *   - <repo>/npm/codewhale/package.json         → node engines
 *   - <repo>/crates/tui/src/tools/*.rs          → tool count (ToolSpec impls)
 *   - <repo>/LICENSE                            → license
 *   - <repo>/web/data/latest-published-release.json → latest published release
 */
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
// __dirname is web/scripts; REPO_ROOT is the workspace root (two levels up).
export const REPO_ROOT = resolve(__dirname, "..", "..");

function read(rel) {
  const p = join(REPO_ROOT, rel);
  if (!existsSync(p)) return null;
  return readFileSync(p, "utf-8");
}

export function deriveVersion() {
  const cargo = read("Cargo.toml");
  if (!cargo) return null;
  const m = cargo.match(/^version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}

export function deriveCrates() {
  const cargo = read("Cargo.toml");
  if (!cargo) return [];
  const block = cargo.match(/members\s*=\s*\[([\s\S]*?)\]/);
  if (!block) return [];
  return [...block[1].matchAll(/"crates\/([^"]+)"/g)].map((m) => m[1]).sort();
}

export function deriveSandboxBackends() {
  const source = read("crates/tui/src/sandbox/mod.rs");
  return source ? deriveSandboxBackendsFromSource(source) : [];
}

export function deriveSandboxBackendsFromSource(source) {
  const marker = source.match(
    /pub const PUBLIC_SANDBOX_BACKENDS\s*:\s*&\[&str\]\s*=\s*&\[([\s\S]*?)\];/,
  );
  if (!marker) return [];
  return [...marker[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

/**
 * Provider label map — the single source of truth for provider → website
 * display mapping. MUST be kept in sync with the copy in
 * web/lib/facts-drift.ts (for the runtime Cloudflare cron path).
 *
 * Excluded variants: DeepseekCN (not wired through shared ProviderKind,
 * #1104), Custom (dynamic meta-provider, #1519), and Antigravity
 * (kept off the website 44 until the cloud-code wire is a first-class
 * advertised outbound route).
 */
const PROVIDER_LABEL_MAP = {
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

// DeepseekCN: not wired through shared ProviderKind (#1104).
// Custom: the dynamic OpenAI-compatible meta-provider (#1519) — a runtime
// catch-all for user-defined endpoints, not a website-listable provider.
// Antigravity: credential-import + text-only cloud-code wire. Kept off the
// website 44-count until it is a first-class advertised outbound route.
const EXCLUDED_PROVIDERS = new Set(["DeepseekCN", "Custom", "Antigravity"]);

function providerEnumVariants() {
  const cfg = read("crates/tui/src/config.rs");
  if (!cfg) return [];
  const enumBlock = cfg.match(/pub enum ApiProvider \{([\s\S]*?)\}/);
  if (!enumBlock) return [];
  return [...enumBlock[1].matchAll(/^\s*(\w+)\s*,\s*$/gm)].map((m) => m[1]);
}

/**
 * ApiProvider variants that are neither mapped to a website label nor
 * intentionally excluded. Exposed so the CI gate (`check-facts.mjs`) can
 * hard-fail on provider-inventory drift (#3772); the generator stays lenient
 * and merely warns so local `prebuild` is not blocked mid-development.
 */
export function unmappedProviderVariants() {
  return providerEnumVariants().filter(
    (v) => !EXCLUDED_PROVIDERS.has(v) && !PROVIDER_LABEL_MAP[v],
  );
}

export function deriveProviders() {
  const variants = providerEnumVariants();

  const unmapped = unmappedProviderVariants();
  if (unmapped.length > 0) {
    console.error(
      `[facts-lib] ApiProvider variants missing from PROVIDER_LABEL_MAP: ${unmapped.join(", ")}. ` +
        "Add them to PROVIDER_LABEL_MAP here AND in web/lib/facts-drift.ts (or to EXCLUDED_PROVIDERS if intentionally hidden).",
    );
    // The generator stays lenient and returns what it can map; the hard gate
    // lives in check-facts.mjs via unmappedProviderVariants() (#3772).
  }
  return variants.map((v) => PROVIDER_LABEL_MAP[v]).filter(Boolean);
}

export function deriveDefaultModel() {
  // DEFAULT_TEXT_MODEL's definition moved to config/models.rs in the #3311 split;
  // read both and match the const *definition* specifically (`= "..."`) so we
  // don't mis-bind to a later string at a mere use site.
  const cfg =
    (read("crates/tui/src/config/models.rs") ?? "") +
    "\n" +
    (read("crates/tui/src/config.rs") ?? "");
  if (!cfg.trim()) return null;
  const m = cfg.match(/DEFAULT_TEXT_MODEL\s*(?::\s*&str\s*)?=\s*"([^"]+)"/);
  return m ? m[1] : null;
}

export function deriveNodeEngines() {
  const pkg = read("npm/codewhale/package.json");
  if (!pkg) return null;
  try {
    return JSON.parse(pkg).engines?.node ?? null;
  } catch {
    return null;
  }
}

export function deriveToolCount() {
  const dir = join(REPO_ROOT, "crates/tui/src/tools");
  if (!existsSync(dir)) return null;
  let count = 0;
  for (const f of readdirSync(dir)) {
    if (!f.endsWith(".rs")) continue;
    const body = readFileSync(join(dir, f), "utf-8");
    count += (body.match(/^impl ToolSpec for /gm) ?? []).length;
  }
  return count > 0 ? count : null;
}

export function deriveLicense() {
  const lic = read("LICENSE");
  if (!lic) return null;
  const first = lic.split(/\r?\n/).find((l) => l.trim().length > 0);
  if (!first) return null;
  if (/^MIT License/i.test(first)) return "MIT";
  if (/Apache.*2\.0/i.test(first)) return "Apache-2.0";
  return first.trim();
}

export function deriveLatestPublishedRelease() {
  const raw = read("web/data/latest-published-release.json");
  if (!raw) return null;
  try {
    const release = JSON.parse(raw);
    if (
      typeof release.tag !== "string" ||
      typeof release.version !== "string" ||
      release.tag !== `v${release.version}` ||
      typeof release.publishedAt !== "string" ||
      !Number.isFinite(Date.parse(release.publishedAt)) ||
      typeof release.url !== "string" ||
      release.url !== `https://github.com/Hmbown/CodeWhale/releases/tag/${release.tag}`
    ) {
      return null;
    }
    return release;
  } catch {
    return null;
  }
}

/**
 * Re-derive all mechanical facts from the current workspace. The returned
 * object is the same shape as web/lib/facts.generated.ts → RepoFacts.
 */
export function buildFacts() {
  const providers = deriveProviders();
  // In check mode, missing provider mappings are a warning, not a crash.
  // But if we truly have zero mapped providers, that signals something
  // went wrong (e.g. config.rs renamed) — still return an empty array
  // rather than null so the checker can report it.

  const facts = {
    generatedAt: new Date().toISOString(),
    // next.config.ts injects these from the exact checkout into the built
    // artifact. They stay null in the tracked snapshot to avoid a
    // self-referential generated-file diff after every commit.
    sourceRevision: null,
    sourceCommittedAt: null,
    version: deriveVersion(),
    crates: deriveCrates(),
    sandboxBackends: deriveSandboxBackends(),
    providers,
    defaultModel: deriveDefaultModel(),
    nodeEngines: deriveNodeEngines(),
    toolCount: deriveToolCount(),
    license: deriveLicense(),
    latestPublishedRelease: deriveLatestPublishedRelease(),
  };

  return facts;
}
