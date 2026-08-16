//! Cost estimation for API usage.
//!
//! Pricing is stored per million tokens. DeepSeek rows include their published
//! CNY rates; OpenRouter-curated rows are USD-only. Direct Xiaomi MiMo Token
//! Plan usage is credit/quota based and is intentionally left unknown until a
//! reliable balance endpoint exists.

use chrono::{DateTime, TimeZone, Utc};
use codewhale_config::pricing::{
    Currency, LIVE_PRICING_MAX_AGE_SECS, LivePricingDefect, OfferingPricing, PricingProvenance,
    TokenClass, TokenUsage,
};

use crate::config::{
    ApiProvider, DEEPSEEK_ALIAS_REPLACEMENT, DEEPSEEK_ALIAS_RETIREMENT_UTC,
    DEFAULT_STEPFUN_BASE_URL, DEFAULT_STEPFUN_MODEL, DEFAULT_STEPFUN_PLAN_BASE_URL,
    canonical_model_id_for_provider,
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
        self.is_finite_nonnegative() && (self.usd > 0.0 || self.cny > 0.0)
    }

    /// A cost is safe to persist/display only when both carried currencies are
    /// finite and nonnegative.
    #[must_use]
    pub fn is_finite_nonnegative(self) -> bool {
        self.usd.is_finite() && self.usd >= 0.0 && self.cny.is_finite() && self.cny >= 0.0
    }

    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            usd: if self.usd.is_finite() && self.usd >= 0.0 {
                self.usd
            } else {
                0.0
            },
            cny: if self.cny.is_finite() && self.cny >= 0.0 {
                self.cny
            } else {
                0.0
            },
        }
    }

    /// Add cost without ever producing NaN, infinity, or a negative total.
    /// Individual pricing rows are validated earlier; the saturation protects
    /// long-running accumulation from floating-point overflow.
    #[must_use]
    pub fn saturating_add(self, rhs: Self) -> Self {
        fn component(left: f64, right: f64) -> f64 {
            let sum = left + right;
            if sum.is_finite() { sum } else { f64::MAX }
        }
        let left = self.sanitized();
        let right = rhs.sanitized();
        Self {
            usd: component(left.usd, right.usd),
            cny: component(left.cny, right.cny),
        }
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
    // Wire fields of the live `GET /user/balance` response deserialized in
    // `tui::ui::fetch_deepseek_balance` and parked in `App::balance_cell`.
    // Nothing renders them today — the footer balance chip went with the
    // legacy FooterWidget — so `dead_code` cannot see the producer. Kept
    // because they are the API contract, matching the sibling fields below.
    #[allow(dead_code)]
    pub currency: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub total_balance: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub topped_up_balance: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub granted_balance: String,
}

impl BalanceInfo {}

/// How a hand-sourced row bills cache-creation (cache-write) tokens.
///
/// The distinction matters because "no separate write rate published" and
/// "documented to cost the same as ordinary input" are different facts that used
/// to collapse onto the same `None`. Folding the unknown case into the input
/// rate invents a price; this enum keeps the invention impossible (#4318).
#[derive(Debug, Clone, Copy, PartialEq)]
enum CacheWritePolicy {
    /// The provider publishes a distinct cache-creation rate (per million).
    Rate(f64),
    /// Provider documentation states that cache creation carries **no separate
    /// charge** beyond the ordinary cache-miss input rate, so the miss rate is
    /// the published write rate rather than a substitute for a missing one.
    ///
    /// The `&'static str` is the documentation receipt this claim rests on, so
    /// the policy is auditable instead of asserted.
    DocumentedAsInputRate(&'static str),
    /// No published cache-write rate was found for this row. A turn that
    /// actually wrote to cache fails closed rather than being billed at a rate
    /// CodeWhale made up.
    Unpublished,
}

/// DeepSeek's context-caching docs: tokens that miss the cache are billed once
/// at the cache-miss rate and writing them into the cache costs nothing extra.
/// <https://api-docs.deepseek.com/guides/kv_cache>
const DEEPSEEK_CACHE_WRITE_IS_FREE: &str = "deepseek-kv-cache-no-write-charge";

impl CacheWritePolicy {
    /// The rate to bill cache-write tokens at, given the row's input rate.
    ///
    /// `None` means the row cannot price cache-write tokens at all.
    fn rate(self, input_cache_miss_per_million: f64) -> Option<f64> {
        match self {
            Self::Rate(rate) => Some(rate),
            Self::DocumentedAsInputRate(_) => Some(input_cache_miss_per_million),
            Self::Unpublished => None,
        }
    }
}

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy)]
struct CurrencyPricing {
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
    /// How cache-creation tokens are billed on this row.
    cache_write: CacheWritePolicy,
}

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    usd: CurrencyPricing,
    cny: Option<CurrencyPricing>,
}

pub(crate) const STEPFUN_PAYG_BILLING_SURFACE: &str = "stepfun-payg";
pub(crate) const STEPFUN_PLAN_BILLING_SURFACE: &str = "stepfun-plan";
const LEGACY_STEPFUN_PLAN_BASE_URL: &str = "https://api.stepfun.com/step_plan/v1";

/// Z.ai's dedicated Coding endpoint — the GLM Coding Plan subscription route.
pub(crate) const ZAI_CODING_PLAN_BILLING_SURFACE: &str = "zai-coding-plan";
/// Z.ai's ordinary public per-token API.
pub(crate) const ZAI_PAYG_BILLING_SURFACE: &str = "zai-payg";
/// Moonshot's Kimi Code subscription endpoint.
pub(crate) const MOONSHOT_KIMI_CODE_BILLING_SURFACE: &str = "moonshot-kimi-code";
/// Moonshot's ordinary public per-token API.
pub(crate) const MOONSHOT_PAYG_BILLING_SURFACE: &str = "moonshot-payg";
/// MiniMax's prepaid Token Plan endpoint.
pub(crate) const MINIMAX_TOKEN_PLAN_BILLING_SURFACE: &str = "minimax-token-plan";
/// MiniMax's ordinary public per-token API.
pub(crate) const MINIMAX_PAYG_BILLING_SURFACE: &str = "minimax-payg";
/// Xiaomi MiMo's prepaid token-plan endpoint.
pub(crate) const XIAOMI_TOKEN_PLAN_BILLING_SURFACE: &str = "xiaomi-mimo-token-plan";
/// Xiaomi MiMo's ordinary public per-token API.
pub(crate) const XIAOMI_PAYG_BILLING_SURFACE: &str = "xiaomi-mimo-payg";
/// An OAuth/subscription-brokered endpoint (Codex, Claude OAuth, Grok OAuth,
/// OpenCode Go). Never per-token metered from CodeWhale's side.
pub(crate) const OAUTH_SUBSCRIPTION_BILLING_SURFACE: &str = "oauth-subscription";
/// A loopback / self-hosted endpoint with no provider bill at all.
pub(crate) const LOCAL_BILLING_SURFACE: &str = "local-no-bill";
/// A provider's own first-party public per-token API, on its documented host.
pub(crate) const FIRST_PARTY_PAYG_BILLING_SURFACE: &str = "first-party-payg";
/// An aggregator/reseller endpoint: metered, but priced by the aggregator's own
/// catalog rather than by the upstream model owner's published rates.
pub(crate) const AGGREGATOR_BILLING_SURFACE: &str = "aggregator-payg";
/// A reachable endpoint CodeWhale could not match to any known billing surface.
/// Distinct from "not classified yet": this is a positive statement that the
/// surface is unknown, and it fails closed everywhere it is consumed.
pub(crate) const UNCLASSIFIED_BILLING_SURFACE: &str = "unclassified";

/// How a classified billing surface meters money.
///
/// This is the fact every cost surface actually needs: whether a dollar figure
/// is even the right unit for the route. `Unknown` is a real answer and is
/// treated as *possibly* metered — it is counted as missing spend rather than
/// excused as a subscription (#4318).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointMetering {
    /// Per-token money, priced against published rates.
    Money,
    /// An exactly-identified subscription or prepaid-quota endpoint. Money is
    /// the wrong unit here, so these turns are excluded from money coverage.
    ExactSubscription,
    /// Local/self-hosted: there is no provider bill.
    LocalNoBill,
    /// Could not be established. Fails closed as possibly-money.
    Unknown,
}

/// Classify a billing-surface id into its metering shape.
///
/// Unrecognized ids — including ones written by a newer build — resolve to
/// [`EndpointMetering::Unknown`] rather than being guessed into a bucket.
#[must_use]
pub fn endpoint_metering_for_billing_surface(billing_surface: Option<&str>) -> EndpointMetering {
    let Some(surface) = billing_surface.map(str::trim).filter(|s| !s.is_empty()) else {
        return EndpointMetering::Unknown;
    };
    // Exact, case-insensitive matches only. A prefix/substring rule here would
    // let an unrecognized future surface impersonate a known one.
    for (known, metering) in [
        (STEPFUN_PAYG_BILLING_SURFACE, EndpointMetering::Money),
        (ZAI_PAYG_BILLING_SURFACE, EndpointMetering::Money),
        (MOONSHOT_PAYG_BILLING_SURFACE, EndpointMetering::Money),
        (MINIMAX_PAYG_BILLING_SURFACE, EndpointMetering::Money),
        (XIAOMI_PAYG_BILLING_SURFACE, EndpointMetering::Money),
        (FIRST_PARTY_PAYG_BILLING_SURFACE, EndpointMetering::Money),
        (AGGREGATOR_BILLING_SURFACE, EndpointMetering::Money),
        (
            STEPFUN_PLAN_BILLING_SURFACE,
            EndpointMetering::ExactSubscription,
        ),
        (
            ZAI_CODING_PLAN_BILLING_SURFACE,
            EndpointMetering::ExactSubscription,
        ),
        (
            MOONSHOT_KIMI_CODE_BILLING_SURFACE,
            EndpointMetering::ExactSubscription,
        ),
        (
            MINIMAX_TOKEN_PLAN_BILLING_SURFACE,
            EndpointMetering::ExactSubscription,
        ),
        (
            XIAOMI_TOKEN_PLAN_BILLING_SURFACE,
            EndpointMetering::ExactSubscription,
        ),
        (
            OAUTH_SUBSCRIPTION_BILLING_SURFACE,
            EndpointMetering::ExactSubscription,
        ),
        (LOCAL_BILLING_SURFACE, EndpointMetering::LocalNoBill),
        (UNCLASSIFIED_BILLING_SURFACE, EndpointMetering::Unknown),
    ] {
        if surface.eq_ignore_ascii_case(known) {
            return metering;
        }
    }
    EndpointMetering::Unknown
}

/// A base URL reduced to the non-secret parts a billing classification may
/// depend on: scheme, host, normalized path. `None` when the URL carries
/// embedded credentials, a query, a fragment, a non-default port, or is not
/// HTTPS — any of which means CodeWhale cannot vouch for which surface it is.
struct EndpointShape {
    host: String,
    path: String,
}

