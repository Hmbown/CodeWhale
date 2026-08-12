//! Web-search provider configuration types.
//!
//! Self-contained `[search]` table types extracted verbatim from `config.rs`.
//! Re-exported from `crate::config` via `pub use search::*;` so existing
//! `crate::config::SearchProvider` (and sibling) paths resolve unchanged
//! (#3311).

use serde::{Deserialize, Serialize};

/// Search provider enumeration — selects the first backend `web_search` uses.
/// API-backed providers may visibly degrade through DuckDuckGo → Bing after
/// runtime failure or an empty result. Configuration and
/// network-policy errors fail closed without crossing providers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    /// Bing HTML scraping. No API key needed.
    Bing,
    /// DuckDuckGo HTML scraping with Bing fallback. No API key needed.
    #[serde(alias = "duckduckgo")]
    DuckDuckGo,
    /// Firecrawl Search API. Works keyless on Firecrawl Cloud with a bounded
    /// per-IP quota; `[search] api_key` or `FIRECRAWL_API_KEY` raises limits.
    #[default]
    Firecrawl,
    /// Tavily AI Search API (<https://tavily.com>). Requires api_key.
    Tavily,
    /// Bocha AI Search API (<https://bochaai.com>). Requires api_key.
    Bocha,
    /// Metaso AI Search API (<https://metaso.cn>). Requires `[search] api_key`
    /// or the `METASO_API_KEY` env var.
    #[serde(alias = "metaso")]
    Metaso,
    /// SearXNG JSON search API. Requires a trusted/self-hosted `base_url`.
    #[serde(alias = "searx", alias = "searx-ng", alias = "searx_ng")]
    Searxng,
    /// Baidu AI Search API (<https://qianfan.baidubce.com>). Requires api_key.
    #[serde(
        alias = "baidu-search",
        alias = "baidu_ai_search",
        alias = "baidu_search",
        alias = "baidu-ai-search"
    )]
    Baidu,
    /// Volcengine Ark web_search via Responses API. Requires api_key.
    /// Free tier: 20K queries/month per API key. Falls back to
    /// `VOLCENGINE_API_KEY` / `VOLCENGINE_ARK_API_KEY` / `ARK_API_KEY`
    /// env vars when `[search] api_key` is not set.
    #[serde(
        alias = "volcengine",
        alias = "ark",
        alias = "volc",
        alias = "volcengine-ark",
        alias = "volcengine_ark",
        alias = "volc-ark"
    )]
    Volcengine,
    /// Sofya web search API (<https://sofya.co>). Requires api_key
    /// (`ay_live_...`). Returns full extracted page content rather than
    /// snippets; falls back to the `SOFYA_API_KEY` env var when
    /// `[search] api_key` is not set.
    Sofya,
}

impl SearchProvider {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bing" => Some(Self::Bing),
            "duckduckgo" | "duck-duck-go" | "duck_duck_go" | "ddg" => Some(Self::DuckDuckGo),
            "firecrawl" | "fire-crawl" | "fire_crawl" => Some(Self::Firecrawl),
            "tavily" => Some(Self::Tavily),
            "bocha" => Some(Self::Bocha),
            "metaso" => Some(Self::Metaso),
            "searxng" | "searx" | "searx-ng" | "searx_ng" => Some(Self::Searxng),
            "baidu" | "baidu-search" | "baidu_search" | "baidu-ai-search" | "baidu_ai_search" => {
                Some(Self::Baidu)
            }
            "volcengine" | "ark" | "volc" | "volcengine-ark" => Some(Self::Volcengine),
            "sofya" => Some(Self::Sofya),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProviderSource {
    Default,
    Config,
    EnvOverride,
}

impl SearchProviderSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Config => "config",
            Self::EnvOverride => "env override",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchProviderResolution {
    pub provider: SearchProvider,
    pub source: SearchProviderSource,
}

/// Web search provider configuration (`[search]` table in config.toml).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SearchConfig {
    /// Search provider. Default: keyless `firecrawl`.
    #[serde(default)]
    pub provider: Option<SearchProvider>,
    /// Optional search endpoint. With `duckduckgo`, this is a
    /// DuckDuckGo-compatible HTML endpoint. With `searxng`, this is the trusted
    /// SearXNG instance root or `/search` endpoint.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Optional for Firecrawl; required for Tavily, Bocha, Metaso, Baidu, Volcengine, or Sofya.
    /// Metaso also falls back to the `METASO_API_KEY` env var.
    /// Baidu also falls back to `BAIDU_SEARCH_API_KEY` env var.
    /// Volcengine also falls back to `VOLCENGINE_API_KEY` / `VOLCENGINE_ARK_API_KEY` / `ARK_API_KEY` env vars.
    #[serde(default)]
    pub api_key: Option<String>,
}
