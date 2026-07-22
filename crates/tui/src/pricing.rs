//! Cost estimation for API usage.
//!
//! Pricing is stored per million tokens. DeepSeek rows include their published
//! CNY rates; OpenRouter-curated rows are USD-only. Direct Xiaomi MiMo Token
//! Plan usage is credit/quota based and is intentionally left unknown until a
//! reliable balance endpoint exists.

use chrono::{DateTime, TimeZone, Utc};
use codewhale_config::pricing::{Currency, OfferingPricing, TokenUsage};

use crate::config::{
    ApiProvider, DEEPSEEK_ALIAS_REPLACEMENT, DEEPSEEK_ALIAS_RETIREMENT_UTC,
    DEFAULT_STEPFUN_BASE_URL, DEFAULT_STEPFUN_MODEL, canonical_model_id_for_provider,
};
use crate::models::{Usage, has_date_snapshot_suffix};

/// Cost display currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostCurrency {
    Usd,
    Cny,
}

impl CostCurrency {
    pub fn from_setting(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "usd" | "dollar" | "dollars" | "$" => Some(Self::Usd),
            "cny" | "rmb" | "yuan" | "¥" => Some(Self::Cny),
            _ => None,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Usd => "$",
            Self::Cny => "¥",
        }
    }
}

/// Cost estimate in displayable currencies.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CostEstimate {
    pub usd: f64,
    pub cny: f64,
}

impl CostEstimate {
    #[allow(dead_code)]
    pub fn usd_only(usd: f64) -> Self {
        Self { usd, cny: 0.0 }
    }

    pub fn is_positive(self) -> bool {
        self.usd > 0.0 || self.cny > 0.0
    }

    pub fn amount(self, currency: CostCurrency) -> f64 {
        match currency {
            CostCurrency::Usd => self.usd,
            CostCurrency::Cny => self.cny,
        }
    }
}

// === DeepSeek Account Balance ===

/// Response from `GET https://api.deepseek.com/user/balance`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct BalanceResponse {
    #[allow(dead_code)]
    pub is_available: bool,
    pub balance_infos: Vec<BalanceInfo>,
}

/// Per-currency balance entry from the balance API.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct BalanceInfo {
    pub currency: String,
    #[serde(default)]
    pub total_balance: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub topped_up_balance: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub granted_balance: String,
}

impl BalanceInfo {
    /// Parse the `total_balance` field as an f64. Returns `None` on parse
    /// failure or empty string.
    #[must_use]
    pub fn total_balance_f64(&self) -> Option<f64> {
        self.total_balance.parse::<f64>().ok()
    }
}

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy)]
struct CurrencyPricing {
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
    /// Cache-write (creation) rate. `None` means write tokens are billed at
    /// the cache-miss / input rate (providers without a separate write tier).
    cache_write_per_million: Option<f64>,
}

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    usd: CurrencyPricing,
    cny: Option<CurrencyPricing>,
}

pub(crate) const STEPFUN_PAYG_BILLING_SURFACE: &str = "stepfun-payg";
pub(crate) const STEPFUN_PLAN_BILLING_SURFACE: &str = "stepfun-plan";
const STEPFUN_PLAN_BASE_URL: &str = "https://api.stepfun.ai/step_plan/v1";
const LEGACY_STEPFUN_PLAN_BASE_URL: &str = "https://api.stepfun.com/step_plan/v1";

/// Reduce a concrete request endpoint to non-secret billing provenance.
/// Unknown/custom endpoints stay unclassified so offline reports fail closed.
pub(crate) fn billing_surface_for_route(
    provider: ApiProvider,
    base_url: Option<&str>,
) -> Option<&'static str> {
    if provider != ApiProvider::Stepfun {
        return None;
    }
    let parsed = reqwest::Url::parse(base_url?.trim()).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let default = reqwest::Url::parse(DEFAULT_STEPFUN_BASE_URL).ok()?;
    let official_host = default.host_str()?;
    let host = parsed.host_str()?;
    let path = parsed.path().trim_end_matches('/');
    if parsed.port_or_known_default() != Some(443) {
        return None;
    }
    if host.eq_ignore_ascii_case(official_host) && matches!(path, "" | "/v1") {
        Some(STEPFUN_PAYG_BILLING_SURFACE)
    } else if [STEPFUN_PLAN_BASE_URL, LEGACY_STEPFUN_PLAN_BASE_URL]
        .iter()
        .filter_map(|url| reqwest::Url::parse(url).ok())
        .any(|plan| {
            plan.host_str()
                .is_some_and(|plan_host| host.eq_ignore_ascii_case(plan_host))
                && matches!(path, "/step_plan" | "/step_plan/v1")
        })
    {
        Some(STEPFUN_PLAN_BILLING_SURFACE)
    } else {
        None
    }
}

fn pricing_for_billing_surface(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
) -> Option<ModelPricing> {
    if provider == ApiProvider::Stepfun
        && model.trim().eq_ignore_ascii_case(DEFAULT_STEPFUN_MODEL)
        && billing_surface
            .is_some_and(|surface| surface.eq_ignore_ascii_case(STEPFUN_PAYG_BILLING_SURFACE))
    {
        // StepFun standard API pricing (2026-07-13 audit). Step Plan uses a
        // separate subscription quota and must never reach this token rate.
        // https://platform.stepfun.ai/docs/en/guides/pricing/details
        Some(usd_only_pricing(0.04, 0.20, 1.15))
    } else {
        None
    }
}

fn route_requires_billing_surface(provider: ApiProvider, model: &str) -> bool {
    provider == ApiProvider::Stepfun || model.trim().eq_ignore_ascii_case(DEFAULT_STEPFUN_MODEL)
}

/// Look up pricing for a model name.
fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    pricing_for_model_at(model, Utc::now())
}

/// Return whether a model has a row in the pricing table.
#[must_use]
pub fn has_pricing_for_model(model: &str) -> bool {
    pricing_for_model(model).is_some()
}

/// Return whether the selected provider route exposes authoritative dollar
/// pricing for this model without endpoint provenance. ChatGPT/Codex OAuth is
/// subscription/account scoped, while StepFun needs PAYG-vs-Plan provenance.
#[must_use]
pub fn has_pricing_for_provider(provider: ApiProvider, model: &str) -> bool {
    calculate_turn_cost_estimate_for_provider(provider, model, &Usage::default()).is_some()
}

/// Return whether a provider/model route has authoritative pricing for an
/// already-classified billing surface.
#[must_use]
pub(crate) fn has_pricing_for_billing_surface(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
) -> bool {
    pricing_for_billing_surface(provider, model, billing_surface).is_some()
}

fn pricing_for_model_at(model: &str, now: DateTime<Utc>) -> Option<ModelPricing> {
    let lower = model.to_lowercase();
    if lower.starts_with("deepseek-ai/") {
        // NVIDIA NIM-hosted DeepSeek uses NVIDIA's catalog/account terms, not
        // DeepSeek Platform pricing. Avoid showing misleading DeepSeek costs.
        return None;
    }
    if lower == "claude-sonnet-5" {
        // Time-aware introductory pricing; resolved ahead of the catalog so
        // the intro rate is honored while it lasts (same pattern as
        // deepseek_v4_pro_pricing() / #2489).
        return Some(claude_sonnet_5_pricing(now));
    }
    if let Some(pricing) = known_pricing_for_model(&lower) {
        return Some(pricing);
    }
    if lower.contains("deepseek") {
        if lower.contains("v4-pro") || lower.contains("v4pro") {
            // DeepSeek's pricing page says the V4-Pro promotional 75% discount
            // becomes the official one-quarter base price after 2026-05-31 15:59
            // UTC. Keep using the adjusted rate after that cutoff (#2489).
            Some(deepseek_v4_pro_pricing())
        } else {
            Some(deepseek_v4_flash_pricing())
        }
    } else {
        None
    }
}