fn endpoint_shape(base_url: &str) -> Option<EndpointShape> {
    let parsed = reqwest::Url::parse(base_url.trim()).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port_or_known_default() != Some(443)
    {
        return None;
    }
    Some(EndpointShape {
        host: parsed.host_str()?.to_ascii_lowercase(),
        path: parsed.path().trim_end_matches('/').to_string(),
    })
}

fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

/// Reduce a concrete request endpoint to non-secret billing provenance.
///
/// Every reachable endpoint now gets a positive classification, including
/// [`UNCLASSIFIED_BILLING_SURFACE`] for one CodeWhale cannot place. `None` is
/// reserved for "no endpoint was supplied", which is a different failure and is
/// also treated as unknown downstream. Nothing here consults credentials or
/// echoes a URL, so the result is safe to persist and log.
pub(crate) fn billing_surface_for_route(
    provider: ApiProvider,
    base_url: Option<&str>,
) -> Option<&'static str> {
    // Routes whose billing shape is a property of the provider itself, not of
    // the endpoint spelling.
    match provider {
        ApiProvider::Ollama | ApiProvider::Sglang | ApiProvider::Vllm => {
            return Some(LOCAL_BILLING_SURFACE);
        }
        // Ollama Cloud publishes plan/account terms, not a Codewhale-owned
        // per-token rate. Hosted is not local/free, but it is also not proof
        // of PAYG dollars: keep it in money coverage as unclassified until an
        // authoritative billing surface is available.
        ApiProvider::OllamaCloud => return Some(UNCLASSIFIED_BILLING_SURFACE),
        ApiProvider::OpenaiCodex | ApiProvider::OpencodeGo => {
            return Some(OAUTH_SUBSCRIPTION_BILLING_SURFACE);
        }
        // A named custom endpoint is never assumed to be metered; the billing
        // presentation layer decides that from explicit config.
        ApiProvider::Custom => return Some(UNCLASSIFIED_BILLING_SURFACE),
        _ => {}
    }

    let base_url = base_url.map(str::trim).filter(|url| !url.is_empty())?;
    let Some(shape) = endpoint_shape(base_url) else {
        return Some(UNCLASSIFIED_BILLING_SURFACE);
    };

    let surface = match provider {
        ApiProvider::Stepfun => stepfun_surface(&shape),
        ApiProvider::Zai => zai_surface(&shape),
        ApiProvider::Moonshot => moonshot_surface(&shape),
        ApiProvider::Minimax | ApiProvider::MinimaxAnthropic => minimax_surface(&shape),
        ApiProvider::XiaomiMimo => xiaomi_surface(&shape),
        ApiProvider::Openrouter | ApiProvider::NvidiaNim | ApiProvider::OpencodeZen => {
            is_official_default_endpoint(provider, &shape).then_some(AGGREGATOR_BILLING_SURFACE)
        }
        _ => is_official_default_endpoint(provider, &shape)
            .then_some(FIRST_PARTY_PAYG_BILLING_SURFACE),
    };
    Some(surface.unwrap_or(UNCLASSIFIED_BILLING_SURFACE))
}

fn stepfun_surface(shape: &EndpointShape) -> Option<&'static str> {
    if host_of(DEFAULT_STEPFUN_BASE_URL).is_some_and(|official| shape.host == official)
        && matches!(shape.path.as_str(), "" | "/v1")
    {
        return Some(STEPFUN_PAYG_BILLING_SURFACE);
    }
    let plan_host = [DEFAULT_STEPFUN_PLAN_BASE_URL, LEGACY_STEPFUN_PLAN_BASE_URL]
        .iter()
        .filter_map(|url| host_of(url))
        .any(|plan| plan == shape.host);
    if plan_host && matches!(shape.path.as_str(), "/step_plan" | "/step_plan/v1") {
        return Some(STEPFUN_PLAN_BILLING_SURFACE);
    }
    None
}

fn zai_surface(shape: &EndpointShape) -> Option<&'static str> {
    // The Coding Plan contract is the exact shipped Z.ai endpoint. Do not let
    // arbitrary future `/api/coding/*` paths, or the separate BigModel host,
    // inherit a subscription classification.
    if shape.host == "api.z.ai" && shape.path == "/api/coding/paas/v4" {
        Some(ZAI_CODING_PLAN_BILLING_SURFACE)
    } else if matches!(shape.host.as_str(), "api.z.ai" | "open.bigmodel.cn")
        && matches!(
            shape.path.as_str(),
            "/api/paas/v4" | "/api/anthropic" | "/v1" | ""
        )
    {
        Some(ZAI_PAYG_BILLING_SURFACE)
    } else {
        None
    }
}

fn moonshot_surface(shape: &EndpointShape) -> Option<&'static str> {
    // Kimi Code is a distinct membership product on api.kimi.com.  Accept the
    // exact shipped endpoint as well as its slash-normalized parent; do not
    // infer a plan from a model id or from an arbitrary host carrying a
    // `/coding` path.
    if shape.host == "api.kimi.com" && matches!(shape.path.as_str(), "/coding" | "/coding/v1") {
        Some(MOONSHOT_KIMI_CODE_BILLING_SURFACE)
    } else if matches!(shape.host.as_str(), "api.moonshot.ai" | "api.moonshot.cn")
        && matches!(shape.path.as_str(), "" | "/v1" | "/anthropic")
    {
        Some(MOONSHOT_PAYG_BILLING_SURFACE)
    } else {
        None
    }
}

fn minimax_surface(shape: &EndpointShape) -> Option<&'static str> {
    // MiniMax API keys and subscription-plan keys use the same normal
    // endpoints. The URL therefore proves neither PAYG nor plan billing; only
    // an explicit saved mode may produce a concrete MiniMax surface.
    let _is_supported_endpoint = matches!(
        shape.host.as_str(),
        "api.minimax.io" | "api.minimaxi.com" | "api.minimax.chat"
    ) && matches!(shape.path.as_str(), "" | "/v1" | "/anthropic");
    None
}

fn xiaomi_surface(shape: &EndpointShape) -> Option<&'static str> {
    if matches!(
        shape.host.as_str(),
        "token-plan-cn.xiaomimimo.com"
            | "token-plan-sgp.xiaomimimo.com"
            | "token-plan-ams.xiaomimimo.com"
    ) && shape.path == "/v1"
    {
        return Some(XIAOMI_TOKEN_PLAN_BILLING_SURFACE);
    }
    if shape.host == "api.xiaomimimo.com" && shape.path == "/v1" {
        return Some(XIAOMI_PAYG_BILLING_SURFACE);
    }
    None
}

/// Exact default endpoint match for built-in providers whose billing surface
/// has no provider-specific split above.
///
/// A provider enum is not proof that a configured URL is that provider's own
/// billing surface.  This allowlist keeps `https://proxy.example/v1` from
/// inheriting OpenAI/Anthropic/DeepSeek/OpenRouter prices merely because the
/// selected protocol/provider name is familiar.
fn is_official_default_endpoint(provider: ApiProvider, shape: &EndpointShape) -> bool {
    let Some(default) = endpoint_shape(provider.default_base_url()) else {
        return false;
    };
    if shape.host != default.host {
        return false;
    }
    if shape.path == default.path {
        return true;
    }
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
            matches!(shape.path.as_str(), "" | "/v1" | "/beta")
        }
        ApiProvider::DeepseekAnthropic => shape.path == "/anthropic",
        ApiProvider::Openai => matches!(shape.path.as_str(), "" | "/v1"),
        ApiProvider::Anthropic => matches!(shape.path.as_str(), "" | "/v1"),
        _ => false,
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
        "meta/muse-spark-1.1" | "muse-spark-1.1" => Some(usd_only_pricing(0.15, 1.25, 4.25)),
        "meta/muse-spark-1.2" | "muse-spark-1.2" => Some(usd_only_pricing(0.15, 1.25, 4.25)),
        "meta/muse-spark-1.2-contributor" | "muse-spark-1.2-contributor" => {
            Some(usd_only_pricing(0.002, 0.10, 0.20))
        }
        // Grok 4.6 doubles all token rates when the prompt reaches 200K.
        // Metadata-only lookups use the standard tier; turn auditing below
        // selects the exact usage-aware tier for the direct xAI route.
        "grok-4.6" => Some(grok_4_6_pricing(false)),
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

/// A USD row whose provider publishes input/cache-read/output rates but **no**
/// cache-creation rate. Cache-write tokens on such a row are unpriced, not free
/// and not silently charged at the input rate (#4318).
fn usd_only_pricing(
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
) -> ModelPricing {
    usd_pricing(
        input_cache_hit_per_million,
        input_cache_miss_per_million,
        output_per_million,
        CacheWritePolicy::Unpublished,
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
        CacheWritePolicy::Rate(cache_write_per_million),
    )
}

fn usd_pricing(
    input_cache_hit_per_million: f64,
    input_cache_miss_per_million: f64,
    output_per_million: f64,
    cache_write: CacheWritePolicy,
) -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million,
            input_cache_miss_per_million,
            output_per_million,
            cache_write,
        },
        cny: None,
    }
}

const MINIMAX_M3_LONG_CONTEXT_THRESHOLD: u32 = 512_000;
const GROK_4_6_LONG_CONTEXT_THRESHOLD: u32 = 200_000;
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

fn grok_4_6_pricing(long_context: bool) -> ModelPricing {
    if long_context {
        usd_only_pricing(1.00, 4.00, 12.00)
    } else {
        usd_only_pricing(0.50, 2.00, 6.00)
    }
}

fn is_grok_4_6(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("grok-4.6")
}

