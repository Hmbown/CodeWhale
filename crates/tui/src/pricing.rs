//! Cost estimation for API usage.
//!
//! Pricing is stored per million tokens. DeepSeek rows include their published
//! CNY rates; OpenRouter-curated rows are USD-only. Direct Xiaomi MiMo Token
//! Plan usage is credit/quota based and is intentionally left unknown until a
//! reliable balance endpoint exists.

#[cfg(test)]
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use codewhale_config::pricing::TokenUsage;

use crate::config::ApiProvider;
use crate::models::Usage;

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
}

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    usd: CurrencyPricing,
    cny: Option<CurrencyPricing>,
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

fn pricing_for_model_at(model: &str, _now: DateTime<Utc>) -> Option<ModelPricing> {
    let lower = model.to_lowercase();
    if lower.starts_with("deepseek-ai/") {
        // NVIDIA NIM-hosted DeepSeek uses NVIDIA's catalog/account terms, not
        // DeepSeek Platform pricing. Avoid showing misleading DeepSeek costs.
        return None;
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
        "moonshotai/kimi-k2.6" | "kimi-k2.6" => Some(usd_only_pricing(0.34, 0.68, 3.41)),
        "z-ai/glm-5.1" | "glm-5.1" => Some(usd_only_pricing(0.182, 0.98, 3.08)),
        "minimax/minimax-m3" | "minimax-m3" => Some(usd_only_pricing(0.06, 0.30, 1.20)),
        "arcee-ai/trinity-large-thinking" | "trinity-large-thinking" => {
            Some(usd_only_pricing(0.06, 0.22, 0.85))
        }
        "openai/gpt-5.5" | "gpt-5.5" => Some(usd_only_pricing(0.50, 5.00, 30.00)),
        "openai/gpt-5.5-pro" | "gpt-5.5-pro" => Some(usd_only_pricing(30.00, 30.00, 180.00)),

        "qwen/qwen3.6-flash" => Some(usd_only_pricing(0.1875, 0.1875, 1.125)),
        "qwen/qwen3.6-35b-a3b" => Some(usd_only_pricing(0.05, 0.15, 1.00)),
        "qwen/qwen3.6-max-preview" => Some(usd_only_pricing(1.04, 1.04, 6.24)),
        "qwen/qwen3.6-27b" => Some(usd_only_pricing(0.2885, 0.2885, 3.17)),
        "qwen/qwen3.6-plus" => Some(usd_only_pricing(0.325, 0.325, 1.95)),
        "qwen/qwen3.7-max" => Some(usd_only_pricing(0.25, 1.25, 3.75)),

        "google/gemma-4-31b-it" => Some(usd_only_pricing(0.09, 0.12, 0.35)),
        "google/gemma-4-26b-a4b-it" => Some(usd_only_pricing(0.06, 0.06, 0.33)),
        "tencent/hy3-preview" => Some(usd_only_pricing(0.021, 0.063, 0.21)),
        "nvidia/nemotron-3-ultra-550b-a55b" | "nvidia/nemotron-3-ultra" => {
            Some(usd_only_pricing(0.15, 0.50, 2.50))
        }
        _ => None,
    }
}

fn usd_only_pricing(
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
) -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million,
            input_cache_miss_per_million,
            output_per_million,
        },
        cny: None,
    }
}

fn deepseek_v4_pro_pricing() -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million: 0.003625,
            input_cache_miss_per_million: 0.435,
            output_per_million: 0.87,
        },
        cny: Some(CurrencyPricing {
            input_cache_hit_per_million: 0.025,
            input_cache_miss_per_million: 3.0,
            output_per_million: 6.0,
        }),
    }
}

fn deepseek_v4_flash_pricing() -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million: 0.0028,
            input_cache_miss_per_million: 0.14,
            output_per_million: 0.28,
        },
        cny: Some(CurrencyPricing {
            input_cache_hit_per_million: 0.02,
            input_cache_miss_per_million: 1.0,
            output_per_million: 2.0,
        }),
    }
}

/// Calculate cost from provider usage, honoring DeepSeek context-cache fields.
#[must_use]
pub fn calculate_turn_cost_from_usage(model: &str, usage: &Usage) -> Option<f64> {
    calculate_turn_cost_estimate_from_usage(model, usage).map(|estimate| estimate.usd)
}

/// Calculate cost from provider usage in both official currencies.
#[must_use]
pub fn calculate_turn_cost_estimate_from_usage(model: &str, usage: &Usage) -> Option<CostEstimate> {
    let pricing = pricing_for_model(model)?;
    Some(CostEstimate {
        usd: calculate_turn_cost_from_usage_with_pricing(pricing.usd, usage),
        cny: pricing
            .cny
            .map(|pricing| calculate_turn_cost_from_usage_with_pricing(pricing, usage))
            .unwrap_or(0.0),
    })
}