fn known_pricing_for_model(model_lower: &str) -> Option<ModelPricing> {
    let explicit = match model_lower {
        "openai/gpt-5.6" | "openai/gpt-5.6-sol" | "gpt-5.6" | "gpt-5.6-sol" => {
            Some(usd_only_pricing(0.50, 5.00, 30.00))
        }
        "openai/gpt-5.6-terra" | "gpt-5.6-terra" => Some(usd_only_pricing(0.25, 2.50, 15.00)),
        "openai/gpt-5.6-luna" | "gpt-5.6-luna" => Some(usd_only_pricing(0.10, 1.00, 6.00)),
        "meta/muse-spark-1.1" | "muse-spark-1.1" => Some(usd_only_pricing(1.25, 1.25, 4.25)),
        // Anthropic first-party rates including the published cache-read
        // discounts and 5-minute cache-write rates (2026-07-09 audit,
        // https://platform.claude.com/docs/en/about-claude/pricing). These sit
        // above the catalog lookup because the bundled catalog cannot carry
        // cache-read/write rates yet. 1h write is 2x input; we price the
        // common 5m tier (1.25x input) here (#4318).
        "claude-opus-4-8" => Some(usd_pricing_with_write(0.50, 5.00, 25.00, 6.25)),
        "claude-sonnet-4-6" => Some(usd_pricing_with_write(0.30, 3.00, 15.00, 3.75)),
        "claude-haiku-4-5" => Some(usd_pricing_with_write(0.10, 1.00, 5.00, 1.25)),
        // Claude Fable 5 (GA 2026-06-09). Its newer tokenizer produces ~30%
        // more tokens for the same text than prior Claude models, so raw
        // per-token rate comparisons against other Claude rows undercount its
        // effective cost. Cache-write is 12.50 (5m) / 20.00 (1h) upstream.
        "claude-fable-5" => Some(usd_pricing_with_write(1.00, 10.00, 50.00, 12.50)),
        // Z.ai GLM-5.2 cache-read rate per https://docs.z.ai/guides/overview/pricing
        // (cache storage limited-time free).
        "z-ai/glm-5.2" | "glm-5.2" => Some(usd_only_pricing(0.26, 1.40, 4.40)),
        // Moonshot K2.7 Code cache-read rate per
        // https://platform.kimi.ai/docs/pricing/chat-k27-code
        "moonshotai/kimi-k2.7-code" | "kimi-k2.7-code" => Some(usd_only_pricing(0.19, 0.95, 4.00)),
        // MiniMax-M3 uses the lower standard tier for metadata-only lookups;
        // cost estimation selects the correct tier from total input usage.
        "minimax-m3" => Some(minimax_m3_standard_pricing(false)),
        "minimax-m2.7" => Some(usd_pricing_with_write(0.06, 0.30, 1.20, 0.375)),
        // gpt-5-codex is deprecated upstream on the ChatGPT-OAuth path
        // (successor: gpt-5.3-codex); API usage is still billed at these rates.
        // https://developers.openai.com/api/docs/models/gpt-5.3-codex
        "openai/gpt-5-codex" | "gpt-5-codex" => Some(usd_only_pricing(0.125, 1.25, 10.00)),
        "openai/gpt-5.3-codex" | "gpt-5.3-codex" => Some(usd_only_pricing(0.175, 1.75, 14.00)),
        _ => None,
    };
    if explicit.is_some() {
        return explicit;
    }
    if let Some((input_usd_per_million, output_usd_per_million)) =
        crate::model_catalog::resolved_usd_pricing(model_lower)
    {
        return Some(usd_only_pricing(
            input_usd_per_million,
            input_usd_per_million,
            output_usd_per_million,
        ));
    }
    match model_lower {
        "moonshotai/kimi-k2.6" | "kimi-k2.6" => Some(usd_only_pricing(0.16, 0.95, 4.00)),
        "z-ai/glm-5.1" | "glm-5.1" => Some(usd_only_pricing(0.26, 1.40, 4.40)),
        // GLM-5 Turbo pricing per https://docs.z.ai/guides/overview/pricing
        "z-ai/glm-5-turbo" | "glm-5-turbo" => Some(usd_only_pricing(0.24, 1.20, 4.00)),
        // Arcee publishes no cache rate for Trinity Large Thinking, so the
        // cache-hit rate equals the input rate (no-discount representation).
        // https://docs.arcee.ai/get-started/pricing
        "arcee-ai/trinity-large-thinking" | "trinity-large-thinking" => {
            Some(usd_only_pricing(0.25, 0.25, 0.80))
        }
        "openai/gpt-5.5" | "gpt-5.5" => Some(usd_only_pricing(0.50, 5.00, 30.00)),
        // GPT-5.5 Pro does not offer a cached input discount, so the cache-hit
        // rate equals the input rate.
        // https://developers.openai.com/api/docs/models/gpt-5.5-pro
        "openai/gpt-5.5-pro" | "gpt-5.5-pro" => Some(usd_only_pricing(30.00, 30.00, 180.00)),
        "qwen/qwen3.6-flash" => Some(usd_only_pricing(0.1875, 0.1875, 1.125)),
        "qwen/qwen3.6-35b-a3b" => Some(usd_only_pricing(0.05, 0.14, 1.00)),
        "qwen/qwen3.6-max-preview" => Some(usd_only_pricing(1.04, 1.04, 6.24)),
        "qwen/qwen3.6-27b" => Some(usd_only_pricing(0.15, 0.285, 2.40)),
        "qwen/qwen3.6-plus" => Some(usd_only_pricing(0.325, 0.325, 1.95)),
        // Cache-write is 0.40 upstream (#4318).
        "qwen/qwen3.7-plus" => Some(usd_pricing_with_write(0.064, 0.32, 1.28, 0.40)),
        "qwen/qwen3.7-max" => Some(usd_only_pricing(0.25, 1.25, 3.75)),

        "google/gemma-4-31b-it" => Some(usd_only_pricing(0.09, 0.12, 0.35)),
        "google/gemma-4-26b-a4b-it" => Some(usd_only_pricing(0.06, 0.06, 0.33)),
        "tencent/hy3-preview" => Some(usd_only_pricing(0.021, 0.063, 0.21)),
        "nvidia/nemotron-3-ultra-550b-a55b" | "nvidia/nemotron-3-ultra" => {
            Some(usd_only_pricing(0.10, 0.50, 2.20))
        }
        _ => None,
    }
}

fn usd_only_pricing(
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
) -> ModelPricing {
    usd_pricing(
        input_cache_hit_per_million,
        input_cache_miss_per_million,
        output_per_million,
        None,
    )
}

fn usd_pricing_with_write(
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
    cache_write_per_million: f64,
) -> ModelPricing {
    usd_pricing(
        input_cache_hit_per_million,
        input_cache_miss_per_million,
        output_per_million,
        Some(cache_write_per_million),
    )
}

fn usd_pricing(
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
    cache_write_per_million: Option<f64>,
) -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million,
            input_cache_miss_per_million,
            output_per_million,
            cache_write_per_million,
        },
        cny: None,
    }
}

const MINIMAX_M3_LONG_CONTEXT_THRESHOLD: u32 = 512_000;
const OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD: u32 = 272_000;

/// OpenAI applies a higher price to the full request once these models exceed
/// 272K input tokens. Until the pricing layer can represent request-wide tiers,
/// refuse to report the lower static catalog price (#4317).
/// <https://developers.openai.com/api/docs/models/gpt-5.4>
/// <https://developers.openai.com/api/docs/models/gpt-5.5>
/// <https://developers.openai.com/api/docs/models/gpt-5.6-sol>
fn direct_openai_long_context_tier_is_unpriced(
    provider: ApiProvider,
    model: &str,
    input_tokens: u32,
) -> bool {
    let model_lower = model.trim().to_ascii_lowercase();
    let affected_model = matches!(
        model_lower.as_str(),
        "gpt-5.4"
            | "gpt-5.4-pro"
            | "gpt-5.5"
            | "gpt-5.6"
            | "gpt-5.6-sol"
            | "gpt-5.6-terra"
            | "gpt-5.6-luna"
    ) || has_date_snapshot_suffix(&model_lower, "gpt-5.4-")
        || has_date_snapshot_suffix(&model_lower, "gpt-5.4-pro-")
        || has_date_snapshot_suffix(&model_lower, "gpt-5.5-");
    provider == ApiProvider::Openai
        && input_tokens > OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD
        && affected_model
}

fn minimax_m3_standard_pricing(long_context: bool) -> ModelPricing {
    if long_context {
        usd_only_pricing(0.12, 0.60, 2.40)
    } else {
        usd_only_pricing(0.06, 0.30, 1.20)
    }
}

fn is_minimax_m3(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "minimax-m3" | "minimax/minimax-m3"
    )
}

fn pricing_for_model_and_usage(model: &str, usage: &Usage) -> Option<ModelPricing> {
    if is_minimax_m3(model) {
        return Some(minimax_m3_standard_pricing(
            usage.input_tokens > MINIMAX_M3_LONG_CONTEXT_THRESHOLD,
        ));
    }
    pricing_for_model(model)
}

/// Claude Sonnet 5 pricing (<https://platform.claude.com/docs/en/about-claude/pricing>):
/// introductory 2.00 / 10.00 (cache-read 0.20, cache-write 2.50) through
/// 2026-08-31 UTC, then the standard 3.00 / 15.00 (cache-read 0.30,
/// cache-write 3.75). Write rates are the published 5-minute tier (#4318).
fn claude_sonnet_5_pricing(now: DateTime<Utc>) -> ModelPricing {
    let intro_ends = Utc
        .with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("valid intro-pricing cutoff");
    if now < intro_ends {
        usd_pricing_with_write(0.20, 2.00, 10.00, 2.50)
    } else {
        usd_pricing_with_write(0.30, 3.00, 15.00, 3.75)
    }
}