fn pricing_for_model_and_usage(model: &str, usage: &Usage) -> Option<ModelPricing> {
    if is_minimax_m3(model) {
        return Some(minimax_m3_standard_pricing(
            usage.input_tokens > MINIMAX_M3_LONG_CONTEXT_THRESHOLD,
        ));
    }
    if is_grok_4_6(model) {
        return Some(grok_4_6_pricing(
            usage.input_tokens >= GROK_4_6_LONG_CONTEXT_THRESHOLD,
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

/// DeepSeek publishes only cache-hit and cache-miss input rates *because* its
/// context cache charges nothing extra to write: a token that misses the cache
/// is billed once at the miss rate and is cached as a side effect. That makes
/// the miss rate the documented write rate, not a stand-in for a missing one.
fn deepseek_v4_pro_pricing() -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million: 0.003625,
            input_cache_miss_per_million: 0.435,
            output_per_million: 0.87,
            cache_write: CacheWritePolicy::DocumentedAsInputRate(DEEPSEEK_CACHE_WRITE_IS_FREE),
        },
        cny: Some(CurrencyPricing {
            input_cache_hit_per_million: 0.025,
            input_cache_miss_per_million: 3.0,
            output_per_million: 6.0,
            cache_write: CacheWritePolicy::DocumentedAsInputRate(DEEPSEEK_CACHE_WRITE_IS_FREE),
        }),
    }
}

fn deepseek_v4_flash_pricing() -> ModelPricing {
    ModelPricing {
        usd: CurrencyPricing {
            input_cache_hit_per_million: 0.0028,
            input_cache_miss_per_million: 0.14,
            output_per_million: 0.28,
            cache_write: CacheWritePolicy::DocumentedAsInputRate(DEEPSEEK_CACHE_WRITE_IS_FREE),
        },
        cny: Some(CurrencyPricing {
            input_cache_hit_per_million: 0.02,
            input_cache_miss_per_million: 1.0,
            output_per_million: 2.0,
            cache_write: CacheWritePolicy::DocumentedAsInputRate(DEEPSEEK_CACHE_WRITE_IS_FREE),
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

/// Cost from a hand-sourced row, or `None` when the row cannot price a class
/// this turn actually used.
///
/// Only cache-write can fail here: input, cache-read, and output rates are
/// mandatory on every hand row, while a cache-creation rate exists only where a
/// provider publishes one or documents that writes cost nothing extra.
fn cost_estimate_with_pricing_checked(
    pricing: ModelPricing,
    usage: &Usage,
) -> Result<CostEstimate, Vec<TokenClass>> {
    let classes = token_usage_for_pricing(usage);
    if classes.cache_write > 0
        && pricing
            .usd
            .cache_write
            .rate(pricing.usd.input_cache_miss_per_million)
            .is_none()
    {
        return Err(vec![TokenClass::CacheWrite]);
    }
    Ok(CostEstimate {
        usd: calculate_turn_cost_from_usage_with_pricing(pricing.usd, usage),
        cny: pricing
            .cny
            .map(|pricing| calculate_turn_cost_from_usage_with_pricing(pricing, usage))
            .unwrap_or(0.0),
    })
}

/// Unchecked projection for the legacy model-only test helpers, which construct
/// usage they have already established the row can price.
///
/// Production paths must use [`cost_estimate_with_pricing_checked`] so an
/// unpublished cache-write rate fails closed instead of billing writes at the
/// input rate.
#[cfg(test)]
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
///
/// Production callers use [`audit_turn_cost_for_route`] instead: a caller that
/// adds to a total must also record why a turn was left out of it.
#[must_use]
#[cfg(test)]
pub fn calculate_turn_cost_estimate_for_route(
    provider: ApiProvider,
    model: &str,
    usage: &Usage,
    billing: crate::route_billing::BillingPresentation,
) -> Option<CostEstimate> {
    audit_turn_cost_for_route(provider, model, None, usage, Utc::now(), billing).estimate
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
    audit_turn_cost_for_provider_at(provider, model, usage, recorded_at).estimate
}

/// Why a route produced no cost estimate.
///
/// Every `None` from the estimator carries one of these so `/cost`, `/cache`,
/// and the scorecard can say *why* a turn is missing from a total instead of
/// letting the total read as complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpricedReason {
    /// The route is **exactly identified** as one where money is not the unit:
    /// a named OAuth subscription, a named prepaid token plan, or a local
    /// endpoint with no provider bill. Only this reason excuses a turn from
    /// money coverage, and only exact evidence may produce it (#4318).
    NotMoneyMetered,
    /// The route may or may not meter money and CodeWhale could not establish
    /// which. Distinct from [`Self::NotMoneyMetered`] on purpose: an unknown
    /// basis is counted as *possibly missing spend*, never waved through as a
    /// subscription. A cross-provider child route with no dispatch config is
    /// the common case.
    UnknownBillingBasis,
    /// One provider/model pair spans several billing systems and the non-secret
    /// endpoint provenance needed to pick one is missing.
    AmbiguousBillingSurface,
    /// No endpoint classification was supplied for the route at all.
    ///
    /// Distinct from [`Self::UnknownBillingBasis`], which means an endpoint was
    /// classified and could not be placed. This means none was offered, so
    /// there is no evidence the turn was served by the provider's own official
    /// surface rather than a proxy, a gateway, or a self-hosted clone that
    /// happens to speak the same protocol. A provider enum plus a familiar
    /// model id is not that evidence (#4318).
    UnestablishedEndpoint,
    /// The turn's endpoint classified as a per-token surface, but the pricing
    /// layer holds no rates for that specific surface (as opposed to no rates
    /// for the model at all).
    UnpricedBillingSurface,
    /// The only pricing row found claims live provider provenance but is stale
    /// or was fetched from a different endpoint, so it is not authoritative for
    /// this turn. Never silently downgraded to "authoritative anyway".
    UnverifiedLivePricing,
    /// A compatibility alias whose published rate has been retired.
    RetiredAlias,
    /// The turn crossed a request-wide pricing tier the pricing layer cannot
    /// represent yet (for example OpenAI's >272K long-context surcharge).
    UnrepresentedTier,
    /// No pricing row exists for this provider/model route.
    NoPricingRow,
    /// A row exists, but a token class this turn actually used has no published
    /// price, so the estimate fails closed rather than under-reporting.
    MissingClassPrice,
    /// A catalog row contains a NaN, infinite, or negative rate. The whole row
    /// is rejected at the trust boundary rather than partially billed.
    InvalidPricingRow,
    /// The row is denominated in a currency CodeWhale does not carry. No
    /// conversion is invented.
    UnsupportedCurrency,
    /// Provider telemetry assigns more cache-hit/miss/write tokens than the
    /// reported input total. Pricing that contradictory partition would
    /// over-count input, so the call is retained but fails closed.
    InconsistentUsage,
}

impl UnpricedReason {
    /// Stable, non-localized identifier for logs, JSON, and scorecards.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NotMoneyMetered => "not_money_metered",
            Self::UnknownBillingBasis => "unknown_billing_basis",
            Self::AmbiguousBillingSurface => "ambiguous_billing_surface",
            Self::UnestablishedEndpoint => "unestablished_endpoint",
            Self::UnpricedBillingSurface => "unpriced_billing_surface",
            Self::UnverifiedLivePricing => "unverified_live_pricing",
            Self::RetiredAlias => "retired_alias",
            Self::UnrepresentedTier => "unrepresented_pricing_tier",
            Self::NoPricingRow => "no_pricing_row",
            Self::MissingClassPrice => "missing_class_price",
            Self::InvalidPricingRow => "invalid_pricing_row",
            Self::UnsupportedCurrency => "unsupported_currency",
            Self::InconsistentUsage => "inconsistent_usage",
        }
    }

    /// Whether a turn with this reason belongs in the money-metered coverage
    /// denominator `/cost` reports against its dollar total.
    ///
    /// Only [`Self::NotMoneyMetered`] — an *exactly* identified subscription,
    /// token plan, or local route — is excluded. Everything else, including an
    /// unknown billing basis, counts as spend the total is missing, because
    /// treating "don't know" as "not billed" is what let unpriced turns
    /// disappear from a total that then read as complete (#4318).
    #[must_use]
    pub fn counts_toward_money_coverage(self) -> bool {
        self != Self::NotMoneyMetered
    }
}

/// A turn cost plus the provenance and completeness needed to audit it.
///
/// `estimate.is_some()` and `unpriced_reason.is_none()` always agree: this type
/// is produced by the same code path that computes the estimate, so an audit
/// can never disagree with the number a total was built from.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnCostAudit {
    /// The cost, when the route is priced for every class this turn used.
    pub estimate: Option<CostEstimate>,
    /// Where the applied (or attempted) pricing row came from.
    pub provenance: Option<PricingProvenance>,
    /// Classes this turn used that carry no published price.
    pub unpriced_classes: Vec<TokenClass>,
    /// Why the estimate is absent, when it is.
    pub unpriced_reason: Option<UnpricedReason>,
    /// Set when a live catalog row could not be verified as authoritative for
    /// this route. Present both when the row was *degraded* to the bundled
    /// snapshot (the estimate is still priced, from the bundled row) and when
    /// there was no fallback at all. It is the receipt for the downgrade, so a
    /// `provider_live` label is never claimed for an unproven row.
    pub live_pricing_defect: Option<LivePricingDefect>,
    /// Whether the estimate is authoritative in each carried currency. A zero
    /// amount is still priced when usage is zero; these flags therefore cannot
    /// be inferred from `estimate > 0`.
    pub usd_priced: bool,
    pub cny_priced: bool,
}

impl TurnCostAudit {
    fn priced(
        estimate: CostEstimate,
        provenance: PricingProvenance,
        usd_priced: bool,
        cny_priced: bool,
    ) -> Self {
        Self {
            estimate: Some(estimate),
            provenance: Some(provenance),
            unpriced_classes: Vec::new(),
            unpriced_reason: None,
            live_pricing_defect: None,
            usd_priced,
            cny_priced,
        }
    }

    pub(crate) fn unpriced(reason: UnpricedReason) -> Self {
        Self {
            estimate: None,
            provenance: None,
            unpriced_classes: Vec::new(),
            unpriced_reason: Some(reason),
            live_pricing_defect: None,
            usd_priced: false,
            cny_priced: false,
        }
    }

    fn missing_classes(provenance: PricingProvenance, classes: Vec<TokenClass>) -> Self {
        Self {
            estimate: None,
            provenance: Some(provenance),
            unpriced_classes: classes,
            unpriced_reason: Some(UnpricedReason::MissingClassPrice),
            live_pricing_defect: None,
            usd_priced: false,
            cny_priced: false,
        }
    }

    fn unverified_live(defect: LivePricingDefect) -> Self {
        Self {
            estimate: None,
            // Deliberately not `ProviderLive`: an unverified row must never be
            // labelled with authoritative live provenance.
            provenance: Some(PricingProvenance::Unknown),
            unpriced_classes: Vec::new(),
            unpriced_reason: Some(UnpricedReason::UnverifiedLivePricing),
            live_pricing_defect: Some(defect),
            usd_priced: false,
            cny_priced: false,
        }
    }

    /// Attach a live-pricing downgrade receipt to an otherwise complete audit.
    fn with_live_defect(mut self, defect: Option<LivePricingDefect>) -> Self {
        if let Some(defect) = defect {
            self.live_pricing_defect = Some(defect);
        }
        self
    }

    /// Whether this turn contributed an authoritative number to a total.
    #[must_use]
    #[cfg(test)]
    pub fn is_priced(&self) -> bool {
        self.estimate.is_some()
    }

    /// Whether the estimate is authoritative in the requested display
    /// currency. Exact zero remains priced; the boolean provenance flags are
    /// intentionally not inferred from the numeric amount.
    #[must_use]
    pub fn is_priced_in(&self, currency: CostCurrency) -> bool {
        self.estimate.is_some()
            && match currency {
                CostCurrency::Usd => self.usd_priced,
                CostCurrency::Cny => self.cny_priced,
            }
    }

    /// Whether this turn belongs in the money-metered coverage denominator.
    ///
    /// Priced turns always do. Unpriced ones do unless the route was *exactly*
    /// identified as non-metered.
    #[must_use]
    pub fn counts_toward_money_coverage(&self) -> bool {
        self.unpriced_reason
            .is_none_or(UnpricedReason::counts_toward_money_coverage)
    }
}

/// Audit a turn on a provider/model route, without knowing which endpoint served
/// it. A live catalog row cannot be *confirmed* for an unknown endpoint, so this
/// path degrades to the bundled published snapshot; use
/// [`audit_turn_cost_for_provider_on_endpoint_at`] when the base URL is known.
#[must_use]
pub(crate) fn audit_turn_cost_for_provider_at(
    provider: ApiProvider,
    model: &str,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
) -> TurnCostAudit {
    audit_turn_cost_for_provider_on_endpoint_at(provider, model, None, usage, recorded_at)
}

/// Audit a turn's cost on a provider/model route at its recorded time.
///
/// This is the single implementation; `calculate_turn_cost_estimate_*` are thin
/// projections of it, so no caller can build a total from one rule set while
/// reporting completeness from another.
#[must_use]
pub(crate) fn audit_turn_cost_for_provider_on_endpoint_at(
    provider: ApiProvider,
    model: &str,
    endpoint_fingerprint: Option<&str>,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
) -> TurnCostAudit {
    if !usage_cache_partition_is_consistent(usage) {
        return TurnCostAudit::unpriced(UnpricedReason::InconsistentUsage);
    }
    if provider == ApiProvider::OpenaiCodex {
        return TurnCostAudit::unpriced(UnpricedReason::NotMoneyMetered);
    }
    if route_requires_billing_surface(provider, model) {
        return TurnCostAudit::unpriced(UnpricedReason::AmbiguousBillingSurface);
    }
    let normalized_model = model.trim();
    let model_lower = normalized_model.to_ascii_lowercase();
    let direct_deepseek = matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    );
    let Some(canonical_model) = canonical_model_id_for_provider(provider, normalized_model) else {
        return TurnCostAudit::unpriced(UnpricedReason::NoPricingRow);
    };
    let catalog_model = if direct_deepseek
        && matches!(model_lower.as_str(), "deepseek-chat" | "deepseek-reasoner")
    {
        let Ok(retirement) = DateTime::parse_from_rfc3339(DEEPSEEK_ALIAS_RETIREMENT_UTC) else {
            return TurnCostAudit::unpriced(UnpricedReason::NoPricingRow);
        };
        if recorded_at >= retirement.with_timezone(&Utc) {
            return TurnCostAudit::unpriced(UnpricedReason::RetiredAlias);
        }
        DEEPSEEK_ALIAS_REPLACEMENT.to_string()
    } else {
        canonical_model
    };

    if direct_openai_long_context_tier_is_unpriced(provider, &catalog_model, usage.input_tokens) {
        return TurnCostAudit::unpriced(UnpricedReason::UnrepresentedTier);
    }

    // MiniMax-M3 doubles its published rates above 512K total input. The
    // catalog row is necessarily static, so retain the usage-aware first-party
    // table for both direct wire protocols after provider/model provenance has
    // been canonicalized.
    if matches!(
        provider,
        ApiProvider::Minimax | ApiProvider::MinimaxAnthropic
    ) && catalog_model.eq_ignore_ascii_case("minimax-m3")
    {
        return hand_priced_audit(pricing_for_model_and_usage(&catalog_model, usage), usage);
    }

    // xAI doubles Grok 4.6 input, cached-input, and output rates once the
    // prompt reaches 200K tokens. Keep this provider-owned and usage-aware so
    // a third-party route reusing the model slug never inherits xAI billing.
    if provider == ApiProvider::Xai && catalog_model.eq_ignore_ascii_case("grok-4.6") {
        return hand_priced_audit(pricing_for_model_and_usage(&catalog_model, usage), usage);
    }

    // Direct DeepSeek pricing carries an authoritative CNY row, and Sonnet 5
    // has a recorded-time introductory window that a static catalog row cannot
    // represent. These exact first-party routes intentionally override the
    // catalog; no other provider/model text match is allowed to do so.
    if direct_deepseek
        || (provider == ApiProvider::Anthropic
            && catalog_model.eq_ignore_ascii_case("claude-sonnet-5"))
    {
        return hand_priced_audit(
            provider_owned_hand_pricing_at(provider, &catalog_model, recorded_at),
            usage,
        );
    }

    let classes = token_usage_for_pricing(usage);
    // A live catalog row is only authoritative when it is fresh *and* was
    // fetched from the endpoint this turn was served on. When it is not, degrade
    // to the bundled published snapshot and receipt the defect; only if there is
    // no bundled row at all does the turn fail closed (#4318).
    let mut live_defect = None;
    let offering = match verified_catalog_offering(
        provider,
        &catalog_model,
        endpoint_fingerprint,
        recorded_at,
    ) {
        VerifiedOffering::Usable(offering) => Some(offering),
        VerifiedOffering::DegradedToBundled { offering, defect } => {
            live_defect = Some(defect);
            Some(offering)
        }
        VerifiedOffering::Unusable(defect) => {
            live_defect = Some(defect);
            None
        }
        VerifiedOffering::Absent => None,
    };

    if let Some(audit) = offering.as_ref().and_then(invalid_catalog_pricing_audit) {
        return audit.with_live_defect(live_defect);
    }

    if let Some(offering) = offering.as_ref()
        && let Some(pricing) =
            effective_offering_pricing(provider, &catalog_model, offering, &classes)
    {
        if let Some(estimate) =
            catalog_cost_estimate_for_route(provider, &catalog_model, offering, usage)
        {
            let (usd_priced, cny_priced) = match pricing.currency {
                Currency::Usd => (true, false),
                Currency::Cny => (false, true),
                Currency::Other(_) => (false, false),
            };
            return TurnCostAudit::priced(
                estimate,
                pricing.provenance.clone(),
                usd_priced,
                cny_priced,
            )
            .with_live_defect(live_defect);
        }
        let classes = pricing.unpriced_used_classes(&classes);
        if classes.is_empty() {
            // Every used class is priced, so the only way the estimate failed
            // is a currency CodeWhale does not carry. Never convert.
            return TurnCostAudit::unpriced(UnpricedReason::UnsupportedCurrency)
                .with_live_defect(live_defect);
        }
        return TurnCostAudit::missing_classes(pricing.provenance, classes)
            .with_live_defect(live_defect);
    }

    // A few first-party rows predate or intentionally omit a Models.dev entry
    // (for example OpenAI API `gpt-5-codex` and MiniMax `minimax-m2.7`).
    // Preserve only an explicit provider-owned allowlist here;
    // a costless foreign/catalog route must remain unpriced.
    let hand_row = provider_owned_hand_pricing_at(provider, &catalog_model, recorded_at);

    // An unverifiable live row with no bundled fallback and no hand row is a
    // route CodeWhale cannot price truthfully. Say which, rather than reporting
    // the unverified rate or a bare "no pricing row".
    match (live_defect, hand_row) {
        (Some(defect), None) => TurnCostAudit::unverified_live(defect),
        (defect, hand_row) => hand_priced_audit(hand_row, usage).with_live_defect(defect),
    }
}

