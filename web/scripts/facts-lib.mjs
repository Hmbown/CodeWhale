/**
 * facts-lib.mjs — shared derivation logic for website fact generation and
 * drift checking. Imported by both derive-facts.mjs (prebuild) and
 * check-facts.mjs (CI gate).
 *
 * Sources of truth:
 *   - <repo>/Cargo.toml                         → version, workspace crates
 *   - <repo>/crates/tui/src/sandbox/*.rs        → sandbox backends
 *   - <repo>/crates/tui/src/config.rs           → provider list (ApiProvider enum), DEFAULT_TEXT_MODEL
 *   - <repo>/npm/codewhale/package.json         → node engines
 *   - <repo>/crates/tui/src/tools/*.rs          → tool count (ToolSpec impls)
 *   - <repo>/LICENSE                            → license
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
  const dir = join(REPO_ROOT, "crates/tui/src/sandbox");
  if (!existsSync(dir)) return [];
  const files = readdirSync(dir)
    .filter((f) => f.endsWith(".rs"))
    .map((f) => f.replace(/\.rs$/, ""))
    .filter((f) => !["mod", "policy", "backend", "opensandbox", "windows"].includes(f))
    .sort();
  const map = { seatbelt: "seatbelt (macOS)", landlock: "landlock (Linux)" };
  return files.map((f) => map[f] ?? f);
}

/**
 * Provider label map — the single source of truth for provider → website
 * display mapping. MUST be kept in sync with the copy in
 * web/lib/facts-drift.ts (for the runtime Cloudflare cron path).
 *
 * Excluded variants: DeepseekCN (not wired through shared ProviderKind, #1104).
 */
const PROVIDER_LABEL_MAP = {
  Deepseek: { id: "deepseek", label: "DeepSeek", env: "DEEPSEEK_API_KEY" },
  NvidiaNim: { id: "nvidia-nim", label: "NVIDIA NIM", env: "NVIDIA_API_KEY / NVIDIA_NIM_API_KEY" },
  Openai: { id: "openai", label: "OpenAI-compatible", env: "OPENAI_API_KEY" },
  Atlascloud: { id: "atlascloud", label: "AtlasCloud", env: "ATLASCLOUD_API_KEY" },
  WanjieArk: { id: "wanjie-ark", label: "Wanjie Ark", env: "WANJIE_ARK_API_KEY / WANJIE_API_KEY / WANJIE_MAAS_API_KEY" },
  Volcengine: { id: "volcengine", label: "Volcengine Ark", env: "VOLCENGINE_API_KEY / VOLCENGINE_ARK_API_KEY / ARK_API_KEY" },
  Openrouter: { id: "openrouter", label: "OpenRouter", env: "OPENROUTER_API_KEY" },
  XiaomiMimo: { id: "xiaomi-mimo", label: "Xiaomi MiMo", env: "XIAOMI_MIMO_API_KEY / XIAOMI_API_KEY / MIMO_API_KEY" },
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
};

const EXCLUDED_PROVIDERS = new Set(["DeepseekCN"]);

export function deriveProviders() {
  const cfg = read("crates/tui/src/config.rs");
  if (!cfg) return [];
  const enumBlock = cfg.match(/pub enum ApiProvider \{([\s\S]*?)\}/);
  if (!enumBlock) return [];
  const variants = [...enumBlock[1].matchAll(/^\s*(\w+)\s*,\s*$/gm)].map((m) => m[1]);

  const unmapped = variants.filter((v) => !EXCLUDED_PROVIDERS.has(v) && !PROVIDER_LABEL_MAP[v]);
  if (unmapped.length > 0) {
    console.error(
      `[facts-lib] ApiProvider variants missing from PROVIDER_LABEL_MAP: ${unmapped.join(", ")}. ` +
        "Add them to PROVIDER_LABEL_MAP here AND in web/lib/facts-drift.ts (or to EXCLUDED_PROVIDERS if intentionally hidden).",
    );
    // In check mode we don't want to crash the entire gate just because
    // a new provider variant hasn't been mapped yet — but we do want to
    // surface it loudly. Return what we can map.
  }
  return variants.map((v) => PROVIDER_LABEL_MAP[v]).filter(Boolean);
}

export function deriveDefaultModel() {
  const cfg = read("crates/tui/src/config.rs");
  if (!cfg) return null;
  const m = cfg.match(/DEFAULT_TEXT_MODEL[^"]*"([^"]+)"/);
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
    version: deriveVersion(),
    crates: deriveCrates(),
    sandboxBackends: deriveSandboxBackends(),
    providers,
    defaultModel: deriveDefaultModel(),
    nodeEngines: deriveNodeEngines(),
    toolCount: deriveToolCount(),
    license: deriveLicense(),
    latestRelease: null, // populated at runtime by facts-drift cron
  };

  return facts;
}