fn deepseek_v4_pro_pricing() -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million: 0.003625,
            input_cache_miss_per_million: 0.435,
            output_per_million: 0.87,
            cache_write_per_million: None,
        },
        cny: Some(CurrencyPricing {
            input_cache_hit_per_million: 0.025,
            input_cache_miss_per_million: 3.0,
            output_per_million: 6.0,
            cache_write_per_million: None,
        }),
    }
}

fn deepseek_v4_flash_pricing() -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million: 0.0028,
            input_cache_miss_per_million: 0.14,
            output_per_million: 0.28,
            cache_write_per_million: None,
        },
        cny: Some(CurrencyPricing {
            input_cache_hit_per_million: 0.02,
            input_cache_miss_per_million: 1.0,
            output_per_million: 2.0,
            cache_write_per_million: None,
        }),
    }
}

/// Calculate cost from provider usage, honoring DeepSeek context-cache fields.
#[must_use]
#[cfg(test)]
pub fn calculate_turn_cost_from_usage(model: &str, usage: &Usage) -> Option<f64> {
    calculate_turn_cost_estimate_from_usage(model, usage).map(|estimate| estimate.usd)
}

/// Calculate cost from provider usage in both official currencies.
#[must_use]
#[cfg(test)]
pub fn calculate_turn_cost_estimate_from_usage(model: &str, usage: &Usage) -> Option<CostEstimate> {
    let pricing = pricing_for_model_and_usage(model, usage)?;
    Some(cost_estimate_with_pricing(pricing, usage))
}

fn cost_estimate_with_pricing(pricing: ModelPricing, usage: &Usage) -> CostEstimate {
    CostEstimate {
        usd: calculate_turn_cost_from_usage_with_pricing(pricing.usd, usage),
        cny: pricing
            .cny
            .map(|pricing| calculate_turn_cost_from_usage_with_pricing(pricing, usage))
            .unwrap_or(0.0),
    }
}

/// Calculate cost from provider/model usage when that pair identifies a single
/// billing surface. ChatGPT/Codex OAuth has no authoritative API dollar price,
/// while StepFun needs endpoint-derived PAYG-vs-Plan provenance; both stay
/// unpriced here rather than fabricating spend.
#[must_use]
pub fn calculate_turn_cost_estimate_for_provider(
    provider: ApiProvider,
    model: &str,
    usage: &Usage,
) -> Option<CostEstimate> {
    calculate_turn_cost_estimate_for_provider_at(provider, model, usage, Utc::now())
}

/// Calculate cost only for routes that are actually money-metered. OAuth and
/// token-plan routes deliberately return `None` even when the underlying model
/// also exists behind a separately-priced public API.
#[must_use]
pub fn calculate_turn_cost_estimate_for_route(
    provider: ApiProvider,
    model: &str,
    usage: &Usage,
    billing: crate::route_billing::BillingPresentation,
) -> Option<CostEstimate> {
    if !billing.shows_money() {
        return None;
    }
    calculate_turn_cost_estimate_for_provider(provider, model, usage)
}

/// Estimate a turn when endpoint-derived billing provenance is available.
/// StepFun's standard API and Step Plan share provider/model text but not a
/// billing system, so that route fails closed unless the PAYG surface is known.
#[must_use]
#[cfg(test)]
pub(crate) fn calculate_turn_cost_estimate_for_billing_surface(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
    usage: &Usage,
) -> Option<CostEstimate> {
    calculate_turn_cost_estimate_for_route_at(provider, model, billing_surface, usage, Utc::now())
}

/// Deterministic provider-aware estimate at the turn's recorded time.
#[must_use]
pub(crate) fn calculate_turn_cost_estimate_for_provider_at(
    provider: ApiProvider,
    model: &str,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
) -> Option<CostEstimate> {
    if provider == ApiProvider::OpenaiCodex || route_requires_billing_surface(provider, model) {
        return None;
    }
    let normalized_model = model.trim();
    let model_lower = normalized_model.to_ascii_lowercase();
    let direct_deepseek = matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    );
    let canonical_model = canonical_model_id_for_provider(provider, normalized_model)?;
    let catalog_model = if direct_deepseek
        && matches!(model_lower.as_str(), "deepseek-chat" | "deepseek-reasoner")
    {
        let retirement = DateTime::parse_from_rfc3339(DEEPSEEK_ALIAS_RETIREMENT_UTC)
            .ok()?
            .with_timezone(&Utc);
        if recorded_at >= retirement {
            return None;
        }
        DEEPSEEK_ALIAS_REPLACEMENT.to_string()
    } else {
        canonical_model
    };

    if direct_openai_long_context_tier_is_unpriced(provider, &catalog_model, usage.input_tokens) {
        return None;
    }

    // MiniMax-M3 doubles its published rates above 512K total input. The
    // catalog row is necessarily static, so retain the usage-aware first-party
    // table for both direct wire protocols after provider/model provenance has
    // been canonicalized.
    if matches!(
        provider,
        ApiProvider::Minimax
            | ApiProvider::MinimaxAnthropic
            | ApiProvider::MinimaxCn
            | ApiProvider::MinimaxAnthropicCn
    ) && catalog_model.eq_ignore_ascii_case("minimax-m3")
    {
        let pricing = pricing_for_model_and_usage(&catalog_model, usage)?;
        return Some(cost_estimate_with_pricing(pricing, usage));
    }

    // Direct DeepSeek pricing carries an authoritative CNY row, and Sonnet 5
    // has a recorded-time introductory window that a static catalog row cannot
    // represent. These exact first-party routes intentionally override the
    // catalog; no other provider/model text match is allowed to do so.
    if direct_deepseek
        || (provider == ApiProvider::Anthropic
            && catalog_model.eq_ignore_ascii_case("claude-sonnet-5"))
    {
        let pricing = provider_owned_hand_pricing_at(provider, &catalog_model, recorded_at)?;
        return Some(cost_estimate_with_pricing(pricing, usage));
    }

    if let Some(offering) =
        crate::provider_lake::catalog_offering_for_model(provider, &catalog_model)
        && OfferingPricing::from_catalog_offering(&offering).is_some()
    {
        if let Some(estimate) =
            catalog_cost_estimate_for_route(provider, &catalog_model, &offering, usage)
        {
            return Some(estimate);
        }
        if catalog_gap_uses_documented_hand_price(provider, &catalog_model, &offering, usage) {
            let pricing = provider_owned_hand_pricing_at(provider, &catalog_model, recorded_at)?;
            return Some(cost_estimate_with_pricing(pricing, usage));
        }
        return None;
    }

    // A few first-party rows predate or intentionally omit a Models.dev entry
    // (for example OpenAI API `gpt-5-codex` and MiniMax `minimax-m2.7`).
    // Preserve only an explicit provider-owned allowlist here;
    // a costless foreign/catalog route must remain unpriced.
    let pricing = provider_owned_hand_pricing_at(provider, &catalog_model, recorded_at)?;
    Some(cost_estimate_with_pricing(pricing, usage))
}

/// Recorded-time variant with explicit billing-surface provenance.
#[must_use]
pub(crate) fn calculate_turn_cost_estimate_for_route_at(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
) -> Option<CostEstimate> {
    if provider == ApiProvider::Stepfun {
        let pricing = pricing_for_billing_surface(provider, model, billing_surface)?;
        return Some(cost_estimate_with_pricing(pricing, usage));
    }
    if model.trim().eq_ignore_ascii_case(DEFAULT_STEPFUN_MODEL) {
        return None;
    }
    calculate_turn_cost_estimate_for_provider_at(provider, model, usage, recorded_at)
}

fn provider_owned_hand_pricing_at(
    provider: ApiProvider,
    model: &str,
    recorded_at: DateTime<Utc>,
) -> Option<ModelPricing> {
    let model_lower = model.trim().to_ascii_lowercase();
    let provider_owns_row = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic => {
            matches!(
                model_lower.as_str(),
                "deepseek-v4-pro" | "deepseek-v4-flash"
            )
        }
        ApiProvider::Openai => matches!(
            model_lower.as_str(),
            "gpt-5-codex"
                | "gpt-5.3-codex"
                | "gpt-5.5"
                | "gpt-5.5-pro"
                | "gpt-5.6"
                | "gpt-5.6-sol"
                | "gpt-5.6-terra"
                | "gpt-5.6-luna"
        ),
        ApiProvider::Anthropic => matches!(
            model_lower.as_str(),
            "claude-opus-4-8"
                | "claude-sonnet-4-6"
                | "claude-haiku-4-5"
                | "claude-fable-5"
                | "claude-sonnet-5"
        ),
        ApiProvider::Zai => matches!(model_lower.as_str(), "glm-5.1" | "glm-5.2" | "glm-5-turbo"),
        ApiProvider::Moonshot => {
            matches!(model_lower.as_str(), "kimi-k2.6" | "kimi-k2.7-code")
        }
        ApiProvider::Minimax
        | ApiProvider::MinimaxAnthropic
        | ApiProvider::MinimaxCn
        | ApiProvider::MinimaxAnthropicCn => {
            matches!(model_lower.as_str(), "minimax-m3" | "minimax-m2.7")
        }
        ApiProvider::Arcee => model_lower == "trinity-large-thinking",
        ApiProvider::Meta => model_lower == "muse-spark-1.1",
        _ => false,
    };
    provider_owns_row
        .then(|| pricing_for_model_at(&model_lower, recorded_at))
        .flatten()
}