/// Convert malformed catalog numerics into an explicit runtime audit reason.
/// Keeping this distinct from the ordinary `None` projection prevents a bad
/// published row from becoming indistinguishable from an absent price.
fn invalid_catalog_pricing_audit(
    offering: &codewhale_config::catalog::CatalogOffering,
) -> Option<TurnCostAudit> {
    offering
        .cost
        .as_ref()
        .is_some_and(|cost| !codewhale_config::pricing::catalog_cost_is_valid(cost))
        .then(|| TurnCostAudit::unpriced(UnpricedReason::InvalidPricingRow))
}

/// Outcome of checking a catalog row's pricing provenance against the route.
enum VerifiedOffering {
    /// The row is authoritative as-is (bundled, user override, or a live row
    /// proven fresh and endpoint-matched).
    Usable(codewhale_config::catalog::CatalogOffering),
    /// The live row could not be verified, so the bundled published row is used
    /// instead. The defect is retained as the receipt for why.
    DegradedToBundled {
        offering: codewhale_config::catalog::CatalogOffering,
        defect: LivePricingDefect,
    },
    /// The live row could not be verified and no bundled row exists.
    Unusable(LivePricingDefect),
    /// No catalog row for this provider/model at all.
    Absent,
}

/// Resolve the catalog row to price against, refusing to treat an unverifiable
/// live row as authoritative.
///
/// `endpoint_fingerprint` is the non-secret SHA-256 digest of the base URL the turn
/// was actually served on (see [`codewhale_config::catalog::base_url_fingerprint`]).
/// Callers that do not know the endpoint pass `None`, which cannot *confirm* a
/// live row — so those callers degrade to the bundled snapshot rather than
/// billing against a rate whose endpoint scope is unproven.
fn verified_catalog_offering(
    provider: ApiProvider,
    catalog_model: &str,
    endpoint_fingerprint: Option<&str>,
    recorded_at: DateTime<Utc>,
) -> VerifiedOffering {
    let Some(offering) = crate::provider_lake::catalog_offering_for_model(provider, catalog_model)
    else {
        return VerifiedOffering::Absent;
    };
    let Some(pricing) = OfferingPricing::from_catalog_offering(&offering) else {
        // No priced row to verify; downstream treats this as unpriced.
        return VerifiedOffering::Usable(offering);
    };
    // `recorded_at` is the turn's own clock, which is the right reference for
    // "was this price current when the turn happened".
    let now_unix = u64::try_from(recorded_at.timestamp()).ok();
    let Some(defect) =
        pricing.live_pricing_defect(endpoint_fingerprint, now_unix, LIVE_PRICING_MAX_AGE_SECS)
    else {
        return VerifiedOffering::Usable(offering);
    };
    match crate::provider_lake::bundled_catalog_offering_for_model(provider, catalog_model) {
        Some(bundled) => VerifiedOffering::DegradedToBundled {
            offering: bundled,
            defect,
        },
        None => VerifiedOffering::Unusable(defect),
    }
}

/// Project a hand-sourced provider row into an audit.
///
/// A hand row always publishes input, cache-read, and output rates. Cache-write
/// is the one class that can be genuinely absent: only providers that publish a
/// write premium, or document that cache creation carries no separate charge,
/// can price it. A turn that wrote to cache on a row with neither fact fails
/// closed and names the class, rather than being billed at the input rate on the
/// strength of an assumption (#4318).
fn hand_priced_audit(pricing: Option<ModelPricing>, usage: &Usage) -> TurnCostAudit {
    let Some(pricing) = pricing else {
        return TurnCostAudit::unpriced(UnpricedReason::NoPricingRow);
    };
    let has_cny = pricing.cny.is_some();
    match cost_estimate_with_pricing_checked(pricing, usage) {
        Ok(estimate) => {
            TurnCostAudit::priced(estimate, PricingProvenance::ProviderDocs, true, has_cny)
        }
        Err(classes) => TurnCostAudit::missing_classes(PricingProvenance::ProviderDocs, classes),
    }
}

/// Recorded-time variant with explicit billing-surface provenance.
#[must_use]
#[cfg(test)]
pub(crate) fn calculate_turn_cost_estimate_for_route_at(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
) -> Option<CostEstimate> {
    audit_turn_cost_for_route_at(provider, model, billing_surface, usage, recorded_at).estimate
}

/// Audit a turn's cost with endpoint-derived billing provenance.
#[must_use]
pub(crate) fn audit_turn_cost_for_route_at(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
) -> TurnCostAudit {
    audit_turn_cost_for_route_on_endpoint_at(
        provider,
        model,
        billing_surface,
        None,
        usage,
        recorded_at,
    )
}

