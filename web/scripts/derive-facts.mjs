#!/usr/bin/env node
/**
 * derive-facts.mjs — extract mechanical facts from the parent repo and write
 * them as a typed TS module. Run as `prebuild`. The same logic also runs in
 * the content-drift cron against raw.githubusercontent.com so the deployed
 * worker can detect repo→site drift between deploys.
 *
 * Sources of truth:
 *   - <repo>/Cargo.toml                         → version, workspace crates
 *   - <repo>/crates/tui/src/sandbox/*.rs        → sandbox backends
 *   - <repo>/crates/tui/src/main.rs             → provider list (--provider arms)
 *   - <repo>/crates/tui/src/config.rs           → DEFAULT_TEXT_MODEL
 *   - <repo>/npm/codewhale/package.json         → node engines
 */
import { readFileSync, readdirSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, "..", "..");

function read(rel) {
  const p = join(REPO_ROOT, rel);
  if (!existsSync(p)) return null;
  return readFileSync(p, "utf-8");
}

function deriveVersion() {
  const cargo = read("Cargo.toml");
  if (!cargo) return null;
  const m = cargo.match(/^version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}

function deriveCrates() {
  const cargo = read("Cargo.toml");
  if (!cargo) return [];
  const block = cargo.match(/members\s*=\s*\[([\s\S]*?)\]/);
  if (!block) return [];
  return [...block[1].matchAll(/"crates\/([^"]+)"/g)].map((m) => m[1]).sort();
}

function deriveSandboxBackends() {
  const dir = join(REPO_ROOT, "crates/tui/src/sandbox");
  if (!existsSync(dir)) return [];
  const files = readdirSync(dir)
    .filter((f) => f.endsWith(".rs"))
    .map((f) => f.replace(/\.rs$/, ""))
    .filter((f) => !["mod", "policy", "backend", "opensandbox", "windows"].includes(f))
    .sort();
  // canonicalize platform names
  const map = { seatbelt: "seatbelt (macOS)", landlock: "landlock (Linux)" };
  return files.map((f) => map[f] ?? f);
}

function deriveProviders() {
  // Source of truth: the ApiProvider enum in config.rs.
  const cfg = read("crates/tui/src/config.rs");
  if (!cfg) return [];
  const enumBlock = cfg.match(/pub enum ApiProvider \{([\s\S]*?)\}/);
  if (!enumBlock) return [];
  const variants = [...enumBlock[1].matchAll(/^\s*(\w+)\s*,\s*$/gm)].map((m) => m[1]);
  // Only list variants the published CLI binary actually accepts via
  // `--provider` (see ProviderArg in crates/cli/src/lib.rs). DeepseekCN
  // exists in the legacy tui/config.rs enum but is not wired through the
  // shared ProviderKind, so we exclude it until that lands. Issue #1104.
  const labelMap = {
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
  };
  return variants.map((v) => labelMap[v]).filter(Boolean);
}

function deriveDefaultModel() {
  const cfg = read("crates/tui/src/config.rs");
  if (!cfg) return null;
  const m = cfg.match(/DEFAULT_TEXT_MODEL[^"]*"([^"]+)"/);
  return m ? m[1] : null;
}

function deriveNodeEngines() {
  const pkg = read("npm/codewhale/package.json");
  if (!pkg) return null;
  try {
    return JSON.parse(pkg).engines?.node ?? null;
  } catch {
    return null;
  }
}

function deriveToolCount() {
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

function deriveLicense() {
  const lic = read("LICENSE");
  if (!lic) return null;
  const first = lic.split(/\r?\n/).find((l) => l.trim().length > 0);
  if (!first) return null;
  // "MIT License" → "MIT"; "Apache License, Version 2.0" → "Apache-2.0"
  if (/^MIT License/i.test(first)) return "MIT";
  if (/Apache.*2\.0/i.test(first)) return "Apache-2.0";
  return first.trim();
}

function build() {
  const facts = {
    generatedAt: new Date().toISOString(),
    version: deriveVersion(),
    crates: deriveCrates(),
    sandboxBackends: deriveSandboxBackends(),
    providers: deriveProviders(),
    defaultModel: deriveDefaultModel(),
    nodeEngines: deriveNodeEngines(),
    toolCount: deriveToolCount(),
    license: deriveLicense(),
    latestRelease: null, // populated at runtime by facts-drift cron
  };

  // latestRelease is intentionally null at build time — populated at runtime by the drift cron.
  const RUNTIME_ONLY = new Set(["latestRelease"]);
  const missing = Object.entries(facts).filter(([k, v]) => k !== "generatedAt" && !RUNTIME_ONLY.has(k) && (v == null || (Array.isArray(v) && v.length === 0)));
  if (missing.length > 0) {
    console.warn("[derive-facts] missing values:", missing.map(([k]) => k).join(", "));
  }

  return facts;
}

const out = build();

const ts = `// AUTO-GENERATED by web/scripts/derive-facts.mjs at prebuild.
// DO NOT EDIT — re-run \`npm run prebuild\` (or just \`npm run build\`) after changing the parent repo.
// To override at runtime, write the same shape to KV under key "facts:current".

export interface ProviderFact { id: string; label: string; env: string }

export interface RepoFacts {
  generatedAt: string;
  version: string | null;
  crates: string[];
  sandboxBackends: string[];
  providers: ProviderFact[];
  defaultModel: string | null;
  nodeEngines: string | null;
  toolCount: number | null;
  license: string | null;
  latestRelease: string | null;
}

export const FACTS: RepoFacts = ${JSON.stringify(out, null, 2)};
`;

const target = resolve(__dirname, "..", "lib", "facts.generated.ts");
writeFileSync(target, ts);
console.log(`[derive-facts] wrote ${target}`);
console.log(`[derive-facts] version=${out.version} crates=${out.crates.length} providers=${out.providers.length} sandboxes=${out.sandboxBackends.length} default-model=${out.defaultModel} node=${out.nodeEngines} tools=${out.toolCount} license=${out.license}`);