/// Calculate cost from provider usage when the provider's billing surface is
/// known. ChatGPT/Codex OAuth does not expose authoritative dollar pricing to
/// this runtime, so usage is shown without fabricating a spend estimate.
#[must_use]
pub fn calculate_turn_cost_estimate_for_provider(
    provider: ApiProvider,
    model: &str,
    usage: &Usage,
) -> Option<CostEstimate> {
    if provider == ApiProvider::OpenaiCodex {
        return None;
    }
    calculate_turn_cost_estimate_from_usage(model, usage)
}

/// Project provider-normalized turn usage into canonical billable token
/// classes for the shared config pricing layer (#2961).
///
/// `Usage::prompt_cache_miss_tokens` is billed as ordinary non-cached input in
/// current CodeWhale pricing rows. `cache_write` remains zero because the TUI
/// `Usage` shape does not yet distinguish cache creation/write tokens from
/// ordinary cache misses.
#[must_use]
pub fn token_usage_for_pricing(usage: &Usage) -> TokenUsage {
    let cache_read = usage.prompt_cache_hit_tokens.unwrap_or(0);
    let non_cached_reported = usage
        .prompt_cache_miss_tokens
        .unwrap_or_else(|| usage.input_tokens.saturating_sub(cache_read));
    let accounted_input = cache_read.saturating_add(non_cached_reported);
    let uncategorized_input = usage.input_tokens.saturating_sub(accounted_input);
    let input = non_cached_reported.saturating_add(uncategorized_input);
    let output = usage
        .output_tokens
        .saturating_add(usage.reasoning_tokens.unwrap_or(0));

    TokenUsage {
        input: u64::from(input),
        output: u64::from(output),
        cache_read: u64::from(cache_read),
        cache_write: 0,
    }
}

fn calculate_turn_cost_from_usage_with_pricing(pricing: CurrencyPricing, usage: &Usage) -> f64 {
    let usage = token_usage_for_pricing(usage);
    let hit_cost = (usage.cache_read as f64 / 1_000_000.0) * pricing.input_cache_hit_per_million;
    let miss_cost = (usage.input as f64 / 1_000_000.0) * pricing.input_cache_miss_per_million;
    let output_cost = (usage.output as f64 / 1_000_000.0) * pricing.output_per_million;
    hit_cost + miss_cost + output_cost
}

/// Estimate how much money was saved by serving `cache_hit_tokens` from the
/// prefix cache instead of billing them at the cache-miss rate.  Returns `None`
/// when the model's pricing is unknown or the number of cache-hit tokens is
/// zero (nothing to save).
#[must_use]
pub fn calculate_cache_savings(model: &str, cache_hit_tokens: u32) -> Option<CostEstimate> {
    if cache_hit_tokens == 0 {
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

#[must_use]
pub fn calculate_cache_savings_for_provider(
    provider: ApiProvider,
    model: &str,
    cache_hit_tokens: u32,
) -> Option<CostEstimate> {
    if provider == ApiProvider::OpenaiCodex {
        return None;
    }
    calculate_cache_savings(model, cache_hit_tokens)
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
    fn catalog_sourced_models_have_usd_pricing() {
        for (model, input, output) in [
            ("glm-5.2", 1.4, 4.4),
            ("z-ai/glm-5.2", 1.4, 4.4),
            ("kimi-k2.7-code", 0.95, 4.0),
            ("moonshotai/kimi-k2.7-code", 0.95, 4.0),
            ("minimax-m2.7", 0.3, 1.2),
            ("minimax/minimax-m2.7", 0.3, 1.2),
            ("trinity-mini", 0.045, 0.15),
            ("arcee-ai/trinity-mini", 0.045, 0.15),
            ("claude-opus-4-8", 5.0, 25.0),
            ("claude-sonnet-4-6", 3.0, 15.0),
            ("claude-haiku-4-5", 1.0, 5.0),
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
    fn curated_usd_only_models_have_pricing_and_accrue_cost() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            prompt_cache_hit_tokens: Some(250_000),
            prompt_cache_miss_tokens: Some(750_000),
            ..Default::default()
        };
        for (model, hit, miss, output) in [
            ("kimi-k2.6", 0.34, 0.68, 3.41),
            ("z-ai/glm-5.1", 0.182, 0.98, 3.08),
            ("qwen/qwen3.6-plus", 0.325, 0.325, 1.95),
            ("trinity-large-thinking", 0.06, 0.22, 0.85),
            ("gpt-5.5", 0.50, 5.00, 30.00),
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
    }

    #[test]
    fn token_usage_for_pricing_maps_cache_and_reasoning_classes() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            prompt_cache_hit_tokens: Some(250),
            prompt_cache_miss_tokens: Some(700),
            reasoning_tokens: Some(50),
            ..Default::default()
        };

        assert_eq!(
            token_usage_for_pricing(&usage),
            TokenUsage {
                input: 750,
                output: 150,
                cache_read: 250,
                cache_write: 0,
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