/// Audit a turn's cost with both endpoint-derived billing provenance and the
/// endpoint fingerprint needed to verify live catalog pricing.
#[must_use]
pub(crate) fn audit_turn_cost_for_route_on_endpoint_at(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
    endpoint_fingerprint: Option<&str>,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
) -> TurnCostAudit {
    // An explicitly recorded surface is evidence.  Exact non-metered surfaces
    // override provider guesses; an explicit unknown/unrecognized surface must
    // fail closed and may never fall through to a familiar model's hand row.
    match endpoint_metering_for_billing_surface(billing_surface) {
        EndpointMetering::ExactSubscription | EndpointMetering::LocalNoBill => {
            return TurnCostAudit::unpriced(UnpricedReason::NotMoneyMetered);
        }
        EndpointMetering::Unknown if billing_surface.is_some() => {
            return TurnCostAudit::unpriced(UnpricedReason::UnknownBillingBasis);
        }
        EndpointMetering::Unknown | EndpointMetering::Money => {}
    }
    if !usage_cache_partition_is_consistent(usage) {
        return TurnCostAudit::unpriced(UnpricedReason::InconsistentUsage);
    }
    if provider == ApiProvider::Stepfun {
        return match pricing_for_billing_surface(provider, model, billing_surface) {
            // StepFun's hand row publishes no cache-write rate, so a turn that
            // wrote to cache fails closed here as well.
            Some(pricing) => match cost_estimate_with_pricing_checked(pricing, usage) {
                Ok(estimate) => {
                    TurnCostAudit::priced(estimate, PricingProvenance::ProviderDocs, true, false)
                }
                Err(classes) => {
                    TurnCostAudit::missing_classes(PricingProvenance::ProviderDocs, classes)
                }
            },
            // The surface classified as per-token but no rates exist for it, or
            // no surface was established at all.
            None => TurnCostAudit::unpriced(match billing_surface {
                Some(_) => UnpricedReason::UnpricedBillingSurface,
                None => UnpricedReason::AmbiguousBillingSurface,
            }),
        };
    }
    if model.trim().eq_ignore_ascii_case(DEFAULT_STEPFUN_MODEL) {
        return TurnCostAudit::unpriced(UnpricedReason::AmbiguousBillingSurface);
    }
    // This is the *route* audit: the caller is asserting it knows which
    // endpoint served the turn. With no classification at all, nothing
    // distinguishes the provider's own official surface from a proxy, a
    // gateway, or a self-hosted clone speaking the same protocol — a provider
    // enum plus a familiar model id is not evidence of an official endpoint.
    // So the turn prices as unknown rather than at official rates.
    //
    // Callers that genuinely hold only a provider and a model use
    // `audit_turn_cost_for_provider_*`, which says so in its name and carries
    // its own weaker claim.
    if billing_surface.is_none() {
        return TurnCostAudit::unpriced(UnpricedReason::UnestablishedEndpoint);
    }
    audit_turn_cost_for_provider_on_endpoint_at(
        provider,
        model,
        endpoint_fingerprint,
        usage,
        recorded_at,
    )
}

/// Audit a turn against the route's billing presentation.
///
/// The three non-metered presentations are **not** interchangeable, and
/// collapsing them was the bug (#4318):
///
/// - [`BillingPresentation::Subscription`] and [`BillingPresentation::Local`]
///   are exact evidence that money is the wrong unit, so those turns are
///   `NotMoneyMetered` and drop out of the coverage denominator.
/// - [`BillingPresentation::Unknown`] is *not* such evidence. It means CodeWhale
///   could not establish the basis, so the turn is `UnknownBillingBasis`: still
///   unpriced, but counted as spend the total may be missing.
///
/// [`BillingPresentation::Subscription`]: crate::route_billing::BillingPresentation::Subscription
/// [`BillingPresentation::Local`]: crate::route_billing::BillingPresentation::Local
/// [`BillingPresentation::Unknown`]: crate::route_billing::BillingPresentation::Unknown
#[must_use]
#[cfg(test)]
pub fn audit_turn_cost_for_route(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
    billing: crate::route_billing::BillingPresentation,
) -> TurnCostAudit {
    audit_turn_cost_for_route_on_endpoint(
        provider,
        model,
        billing_surface,
        None,
        usage,
        recorded_at,
        billing,
    )
}

/// [`audit_turn_cost_for_route`] plus the endpoint fingerprint that lets live
/// catalog pricing be verified for this exact route.
#[must_use]
#[cfg(test)]
pub fn audit_turn_cost_for_route_on_endpoint(
    provider: ApiProvider,
    model: &str,
    billing_surface: Option<&str>,
    endpoint_fingerprint: Option<&str>,
    usage: &Usage,
    recorded_at: DateTime<Utc>,
    billing: crate::route_billing::BillingPresentation,
) -> TurnCostAudit {
    use crate::route_billing::BillingPresentation;
    match billing {
        BillingPresentation::Subscription(_) | BillingPresentation::Local => {
            return TurnCostAudit::unpriced(UnpricedReason::NotMoneyMetered);
        }
        BillingPresentation::Unknown => {
            return TurnCostAudit::unpriced(UnpricedReason::UnknownBillingBasis);
        }
        BillingPresentation::Metered => {}
    }
    // A metered presentation still has to survive the endpoint classification:
    // an endpoint that classifies as an exact subscription surface overrides a
    // metered guess, and an unclassifiable one fails closed.
    match endpoint_metering_for_billing_surface(billing_surface) {
        EndpointMetering::ExactSubscription | EndpointMetering::LocalNoBill => {
            return TurnCostAudit::unpriced(UnpricedReason::NotMoneyMetered);
        }
        // `Unknown` here is the common, benign case of a caller that has no
        // endpoint to classify; the provider/model path below still decides.
        EndpointMetering::Unknown if billing_surface.is_some() => {
            return TurnCostAudit::unpriced(UnpricedReason::UnknownBillingBasis);
        }
        EndpointMetering::Unknown | EndpointMetering::Money => {}
    }
    audit_turn_cost_for_route_on_endpoint_at(
        provider,
        model,
        billing_surface,
        endpoint_fingerprint,
        usage,
        recorded_at,
    )
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
        ApiProvider::Xai => model_lower == "grok-4.6",
        // GLM-5.3 is deliberately absent: this allowlist declares that Z.ai
        // owns a *hand-written price row* for the model, and no GLM-5.3 rate
        // has been published. An absent price is honest; an owned-but-empty
        // row is not. See `glm_5_3_has_no_hardcoded_price` below.
        ApiProvider::Zai => matches!(model_lower.as_str(), "glm-5.1" | "glm-5.2" | "glm-5-turbo"),
        ApiProvider::Moonshot => {
            matches!(model_lower.as_str(), "kimi-k2.6" | "kimi-k2.7-code")
        }
        ApiProvider::Minimax | ApiProvider::MinimaxAnthropic => {
            matches!(model_lower.as_str(), "minimax-m3" | "minimax-m2.7")
        }
        ApiProvider::Arcee => model_lower == "trinity-large-thinking",
        // 1.2 and its contributor tier own hand-written rows the same way 1.1
        // does (see `pricing_for_model_at`). 1.2 is now `DEFAULT_META_MODEL`,
        // so omitting them here left the default Meta route without a
        // provider-owned fallback row.
        ApiProvider::Meta => matches!(
            model_lower.as_str(),
            "muse-spark-1.1" | "muse-spark-1.2" | "muse-spark-1.2-contributor"
        ),
        // Deployment-style ids (Fireworks account prefix, OpenCode Zen
        // gateway) have no Models.dev cost fields. When the live control
        // plane 503s, these bundled family rates keep the session priced
        // instead of `unverified_live_pricing` forever (#5241).
        ApiProvider::Fireworks => {
            let bare = model_lower
                .strip_prefix("accounts/fireworks/models/")
                .unwrap_or(model_lower.as_str());
            matches!(bare, "deepseek-v4-flash" | "deepseek-v4-pro")
        }
        ApiProvider::OpencodeZen => {
            matches!(
                model_lower.as_str(),
                "deepseek-v4-flash" | "deepseek-v4-pro"
            )
        }
        _ => false,
    };
    let lookup = if provider == ApiProvider::Fireworks {
        model_lower
            .strip_prefix("accounts/fireworks/models/")
            .unwrap_or(model_lower.as_str())
            .to_string()
    } else {
        model_lower
    };
    provider_owns_row
        .then(|| pricing_for_model_at(&lookup, recorded_at))
        .flatten()
}

