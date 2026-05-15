//! Cost estimation for DeepSeek API usage.
//!
//! Pricing based on DeepSeek's published rates (per million tokens).

use chrono::{DateTime, TimeZone, Utc};

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

fn v4_pro_discount_ends_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 31, 15, 59, 0)
        .single()
        .expect("valid DeepSeek V4 Pro discount end timestamp")
}

/// Look up pricing for a model name.
fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    pricing_for_model_at(model, Utc::now())
}

fn pricing_for_model_at(model: &str, now: DateTime<Utc>) -> Option<ModelPricing> {
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
        if now <= v4_pro_discount_ends_at() {
            // DeepSeek lists these as a limited-time 75% discount through
            // 2026-05-31 15:59 UTC.
            return Some(ModelPricing {
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
            });
        }
        Some(ModelPricing {
            usd: CurrencyPricing {
                input_cache_hit_per_million: 0.0145,
                input_cache_miss_per_million: 1.74,
                output_per_million: 3.48,
            },
            cny: CurrencyPricing {
                input_cache_hit_per_million: 0.1,
                input_cache_miss_per_million: 12.0,
                output_per_million: 24.0,
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
    fn v4_pro_returns_to_base_rates_after_discount_expiry() {
        let after_expiry = Utc
            .with_ymd_and_hms(2026, 5, 31, 16, 0, 0)
            .single()
            .unwrap();
        let pricing = pricing_for_model_at("deepseek-v4-pro", after_expiry).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.0145);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 1.74);
        assert_eq!(pricing.usd.output_per_million, 3.48);
        assert_eq!(pricing.cny.input_cache_hit_per_million, 0.1);
        assert_eq!(pricing.cny.input_cache_miss_per_million, 12.0);
        assert_eq!(pricing.cny.output_per_million, 24.0);
    }

    #[test]
    fn v4_pro_discount_still_applies_just_before_old_may5_expiry() {
        // Regression for #267: extension to 2026-05-31 15:59 UTC.
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
}
