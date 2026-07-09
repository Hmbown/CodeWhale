//! Single source of model facts for CodeWhale (#3071, #3073).
//!
//! Historically, "what is this model's context window / max output / does it
//! reason?" was answered by several hard-coded sites:
//!
//! * [`crate::models::context_window_for_model`] /
//!   [`crate::models::known_context_window_for_model`] for context windows,
//! * [`crate::models::max_output_tokens_for_model`] for output caps,
//! * [`crate::models::model_supports_reasoning`] for the reasoning flag,
//! * the `DEFAULT_*` model-id constants in `crates/config/src/lib.rs` for the
//!   canonical model each provider ships by default.
//!
//! This module is the **foundation** for collapsing those into one place: a
//! [`ModelMetadata`] registry keyed by model id, plus a single [`lookup`]
//! entry point. It is intentionally *additive* — the existing call sites are
//! left untouched in this pass and will be migrated to consume the registry in
//! a later change (so behaviour is unchanged today).
//!
//! ## Seeding discipline (no drift)
//!
//! The registry does not re-declare context-window / max-output / reasoning
//! numbers. Instead it **seeds** each entry by calling the existing
//! `crate::models` functions, so the registry can never silently disagree with
//! `models.rs`. The canonical model ids come from the same provider defaults
//! the config crate ships (see [`SEED_MODEL_IDS`]). The
//! [`tests::registry_context_window_matches_models_rs`] drift guard then
//! re-asserts the equivalence for a sample so that if a future change replaces
//! a seed with a hard-coded literal, CI catches the drift immediately.
//!
//! NOTE: the public surface here is intentionally not yet consumed by
//! production call sites (consumers are wired in a later pass), so
//! `dead_code` is allowed at the module level until then.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::models::{
    context_window_for_model, max_output_tokens_for_model, model_supports_reasoning,
};

/// Coarse provider grouping for a model entry.
///
/// This is deliberately a small, stable enum rather than a re-export of
/// `config::ApiProvider`: the registry's job is to answer "what kind of model
/// is this", and many models (Kimi, GLM, Qwen, …) are reachable through
/// several concrete providers. Routing decisions still live in
/// `config::ApiProvider` / `model_routing`; this is only a hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProvider {
    /// DeepSeek-family models (first-class; preserve full support).
    DeepSeek,
    /// Anthropic Claude models.
    Anthropic,
    /// OpenAI public API models (gpt-5.5 family).
    OpenAi,
    /// OpenAI Codex route models (gpt-5*-codex).
    OpenAiCodex,
    /// Moonshot / Kimi models.
    Moonshot,
    /// Z.ai GLM models.
    Zai,
    /// MiniMax models.
    Minimax,
    /// Alibaba Qwen models.
    Qwen,
    /// Arcee Trinity models.
    Arcee,
    /// Xiaomi MiMo models.
    XiaomiMimo,
    /// xAI / Grok models.
    Xai,
    /// Anything not otherwise classified (still gets real metadata via the
    /// `models.rs` heuristics where possible).
    Other,
}

/// One row of model facts, looked up in [`lookup`].
///
/// All numeric fields are seeded from `crate::models` so they stay in lockstep
/// with the legacy lookups (see module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMetadata {
    /// Canonical model id as sent to the provider (e.g. `"deepseek-v4-pro"`).
    pub id: &'static str,
    /// Coarse provider grouping.
    pub provider: ModelProvider,
    /// Approximate context window in tokens, if known.
    pub context_window: Option<u32>,
    /// Approximate maximum output tokens, if known.
    pub max_output: Option<u32>,
    /// Whether the model emits reasoning / thinking content that must be kept
    /// out of answer prose.
    pub supports_reasoning: bool,
}

impl ModelMetadata {
    /// Build a metadata row for `id` by seeding every fact from the existing
    /// `crate::models` lookups. This is the only constructor, which is what
    /// keeps the registry from drifting away from `models.rs`.
    fn seed(id: &'static str, provider: ModelProvider) -> Self {
        Self {
            id,
            provider,
            context_window: context_window_for_model(id),
            max_output: max_output_tokens_for_model(id),
            supports_reasoning: model_supports_reasoning(id),
        }
    }
}