/// The offering's pricing row as it actually applies to this route.
///
/// Two documented first-party routes publish no separate cache rate *because*
/// cache tokens are billed at the plain input rate; that substitution happens
/// here so cost estimation and the unpriced-class audit read the same row.
fn effective_offering_pricing(
    provider: ApiProvider,
    model: &str,
    offering: &codewhale_config::catalog::CatalogOffering,
    classes: &TokenUsage,
) -> Option<OfferingPricing> {
    let mut pricing = OfferingPricing::from_catalog_offering(offering)?;
    let model_lower = model.trim().to_ascii_lowercase();
    let cache_uses_input_rate = matches!(
        (provider, model_lower.as_str()),
        (ApiProvider::Openai, "gpt-5.5-pro") | (ApiProvider::Arcee, "trinity-large-thinking")
    );
    if cache_uses_input_rate {
        if classes.cache_read > 0 && pricing.cache_read_per_million.is_none() {
            pricing.cache_read_per_million = pricing.input_per_million;
        }
        if classes.cache_write > 0 && pricing.cache_write_per_million.is_none() {
            pricing.cache_write_per_million = pricing.input_per_million;
        }
    }
    Some(pricing)
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
    let classes = token_usage_for_pricing(usage);
    let pricing = effective_offering_pricing(provider, model, offering, &classes)?;

    let amount = pricing.estimate_cost(&classes)?;
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
///
/// `Usage::reasoning_tokens` is deliberately **not** added to the billable
/// output. Every provider CodeWhale normalizes reports reasoning as a *subset*
/// of the completion count it already bills — OpenAI Responses nests
/// `reasoning_tokens` under `output_tokens_details` while `output_tokens` is
/// the total, and Chat Completions nests it under `completion_tokens_details`
/// while `completion_tokens` is the total. Adding it charged reasoning turns
/// twice for the same tokens (up to 2x on reasoning-heavy turns). It stays on
/// `Usage` as informational telemetry (`/usage`, hooks, sub-agent metadata).
#[must_use]
pub fn token_usage_for_pricing(usage: &Usage) -> TokenUsage {
    // `input_tokens` is the authoritative total. Even malformed provider
    // telemetry must never produce token classes whose sum exceeds it. The
    // audit path rejects contradictory partitions; this bounded projection
    // keeps token-only displays truthful while retaining deterministic class
    // priority (read, write, then miss/unclassified input).
    let total_input = usage.input_tokens;
    let cache_read = usage.prompt_cache_hit_tokens.unwrap_or(0).min(total_input);
    let after_read = total_input.saturating_sub(cache_read);
    let cache_write = usage.prompt_cache_write_tokens.unwrap_or(0).min(after_read);
    let after_write = after_read.saturating_sub(cache_write);
    let non_cached_reported = usage
        .prompt_cache_miss_tokens
        .unwrap_or(after_write)
        .min(after_write);
    let uncategorized_input = after_write.saturating_sub(non_cached_reported);
    let input = non_cached_reported.saturating_add(uncategorized_input);
    // Reasoning tokens are already inside `output_tokens`; see the doc comment.
    let output = usage.output_tokens;

    TokenUsage {
        input: u64::from(input),
        output: u64::from(output),
        cache_read: u64::from(cache_read),
        cache_write: u64::from(cache_write),
    }
}

fn usage_cache_partition_is_consistent(usage: &Usage) -> bool {
    let reported = u64::from(usage.prompt_cache_hit_tokens.unwrap_or(0))
        + u64::from(usage.prompt_cache_miss_tokens.unwrap_or(0))
        + u64::from(usage.prompt_cache_write_tokens.unwrap_or(0));
    reported <= u64::from(usage.input_tokens)
}

fn calculate_turn_cost_from_usage_with_pricing(pricing: CurrencyPricing, usage: &Usage) -> f64 {
    let usage = token_usage_for_pricing(usage);
    let hit_cost = (usage.cache_read as f64 / 1_000_000.0) * pricing.input_cache_hit_per_million;
    let miss_cost = (usage.input as f64 / 1_000_000.0) * pricing.input_cache_miss_per_million;
    // An unpublished write policy is only reachable here for usage with zero
    // cache-write tokens; `cost_estimate_with_pricing_checked` rejects the rest
    // before any money is computed.
    let write_rate = pricing
        .cache_write
        .rate(pricing.input_cache_miss_per_million)
        .unwrap_or(0.0);
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

/// Format a cost amount for compact display in the chosen currency.
#[must_use]
pub fn format_cost_amount(cost: f64, currency: CostCurrency) -> String {
    let symbol = currency.symbol();
    if cost == 0.0 {
        format!("{symbol}0.00")
    } else if cost > 0.0 && cost < 0.0001 {
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
    if cost == 0.0 {
        format!("{symbol}0.0000")
    } else if cost > 0.0 && cost < 0.0001 {
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
    fn malformed_catalog_row_has_an_explicit_runtime_reason() {
        let offering = codewhale_config::catalog::CatalogOffering {
            provider: "openrouter".to_string(),
            wire_model_id: "openai/gpt-5.5".to_string(),
            cost: Some(codewhale_config::models_dev::ModelsDevCost {
                input: Some(f64::NAN),
                output: Some(30.0),
                cache_read: Some(0.05),
                cache_write: None,
            }),
            ..Default::default()
        };

        let audit = invalid_catalog_pricing_audit(&offering)
            .expect("malformed row must become an explicit failed-closed audit");
        assert!(!audit.is_priced());
        assert_eq!(
            audit.unpriced_reason,
            Some(UnpricedReason::InvalidPricingRow)
        );
        assert_eq!(
            audit.unpriced_reason.unwrap().label(),
            "invalid_pricing_row"
        );
    }

    /// A hand-sourced row with **no published** cache-write rate must fail closed
    /// for a turn that wrote to cache, while a row whose provider *documents*
    /// that writes carry no separate charge prices it at the input rate.
    ///
    /// Both used to be `None` and both silently billed writes at the input rate,
    /// which invented a price for the first case (#4318).
    #[test]
    fn unpublished_cache_write_fails_closed_but_documented_same_rate_prices() {
        let write_heavy = Usage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            prompt_cache_hit_tokens: Some(0),
            prompt_cache_miss_tokens: Some(900_000),
            prompt_cache_write_tokens: Some(100_000),
            ..Usage::default()
        };
        let now = Utc::now();

        // DeepSeek documents that a cache miss is billed once and cached for
        // free, so the miss rate *is* the published write rate. The policy
        // carries the documentation receipt rather than being an assumption.
        let deepseek = deepseek_v4_flash_pricing();
        assert_eq!(
            deepseek.usd.cache_write,
            CacheWritePolicy::DocumentedAsInputRate(DEEPSEEK_CACHE_WRITE_IS_FREE)
        );
        let priced = audit_turn_cost_for_provider_at(
            ApiProvider::Deepseek,
            "deepseek-v4-flash",
            &write_heavy,
            now,
        );
        assert!(priced.is_priced(), "{priced:?}");
        // 900k miss + 100k write, both at the 0.14/M miss rate.
        let expected = (0.9 + 0.1) * 0.14;
        assert!(
            (priced.estimate.expect("priced").usd - expected).abs() < 1e-12,
            "{priced:?}"
        );

        // StepFun's hand row publishes input/cache-read/output only. A write
        // turn is unpriced and names the class instead of borrowing the input
        // rate.
        let stepfun = pricing_for_billing_surface(
            ApiProvider::Stepfun,
            DEFAULT_STEPFUN_MODEL,
            Some(STEPFUN_PAYG_BILLING_SURFACE),
        )
        .expect("StepFun PAYG row");
        assert_eq!(stepfun.usd.cache_write, CacheWritePolicy::Unpublished);
        let failed = audit_turn_cost_for_route_at(
            ApiProvider::Stepfun,
            DEFAULT_STEPFUN_MODEL,
            Some(STEPFUN_PAYG_BILLING_SURFACE),
            &write_heavy,
            now,
        );
        assert!(!failed.is_priced(), "{failed:?}");
        assert_eq!(
            failed.unpriced_reason,
            Some(UnpricedReason::MissingClassPrice)
        );
        assert_eq!(failed.unpriced_classes, vec![TokenClass::CacheWrite]);

        // The same route with no cache-write tokens prices normally, proving the
        // gap is class-scoped rather than route-scoped.
        let no_write = Usage {
            prompt_cache_write_tokens: None,
            ..write_heavy.clone()
        };
        assert!(
            audit_turn_cost_for_route_at(
                ApiProvider::Stepfun,
                DEFAULT_STEPFUN_MODEL,
                Some(STEPFUN_PAYG_BILLING_SURFACE),
                &no_write,
                now,
            )
            .is_priced()
        );
    }

    /// Every exact billing surface a route can carry must be understood, and
    /// anything unrecognized must fail closed as unknown rather than defaulting
    /// into per-token dollars (#4318).
    #[test]
    fn endpoint_classification_covers_every_exact_billing_surface() {
        for (provider, base_url, expected_surface, expected_metering) in [
            (
                ApiProvider::Zai,
                "https://api.z.ai/api/coding/paas/v4",
                ZAI_CODING_PLAN_BILLING_SURFACE,
                EndpointMetering::ExactSubscription,
            ),
            (
                ApiProvider::Zai,
                "https://api.z.ai/api/paas/v4",
                ZAI_PAYG_BILLING_SURFACE,
                EndpointMetering::Money,
            ),
            (
                ApiProvider::Moonshot,
                crate::config::DEFAULT_KIMI_CODE_BASE_URL,
                MOONSHOT_KIMI_CODE_BILLING_SURFACE,
                EndpointMetering::ExactSubscription,
            ),
            (
                ApiProvider::Moonshot,
                "https://api.moonshot.ai/v1",
                MOONSHOT_PAYG_BILLING_SURFACE,
                EndpointMetering::Money,
            ),
            (
                ApiProvider::XiaomiMimo,
                crate::config::XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL,
                XIAOMI_PAYG_BILLING_SURFACE,
                EndpointMetering::Money,
            ),
            (
                ApiProvider::XiaomiMimo,
                crate::config::DEFAULT_XIAOMI_MIMO_BASE_URL,
                XIAOMI_TOKEN_PLAN_BILLING_SURFACE,
                EndpointMetering::ExactSubscription,
            ),
            (
                ApiProvider::Stepfun,
                "https://api.stepfun.ai/step_plan/v1",
                STEPFUN_PLAN_BILLING_SURFACE,
                EndpointMetering::ExactSubscription,
            ),
            (
                ApiProvider::Stepfun,
                "https://api.stepfun.ai/v1",
                STEPFUN_PAYG_BILLING_SURFACE,
                EndpointMetering::Money,
            ),
            (
                ApiProvider::Anthropic,
                "https://api.anthropic.com/v1",
                FIRST_PARTY_PAYG_BILLING_SURFACE,
                EndpointMetering::Money,
            ),
            (
                ApiProvider::Openrouter,
                "https://openrouter.ai/api/v1",
                AGGREGATOR_BILLING_SURFACE,
                EndpointMetering::Money,
            ),
        ] {
            let surface = billing_surface_for_route(provider, Some(base_url));
            assert_eq!(surface, Some(expected_surface), "{provider:?} {base_url}");
            assert_eq!(
                endpoint_metering_for_billing_surface(surface),
                expected_metering,
                "{provider:?} {base_url}"
            );
        }

        // Provider-intrinsic surfaces need no URL at all.
        for (provider, expected_surface, expected_metering) in [
            (
                ApiProvider::OpenaiCodex,
                OAUTH_SUBSCRIPTION_BILLING_SURFACE,
                EndpointMetering::ExactSubscription,
            ),
            (
                ApiProvider::OpencodeGo,
                OAUTH_SUBSCRIPTION_BILLING_SURFACE,
                EndpointMetering::ExactSubscription,
            ),
            (
                ApiProvider::Ollama,
                LOCAL_BILLING_SURFACE,
                EndpointMetering::LocalNoBill,
            ),
            (
                ApiProvider::OllamaCloud,
                UNCLASSIFIED_BILLING_SURFACE,
                EndpointMetering::Unknown,
            ),
            (
                ApiProvider::Vllm,
                LOCAL_BILLING_SURFACE,
                EndpointMetering::LocalNoBill,
            ),
            // A named custom endpoint's pay mode is config, not URL shape.
            (
                ApiProvider::Custom,
                UNCLASSIFIED_BILLING_SURFACE,
                EndpointMetering::Unknown,
            ),
        ] {
            let surface = billing_surface_for_route(provider, None);
            assert_eq!(surface, Some(expected_surface), "{provider:?}");
            assert_eq!(
                endpoint_metering_for_billing_surface(surface),
                expected_metering,
                "{provider:?}"
            );
        }

        // An unrecognized surface id — including one a newer build might write —
        // is never guessed into a known bucket.
        for unknown in [
            Some("some-future-surface"),
            Some(""),
            Some("   "),
            Some(UNCLASSIFIED_BILLING_SURFACE),
            None,
        ] {
            assert_eq!(
                endpoint_metering_for_billing_surface(unknown),
                EndpointMetering::Unknown,
                "{unknown:?}"
            );
        }
    }

    /// An endpoint that was never established is not the official endpoint.
    ///
    /// The route audit used to fall through to the provider/model catalog when
    /// no billing surface was supplied, which meant a persisted or recorded row
    /// carrying nothing but `provider: "openai"` and a familiar model id got
    /// billed at OpenAI's published first-party rates — even though the turn
    /// could equally have been served by a proxy, a gateway, or a self-hosted
    /// clone speaking the same protocol. Absence of endpoint evidence is not
    /// evidence of the official endpoint.
    #[test]
    fn an_unestablished_endpoint_is_never_priced_as_the_official_one() {
        let usage = Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Usage::default()
        };
        let now = Utc::now();
        for (provider, model) in [
            (ApiProvider::Openai, "gpt-5.5"),
            (ApiProvider::Anthropic, "claude-haiku-4-5"),
            (ApiProvider::Deepseek, "deepseek-v4-flash"),
            (ApiProvider::Openrouter, "openai/gpt-5.5"),
            (ApiProvider::Moonshot, "kimi-k2.7-code"),
        ] {
            let audit = audit_turn_cost_for_route_at(provider, model, None, &usage, now);
            assert_eq!(
                audit.unpriced_reason,
                Some(UnpricedReason::UnestablishedEndpoint),
                "{provider:?}/{model}: {audit:?}"
            );
            assert!(!audit.is_priced(), "{provider:?}/{model}: {audit:?}");
            assert_eq!(audit.estimate, None, "{provider:?}/{model}");
            // An unknown route is still possibly-spent money, so it stays in
            // the coverage denominator rather than being excused like an OAuth
            // or local route.
            assert!(
                audit.counts_toward_money_coverage(),
                "{provider:?}/{model}: an unknown route must not leave money coverage"
            );

            // The same route with its endpoint actually classified prices
            // normally: this is a fail-closed rule, not a refusal to price.
            // (OpenRouter is excluded here only because its aggregator surface
            // carries no bundled rate at all, which is a different gap.)
            if provider == ApiProvider::Openrouter {
                continue;
            }
            let classified = audit_turn_cost_for_route_at(
                provider,
                model,
                billing_surface_for_route(provider, Some(provider.default_base_url())),
                &usage,
                now,
            );
            assert!(
                classified.is_priced(),
                "{provider:?}/{model} must price on its own official endpoint: {classified:?}"
            );
        }

        // The distinction is preserved end to end: "no endpoint offered" and
        // "endpoint offered but unplaceable" are different findings, and
        // neither is a price.
        let unplaceable = audit_turn_cost_for_route_at(
            ApiProvider::Openai,
            "gpt-5.5",
            billing_surface_for_route(ApiProvider::Openai, Some("https://proxy.example/v1")),
            &usage,
            now,
        );
        assert_eq!(
            unplaceable.unpriced_reason,
            Some(UnpricedReason::UnknownBillingBasis)
        );
    }

    #[test]
    fn builtin_provider_names_do_not_price_unofficial_proxy_endpoints() {
        let usage = Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Usage::default()
        };
        for (provider, model) in [
            (ApiProvider::Deepseek, "deepseek-v4-flash"),
            (ApiProvider::Openai, "gpt-5.5"),
            (ApiProvider::Anthropic, "claude-haiku-4-5"),
            (ApiProvider::Openrouter, "openai/gpt-5.5"),
        ] {
            let surface = billing_surface_for_route(provider, Some("https://proxy.example/v1"));
            assert_eq!(surface, Some(UNCLASSIFIED_BILLING_SURFACE), "{provider:?}");
            let audit = audit_turn_cost_for_route_at(provider, model, surface, &usage, Utc::now());
            assert_eq!(
                audit.unpriced_reason,
                Some(UnpricedReason::UnknownBillingBasis),
                "{provider:?}: {audit:?}"
            );
            assert!(!audit.is_priced(), "{provider:?}: {audit:?}");
        }

        assert_eq!(
            billing_surface_for_route(
                ApiProvider::Moonshot,
                Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL)
            ),
            Some(MOONSHOT_KIMI_CODE_BILLING_SURFACE)
        );
        for (provider, endpoint) in [
            (ApiProvider::Minimax, "https://api.minimax.io/v1"),
            (
                ApiProvider::MinimaxAnthropic,
                "https://api.minimax.io/anthropic",
            ),
            (ApiProvider::Minimax, "https://api.minimax.io/v1/token-plan"),
            (
                ApiProvider::XiaomiMimo,
                "https://token-plan-proxy.example/v1",
            ),
            (
                ApiProvider::Zai,
                "https://api.z.ai/api/coding/something-else",
            ),
        ] {
            assert_eq!(
                billing_surface_for_route(provider, Some(endpoint)),
                Some(UNCLASSIFIED_BILLING_SURFACE),
                "{provider:?} {endpoint}"
            );
        }
    }

    /// A route classified as an exact subscription surface is not money-metered
    /// even when the provider-level presentation guessed "metered", and it must
    /// never reach a per-token rate.
    #[test]
    fn exact_plan_surface_overrides_a_metered_presentation() {
        let usage = Usage {
            input_tokens: 100_000,
            output_tokens: 10_000,
            ..Usage::default()
        };
        let audit = audit_turn_cost_for_route(
            ApiProvider::Zai,
            "glm-5.2",
            Some(ZAI_CODING_PLAN_BILLING_SURFACE),
            &usage,
            Utc::now(),
            crate::route_billing::BillingPresentation::Metered,
        );
        assert!(!audit.is_priced(), "{audit:?}");
        assert_eq!(audit.unpriced_reason, Some(UnpricedReason::NotMoneyMetered));
        assert!(!audit.counts_toward_money_coverage());

        // The same model on the per-token surface is money-metered, so it stays
        // in the coverage denominator whether or not a price is found.
        let payg = audit_turn_cost_for_route(
            ApiProvider::Zai,
            "glm-5.2",
            Some(ZAI_PAYG_BILLING_SURFACE),
            &usage,
            Utc::now(),
            crate::route_billing::BillingPresentation::Metered,
        );
        assert!(payg.counts_toward_money_coverage(), "{payg:?}");
    }

    /// An unknown billing basis is *not* a subscription. It stays unpriced and
    /// stays inside the money-coverage denominator, so its spend is reported as
    /// missing rather than excused (#4318).
    #[test]
    fn unknown_billing_basis_is_not_excused_as_not_money_metered() {
        let usage = Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Usage::default()
        };
        let unknown = audit_turn_cost_for_route(
            ApiProvider::Anthropic,
            "claude-haiku-4-5",
            None,
            &usage,
            Utc::now(),
            crate::route_billing::BillingPresentation::Unknown,
        );
        assert!(!unknown.is_priced());
        assert_eq!(
            unknown.unpriced_reason,
            Some(UnpricedReason::UnknownBillingBasis)
        );
        assert!(unknown.counts_toward_money_coverage());

        // Local and subscription presentations are exact, so they *are* excused.
        for billing in [
            crate::route_billing::BillingPresentation::Local,
            crate::route_billing::BillingPresentation::Subscription("plan"),
        ] {
            let audit = audit_turn_cost_for_route(
                ApiProvider::Anthropic,
                "claude-haiku-4-5",
                None,
                &usage,
                Utc::now(),
                billing,
            );
            assert_eq!(audit.unpriced_reason, Some(UnpricedReason::NotMoneyMetered));
            assert!(!audit.counts_toward_money_coverage());
        }
    }

    #[test]
    fn audit_names_why_a_turn_is_missing_from_a_total() {
        let write_heavy = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            prompt_cache_hit_tokens: Some(200_000),
            prompt_cache_write_tokens: Some(100_000),
            ..Usage::default()
        };

        // Anthropic publishes a cache-write rate: fully priced, provenance kept.
        let priced = audit_turn_cost_for_provider_at(
            ApiProvider::Anthropic,
            "claude-haiku-4-5",
            &write_heavy,
            Utc::now(),
        );
        assert!(priced.is_priced());
        assert_eq!(priced.unpriced_reason, None);
        assert!(priced.unpriced_classes.is_empty());
        assert!(priced.provenance.is_some());

        // Moonshot does not: the turn fails closed and names the class.
        let missing = audit_turn_cost_for_provider_at(
            ApiProvider::Moonshot,
            "kimi-k2.7-code",
            &write_heavy,
            Utc::now(),
        );
        assert!(!missing.is_priced());
        assert_eq!(
            missing.unpriced_reason,
            Some(UnpricedReason::MissingClassPrice)
        );
        assert_eq!(missing.unpriced_classes, vec![TokenClass::CacheWrite]);
        // Dropping the write tokens makes the very same route priceable, which
        // proves the gap is class-scoped rather than route-scoped.
        let no_write = Usage {
            prompt_cache_write_tokens: None,
            ..write_heavy.clone()
        };
        assert!(
            audit_turn_cost_for_provider_at(
                ApiProvider::Moonshot,
                "kimi-k2.7-code",
                &no_write,
                Utc::now(),
            )
            .is_priced()
        );

        // Subscription/OAuth and ambiguous-surface routes report their own
        // reasons rather than an absent price.
        assert_eq!(
            audit_turn_cost_for_provider_at(
                ApiProvider::OpenaiCodex,
                "gpt-5.5",
                &write_heavy,
                Utc::now(),
            )
            .unpriced_reason,
            Some(UnpricedReason::NotMoneyMetered)
        );
        assert_eq!(
            audit_turn_cost_for_route_at(
                ApiProvider::Stepfun,
                DEFAULT_STEPFUN_MODEL,
                None,
                &write_heavy,
                Utc::now(),
            )
            .unpriced_reason,
            Some(UnpricedReason::AmbiguousBillingSurface)
        );
        assert_eq!(
            audit_turn_cost_for_provider_at(
                ApiProvider::Openai,
                "gpt-5.5",
                &Usage {
                    input_tokens: OPENAI_LONG_CONTEXT_SURCHARGE_THRESHOLD + 1,
                    ..Usage::default()
                },
                Utc::now(),
            )
            .unpriced_reason,
            Some(UnpricedReason::UnrepresentedTier)
        );
    }

    /// The audit and the estimator are the same computation, so every route
    /// must agree on whether it produced a number.
    #[test]
    fn audit_and_estimate_never_disagree() {
        let usage = Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            prompt_cache_hit_tokens: Some(2_000),
            prompt_cache_write_tokens: Some(1_000),
            ..Usage::default()
        };
        let now = Utc::now();
        for (provider, model) in [
            (ApiProvider::Anthropic, "claude-haiku-4-5"),
            (ApiProvider::Anthropic, "claude-sonnet-5"),
            (ApiProvider::Moonshot, "kimi-k2.7-code"),
            (ApiProvider::Openai, "gpt-5.5"),
            (ApiProvider::OpenaiCodex, "gpt-5.5"),
            (ApiProvider::Deepseek, "deepseek-v4-pro"),
            (ApiProvider::Ollama, "gpt-5.5"),
            (ApiProvider::Stepfun, DEFAULT_STEPFUN_MODEL),
        ] {
            let audit = audit_turn_cost_for_route_at(provider, model, None, &usage, now);
            let estimate =
                calculate_turn_cost_estimate_for_route_at(provider, model, None, &usage, now);
            assert_eq!(audit.estimate, estimate, "{provider:?}/{model}");
            assert_eq!(
                audit.is_priced(),
                audit.unpriced_reason.is_none(),
                "{provider:?}/{model}"
            );
        }
    }

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
        // Endpoints CodeWhale cannot place now classify *positively* as
        // unclassified rather than returning `None`. Both fail closed
        // identically, but "we looked and could not place this" is a different
        // fact from "no endpoint was supplied", and the audit reports it as
        // such (#4318).
        for base_url in [
            "http://api.stepfun.ai/v1",
            "https://token@api.stepfun.ai/v1",
            "https://api.stepfun.ai/v1?account=other",
            "https://api.stepfun.ai/STEP_PLAN/v1",
            "https://stepfun.example/v1",
        ] {
            assert_eq!(
                billing_surface_for_route(ApiProvider::Stepfun, Some(base_url)),
                Some(UNCLASSIFIED_BILLING_SURFACE),
                "{base_url}"
            );
            assert_eq!(
                endpoint_metering_for_billing_surface(Some(UNCLASSIFIED_BILLING_SURFACE)),
                EndpointMetering::Unknown
            );
        }
        // A StepFun URL paired with the OpenRouter protocol is a foreign custom
        // endpoint, not proof of either provider's billing surface.
        assert_eq!(
            billing_surface_for_route(ApiProvider::Openrouter, Some(DEFAULT_STEPFUN_BASE_URL)),
            Some(UNCLASSIFIED_BILLING_SURFACE)
        );
        // No endpoint at all stays `None`.
        assert_eq!(
            billing_surface_for_route(ApiProvider::Stepfun, None),
            None,
            "an absent endpoint is not a classification"
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

        // Provider/model-only legacy callers cannot distinguish PAYG from Step
        // Plan and must not add either route to spend or savings totals.
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
    fn grok_46_pricing_tracks_the_200k_prompt_boundary() {
        for (input_tokens, cache_read, input, output) in
            [(199_999, 0.50, 2.00, 6.00), (200_000, 1.00, 4.00, 12.00)]
        {
            let usage = Usage {
                input_tokens,
                ..Usage::default()
            };
            let pricing =
                pricing_for_model_and_usage("grok-4.6", &usage).expect("Grok 4.6 pricing");
            assert_eq!(pricing.usd.input_cache_hit_per_million, cache_read);
            assert_eq!(pricing.usd.input_cache_miss_per_million, input);
            assert_eq!(pricing.usd.output_per_million, output);
        }
    }

    #[test]
    fn direct_xai_grok_46_owns_usage_tier_without_leaking_to_other_providers() {
        for (input_tokens, input_rate) in [(199_999, 2.00), (200_000, 4.00)] {
            let usage = Usage {
                input_tokens,
                ..Usage::default()
            };
            let estimate = calculate_turn_cost_estimate_for_provider_at(
                ApiProvider::Xai,
                "grok-4.6",
                &usage,
                Utc::now(),
            )
            .expect("direct xAI route has authoritative tiered pricing");
            let expected = f64::from(input_tokens) / 1_000_000.0 * input_rate;
            assert!((estimate.usd - expected).abs() < 1e-12);
        }

        assert!(
            provider_owned_hand_pricing_at(ApiProvider::Openrouter, "grok-4.6", Utc::now(),)
                .is_none()
        );
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
        assert_eq!(pricing.usd.cache_write, CacheWritePolicy::Rate(0.375));
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
            ("muse-spark-1.1", 0.15, 1.25, 4.25),
            ("muse-spark-1.2", 0.15, 1.25, 4.25),
            ("muse-spark-1.2-contributor", 0.002, 0.10, 0.20),
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

        // Anthropic / Qwen rows that publish a cache-write premium, and one row
        // (`gpt-5.5`) that publishes none — which is `Unpublished`, not a
        // licence to bill writes at the input rate (#4318).
        for (model, write) in [
            ("claude-opus-4-8", CacheWritePolicy::Rate(6.25)),
            ("claude-sonnet-4-6", CacheWritePolicy::Rate(3.75)),
            ("claude-haiku-4-5", CacheWritePolicy::Rate(1.25)),
            ("claude-fable-5", CacheWritePolicy::Rate(12.50)),
            ("qwen/qwen3.7-plus", CacheWritePolicy::Rate(0.40)),
            ("gpt-5.5", CacheWritePolicy::Unpublished),
        ] {
            let pricing = pricing_for_model_at(model, Utc::now()).expect(model);
            assert_eq!(
                pricing.usd.cache_write, write,
                "cache-write policy for {model}"
            );
        }
    }

    #[test]
    fn glm_5_3_has_no_hardcoded_price() {
        // GLM-5.3's catalog metadata is inherited from GLM-5.2, but Z.ai has
        // published no GLM-5.3 rate. Inheriting the 5.2 price would invent one,
        // so every price surface must report *unknown*, never a number and
        // never $0. If Z.ai publishes rates, delete this test and add the real
        // row — do not "fix" it by copying 5.2's.
        for model in ["glm-5.3", "z-ai/glm-5.3"] {
            assert!(
                pricing_for_model_at(model, Utc::now()).is_none(),
                "{model} must have no price row until Z.ai publishes one"
            );
            assert!(!has_pricing_for_model(model), "{model} must be unpriced");
            assert!(
                calculate_turn_cost_estimate_from_usage(
                    model,
                    &Usage {
                        input_tokens: 1_000_000,
                        output_tokens: 500_000,
                        ..Default::default()
                    },
                )
                .is_none(),
                "{model} must not accrue an invented cost estimate"
            );
        }
        // The priced sibling it inherits capabilities from is unaffected.
        assert!(has_pricing_for_model("glm-5.2"));
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
    fn provider_price_does_not_invent_catalog_missing_cache_write_class() {
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

        let audit =
            audit_turn_cost_for_provider_at(ApiProvider::Openai, "gpt-5.5", &usage, Utc::now());

        assert!(audit.estimate.is_none());
        assert_eq!(
            audit.unpriced_reason,
            Some(UnpricedReason::MissingClassPrice)
        );
        assert_eq!(audit.unpriced_classes, vec![TokenClass::CacheWrite]);
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
    fn token_usage_for_pricing_maps_cache_classes_without_double_billing_reasoning() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            prompt_cache_hit_tokens: Some(250),
            prompt_cache_miss_tokens: Some(700),
            prompt_cache_write_tokens: Some(50),
            // Reasoning is a subset of the 100 reported output tokens, not an
            // extra 50 tokens of billable output.
            reasoning_tokens: Some(50),
            ..Default::default()
        };

        assert_eq!(
            token_usage_for_pricing(&usage),
            TokenUsage {
                input: 700,
                output: 100,
                cache_read: 250,
                cache_write: 50,
            }
        );

        // Informational reasoning telemetry must not move the billed output at
        // all: the same completion count costs the same with or without it.
        let without_reasoning = Usage {
            reasoning_tokens: None,
            ..usage.clone()
        };
        assert_eq!(
            token_usage_for_pricing(&usage).output,
            token_usage_for_pricing(&without_reasoning).output
        );
        assert_eq!(
            calculate_turn_cost_estimate_for_provider(
                ApiProvider::Anthropic,
                "claude-haiku-4-5",
                &usage,
            ),
            calculate_turn_cost_estimate_for_provider(
                ApiProvider::Anthropic,
                "claude-haiku-4-5",
                &without_reasoning,
            )
        );
    }

    #[test]
    fn contradictory_cache_partition_is_bounded_and_fails_closed() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 10,
            prompt_cache_hit_tokens: Some(80),
            prompt_cache_miss_tokens: Some(40),
            prompt_cache_write_tokens: Some(30),
            ..Usage::default()
        };

        let classes = token_usage_for_pricing(&usage);
        assert_eq!(
            classes.input + classes.cache_read + classes.cache_write,
            u64::from(usage.input_tokens),
            "token projection may never exceed the provider's input total"
        );
        let audit = audit_turn_cost_for_provider_on_endpoint_at(
            ApiProvider::Deepseek,
            "deepseek-v4-flash",
            None,
            &usage,
            Utc::now(),
        );
        assert!(audit.estimate.is_none());
        assert_eq!(
            audit.unpriced_reason,
            Some(UnpricedReason::InconsistentUsage)
        );

        let overflow_shape = Usage {
            input_tokens: u32::MAX,
            prompt_cache_hit_tokens: Some(u32::MAX),
            prompt_cache_miss_tokens: Some(1),
            ..Usage::default()
        };
        assert!(
            !usage_cache_partition_is_consistent(&overflow_shape),
            "consistency validation must not hide overflow via saturation"
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
    }

    #[test]
    fn subscription_route_does_not_inherit_same_models_api_price() {
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            ..Default::default()
        };
        assert!(
            calculate_turn_cost_estimate_for_billing_surface(
                ApiProvider::Anthropic,
                "claude-sonnet-5",
                Some(FIRST_PARTY_PAYG_BILLING_SURFACE),
                &usage,
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
        assert_eq!(pricing.usd.cache_write, CacheWritePolicy::Rate(2.50));
        assert!(pricing.cny.is_none());
    }

    #[test]
    fn sonnet_5_uses_standard_pricing_after_intro_window() {
        let after_expiry = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).single().unwrap();
        let pricing = pricing_for_model_at("claude-sonnet-5", after_expiry).unwrap();

        assert_eq!(pricing.usd.input_cache_hit_per_million, 0.30);
        assert_eq!(pricing.usd.input_cache_miss_per_million, 3.00);
        assert_eq!(pricing.usd.output_per_million, 15.00);
        assert_eq!(pricing.usd.cache_write, CacheWritePolicy::Rate(3.75));
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
    fn fireworks_and_zen_flash_use_bundled_family_rates() {
        let now = Utc.with_ymd_and_hms(2026, 8, 14, 0, 0, 0).single().unwrap();
        let fireworks = provider_owned_hand_pricing_at(
            ApiProvider::Fireworks,
            "accounts/fireworks/models/deepseek-v4-flash",
            now,
        )
        .expect("Fireworks Flash should inherit the bundled DeepSeek family row");
        let zen =
            provider_owned_hand_pricing_at(ApiProvider::OpencodeZen, "deepseek-v4-flash", now)
                .expect("OpenCode Zen Flash should inherit the bundled DeepSeek family row");
        assert_eq!(fireworks.usd.output_per_million, zen.usd.output_per_million);
        assert!(
            provider_owned_hand_pricing_at(
                ApiProvider::Fireworks,
                "accounts/fireworks/models/kimi-k3",
                now,
            )
            .is_none(),
            "kimi-k3 has no published bundled rate; do not invent one"
        );
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
        assert_eq!(format_cost_amount(0.0, CostCurrency::Usd), "$0.00");
        assert_eq!(format_cost_amount(0.00001, CostCurrency::Usd), "<$0.0001");
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
        assert_eq!(
            format_cost_amount_precise(0.0, CostCurrency::Usd),
            "$0.0000"
        );
        assert_eq!(
            format_cost_amount_precise(0.00001, CostCurrency::Usd),
            "<$0.0001"
        );
    }

    #[test]
    fn accumulated_cost_stays_finite_and_nonnegative() {
        let saturated = CostEstimate {
            usd: f64::MAX,
            cny: 1.0,
        }
        .saturating_add(CostEstimate {
            usd: f64::MAX,
            cny: -1.0,
        });
        assert_eq!(saturated.usd, f64::MAX);
        assert_eq!(saturated.cny, 1.0);
        assert!(saturated.is_finite_nonnegative());

        assert_eq!(
            CostEstimate {
                usd: f64::NAN,
                cny: f64::INFINITY,
            }
            .sanitized(),
            CostEstimate::default()
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
}
