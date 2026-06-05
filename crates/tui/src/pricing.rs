//! Cost estimation for DeepSeek API usage.
//!
//! Pricing based on DeepSeek's published rates (per million tokens).

#[cfg(test)]
use chrono::TimeZone;
use chrono::{DateTime, Utc};

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

/// Cost estimate in the two official DeepSeek pricing currencies.
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

/// Per-million-token pricing for a model in both official currencies.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    usd: CurrencyPricing,
    cny: CurrencyPricing,
}

/// Look up pricing for a model name.
fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    pricing_for_model_at(model, Utc::now())
}

fn pricing_for_model_at(model: &str, _now: DateTime<Utc>) -> Option<ModelPricing> {
    let lower = model.to_lowercase();
    if lower.starts_with("deepseek-ai/") {
        // NVIDIA NIM-hosted DeepSeek uses NVIDIA's catalog/account terms, not
        // DeepSeek Platform pricing. Avoid showing misleading DeepSeek costs.
        return None;
    }
    if !lower.contains("deepseek") {
        return None;
    }
    if lower.contains("v4-pro") || lower.contains("v4pro") {
        // DeepSeek's pricing page says the V4-Pro promotional 75% discount
        // becomes the official one-quarter base price after 2026-05-31 15:59
        // UTC. Keep using the adjusted rate after that cutoff (#2489).
        Some(ModelPricing {
            usd: CurrencyPricing {
                input_cache_hit_per_million: 0.003625,
                input_cache_miss_per_million: 0.435,
                output_per_million: 0.87,
            },
            cny: CurrencyPricing {
                input_cache_hit_per_million: 0.025,
                input_cache_miss_per_million: 3.0,
                output_per_million: 6.0,
            },
        })
    } else {
        // deepseek-v4-flash pricing.
        Some(ModelPricing {
            usd: CurrencyPricing {
                input_cache_hit_per_million: 0.0028,
                input_cache_miss_per_million: 0.14,
                output_per_million: 0.28,
            },
            cny: CurrencyPricing {
                input_cache_hit_per_million: 0.02,
                input_cache_miss_per_million: 1.0,
                output_per_million: 2.0,
            },
        })
    }
}

/// Calculate cost for a turn given token usage and model.
#[must_use]
#[allow(dead_code)]
pub fn calculate_turn_cost(model: &str, input_tokens: u32, output_tokens: u32) -> Option<f64> {
    calculate_turn_cost_estimate(model, input_tokens, output_tokens).map(|estimate| estimate.usd)
}

/// Calculate cost for a turn in both official currencies.
///
/// This legacy helper has no cache telemetry, so it prices all input tokens as
/// cache misses. Prefer [`calculate_turn_cost_estimate_from_usage`] when the
/// provider returned usage details.
#[must_use]
pub fn calculate_turn_cost_estimate(
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> Option<CostEstimate> {
    let pricing = pricing_for_model(model)?;
    Some(CostEstimate {
        usd: calculate_turn_cost_with_pricing(pricing.usd, input_tokens, output_tokens),
        cny: calculate_turn_cost_with_pricing(pricing.cny, input_tokens, output_tokens),
    })
}

fn calculate_turn_cost_with_pricing(
    pricing: CurrencyPricing,
    input_tokens: u32,
    output_tokens: u32,
) -> f64 {
    let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_cache_miss_per_million;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_per_million;
    input_cost + output_cost
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
        cny: calculate_turn_cost_from_usage_with_pricing(pricing.cny, usage),
    })
}

fn calculate_turn_cost_from_usage_with_pricing(pricing: CurrencyPricing, usage: &Usage) -> f64 {
    let hit_tokens = usage.prompt_cache_hit_tokens.unwrap_or(0);
    let miss_tokens = usage
        .prompt_cache_miss_tokens
        .unwrap_or_else(|| usage.input_tokens.saturating_sub(hit_tokens));
    let accounted_input = hit_tokens.saturating_add(miss_tokens);
    let uncategorized_input = usage.input_tokens.saturating_sub(accounted_input);

    let hit_cost = (hit_tokens as f64 / 1_000_000.0) * pricing.input_cache_hit_per_million;
    let miss_cost = ((miss_tokens.saturating_add(uncategorized_input)) as f64 / 1_000_000.0)
        * pricing.input_cache_miss_per_million;
    let reasoning = usage.reasoning_tokens.unwrap_or(0);
    let effective_output = usage.output_tokens.saturating_add(reasoning);
    let output_cost = (effective_output as f64 / 1_000_000.0) * pricing.output_per_million;
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
        cny: tokens
            * (pricing.cny.input_cache_miss_per_million - pricing.cny.input_cache_hit_per_million),
    })
}

/// Format a USD cost for compact display.
#[must_use]
#[allow(dead_code)]
pub fn format_cost(cost: f64) -> String {
    format_cost_amount(cost, CostCurrency::Usd)
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

    #[test]
    fn nvidia_nim_deepseek_model_does_not_use_deepseek_platform_pricing() {
        assert!(calculate_turn_cost("deepseek-ai/deepseek-v4-pro", 1_000, 1_000).is_none());
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
        assert_eq!(pricing.cny.input_cache_hit_per_million, 0.025);
        assert_eq!(pricing.cny.input_cache_miss_per_million, 3.0);
        assert_eq!(pricing.cny.output_per_million, 6.0);
    }

    #[test]
    fn v4_pro_keeps_adjusted_rates_after_discount_window() {
        let after_expiry = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).single().unwrap();
        let pricing = pricing_for_model_at("deepseek-v4-pro", after_expiry).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.003625);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 0.435);
        assert_eq!(pricing.usd.output_per_million, 0.87);
        assert_eq!(pricing.cny.input_cache_hit_per_million, 0.025);
        assert_eq!(pricing.cny.input_cache_miss_per_million, 3.0);
        assert_eq!(pricing.cny.output_per_million, 6.0);
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
        assert_eq!(pricing.cny.input_cache_hit_per_million, 0.02);
        assert_eq!(pricing.cny.input_cache_miss_per_million, 1.0);
        assert_eq!(pricing.cny.output_per_million, 2.0);
    }

    #[test]
    fn cost_estimate_calculates_usd_and_cny() {
        let estimate = calculate_turn_cost_estimate("deepseek-v4-flash", 1_000_000, 500_000)
            .expect("estimate");

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