/// Canonical `(model id, provider)` seeds for the registry.
///
/// These mirror the provider defaults shipped by `crates/config/src/lib.rs`
/// (the `DEFAULT_*_MODEL` constants) plus the explicitly-enumerated models in
/// [`crate::models::known_context_window_for_model`]. Keep this list curated:
/// it is the set of models we make first-class promises about. Unknown ids are
/// still answered by [`lookup`] via the `models.rs` heuristics, they just are
/// not pre-seeded here.
const SEED_MODEL_IDS: &[(&str, ModelProvider)] = &[
    // --- DeepSeek (first-class; config DEFAULT_DEEPSEEK_MODEL / NIM / OpenAI
    // / Atlascloud / Novita / Fireworks / Siliconflow / SGLang / vLLM /
    // Huggingface / Together / Volcengine / WanjieArk / Ollama defaults) ---
    ("deepseek-v4-pro", ModelProvider::DeepSeek),
    ("deepseek-v4-flash", ModelProvider::DeepSeek),
    ("deepseek-ai/deepseek-v4-pro", ModelProvider::DeepSeek),
    ("deepseek-ai/deepseek-v4-flash", ModelProvider::DeepSeek),
    ("deepseek/deepseek-v4-pro", ModelProvider::DeepSeek),
    ("deepseek/deepseek-v4-flash", ModelProvider::DeepSeek),
    ("deepseek-reasoner", ModelProvider::DeepSeek),
    ("deepseek-coder:1.3b", ModelProvider::DeepSeek),
    // --- Anthropic (config DEFAULT_ANTHROPIC_MODEL + models.rs rows) ---
    ("claude-opus-4-8", ModelProvider::Anthropic),
    ("claude-sonnet-4-6", ModelProvider::Anthropic),
    ("claude-haiku-4-5", ModelProvider::Anthropic),
    // --- OpenAI public API + Codex (config DEFAULT_OPENAI_CODEX_MODEL) ---
    ("gpt-5.5", ModelProvider::OpenAi),
    ("gpt-5.5-pro", ModelProvider::OpenAi),
    ("gpt-5-codex", ModelProvider::OpenAiCodex),
    ("gpt-5.3-codex", ModelProvider::OpenAiCodex),
    // --- Moonshot / Kimi (config DEFAULT_MOONSHOT_MODEL / KIMI_CODE) ---
    ("kimi-k2.7-code", ModelProvider::Moonshot),
    ("kimi-k2.6", ModelProvider::Moonshot),
    ("kimi-for-coding", ModelProvider::Moonshot),
    ("moonshotai/kimi-k2.7-code", ModelProvider::Moonshot),
    ("moonshotai/kimi-k2.6", ModelProvider::Moonshot),
    // --- Z.ai GLM (config DEFAULT_ZAI_MODEL) ---
    ("z-ai/glm-5.1", ModelProvider::Zai),
    ("z-ai/glm-5.2", ModelProvider::Zai),
    ("glm-5.1", ModelProvider::Zai),
    ("glm-5.2", ModelProvider::Zai),
    // --- MiniMax (config DEFAULT_MINIMAX_MODEL) ---
    ("minimax/minimax-m3", ModelProvider::Minimax),
    ("minimax-m3", ModelProvider::Minimax),
    ("minimax/minimax-m2.7", ModelProvider::Minimax),
    ("minimax-m2.7", ModelProvider::Minimax),
    // --- Qwen (OpenRouter routing defaults) ---
    ("qwen/qwen3.6-flash", ModelProvider::Qwen),
    ("qwen/qwen3.6-plus", ModelProvider::Qwen),
    ("qwen/qwen3.6-35b-a3b", ModelProvider::Qwen),
    // --- Arcee Trinity (config DEFAULT_ARCEE_MODEL) ---
    ("trinity-large-thinking", ModelProvider::Arcee),
    ("arcee-ai/trinity-large-thinking", ModelProvider::Arcee),
    ("trinity-mini", ModelProvider::Arcee),
    // --- Sakana / Fugu (config DEFAULT_SAKANA_MODEL) ---
    ("fugu-ultra-20260615", ModelProvider::Other),
    ("fugu-ultra", ModelProvider::Other),
    // --- StepFun (config DEFAULT_STEPFUN_MODEL) ---
    ("step-3.7-flash", ModelProvider::Other),
    // --- Xiaomi MiMo (config DEFAULT_XIAOMI_MIMO_MODEL) ---
    ("mimo-v2.5-pro", ModelProvider::XiaomiMimo),
    ("mimo-v2.5-pro-ultraspeed", ModelProvider::XiaomiMimo),
    ("mimo-v2.5", ModelProvider::XiaomiMimo),
    // --- xAI / Grok (config DEFAULT_XAI_MODEL) ---
    ("grok-4.5", ModelProvider::Xai),
    ("grok-4.3", ModelProvider::Xai),
    ("grok-build", ModelProvider::Xai),
    ("grok-composer-2.5-fast", ModelProvider::Xai),
    ("grok-4.20-0309-reasoning", ModelProvider::Xai),
    ("grok-4.20-0309-non-reasoning", ModelProvider::Xai),
];

