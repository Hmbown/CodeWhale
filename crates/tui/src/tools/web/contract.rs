//! Provider-neutral web-search request, result, and receipt types.
//!
//! Search backends vary widely in what they accept and return. These types are
//! the model-visible honesty boundary shared by `web_search` and `web.run`:
//! callers can distinguish requested knobs, actually honored behavior, and
//! degraded/post-filtered execution without depending on provider payloads.

use serde::{Deserialize, Serialize};

pub(crate) const MAX_SEARCH_RESULTS: u8 = 10;

/// Shared retrieval defaults. `web_search` and `web.run` both derive their
/// search knobs from these so the surfaces cannot drift apart.
pub(crate) const DEFAULT_SEARCH_RESULTS: usize = 5;
pub(crate) const DEFAULT_SEARCH_TIMEOUT_MS: u64 = 15_000;
pub(crate) const MAX_SEARCH_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackendId {
    ProviderNative,
    Bing,
    #[serde(rename = "duckduckgo")]
    DuckDuckGo,
    Firecrawl,
    Tavily,
    Bocha,
    Metaso,
    Searxng,
    Baidu,
    Volcengine,
    Sofya,
}

impl BackendId {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderNative => "provider_native",
            Self::Bing => "bing",
            Self::DuckDuckGo => "duckduckgo",
            Self::Firecrawl => "firecrawl",
            Self::Tavily => "tavily",
            Self::Bocha => "bocha",
            Self::Metaso => "metaso",
            Self::Searxng => "searxng",
            Self::Baidu => "baidu",
            Self::Volcengine => "volcengine",
            Self::Sofya => "sofya",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Recency {
    Day,
    Week,
    Month,
    Year,
    Days(u16),
}

impl Recency {
    #[must_use]
    #[cfg(test)]
    pub(crate) const fn days(self) -> u16 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
            Self::Year => 365,
            Self::Days(days) => days,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct SearchQuery {
    pub(crate) query: String,
    pub(crate) max_results: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recency: Option<Recency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) locale: Option<String>,
}

impl SearchQuery {
    #[must_use]
    pub(crate) fn new(
        query: String,
        max_results: usize,
        recency: Option<Recency>,
        domains: Vec<String>,
        locale: Option<String>,
    ) -> Self {
        let mut domains = domains
            .into_iter()
            .map(|domain| {
                let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
                domain.trim_start_matches("www.").to_string()
            })
            .filter(|domain| !domain.is_empty())
            .collect::<Vec<_>>();
        domains.sort_unstable();
        domains.dedup();
        Self {
            query,
            max_results: u8::try_from(max_results.clamp(1, usize::from(MAX_SEARCH_RESULTS)))
                .unwrap_or(MAX_SEARCH_RESULTS),
            recency,
            domains,
            locale: locale
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CapabilityState {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QueryCapabilities {
    pub(crate) max_results: CapabilityState,
    pub(crate) recency: CapabilityState,
    pub(crate) domains: CapabilityState,
    pub(crate) locale: CapabilityState,
    pub(crate) published_date: CapabilityState,
}

impl QueryCapabilities {
    #[must_use]
    pub(crate) const fn count_only() -> Self {
        Self {
            max_results: CapabilityState::Supported,
            recency: CapabilityState::Unsupported,
            domains: CapabilityState::Unsupported,
            locale: CapabilityState::Unsupported,
            published_date: CapabilityState::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HonoredQueryCapabilities {
    pub(crate) max_results: bool,
    pub(crate) recency: bool,
    pub(crate) domains: bool,
    pub(crate) locale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QueryKnob {
    Recency,
    Domains,
    Locale,
}

impl QueryKnob {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Recency => "recency",
            Self::Domains => "domains",
            Self::Locale => "locale",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DegradedReason {
    BackendUnavailable { backend: BackendId },
    NoUsableResults { backend: BackendId },
    BackendFallback { from: BackendId, to: BackendId },
    ChallengeDetected { backend: BackendId },
    ScrapeFallback { from: BackendId, to: BackendId },
    KnobIgnored { knob: QueryKnob },
    PostFiltered { knob: QueryKnob },
    SynthesizedResults,
}

impl DegradedReason {
    #[must_use]
    pub(crate) fn message(&self) -> String {
        match self {
            Self::BackendUnavailable { backend } => {
                format!("{} was unavailable", backend.as_str())
            }
            Self::NoUsableResults { backend } => {
                format!("{} returned no usable results", backend.as_str())
            }
            Self::BackendFallback { from, to } => format!(
                "{} did not answer; tried {} next",
                from.as_str(),
                to.as_str()
            ),
            Self::ChallengeDetected { backend } => {
                format!("{} returned a bot challenge", backend.as_str())
            }
            Self::ScrapeFallback { from, to } => format!(
                "{} returned no usable results; used {} fallback",
                from.as_str(),
                to.as_str()
            ),
            Self::KnobIgnored { knob } => {
                format!("{} filter was not enforced by this backend", knob.as_str())
            }
            Self::PostFiltered { knob } => {
                format!("results were post-filtered by {}", knob.as_str())
            }
            Self::SynthesizedResults => {
                "results were synthesized by a model-backed search response".to_string()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SearchResult {
    pub(crate) rank: u8,
    /// Session-scoped citation handle. Backends leave this empty; the shared
    /// execution surface mints it before any result crosses the tool boundary.
    pub(crate) ref_id: String,
    pub(crate) title: String,
    pub(crate) url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) published: Option<String>,
    pub(crate) domain: String,
}

impl SearchResult {
    #[must_use]
    pub(crate) fn new(
        rank: usize,
        title: String,
        url: String,
        snippet: Option<String>,
        published: Option<String>,
    ) -> Self {
        let domain = reqwest::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_default();
        Self {
            rank: u8::try_from(rank.clamp(1, 255)).unwrap_or(u8::MAX),
            ref_id: String::new(),
            title,
            url,
            snippet,
            published,
            domain,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BackendSearch {
    pub(crate) backend: BackendId,
    pub(crate) source: String,
    pub(crate) backend_detail: Option<String>,
    pub(crate) results: Vec<SearchResult>,
    pub(crate) degraded: Vec<DegradedReason>,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SearchReceipt {
    pub(crate) backend: BackendId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) backend_detail: Option<String>,
    pub(crate) requested: SearchQuery,
    pub(crate) capabilities: QueryCapabilities,
    pub(crate) honored: HonoredQueryCapabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) degraded: Vec<DegradedReason>,
    pub(crate) latency_ms: u32,
    pub(crate) cache_hit: bool,
}

impl SearchReceipt {
    #[must_use]
    pub(crate) fn warning(&self) -> Option<String> {
        if self.degraded.is_empty() {
            None
        } else {
            Some(
                self.degraded
                    .iter()
                    .map(DegradedReason::message)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SearchResponse {
    pub(crate) query: String,
    pub(crate) source: String,
    pub(crate) count: usize,
    pub(crate) message: String,
    pub(crate) results: Vec<SearchResult>,
    pub(crate) receipt: SearchReceipt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieval_defaults_are_coherent_across_search_and_fetch() {
        use super::super::fetch::{DEFAULT_TIMEOUT, HARD_MAX_TIMEOUT};

        // Search and fetch share one default and one hard-cap timeout so the
        // two halves of the retrieval path behave identically by default.
        assert_eq!(
            u128::from(DEFAULT_SEARCH_TIMEOUT_MS),
            DEFAULT_TIMEOUT.as_millis()
        );
        assert_eq!(
            u128::from(MAX_SEARCH_TIMEOUT_MS),
            HARD_MAX_TIMEOUT.as_millis()
        );
        assert!(DEFAULT_SEARCH_RESULTS <= usize::from(MAX_SEARCH_RESULTS));
    }

    #[test]
    fn search_query_normalizes_domains_and_bounds_count() {
        let query = SearchQuery::new(
            "rust async".to_string(),
            99,
            Some(Recency::Week),
            vec![" WWW.Example.COM. ".to_string(), "example.com".to_string()],
            Some(" en-US ".to_string()),
        );

        assert_eq!(query.max_results, 10);
        assert_eq!(query.domains, vec!["example.com"]);
        assert_eq!(query.locale.as_deref(), Some("en-US"));
        assert_eq!(query.recency.map(Recency::days), Some(7));
    }

    #[test]
    fn normalized_result_derives_domain_and_rank() {
        let result = SearchResult::new(
            2,
            "Example".to_string(),
            "https://Docs.Example.com/path".to_string(),
            Some("summary".to_string()),
            None,
        );

        assert_eq!(result.rank, 2);
        assert_eq!(result.domain, "docs.example.com");
    }

    #[test]
    fn degraded_receipt_is_machine_readable_and_human_visible() {
        let receipt = SearchReceipt {
            backend: BackendId::DuckDuckGo,
            backend_detail: None,
            requested: SearchQuery::new(
                "fresh result".to_string(),
                5,
                Some(Recency::Day),
                Vec::new(),
                None,
            ),
            capabilities: QueryCapabilities::count_only(),
            honored: HonoredQueryCapabilities {
                max_results: true,
                ..HonoredQueryCapabilities::default()
            },
            degraded: vec![DegradedReason::KnobIgnored {
                knob: QueryKnob::Recency,
            }],
            latency_ms: 4,
            cache_hit: false,
        };

        let value = serde_json::to_value(&receipt).expect("receipt serializes");
        assert_eq!(value["backend"], "duckduckgo");
        assert_eq!(value["degraded"][0]["kind"], "knob_ignored");
        assert!(receipt.warning().expect("warning").contains("recency"));
    }

    #[test]
    fn scrape_fallback_receipt_preserves_backend_transitions() {
        let receipt = SearchReceipt {
            backend: BackendId::Bing,
            backend_detail: None,
            requested: SearchQuery::new("fallback query".to_string(), 5, None, Vec::new(), None),
            capabilities: QueryCapabilities::count_only(),
            honored: HonoredQueryCapabilities {
                max_results: true,
                ..HonoredQueryCapabilities::default()
            },
            degraded: vec![
                DegradedReason::ChallengeDetected {
                    backend: BackendId::DuckDuckGo,
                },
                DegradedReason::ScrapeFallback {
                    from: BackendId::DuckDuckGo,
                    to: BackendId::Bing,
                },
            ],
            latency_ms: 12,
            cache_hit: false,
        };

        let value = serde_json::to_value(&receipt).expect("receipt serializes");
        assert_eq!(value["backend"], "bing");
        assert_eq!(value["degraded"][0]["kind"], "challenge_detected");
        assert_eq!(value["degraded"][0]["backend"], "duckduckgo");
        assert_eq!(value["degraded"][1]["kind"], "scrape_fallback");
        assert_eq!(value["degraded"][1]["from"], "duckduckgo");
        assert_eq!(value["degraded"][1]["to"], "bing");
        let warning = receipt.warning().expect("warning");
        assert!(warning.contains("bot challenge"));
        assert!(warning.contains("used bing fallback"));
    }
}