/// Whether a failed catalog estimate is missing only a class whose billing is
/// explicitly documented by the provider-owned row. Keep this narrow: a hand
/// row must not fill unrelated catalog gaps (for example an unpublished cache
/// read rate) merely because the model name is known locally.
fn catalog_gap_uses_documented_hand_price(
    provider: ApiProvider,
    model: &str,
    offering: &codewhale_config::catalog::CatalogOffering,
    usage: &Usage,
) -> bool {
    if provider != ApiProvider::Openai || !model.eq_ignore_ascii_case("gpt-5.5") {
        return false;
    }
    let Some(pricing) = OfferingPricing::from_catalog_offering(offering) else {
        return false;
    };
    let usage = token_usage_for_pricing(usage);
    usage.cache_write > 0
        && pricing.cache_write_per_million.is_none()
        && (usage.input == 0 || pricing.input_per_million.is_some())
        && (usage.output == 0 || pricing.output_per_million.is_some())
        && (usage.cache_read == 0 || pricing.cache_read_per_million.is_some())
}

/// Estimate usage only from the exact provider offering. Missing prices for a
/// used token class fail closed, except on the two documented first-party
/// routes where cache tokens are explicitly billed at the input rate.
fn catalog_cost_estimate_for_route(
    provider: ApiProvider,
    model: &str,
    offering: &codewhale_config::catalog::CatalogOffering,
    usage: &Usage,
) -> Option<CostEstimate> {
    let usage = token_usage_for_pricing(usage);
    let mut pricing = OfferingPricing::from_catalog_offering(offering)?;
    let model_lower = model.trim().to_ascii_lowercase();
    let cache_uses_input_rate = matches!(
        (provider, model_lower.as_str()),
        (ApiProvider::Openai, "gpt-5.5-pro") | (ApiProvider::Arcee, "trinity-large-thinking")
    );
    if cache_uses_input_rate {
        if usage.cache_read > 0 && pricing.cache_read_per_million.is_none() {
            pricing.cache_read_per_million = pricing.input_per_million;
        }
        if usage.cache_write > 0 && pricing.cache_write_per_million.is_none() {
            pricing.cache_write_per_million = pricing.input_per_million;
        }
    }

    let amount = pricing.estimate_cost(&usage)?;
    match pricing.currency {
        Currency::Usd => Some(CostEstimate::usd_only(amount)),
        Currency::Cny => Some(CostEstimate {
            usd: 0.0,
            cny: amount,
        }),
        Currency::Other(_) => None,
    }
}

/// Project provider-normalized turn usage into canonical billable token
/// classes for the shared config pricing layer (#2961 / #4318).
///
/// `Usage::prompt_cache_miss_tokens` is billed as ordinary non-cached input.
/// `Usage::prompt_cache_write_tokens` maps to `TokenUsage::cache_write` so
/// providers that publish a write premium (Anthropic 1.25x–2x) are not
/// undercounted.
#[must_use]
pub fn token_usage_for_pricing(usage: &Usage) -> TokenUsage {
    let cache_read = usage.prompt_cache_hit_tokens.unwrap_or(0);
    let cache_write = usage.prompt_cache_write_tokens.unwrap_or(0);
    let non_cached_reported = usage.prompt_cache_miss_tokens.unwrap_or_else(|| {
        usage
            .input_tokens
            .saturating_sub(cache_read)
            .saturating_sub(cache_write)
    });
    let accounted_input = cache_read
        .saturating_add(non_cached_reported)
        .saturating_add(cache_write);
    let uncategorized_input = usage.input_tokens.saturating_sub(accounted_input);
    let input = non_cached_reported.saturating_add(uncategorized_input);
    let output = usage
        .output_tokens
        .saturating_add(usage.reasoning_tokens.unwrap_or(0));

    TokenUsage {
        input: u64::from(input),
        output: u64::from(output),
        cache_read: u64::from(cache_read),
        cache_write: u64::from(cache_write),
    }
}

fn calculate_turn_cost_from_usage_with_pricing(pricing: CurrencyPricing, usage: &Usage) -> f64 {
    let usage = token_usage_for_pricing(usage);
    let hit_cost = (usage.cache_read as f64 / 1_000_000.0) * pricing.input_cache_hit_per_million;
    let miss_cost = (usage.input as f64 / 1_000_000.0) * pricing.input_cache_miss_per_million;
    let write_rate = pricing
        .cache_write_per_million
        .unwrap_or(pricing.input_cache_miss_per_million);
    let write_cost = (usage.cache_write as f64 / 1_000_000.0) * write_rate;
    let output_cost = (usage.output as f64 / 1_000_000.0) * pricing.output_per_million;
    hit_cost + miss_cost + write_cost + output_cost
}

/// Estimate how much money was saved by serving `cache_hit_tokens` from the
/// prefix cache instead of billing them at the cache-miss rate.  Returns `None`
/// when the model's pricing is unknown or the number of cache-hit tokens is
/// zero (nothing to save).
#[must_use]
#[cfg(test)]
pub fn calculate_cache_savings(model: &str, cache_hit_tokens: u32) -> Option<CostEstimate> {
    if cache_hit_tokens == 0 {
        return None;
    }
    // M3's cache-read savings depend on whether total input crosses 512k;
    // this helper receives only cache-hit tokens, so an estimate would guess
    // the tier. The full turn-cost path has total input and remains precise.
    if is_minimax_m3(model) {
        return None;
    }
    let pricing = pricing_for_model(model)?;
    let tokens = cache_hit_tokens as f64 / 1_000_000.0;
    Some(CostEstimate {
        usd: tokens
            * (pricing.usd.input_cache_miss_per_million - pricing.usd.input_cache_hit_per_million),
        cny: pricing
            .cny
            .map(|pricing| {
                tokens
                    * (pricing.input_cache_miss_per_million - pricing.input_cache_hit_per_million)
            })
            .unwrap_or(0.0),
    })
}

/// Estimate cache savings from the exact provider route by comparing the same
/// tokens as cache hits and ordinary input. Unknown or costless routes remain
/// unavailable instead of inheriting a model-only rate.
#[must_use]
pub fn calculate_cache_savings_for_provider(
    provider: ApiProvider,
    model: &str,
    cache_hit_tokens: u32,
) -> Option<CostEstimate> {
    if cache_hit_tokens == 0 {
        return None;
    }
    let cached = Usage {
        input_tokens: cache_hit_tokens,
        prompt_cache_hit_tokens: Some(cache_hit_tokens),
        prompt_cache_miss_tokens: Some(0),
        ..Usage::default()
    };
    let uncached = Usage {
        input_tokens: cache_hit_tokens,
        prompt_cache_hit_tokens: Some(0),
        prompt_cache_miss_tokens: Some(cache_hit_tokens),
        ..Usage::default()
    };
    let cached = calculate_turn_cost_estimate_for_provider(provider, model, &cached)?;
    let uncached = calculate_turn_cost_estimate_for_provider(provider, model, &uncached)?;
    Some(CostEstimate {
        usd: uncached.usd - cached.usd,
        cny: uncached.cny - cached.cny,
    })
}

/// Format a cost amount for compact display in the chosen currency.
#[must_use]
pub fn format_cost_amount(cost: f64, currency: CostCurrency) -> String {
    let symbol = currency.symbol();
    if cost < 0.0001 {
        format!("<{symbol}0.0001")
    } else if cost < 0.01 {
        format!("{symbol}{cost:.4}")
    } else {
        format!("{symbol}{cost:.2}")
    }
}

/// Format a cost amount for detailed reports in the chosen currency.
#[must_use]
pub fn format_cost_amount_precise(cost: f64, currency: CostCurrency) -> String {
    let symbol = currency.symbol();
    if cost < 0.0001 {
        format!("<{symbol}0.0001")
    } else {
        format!("{symbol}{cost:.4}")
    }
}