fn registry() -> &'static BTreeMap<&'static str, ModelMetadata> {
    static REGISTRY: OnceLock<BTreeMap<&'static str, ModelMetadata>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        SEED_MODEL_IDS
            .iter()
            .map(|&(id, provider)| (id, ModelMetadata::seed(id, provider)))
            .collect()
    })
}

/// Look up model facts by id.
///
/// Returns a pre-seeded [`ModelMetadata`] when `model` is one of the canonical
/// [`SEED_MODEL_IDS`] (case-insensitive). For any other id, this falls back to
/// the same `crate::models` heuristics (explicit `_Nk` suffix, DeepSeek/Claude
/// family rules, etc.) and reports the provider as [`ModelProvider::Other`], so
/// callers always get a usable answer rather than `None` for a real model.
///
/// Returns `None` only when the id is unrecognised by every existing source
/// (no seed match and `models.rs` yields no context window).
#[must_use]
pub fn lookup(model: &str) -> Option<ModelMetadata> {
    if let Some(meta) = registry().get(model) {
        return Some(meta.clone());
    }
    // Case-insensitive seed match (model ids are compared lowercased by the
    // legacy `models.rs` helpers, so honour that here too).
    let lowered = model.to_lowercase();
    if lowered != model
        && let Some(meta) = registry().get(lowered.as_str())
    {
        return Some(meta.clone());
    }

    // Not pre-seeded: defer to the existing heuristics. If they recognise the
    // model at all (any known context window), surface a synthetic row so the
    // single lookup entry point still works for the long tail of ids.
    let context_window = context_window_for_model(model);
    let max_output = max_output_tokens_for_model(model);
    let supports_reasoning = model_supports_reasoning(model);
    if context_window.is_none() && max_output.is_none() && !supports_reasoning {
        return None;
    }
    Some(ModelMetadata {
        // The id is not 'static here; we cannot store it, so this synthetic row
        // reports an empty id. Pre-seeded rows (the common case) carry the real
        // id. This keeps the public type `'static`-clean without leaking.
        id: "",
        provider: ModelProvider::Other,
        context_window,
        max_output,
        supports_reasoning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DRIFT GUARD (#3071, #3073).
    ///
    /// The registry must agree with `crate::models` for the context window of
    /// every model it claims to know. Today they agree because the registry is
    /// *seeded* from `models.rs`; this test exists so that if a future change
    /// replaces a seed with a hard-coded literal that drifts from `models.rs`,
    /// CI fails here instead of shipping two disagreeing sources of truth.
    #[test]
    fn registry_context_window_matches_models_rs() {
        // A representative sample spanning every provider grouping and every
        // distinct window bucket the legacy table produces.
        let sample = [
            ("deepseek-v4-pro", Some(1_000_000)),
            ("deepseek-v4-flash", Some(1_000_000)),
            ("deepseek-coder:1.3b", Some(128_000)),
            ("claude-opus-4-8", Some(1_000_000)),
            ("claude-sonnet-4-6", Some(1_000_000)),
            ("claude-haiku-4-5", Some(200_000)),
            ("gpt-5.5", Some(1_050_000)),
            ("gpt-5-codex", Some(400_000)),
            ("kimi-k2.7-code", Some(262_144)),
            ("kimi-k2.6", Some(262_144)),
            ("z-ai/glm-5.1", Some(202_752)),
            ("z-ai/glm-5.2", Some(1_000_000)),
            ("minimax/minimax-m3", Some(1_000_000)),
            ("minimax-m2.7", Some(204_800)),
            ("qwen/qwen3.6-flash", Some(1_000_000)),
            ("qwen/qwen3.6-35b-a3b", Some(262_144)),
            ("trinity-large-thinking", Some(262_144)),
            ("trinity-mini", Some(128_000)),
            ("mimo-v2.5-pro", Some(1_000_000)),
            ("mimo-v2.5-pro-ultraspeed", Some(1_000_000)),
            ("mimo-v2.5", Some(1_000_000)),
            ("grok-4.5", Some(500_000)),
            ("grok-4.3", Some(1_000_000)),
            ("grok-4.20-0309-reasoning", Some(2_000_000)),
        ];
        for (model, expected) in sample {
            let meta = lookup(model)
                .unwrap_or_else(|| panic!("seeded model {model} should be in the registry"));
            // 1. Registry value equals the documented expectation.
            assert_eq!(
                meta.context_window, expected,
                "registry context window for {model} drifted from expected"
            );
            // 2. Registry value equals the LIVE models.rs value (the real guard:
            //    catches any future hard-coded literal that drifts).
            assert_eq!(
                meta.context_window,
                context_window_for_model(model),
                "registry context window for {model} drifted from models.rs"
            );
        }
    }

    #[test]
    fn registry_max_output_and_reasoning_match_models_rs() {
        for &(id, _) in SEED_MODEL_IDS {
            let meta = lookup(id).unwrap_or_else(|| panic!("{id} should be seeded"));
            assert_eq!(
                meta.max_output,
                max_output_tokens_for_model(id),
                "registry max_output for {id} drifted from models.rs"
            );
            assert_eq!(
                meta.supports_reasoning,
                model_supports_reasoning(id),
                "registry supports_reasoning for {id} drifted from models.rs"
            );
        }
    }

    #[test]
    fn deepseek_models_are_classified_as_deepseek() {
        // Branding / first-class DeepSeek support guard: the default DeepSeek
        // models must be present and classified as DeepSeek.
        for id in [
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "deepseek-ai/deepseek-v4-pro",
        ] {
            let meta = lookup(id).expect("DeepSeek default should be seeded");
            assert_eq!(meta.provider, ModelProvider::DeepSeek);
            assert_eq!(meta.context_window, Some(1_000_000));
        }
    }

    #[test]
    fn xai_models_are_classified_as_xai() {
        let meta = lookup("grok-4.5").expect("xAI default should be seeded");
        assert_eq!(meta.provider, ModelProvider::Xai);
        assert_eq!(meta.context_window, Some(500_000));
        assert!(meta.supports_reasoning);

        let fast = lookup("grok-4.20-0309-non-reasoning").expect("xAI fast model should be seeded");
        assert_eq!(fast.provider, ModelProvider::Xai);
        assert_eq!(fast.context_window, Some(2_000_000));
        assert!(!fast.supports_reasoning);
    }

    #[test]
    fn lookup_is_case_insensitive_for_seeded_ids() {
        let lower = lookup("deepseek-v4-pro").expect("seeded");
        let upper = lookup("DeepSeek-V4-Pro").expect("case-insensitive seed match");
        assert_eq!(upper.id, "deepseek-v4-pro");
        assert_eq!(upper.context_window, lower.context_window);
        assert_eq!(upper.provider, ModelProvider::DeepSeek);
    }

    #[test]
    fn lookup_falls_back_to_models_rs_for_unseeded_known_ids() {
        // `deepseek-v3.2-256k-preview` is not in SEED_MODEL_IDS but models.rs
        // recognises it via the explicit `_Nk` hint. The single lookup entry
        // point must still answer it rather than returning None.
        let meta = lookup("deepseek-v3.2-256k-preview").expect("known via models.rs heuristics");
        assert_eq!(meta.context_window, Some(256_000));
        assert_eq!(
            meta.context_window,
            context_window_for_model("deepseek-v3.2-256k-preview")
        );
        assert_eq!(meta.provider, ModelProvider::Other);
    }

    #[test]
    fn lookup_returns_none_for_completely_unknown_model() {
        assert!(lookup("totally-made-up-model-xyz").is_none());
    }
}