/// Format a dual-currency estimate using the selected display currency.
#[must_use]
pub fn format_cost_estimate(estimate: CostEstimate, currency: CostCurrency) -> String {
    format_cost_amount(estimate.amount(currency), currency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn nvidia_nim_deepseek_model_does_not_use_deepseek_platform_pricing() {
        assert!(!has_pricing_for_model("deepseek-ai/deepseek-v4-pro"));
    }

    #[test]
    fn stepfun_billing_surface_keeps_payg_separate_from_step_plan() {
        for base_url in [
            "https://api.stepfun.ai",
            "https://api.stepfun.ai/",
            "https://api.stepfun.ai/v1",
            "https://API.STEPFUN.AI/v1/",
        ] {
            assert_eq!(
                billing_surface_for_route(ApiProvider::Stepfun, Some(base_url)),
                Some(STEPFUN_PAYG_BILLING_SURFACE),
                "{base_url}"
            );
        }
        for base_url in [
            "https://api.stepfun.ai/step_plan",
            "https://api.stepfun.ai/step_plan/v1/",
            "https://api.stepfun.com/step_plan/v1",
        ] {
            assert_eq!(
                billing_surface_for_route(ApiProvider::Stepfun, Some(base_url)),
                Some(STEPFUN_PLAN_BILLING_SURFACE),
                "{base_url}"
            );
        }
        for base_url in [
            "http://api.stepfun.ai/v1",
            "https://token@api.stepfun.ai/v1",
            "https://api.stepfun.ai/v1?account=other",
            "https://api.stepfun.ai/STEP_PLAN/v1",
            "https://stepfun.example/v1",
        ] {
            assert_eq!(
                billing_surface_for_route(ApiProvider::Stepfun, Some(base_url)),
                None,
                "{base_url}"
            );
        }
        assert_eq!(
            billing_surface_for_route(ApiProvider::Openrouter, Some(DEFAULT_STEPFUN_BASE_URL)),
            None
        );

        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            prompt_cache_hit_tokens: Some(250_000),
            ..Default::default()
        };
        let payg = calculate_turn_cost_estimate_for_billing_surface(
            ApiProvider::Stepfun,
            DEFAULT_STEPFUN_MODEL,
            Some(STEPFUN_PAYG_BILLING_SURFACE),
            &usage,
        )
        .expect("standard StepFun API has an authoritative token price");
        assert!((payg.usd - 0.735).abs() < 1e-12);
        assert_eq!(payg.cny, 0.0);

        // Provider/model-only callers (background compaction, sub-agents, and
        // legacy records) cannot distinguish PAYG from Step Plan and must not
        // add either route to spend or savings totals.
        assert!(
            calculate_turn_cost_estimate_for_provider(
                ApiProvider::Stepfun,
                DEFAULT_STEPFUN_MODEL,
                &usage,
            )
            .is_none()
        );
        assert!(
            calculate_turn_cost_estimate_for_provider_at(
                ApiProvider::Stepfun,
                DEFAULT_STEPFUN_MODEL,
                &usage,
                Utc::now(),
            )
            .is_none()
        );
        assert!(
            calculate_cache_savings_for_provider(
                ApiProvider::Stepfun,
                DEFAULT_STEPFUN_MODEL,
                250_000,
            )
            .is_none()
        );
        assert!(!has_pricing_for_provider(
            ApiProvider::Stepfun,
            DEFAULT_STEPFUN_MODEL
        ));

        for surface in [None, Some(STEPFUN_PLAN_BILLING_SURFACE)] {
            assert!(
                calculate_turn_cost_estimate_for_billing_surface(
                    ApiProvider::Stepfun,
                    DEFAULT_STEPFUN_MODEL,
                    surface,
                    &usage,
                )
                .is_none()
            );
        }
        assert!(
            calculate_turn_cost_estimate_for_billing_surface(
                ApiProvider::Stepfun,
                "step-3.5-flash",
                Some(STEPFUN_PAYG_BILLING_SURFACE),
                &usage,
            )
            .is_none()
        );
        for provider in [
            ApiProvider::Openrouter,
            ApiProvider::Ollama,
            ApiProvider::Custom,
        ] {
            assert!(
                calculate_turn_cost_estimate_for_billing_surface(
                    provider,
                    DEFAULT_STEPFUN_MODEL,
                    Some(STEPFUN_PAYG_BILLING_SURFACE),
                    &usage,
                )
                .is_none(),
                "{provider:?}"
            );
            assert!(
                calculate_turn_cost_estimate_for_provider(provider, DEFAULT_STEPFUN_MODEL, &usage,)
                    .is_none(),
                "{provider:?}"
            );
            assert!(
                calculate_turn_cost_estimate_for_provider_at(
                    provider,
                    DEFAULT_STEPFUN_MODEL,
                    &usage,
                    Utc::now(),
                )
                .is_none(),
                "{provider:?}"
            );
            assert!(
                calculate_cache_savings_for_provider(provider, DEFAULT_STEPFUN_MODEL, 250_000,)
                    .is_none(),
                "{provider:?}"
            );
            assert!(
                !has_pricing_for_provider(provider, DEFAULT_STEPFUN_MODEL),
                "{provider:?}"
            );
        }

        let recorded = calculate_turn_cost_estimate_for_route_at(
            ApiProvider::Stepfun,
            DEFAULT_STEPFUN_MODEL,
            Some(STEPFUN_PAYG_BILLING_SURFACE),
            &usage,
            Utc::now(),
        )
        .expect("recorded PAYG route retains provider-scoped pricing");
        assert_eq!(recorded, payg);
    }

    #[test]
    fn catalog_sourced_models_have_usd_pricing() {
        for (model, input, output) in [
            ("minimax-m2.7", 0.3, 1.2),
            ("minimax/minimax-m2.7", 0.3, 1.2),
            ("step-3.7-flash", 0.2, 1.15),
            ("fugu-ultra-20260615", 5.0, 30.0),
            ("fugu-ultra", 5.0, 30.0),
        ] {
            let pricing = pricing_for_model_at(model, Utc::now()).expect(model);
            assert_eq!(pricing.usd.input_cache_miss_per_million, input, "{model}");
            assert_eq!(pricing.usd.output_per_million, output, "{model}");
            assert!(has_pricing_for_model(model));
        }
    }

    #[test]
    fn trinity_mini_stays_unpriced_without_verified_provider_rates() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            ..Usage::default()
        };

        assert!(pricing_for_model_at("trinity-mini", Utc::now()).is_none());
        assert!(!has_pricing_for_model("trinity-mini"));
        assert!(!has_pricing_for_provider(
            ApiProvider::Arcee,
            "trinity-mini"
        ));
        assert!(
            calculate_turn_cost_estimate_for_provider(ApiProvider::Arcee, "trinity-mini", &usage,)
                .is_none()
        );
    }

    #[test]
    fn minimax_m3_standard_pricing_tracks_the_512k_input_boundary() {
        for model in ["MiniMax-M3", "minimax/minimax-m3"] {
            for (input_tokens, cache_read, input, output) in
                [(512_000, 0.06, 0.30, 1.20), (512_001, 0.12, 0.60, 2.40)]
            {
                let usage = Usage {
                    input_tokens,
                    ..Usage::default()
                };
                let pricing = pricing_for_model_and_usage(model, &usage).expect("M3 pricing");
                assert_eq!(pricing.usd.input_cache_hit_per_million, cache_read);
                assert_eq!(pricing.usd.input_cache_miss_per_million, input);
                assert_eq!(pricing.usd.output_per_million, output);
            }
            assert!(calculate_cache_savings(model, 1).is_none());
        }
    }

    #[test]
    fn provider_scoped_minimax_m3_keeps_usage_tiers_for_both_wire_protocols() {
        for provider in [ApiProvider::Minimax, ApiProvider::MinimaxAnthropic] {
            for (input_tokens, input_rate) in [(512_000, 0.30), (512_001, 0.60)] {
                let usage = Usage {
                    input_tokens,
                    ..Usage::default()
                };
                let estimate = calculate_turn_cost_estimate_for_provider_at(
                    provider,
                    "MiniMax-M3",
                    &usage,
                    Utc::now(),
                )
                .expect("direct MiniMax route has authoritative pricing");
                let expected = f64::from(input_tokens) / 1_000_000.0 * input_rate;
                assert!((estimate.usd - expected).abs() < 1e-12, "{provider:?}");
            }
        }
    }

    #[test]
    fn direct_openai_long_context_estimates_fail_closed_above_272k() {
        for model in [
            "gpt-5.5",
            "gpt-5.6",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
        ] {
            let at_boundary = Usage {
                input_tokens: OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD,
                ..Usage::default()
            };
            let above_boundary = Usage {
                input_tokens: OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD + 1,
                ..Usage::default()
            };

            assert!(
                calculate_turn_cost_estimate_for_provider(
                    ApiProvider::Openai,
                    model,
                    &at_boundary,
                )
                .is_some(),
                "{model} should retain its standard price at 272K"
            );
            assert!(
                calculate_turn_cost_estimate_for_provider(
                    ApiProvider::Openai,
                    model,
                    &above_boundary,
                )
                .is_none(),
                "{model} must not report the lower static price above 272K"
            );
        }
    }

    #[test]
    fn direct_openai_gpt54_family_is_guarded_even_without_a_bundled_catalog_row() {
        for model in ["gpt-5.4", "gpt-5.4-pro"] {
            assert!(!direct_openai_long_context_tier_is_unpriced(
                ApiProvider::Openai,
                model,
                OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD,
            ));
            assert!(direct_openai_long_context_tier_is_unpriced(
                ApiProvider::Openai,
                model,
                OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD + 1,
            ));

            let above_boundary = Usage {
                input_tokens: OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD + 1,
                ..Usage::default()
            };
            assert!(
                calculate_turn_cost_estimate_for_provider(
                    ApiProvider::Openai,
                    model,
                    &above_boundary,
                )
                .is_none(),
                "{model} must remain unpriced if a live catalog row is available"
            );
        }
    }

    #[test]
    fn openai_long_context_guard_is_exact_and_provider_scoped() {
        let input_tokens = OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD + 1;

        for provider in [
            ApiProvider::Openrouter,
            ApiProvider::OpenaiCodex,
            ApiProvider::Ollama,
            ApiProvider::Custom,
        ] {
            assert!(
                !direct_openai_long_context_tier_is_unpriced(provider, "gpt-5.5", input_tokens,),
                "{provider:?} must not inherit direct OpenAI tier handling"
            );
        }
        for model in [
            "gpt-5.4-mini",
            "gpt-5.4-nano",
            "gpt-5.5-pro",
            "gpt-5.5-pro-2026-04-23",
            "gpt-5.5-2026-04-23-extra",
            "openai/gpt-5.5",
            "gpt-5.6-sol-preview",
        ] {
            assert!(
                !direct_openai_long_context_tier_is_unpriced(
                    ApiProvider::Openai,
                    model,
                    input_tokens,
                ),
                "non-documented id {model} must not be treated as an alias"
            );
        }

        let usage = Usage {
            input_tokens,
            output_tokens: 1,
            ..Usage::default()
        };
        assert!(calculate_turn_cost_estimate_from_usage("gpt-5.5", &usage).is_some());
        assert!(
            calculate_turn_cost_estimate_for_provider(ApiProvider::OpenaiCodex, "gpt-5.5", &usage,)
                .is_none()
        );
    }

    #[test]
    fn direct_openai_snapshots_use_the_same_strict_272k_boundary() {
        for snapshot in [
            "gpt-5.4-2026-03-05",
            "gpt-5.4-pro-2026-03-05",
            "gpt-5.5-2026-04-23",
        ] {
            assert!(!direct_openai_long_context_tier_is_unpriced(
                ApiProvider::Openai,
                snapshot,
                OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD,
            ));
            assert!(direct_openai_long_context_tier_is_unpriced(
                ApiProvider::Openai,
                snapshot,
                OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD + 1,
            ));

            let above_boundary = Usage {
                input_tokens: OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD + 1,
                ..Usage::default()
            };
            assert!(
                calculate_turn_cost_estimate_for_provider(
                    ApiProvider::Openai,
                    snapshot,
                    &above_boundary,
                )
                .is_none(),
                "{snapshot} must not report the lower static price above 272K"
            );
        }
    }

    #[test]
    fn direct_openai_long_context_guard_uses_total_input_with_mixed_cache_classes() {
        let at_boundary = Usage {
            input_tokens: OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD,
            output_tokens: 1_000,
            prompt_cache_hit_tokens: Some(100_000),
            prompt_cache_miss_tokens: Some(100_000),
            prompt_cache_write_tokens: Some(72_000),
            ..Usage::default()
        };
        let above_boundary = Usage {
            input_tokens: OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD + 1,
            prompt_cache_write_tokens: Some(72_001),
            ..at_boundary.clone()
        };

        assert!(
            calculate_turn_cost_estimate_for_provider(
                ApiProvider::Openai,
                "gpt-5.6-sol",
                &at_boundary,
            )
            .is_some()
        );
        assert!(
            calculate_turn_cost_estimate_for_provider(
                ApiProvider::Openai,
                "gpt-5.6-sol",
                &above_boundary,
            )
            .is_none()
        );
    }

    #[test]
    fn minimax_m2_7_preserves_cache_read_and_write_rates() {
        let pricing = pricing_for_model_at("MiniMax-M2.7", Utc::now()).expect("M2.7 pricing");
        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.06);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 0.30);
        assert_eq!(pricing.usd.output_per_million, 1.20);
        assert_eq!(pricing.usd.cache_write_per_million, Some(0.375));
    }

    #[test]
    fn curated_usd_only_models_have_pricing_and_accrue_cost() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            prompt_cache_hit_tokens: Some(250_000),
            prompt_cache_miss_tokens: Some(750_000),
            ..Default::default()
        };
        for (model, hit, miss, output) in [
            ("kimi-k2.6", 0.16, 0.95, 4.00),
            ("kimi-k2.7-code", 0.19, 0.95, 4.00),
            ("moonshotai/kimi-k2.7-code", 0.19, 0.95, 4.00),
            ("z-ai/glm-5.1", 0.26, 1.40, 4.40),
            ("glm-5.2", 0.26, 1.40, 4.40),
            ("z-ai/glm-5.2", 0.26, 1.40, 4.40),
            ("glm-5-turbo", 0.24, 1.20, 4.00),
            ("z-ai/glm-5-turbo", 0.24, 1.20, 4.00),
            ("qwen/qwen3.6-plus", 0.325, 0.325, 1.95),
            ("qwen/qwen3.6-35b-a3b", 0.05, 0.14, 1.00),
            ("qwen/qwen3.6-27b", 0.15, 0.285, 2.40),
            // No published cache rate: cache-hit billed at the input rate.
            ("trinity-large-thinking", 0.25, 0.25, 0.80),
            ("nvidia/nemotron-3-ultra-550b-a55b", 0.10, 0.50, 2.20),
            ("claude-opus-4-8", 0.50, 5.00, 25.00),
            ("claude-sonnet-4-6", 0.30, 3.00, 15.00),
            ("claude-haiku-4-5", 0.10, 1.00, 5.00),
            ("claude-fable-5", 1.00, 10.00, 50.00),
            ("gpt-5.5", 0.50, 5.00, 30.00),
            // GPT-5.5 Pro has no cached-input discount: cache-hit == input.
            ("gpt-5.5-pro", 30.00, 30.00, 180.00),
            ("gpt-5.6-sol", 0.50, 5.00, 30.00),
            ("gpt-5.6-terra", 0.25, 2.50, 15.00),
            ("gpt-5.6-luna", 0.10, 1.00, 6.00),
            ("gpt-5-codex", 0.125, 1.25, 10.00),
            ("gpt-5.3-codex", 0.175, 1.75, 14.00),
            ("qwen/qwen3.7-plus", 0.064, 0.32, 1.28),
            ("muse-spark-1.1", 1.25, 1.25, 4.25),
        ] {
            let pricing = pricing_for_model_at(model, Utc::now()).expect(model);
            assert_eq!(pricing.usd.input_cache_hit_per_million, hit);
            assert_eq!(pricing.usd.input_cache_miss_per_million, miss);
            assert_eq!(pricing.usd.output_per_million, output);
            assert!(pricing.cny.is_none());
            assert!(has_pricing_for_model(model));

            let estimate = calculate_turn_cost_estimate_from_usage(model, &usage).expect(model);
            assert!(estimate.usd > 0.0, "expected positive USD for {model}");
            assert_eq!(estimate.cny, 0.0);
        }

        // Anthropic / Qwen rows that publish a cache-write premium (#4318).
        for (model, write) in [
            ("claude-opus-4-8", Some(6.25)),
            ("claude-sonnet-4-6", Some(3.75)),
            ("claude-haiku-4-5", Some(1.25)),
            ("claude-fable-5", Some(12.50)),
            ("qwen/qwen3.7-plus", Some(0.40)),
            ("gpt-5.5", None),
        ] {
            let pricing = pricing_for_model_at(model, Utc::now()).expect(model);
            assert_eq!(
                pricing.usd.cache_write_per_million, write,
                "cache-write rate for {model}"
            );
        }
    }

    #[test]
    fn cache_write_tokens_increase_anthropic_cost_estimate() {
        let with_write = Usage {
            input_tokens: 12_048,
            output_tokens: 1,
            prompt_cache_hit_tokens: Some(10_000),
            prompt_cache_miss_tokens: Some(3),
            prompt_cache_write_tokens: Some(2_045),
            ..Default::default()
        };
        let write_as_miss = Usage {
            input_tokens: 12_048,
            output_tokens: 1,
            prompt_cache_hit_tokens: Some(10_000),
            prompt_cache_miss_tokens: Some(2_048),
            prompt_cache_write_tokens: None,
            ..Default::default()
        };

        let priced =
            calculate_turn_cost_estimate_from_usage("claude-fable-5", &with_write).expect("priced");
        let undercounted =
            calculate_turn_cost_estimate_from_usage("claude-fable-5", &write_as_miss)
                .expect("priced");
        // 2045 write @ 12.50 vs same tokens @ miss 10.00 → ~0.005 USD premium.
        assert!(
            priced.usd > undercounted.usd,
            "write premium should raise cost: priced={} undercounted={}",
            priced.usd,
            undercounted.usd
        );
        let expected_premium = (2_045.0 / 1_000_000.0) * (12.50 - 10.00);
        assert!(
            (priced.usd - undercounted.usd - expected_premium).abs() < 1e-9,
            "premium delta mismatch: {}",
            priced.usd - undercounted.usd
        );
    }

    #[test]
    fn catalog_pricing_uses_its_cache_write_rate() {
        let offering = codewhale_config::catalog::CatalogOffering {
            provider: "anthropic".to_string(),
            wire_model_id: "catalog-priced-model".to_string(),
            endpoint_key: "chat".to_string(),
            cost: Some(codewhale_config::models_dev::ModelsDevCost {
                input: Some(10.0),
                output: Some(50.0),
                cache_read: Some(1.0),
                cache_write: Some(12.5),
            }),
            ..Default::default()
        };
        let usage = Usage {
            input_tokens: 13,
            output_tokens: 5,
            prompt_cache_hit_tokens: Some(2),
            prompt_cache_miss_tokens: Some(3),
            prompt_cache_write_tokens: Some(8),
            ..Default::default()
        };

        let estimate = catalog_cost_estimate_for_route(
            ApiProvider::Anthropic,
            "catalog-priced-model",
            &offering,
            &usage,
        )
        .expect("catalog cost estimate");
        assert!((estimate.usd - 0.000_382).abs() < 1e-15);
        assert_eq!(estimate.cny, 0.0);
    }

    #[test]
    fn recorded_time_provider_cost_keeps_catalog_cache_write_tier() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            prompt_cache_hit_tokens: Some(0),
            prompt_cache_miss_tokens: Some(0),
            prompt_cache_write_tokens: Some(1_000_000),
            ..Default::default()
        };

        let estimate = calculate_turn_cost_estimate_for_provider_at(
            ApiProvider::Openrouter,
            "qwen/qwen3.7-plus",
            &usage,
            Utc::now(),
        )
        .expect("provider catalog write price");

        assert!((estimate.usd - 0.40).abs() < f64::EPSILON);
        assert_eq!(estimate.cny, 0.0);
    }

    #[test]
    fn recorded_time_provider_cost_rejects_foreign_model_ids() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            ..Default::default()
        };

        assert!(
            calculate_turn_cost_estimate_for_provider_at(
                ApiProvider::Ollama,
                "gpt-5.5",
                &usage,
                Utc::now(),
            )
            .is_none()
        );
    }

    #[test]
    fn provider_cost_keeps_owned_hand_price_without_catalog_offering() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            ..Default::default()
        };
        assert!(
            crate::provider_lake::catalog_offering_for_model(ApiProvider::Openai, "gpt-5-codex")
                .is_none(),
            "regression fixture must exercise the hand-price fallback"
        );

        let estimate = calculate_turn_cost_estimate_for_provider_at(
            ApiProvider::Openai,
            "gpt-5-codex",
            &usage,
            Utc::now(),
        )
        .expect("OpenAI API owns the hand-priced model");

        assert!((estimate.usd - 1.25).abs() < f64::EPSILON);
        assert_eq!(estimate.cny, 0.0);
        assert!(has_pricing_for_provider(ApiProvider::Openai, "gpt-5-codex"));
    }

    #[test]
    fn provider_hand_price_fills_catalog_missing_used_class() {
        let offering =
            crate::provider_lake::catalog_offering_for_model(ApiProvider::Openai, "gpt-5.5")
                .expect("bundled OpenAI route");
        let catalog_pricing =
            OfferingPricing::from_catalog_offering(&offering).expect("catalog pricing");
        assert!(catalog_pricing.cache_write_per_million.is_none());
        let usage = Usage {
            input_tokens: 250_000,
            output_tokens: 0,
            prompt_cache_miss_tokens: Some(0),
            prompt_cache_write_tokens: Some(250_000),
            ..Default::default()
        };

        let estimate = calculate_turn_cost_estimate_for_provider_at(
            ApiProvider::Openai,
            "gpt-5.5",
            &usage,
            Utc::now(),
        )
        .expect("provider hand price supplies the missing cache-write class");

        assert!((estimate.usd - 1.25).abs() < f64::EPSILON);
        assert_eq!(estimate.cny, 0.0);
    }

    #[test]
    fn provider_cost_does_not_fabricate_price_for_costless_catalog_route() {
        let offering = crate::provider_lake::catalog_offering_for_model(
            ApiProvider::Openai,
            "deepseek-v4-pro",
        )
        .expect("bundled OpenAI-compatible route");
        assert!(OfferingPricing::from_catalog_offering(&offering).is_none());
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            ..Default::default()
        };

        assert!(
            calculate_turn_cost_estimate_for_provider_at(
                ApiProvider::Openai,
                "deepseek-v4-pro",
                &usage,
                Utc::now(),
            )
            .is_none()
        );
        assert!(
            calculate_turn_cost_estimate_for_provider(
                ApiProvider::Openai,
                "deepseek-v4-pro",
                &usage,
            )
            .is_none()
        );
        assert!(!has_pricing_for_provider(
            ApiProvider::Openai,
            "deepseek-v4-pro"
        ));
        assert!(
            calculate_cache_savings_for_provider(
                ApiProvider::Openai,
                "deepseek-v4-pro",
                1_000_000,
            )
            .is_none()
        );
    }

    #[test]
    fn recorded_time_provider_cost_bounds_deepseek_compatibility_aliases() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            ..Default::default()
        };
        let before_retirement: DateTime<Utc> =
            "2026-07-24T15:58:59Z".parse().expect("pre-retirement time");
        let at_retirement: DateTime<Utc> = DEEPSEEK_ALIAS_RETIREMENT_UTC
            .parse()
            .expect("retirement time");

        assert!(
            calculate_turn_cost_estimate_for_provider_at(
                ApiProvider::Deepseek,
                "deepseek-chat",
                &usage,
                before_retirement,
            )
            .is_some()
        );
        assert!(
            calculate_turn_cost_estimate_for_provider_at(
                ApiProvider::Deepseek,
                "deepseek-reasoner",
                &usage,
                at_retirement,
            )
            .is_none()
        );
    }

    #[test]
    fn token_usage_for_pricing_maps_cache_and_reasoning_classes() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            prompt_cache_hit_tokens: Some(250),
            prompt_cache_miss_tokens: Some(700),
            prompt_cache_write_tokens: Some(50),
            reasoning_tokens: Some(50),
            ..Default::default()
        };

        assert_eq!(
            token_usage_for_pricing(&usage),
            TokenUsage {
                input: 700,
                output: 150,
                cache_read: 250,
                cache_write: 50,
            }
        );
    }

    #[test]
    fn openai_codex_gpt55_cost_is_unavailable_even_with_usage() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            prompt_cache_hit_tokens: Some(250),
            prompt_cache_miss_tokens: Some(750),
            ..Default::default()
        };

        assert!(calculate_turn_cost_estimate_from_usage("gpt-5.5", &usage).is_some());
        assert!(has_pricing_for_provider(ApiProvider::Openai, "gpt-5.5"));
        assert!(!has_pricing_for_provider(
            ApiProvider::OpenaiCodex,
            "gpt-5.5"
        ));
        assert!(
            calculate_turn_cost_estimate_for_provider(ApiProvider::OpenaiCodex, "gpt-5.5", &usage)
                .is_none()
        );
        assert!(
            calculate_cache_savings_for_provider(ApiProvider::OpenaiCodex, "gpt-5.5", 250)
                .is_none()
        );
    }

    #[test]
    fn subscription_route_does_not_inherit_same_models_api_price() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            ..Default::default()
        };
        assert!(
            calculate_turn_cost_estimate_for_route(
                ApiProvider::Anthropic,
                "claude-sonnet-5",
                &usage,
                crate::route_billing::BillingPresentation::Metered,
            )
            .is_some()
        );
        assert!(
            calculate_turn_cost_estimate_for_route(
                ApiProvider::Anthropic,
                "claude-sonnet-5",
                &usage,
                crate::route_billing::BillingPresentation::Subscription("Claude OAuth quota"),
            )
            .is_none()
        );
    }

    #[test]
    fn token_usage_for_pricing_infers_missing_cache_miss_from_hit_source() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            prompt_cache_hit_tokens: Some(250),
            prompt_cache_miss_tokens: None,
            ..Default::default()
        };

        assert_eq!(
            token_usage_for_pricing(&usage),
            TokenUsage {
                input: 750,
                output: 100,
                cache_read: 250,
                cache_write: 0,
            }
        );
    }

    #[test]
    fn catalog_pricing_overrides_known_row_when_present() {
        let _lock = crate::model_catalog::test_catalog_lock();
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "catalog-priced-model".to_string(),
            crate::model_catalog::CatalogEntry {
                id: "catalog-priced-model".to_string(),
                context_window: None,
                max_output: None,
                supports_reasoning: None,
                input_usd_per_million: Some(0.25),
                output_usd_per_million: Some(1.25),
                modalities: Vec::new(),
                supported_parameters: Vec::new(),
                provider_model_id: None,
                provenance: crate::model_catalog::MetadataProvenance::UserOverride,
            },
        );
        let catalog = crate::model_catalog::MergedCatalog::from_sources(
            overrides,
            None,
            crate::model_catalog::bundled_catalog(),
            Utc::now(),
        );
        let _guard = crate::model_catalog::replace_active_catalog_for_test(catalog);

        let pricing = pricing_for_model_at("catalog-priced-model", Utc::now()).expect("pricing");
        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.25);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 0.25);
        assert_eq!(pricing.usd.output_per_million, 1.25);
        assert!(pricing.cny.is_none());
    }

    #[test]
    fn sonnet_5_uses_intro_pricing_before_2026_08_31_expiry() {
        let before_expiry = Utc
            .with_ymd_and_hms(2026, 8, 31, 23, 59, 59)
            .single()
            .unwrap();
        let pricing = pricing_for_model_at("claude-sonnet-5", before_expiry).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.20);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 2.00);
        assert_eq!(pricing.usd.output_per_million, 10.00);
        assert_eq!(pricing.usd.cache_write_per_million, Some(2.50));
        assert!(pricing.cny.is_none());
    }

    #[test]
    fn sonnet_5_uses_standard_pricing_after_intro_window() {
        let after_expiry = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).single().unwrap();
        let pricing = pricing_for_model_at("claude-sonnet-5", after_expiry).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.30);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 3.00);
        assert_eq!(pricing.usd.output_per_million, 15.00);
        assert_eq!(pricing.usd.cache_write_per_million, Some(3.75));
        assert!(pricing.cny.is_none());
        assert!(has_pricing_for_model("claude-sonnet-5"));
    }

    #[test]
    fn v4_pro_uses_limited_time_discount_before_expiry() {
        let before_expiry = Utc
            .with_ymd_and_hms(2026, 5, 31, 15, 58, 59)
            .single()
            .unwrap();
        let pricing = pricing_for_model_at("deepseek-v4-pro", before_expiry).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.003625);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 0.435);
        assert_eq!(pricing.usd.output_per_million, 0.87);
        let cny = pricing.cny.expect("DeepSeek pricing has CNY");
        assert_eq!(cny.input_cache_hit_per_million, 0.025);
        assert_eq!(cny.input_cache_miss_per_million, 3.0);
        assert_eq!(cny.output_per_million, 6.0);
    }

    #[test]
    fn v4_pro_keeps_adjusted_rates_after_discount_window() {
        let after_expiry = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).single().unwrap();
        let pricing = pricing_for_model_at("deepseek-v4-pro", after_expiry).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.003625);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 0.435);
        assert_eq!(pricing.usd.output_per_million, 0.87);
        let cny = pricing.cny.expect("DeepSeek pricing has CNY");
        assert_eq!(cny.input_cache_hit_per_million, 0.025);
        assert_eq!(cny.input_cache_miss_per_million, 3.0);
        assert_eq!(cny.output_per_million, 6.0);
    }

    #[test]
    fn v4_pro_discount_still_applies_just_before_old_may5_expiry() {
        // Regression for #267 and #2489: the adjusted V4-Pro pricing should
        // not drift back to the original higher launch rates.
        let after_old_expiry = Utc.with_ymd_and_hms(2026, 5, 6, 0, 0, 0).single().unwrap();
        let pricing = pricing_for_model_at("deepseek-v4-pro", after_old_expiry).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.003625);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 0.435);
        assert_eq!(pricing.usd.output_per_million, 0.87);
    }

    #[test]
    fn v4_flash_keeps_current_published_rates() {
        let now = Utc.with_ymd_and_hms(2026, 4, 25, 0, 0, 0).single().unwrap();
        let pricing = pricing_for_model_at("deepseek-v4-flash", now).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.0028);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 0.14);
        assert_eq!(pricing.usd.output_per_million, 0.28);
        let cny = pricing.cny.expect("DeepSeek pricing has CNY");
        assert_eq!(cny.input_cache_hit_per_million, 0.02);
        assert_eq!(cny.input_cache_miss_per_million, 1.0);
        assert_eq!(cny.output_per_million, 2.0);
    }

    #[test]
    fn xiaomi_mimo_token_plan_models_leave_cost_unknown() {
        let now = Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).single().unwrap();

        for model in [
            "mimo-v2.5-pro",
            "mimo-v2.5-pro-ultraspeed",
            "mimo-v2.5",
            "xiaomi/mimo-v2.5",
        ] {
            assert!(pricing_for_model_at(model, now).is_none());
            assert!(!has_pricing_for_model(model));
        }
    }

    #[test]
    fn cost_estimate_calculates_usd_and_cny() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            ..Default::default()
        };
        let estimate =
            calculate_turn_cost_estimate_from_usage("deepseek-v4-flash", &usage).expect("estimate");

        assert_eq!(estimate.usd, 0.28);
        assert_eq!(estimate.cny, 2.0);
    }

    #[test]
    fn cost_currency_accepts_yuan_aliases() {
        assert_eq!(CostCurrency::from_setting("usd"), Some(CostCurrency::Usd));
        assert_eq!(CostCurrency::from_setting("yuan"), Some(CostCurrency::Cny));
        assert_eq!(CostCurrency::from_setting("rmb"), Some(CostCurrency::Cny));
        assert_eq!(CostCurrency::from_setting("cny"), Some(CostCurrency::Cny));
        assert_eq!(CostCurrency::from_setting("eur"), None);
    }

    #[test]
    fn format_cost_amount_uses_selected_symbol() {
        assert_eq!(format_cost_amount(0.42, CostCurrency::Usd), "$0.42");
        assert_eq!(format_cost_amount(2.0, CostCurrency::Cny), "¥2.00");
    }

    #[test]
    fn format_cost_amount_precise_keeps_report_precision() {
        assert_eq!(
            format_cost_amount_precise(0.1234, CostCurrency::Usd),
            "$0.1234"
        );
        assert_eq!(
            format_cost_amount_precise(0.1234, CostCurrency::Cny),
            "¥0.1234"
        );
    }

    // ── BalanceResponse / BalanceInfo ──────────────────────────────

    #[test]
    fn balance_response_deserializes_from_json() {
        let json = r#"{
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "123.45",
                    "topped_up_balance": "100.00",
                    "granted_balance": "23.45"
                }
            ]
        }"#;
        let resp: BalanceResponse = serde_json::from_str(json).expect("valid JSON");
        assert!(resp.is_available);
        assert_eq!(resp.balance_infos.len(), 1);
        let info = &resp.balance_infos[0];
        assert_eq!(info.currency, "CNY");
        assert_eq!(info.total_balance, "123.45");
        assert_eq!(info.topped_up_balance, "100.00");
        assert_eq!(info.granted_balance, "23.45");
    }

    #[test]
    fn balance_response_defaults_empty_balance_infos_when_unavailable() {
        let json = r#"{"is_available": false, "balance_infos": []}"#;
        let resp: BalanceResponse = serde_json::from_str(json).expect("valid JSON");
        assert!(!resp.is_available);
        assert!(resp.balance_infos.is_empty());
    }

    #[test]
    fn balance_response_empty_list_is_valid() {
        let json = r#"{"is_available": true, "balance_infos": []}"#;
        let resp: BalanceResponse = serde_json::from_str(json).expect("valid JSON");
        assert!(resp.is_available);
        assert!(resp.balance_infos.is_empty());
    }

    // ── BalanceInfo::total_balance_f64 ─────────────────────────────

    #[test]
    fn total_balance_f64_parses_decimal() {
        let info = BalanceInfo {
            currency: "CNY".into(),
            total_balance: "123.45".into(),
            ..Default::default()
        };
        assert_eq!(info.total_balance_f64(), Some(123.45));
    }

    #[test]
    fn total_balance_f64_returns_none_on_empty() {
        let info = BalanceInfo {
            currency: "USD".into(),
            total_balance: String::new(),
            ..Default::default()
        };
        assert_eq!(info.total_balance_f64(), None);
    }

    #[test]
    fn total_balance_f64_returns_none_on_invalid() {
        let info = BalanceInfo {
            currency: "USD".into(),
            total_balance: "not-a-number".into(),
            ..Default::default()
        };
        assert_eq!(info.total_balance_f64(), None);
    }
}
