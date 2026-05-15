//! Web search tool backed by multiple providers: Bing HTML scrape, DuckDuckGo
//! (HTML scrape with Bing fallback), Tavily API, and Bocha (博查) API.
//!
//! This is the primary web search surface for agents. For browsing workflows
//! (page open, click, screenshot) use a direct URL approach instead.
//!
//! Set `[search]` in config.toml to switch providers:
//!   provider = "duckduckgo"  # or tavily/bocha
//!   api_key = "tvly-..."

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_u64,
};
use crate::config::SearchProvider;
use crate::network_policy::{Decision, NetworkPolicyDecider};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::Duration;

const DUCKDUCKGO_HOST: &str = "html.duckduckgo.com";
const BING_HOST: &str = "www.bing.com";
const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";
const BOCHA_ENDPOINT: &str = "https://api.bochaai.com/v1/ai/search";
const ERROR_BODY_PREVIEW_BYTES: usize = 512;

/// Returns `Ok(())` if the policy allows the call, or a `ToolError` otherwise.
/// Falls through silently when no policy is attached (back-compat).
fn check_policy(decider: Option<&NetworkPolicyDecider>, host: &str) -> Result<(), ToolError> {
    let Some(decider) = decider else {
        return Ok(());
    };
    match decider.evaluate(host, "web_search") {
        Decision::Allow => Ok(()),
        Decision::Deny => Err(ToolError::permission_denied(format!(
            "web search to '{host}' blocked by network policy"
        ))),
        Decision::Prompt => Err(ToolError::permission_denied(format!(
            "web search to '{host}' requires approval; \
             re-run after `/network allow {host}` or set network.default = \"allow\" in config"
        ))),
    }
}

// Cached regex patterns for HTML parsing
static TITLE_RE: OnceLock<Regex> = OnceLock::new();
static SNIPPET_RE: OnceLock<Regex> = OnceLock::new();
static TAG_RE: OnceLock<Regex> = OnceLock::new();
static BING_RESULT_RE: OnceLock<Regex> = OnceLock::new();
static BING_TITLE_RE: OnceLock<Regex> = OnceLock::new();
static BING_SNIPPET_RE: OnceLock<Regex> = OnceLock::new();

fn get_title_re() -> &'static Regex {
    TITLE_RE.get_or_init(|| {
        Regex::new(r#"<a[^>]*class=\"result__a\"[^>]*href=\"([^\"]+)\"[^>]*>(.*?)</a>"#)
            .expect("title regex pattern is valid")
    })
}

fn get_snippet_re() -> &'static Regex {
    SNIPPET_RE.get_or_init(|| {
        Regex::new(
            r#"<a[^>]*class=\"result__snippet\"[^>]*>(.*?)</a>|<div[^>]*class=\"result__snippet\"[^>]*>(.*?)</div>"#,
        )
        .expect("snippet regex pattern is valid")
    })
}

fn get_tag_re() -> &'static Regex {
    TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").expect("tag regex pattern is valid"))
}

fn get_bing_result_re() -> &'static Regex {
    BING_RESULT_RE.get_or_init(|| {
        Regex::new(r#"(?is)<li[^>]*class=\"[^\"]*\bb_algo\b[^\"]*\"[^>]*>(.*?)</li>"#)
            .expect("bing result regex pattern is valid")
    })
}

fn get_bing_title_re() -> &'static Regex {
    BING_TITLE_RE.get_or_init(|| {
        Regex::new(r#"(?is)<h2[^>]*>.*?<a[^>]*href=\"([^\"]+)\"[^>]*>(.*?)</a>"#)
            .expect("bing title regex pattern is valid")
    })
}

fn get_bing_snippet_re() -> &'static Regex {
    BING_SNIPPET_RE.get_or_init(|| {
        Regex::new(r#"(?is)<div[^>]*class=\"[^\"]*\bb_caption\b[^\"]*\"[^>]*>.*?<p[^>]*>(.*?)</p>"#)
            .expect("bing snippet regex pattern is valid")
    })
}

const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_RESULTS: usize = 10;
const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

#[derive(Debug, Clone, Serialize)]
struct WebSearchEntry {
    title: String,
    url: String,
    snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct WebSearchResponse {
    query: String,
    source: String,
    count: usize,
    message: String,
    results: Vec<WebSearchEntry>,
}

pub struct WebSearchTool;

#[async_trait]
impl ToolSpec for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web and return ranked results with URLs and snippets. Default backend is Bing; set `[search] provider = \"duckduckgo\" | \"tavily\" | \"bocha\"` in config.toml to switch backends. Use this instead of scraping search engines with `curl` in `exec_shell`. For a known canonical URL, prefer `fetch_url` directly."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. Compatibility aliases: q, or search_query[0].q."
                },
                "q": {
                    "type": "string",
                    "description": "Search query."
                },
                "search_query": {
                    "type": "array",
                    "description": "Array form for advanced queries: [{\"q\":\"...\", \"max_results\": 5}]",
                    "items": {
                        "type": "object",
                        "properties": {
                            "q": { "type": "string" },
                            "query": { "type": "string" },
                            "max_results": { "type": "integer" }
                        }
                    }
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 5, max: 10)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 15000, max: 60000)"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Network]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let query = extract_search_query(&input)?;
        if query.is_empty() {
            return Err(ToolError::invalid_input("Query cannot be empty"));
        }
        let max_results =
            usize::try_from(optional_search_max_results(&input)).unwrap_or(DEFAULT_MAX_RESULTS);
        let max_results = max_results.clamp(1, MAX_RESULTS);
        let timeout_ms = optional_u64(&input, "timeout_ms", DEFAULT_TIMEOUT_MS).min(60_000);

        // Dispatch to the configured API-backed search providers before
        // building the HTML-scraping client used by Bing/DuckDuckGo.
        match context.search_provider {
            SearchProvider::Tavily => {
                let decider = context.network_policy.as_ref();
                check_policy(decider, "api.tavily.com")?;
                return self
                    .run_tavily_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Bocha => {
                let decider = context.network_policy.as_ref();
                check_policy(decider, "api.bochaai.com")?;
                return self
                    .run_bocha_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Bing | SearchProvider::DuckDuckGo => {}
        }

        let decider = context.network_policy.as_ref();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        if matches!(context.search_provider, SearchProvider::Bing) {
            check_policy(decider, BING_HOST)?;
            let results = run_bing_search(&client, &query, max_results).await?;
            return search_tool_result(query, "bing", results, None);
        }

        // Per-domain network policy gate (#135). The "host" for web search is
        // the upstream search engine domain — DuckDuckGo first, Bing on
        // fallback. We gate DuckDuckGo here; Bing is gated separately inside
        // the fallback path so a deny on one engine doesn't block the other.
        check_policy(decider, DUCKDUCKGO_HOST)?;

        let encoded = url_encode(&query);
        let url = format!("https://html.duckduckgo.com/html/?q={encoded}");
        let resp = client
            .get(&url)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "en-US,en;q=0.5")
            .send()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Web search request failed: {e}")))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to read response: {e}")))?;

        if !status.is_success() {
            return Err(ToolError::execution_failed(format!(
                "Web search failed: HTTP {}",
                status.as_u16()
            )));
        }

        let mut results = parse_duckduckgo_results(&body, max_results);
        let mut source = "duckduckgo";
        let mut message_suffix = None;
        if results.is_empty() {
            let duckduckgo_blocked = is_duckduckgo_challenge(&body);
            // Bing is a separate host — gate it independently so a deny on
            // DuckDuckGo doesn't silently let Bing through (and vice versa).
            check_policy(decider, BING_HOST)?;
            match run_bing_search(&client, &query, max_results).await {
                Ok(fallback_results) if !fallback_results.is_empty() => {
                    results = fallback_results;
                    source = "bing";
                    message_suffix = Some(if duckduckgo_blocked {
                        "DuckDuckGo returned a bot challenge; used Bing fallback"
                    } else {
                        "DuckDuckGo returned no parseable results; used Bing fallback"
                    });
                }
                Ok(_) if duckduckgo_blocked => {
                    return Err(ToolError::execution_failed(
                        "DuckDuckGo returned a bot challenge and Bing fallback returned no results",
                    ));
                }
                Err(err) if duckduckgo_blocked => {
                    return Err(ToolError::execution_failed(format!(
                        "DuckDuckGo returned a bot challenge and Bing fallback failed: {err}"
                    )));
                }
                Ok(_) | Err(_) => {}
            }
        }

        search_tool_result(query, source, results, message_suffix)
    }
}

fn search_tool_result(
    query: String,
    source: &'static str,
    results: Vec<WebSearchEntry>,
    message_suffix: Option<&str>,
) -> Result<ToolResult, ToolError> {
    let message = if results.is_empty() {
        "No results found".to_string()
    } else if let Some(suffix) = message_suffix {
        format!("Found {} result(s). {suffix}", results.len())
    } else {
        format!("Found {} result(s)", results.len())
    };

    let response = WebSearchResponse {
        query,
        source: source.to_string(),
        count: results.len(),
        message,
        results,
    };

    ToolResult::json(&response).map_err(|e| ToolError::execution_failed(e.to_string()))
}

impl WebSearchTool {
    /// Search via Tavily AI Search API (<https://tavily.com>).
    async fn run_tavily_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let api_key = context
            .search_api_key
            .as_deref()
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "Tavily search requires an API key. Set `[search] api_key = \"tvly-...\"` in config.toml.",
                )
            })?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let payload = json!({
            "api_key": api_key, // noqa: api-key-in-body
            "query": query,
            "search_depth": "basic",
            "max_results": max_results,
        });

        let resp = client
            .post(TAVILY_ENDPOINT)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("Tavily search request failed: {e}"))
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read Tavily response: {e}"))
        })?;

        if !status.is_success() {
            let truncated = truncate_error_body(&body);
            return Err(ToolError::execution_failed(format!(
                "Tavily search failed: HTTP {} — {truncated}",
                status.as_u16()
            )));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse Tavily response: {e}"))
        })?;

        let results: Vec<WebSearchEntry> = parsed
            .get("results")
            .and_then(|v| v.as_array())
            .into_iter()
            .flat_map(|arr| arr.iter())
            .filter_map(|item| {
                let title = item.get("title")?.as_str()?.to_string();
                let url = item.get("url")?.as_str()?.to_string();
                let snippet = item
                    .get("content")
                    .or_else(|| item.get("snippet"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
                Some(WebSearchEntry {
                    title,
                    url,
                    snippet,
                })
            })
            .take(max_results)
            .collect();

        let message = if results.is_empty() {
            "No results found".to_string()
        } else {
            format!("Found {} result(s)", results.len())
        };

        let response = WebSearchResponse {
            query: query.to_string(),
            source: "tavily".to_string(),
            count: results.len(),
            message,
            results,
        };

        ToolResult::json(&response).map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    /// Search via Bocha AI Search API (<https://bochaai.com>).
    async fn run_bocha_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let api_key = context
            .search_api_key
            .as_deref()
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "Bocha search requires an API key. Set `[search] api_key = \"sk-...\"` in config.toml.",
                )
            })?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let payload = json!({
            "query": query,
            "freshness": "noLimit",
            "count": max_results,
        });

        let resp = client
            .post(BOCHA_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("Bocha search request failed: {e}"))
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read Bocha response: {e}"))
        })?;

        if !status.is_success() {
            let truncated = truncate_error_body(&body);
            return Err(ToolError::execution_failed(format!(
                "Bocha search failed: HTTP {} — {truncated}",
                status.as_u16()
            )));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse Bocha response: {e}"))
        })?;

        // Bocha returns `{"code": 200, "data": {"pages": [...]}}`
        let results: Vec<WebSearchEntry> = parsed
            .get("data")
            .and_then(|d| d.get("pages"))
            .or_else(|| parsed.get("pages"))
            .and_then(|v| v.as_array())
            .into_iter()
            .flat_map(|arr| arr.iter())
            .filter_map(|item| {
                let title = item
                    .get("name")
                    .or_else(|| item.get("title"))
                    .and_then(|s| s.as_str())?
                    .to_string();
                let url = item
                    .get("url")
                    .or_else(|| item.get("link"))
                    .and_then(|s| s.as_str())?
                    .to_string();
                let snippet = item
                    .get("summary")
                    .or_else(|| item.get("snippet"))
                    .or_else(|| item.get("description"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
                Some(WebSearchEntry {
                    title,
                    url,
                    snippet,
                })
            })
            .take(max_results)
            .collect();

        let message = if results.is_empty() {
            "No results found".to_string()
        } else {
            format!("Found {} result(s)", results.len())
        };

        let response = WebSearchResponse {
            query: query.to_string(),
            source: "bocha".to_string(),
            count: results.len(),
            message,
            results,
        };

        ToolResult::json(&response).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

fn truncate_error_body(body: &str) -> String {
    let stripped = sanitize_error_body(body);
    if stripped.len() <= ERROR_BODY_PREVIEW_BYTES {
        stripped
    } else {
        let mut end = ERROR_BODY_PREVIEW_BYTES;
        while !stripped.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &stripped[..end])
    }
}

fn sanitize_error_body(body: &str) -> String {
    let stripped = strip_html_tags(body);
    stripped
        .chars()
        .filter(|c| !c.is_control() || c.is_ascii_whitespace())
        .collect()
}

fn extract_search_query(input: &Value) -> Result<String, ToolError> {
    for key in ["query", "q"] {
        if let Some(value) = input.get(key) {
            let Some(query) = value.as_str() else {
                return Err(ToolError::invalid_input(format!(
                    "Field '{key}' must be a string"
                )));
            };
            let query = query.trim();
            if !query.is_empty() {
                return Ok(query.to_string());
            }
        }
    }

    for item in search_query_items(input) {
        for key in ["q", "query"] {
            if let Some(value) = item.get(key) {
                let Some(query) = value.as_str() else {
                    return Err(ToolError::invalid_input(format!(
                        "Field 'search_query[].{key}' must be a string"
                    )));
                };
                let query = query.trim();
                if !query.is_empty() {
                    return Ok(query.to_string());
                }
            }
        }
    }

    Err(ToolError::missing_field("query"))
}

fn optional_search_max_results(input: &Value) -> u64 {
    if let Some(value) = input.get("max_results").and_then(Value::as_u64) {
        return value;
    }
    search_query_items(input)
        .filter_map(|item| item.get("max_results").and_then(Value::as_u64))
        .next()
        .unwrap_or(DEFAULT_MAX_RESULTS as u64)
}

fn search_query_items(input: &Value) -> impl Iterator<Item = &Value> {
    input
        .get("search_query")
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|items| items.iter())
}

async fn run_bing_search(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<WebSearchEntry>, ToolError> {
    let encoded = url_encode(query);
    let url = format!("https://www.bing.com/search?q={encoded}");
    let resp = client
        .get(&url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| ToolError::execution_failed(format!("Bing search request failed: {e}")))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
        ToolError::execution_failed(format!("Failed to read Bing search response: {e}"))
    })?;

    if !status.is_success() {
        return Err(ToolError::execution_failed(format!(
            "Bing search failed: HTTP {}",
            status.as_u16()
        )));
    }

    Ok(parse_bing_results(&body, max_results))
}

fn parse_duckduckgo_results(html: &str, max_results: usize) -> Vec<WebSearchEntry> {
    let title_re = get_title_re();
    let snippet_re = get_snippet_re();
    let snippets: Vec<String> = snippet_re
        .captures_iter(html)
        .filter_map(|cap| cap.get(1).or_else(|| cap.get(2)))
        .map(|m| normalize_text(m.as_str()))
        .collect();

    let mut results = Vec::new();
    for (idx, cap) in title_re.captures_iter(html).enumerate() {
        if results.len() >= max_results {
            break;
        }
        let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_raw = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = normalize_text(title_raw);
        if title.is_empty() {
            continue;
        }
        let url = normalize_url(href);
        let snippet = snippets
            .get(idx)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        results.push(WebSearchEntry {
            title,
            url,
            snippet,
        });
    }

    if is_likely_spam_results(&results) {
        // Same defence as the Bing path (#964): a DDG fallback page can
        // also serve a single-domain stuffed result set when the upstream
        // is degraded. Drop rather than mislead the model.
        return Vec::new();
    }
    results
}

fn is_duckduckgo_challenge(html: &str) -> bool {
    html.contains("anomaly-modal") || html.contains("Unfortunately, bots use DuckDuckGo too")
}

fn parse_bing_results(html: &str, max_results: usize) -> Vec<WebSearchEntry> {
    let mut results = Vec::new();
    for cap in get_bing_result_re().captures_iter(html) {
        if results.len() >= max_results {
            break;
        }
        let Some(block) = cap.get(1).map(|m| m.as_str()) else {
            continue;
        };
        let Some(title_cap) = get_bing_title_re().captures(block) else {
            continue;
        };
        let href = title_cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_raw = title_cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = normalize_text(title_raw);
        if title.is_empty() {
            continue;
        }
        let snippet = get_bing_snippet_re()
            .captures(block)
            .and_then(|snippet_cap| snippet_cap.get(1))
            .map(|m| normalize_text(m.as_str()))
            .filter(|s| !s.is_empty());

        results.push(WebSearchEntry {
            title,
            url: normalize_bing_url(href),
            snippet,
        });
    }

    if is_likely_spam_results(&results) {
        // Bing's scraping endpoint occasionally serves a stuffed page
        // where the same low-quality domain owns most of the b_algo
        // entries — #964 reported eight in a row from
        // `astralia.forumgratuit.org` for unrelated queries. Treat the
        // batch as "no results" so the caller surfaces a clean failure
        // message instead of routing the model toward junk.
        return Vec::new();
    }
    results
}

/// Heuristic spam detector for scraped SERP HTML (#964).
///
/// Returns `true` when one root domain owns at least 60% of the result
/// set and there are at least three results. A real-world top-five page
/// from Google/Bing/DDG mixes domains; a result page dominated by one
/// host is almost always SEO spam or a bot-detection-stuffed substitute.
fn is_likely_spam_results(results: &[WebSearchEntry]) -> bool {
    if results.len() < 3 {
        return false;
    }
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in results {
        if let Some(host) = root_domain(&r.url) {
            *counts.entry(host).or_insert(0) += 1;
        }
    }
    let Some(&max) = counts.values().max() else {
        return false;
    };
    // 60% threshold: 3-of-5, 4-of-6, 5-of-8 all trip; 2-of-5 doesn't.
    max * 5 >= results.len() * 3
}

/// Extract the registrable root domain (eTLD+1 approximation) from a URL
/// so spam detection groups `astralia.forumgratuit.org` with
/// `russia.forumgratuit.org`. Returns lowercase host minus the leftmost
/// label, or the bare host when there are only two labels.
fn root_domain(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = after_scheme.split(['/', '?', '#']).next()?;
    let host = host.split('@').next_back()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let labels: Vec<&str> = host.split('.').filter(|s| !s.is_empty()).collect();
    if labels.len() <= 2 {
        return Some(host);
    }
    Some(labels[labels.len().saturating_sub(2)..].join("."))
}

fn normalize_url(href: &str) -> String {
    if let Some(uddg) = extract_query_param(href, "uddg") {
        let decoded = percent_decode(&uddg);
        if !decoded.is_empty() {
            return decoded;
        }
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    if href.starts_with('/') {
        return format!("https://duckduckgo.com{href}");
    }
    href.to_string()
}

fn normalize_bing_url(href: &str) -> String {
    if let Some(encoded) = extract_query_param(href, "u") {
        let decoded = percent_decode(&encoded);
        let token = decoded.strip_prefix("a1").unwrap_or(&decoded);
        let mut padded = token.replace('-', "+").replace('_', "/");
        while !padded.len().is_multiple_of(4) {
            padded.push('=');
        }
        if let Ok(bytes) = general_purpose::STANDARD.decode(padded)
            && let Ok(url) = String::from_utf8(bytes)
            && (url.starts_with("http://") || url.starts_with("https://"))
        {
            return url;
        }
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    if href.starts_with('/') {
        return format!("https://www.bing.com{href}");
    }
    href.to_string()
}

fn normalize_text(text: &str) -> String {
    let stripped = strip_html_tags(text);
    let decoded = decode_html_entities(&stripped);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_html_tags(text: &str) -> String {
    get_tag_re().replace_all(text, "").to_string()
}

fn decode_html_entities(text: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    static ENTITY_RE: OnceLock<Regex> = OnceLock::new();
    let re = ENTITY_RE.get_or_init(|| {
        Regex::new(r"&(?:#(\d+)|#x([0-9A-Fa-f]+)|([a-zA-Z]+));").expect("HTML entity regex")
    });

    re.replace_all(text, |caps: &regex::Captures| {
        if let Some(dec) = caps.get(1) {
            return dec
                .as_str()
                .parse::<u32>()
                .ok()
                .and_then(std::char::from_u32)
                .unwrap_or('\u{FFFD}')
                .to_string();
        }
        if let Some(hex) = caps.get(2) {
            return u32::from_str_radix(hex.as_str(), 16)
                .ok()
                .and_then(std::char::from_u32)
                .unwrap_or('\u{FFFD}')
                .to_string();
        }
        let named = caps.get(3).map(|m| m.as_str());
        match named {
            Some("amp") => "&",
            Some("lt") => "<",
            Some("gt") => ">",
            Some("quot") => "\"",
            Some("apos") => "'",
            Some("nbsp") => " ",
            Some("copy") => "\u{00A9}",
            Some("reg") => "\u{00AE}",
            Some("mdash") => "\u{2014}",
            Some("ndash") => "\u{2013}",
            Some("lsquo") => "\u{2018}",
            Some("rsquo") => "\u{2019}",
            Some("ldquo") => "\u{201C}",
            Some("rdquo") => "\u{201D}",
            Some("hellip") => "\u{2026}",
            _ => return caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string(),
        }
        .to_string()
    })
    .to_string()
}

fn url_encode(input: &str) -> String {
    crate::utils::url_encode(input)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = &input[i + 1..i + 3];
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
            }
            b'+' => out.push(b' '),
            _ => out.push(bytes[i]),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for part in query.split('&') {
        let mut iter = part.splitn(2, '=');
        let name = iter.next().unwrap_or("");
        if name == key {
            return iter.next().map(str::to_string);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        ERROR_BODY_PREVIEW_BYTES, WebSearchEntry, WebSearchTool, decode_html_entities,
        extract_search_query, is_likely_spam_results, optional_search_max_results, root_domain,
        sanitize_error_body, truncate_error_body,
    };
    use serde_json::json;

    fn entry(url: &str) -> WebSearchEntry {
        WebSearchEntry {
            title: "x".into(),
            url: url.into(),
            snippet: None,
        }
    }

    #[test]
    fn root_domain_strips_subdomain_keeps_two_labels() {
        assert_eq!(
            root_domain("https://astralia.forumgratuit.org/path/page").as_deref(),
            Some("forumgratuit.org"),
        );
        assert_eq!(
            root_domain("http://www.example.com/").as_deref(),
            Some("example.com"),
        );
        assert_eq!(
            root_domain("https://example.com").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn root_domain_handles_port_and_userinfo() {
        assert_eq!(
            root_domain("http://user:pass@blog.example.com:8080/x").as_deref(),
            Some("example.com"),
        );
    }

    #[test]
    fn root_domain_returns_none_for_garbage() {
        assert!(
            root_domain("not-a-url").as_deref().is_some(),
            "bare token is treated as host"
        );
        assert!(root_domain("https:///path").is_none());
    }

    #[test]
    fn spam_detector_flags_single_domain_dominance() {
        // #964 reproduction: 5/5 results from the same low-quality host.
        let r = vec![
            entry("https://astralia.forumgratuit.org/page1"),
            entry("https://russia.forumgratuit.org/page2"),
            entry("https://other.forumgratuit.org/page3"),
            entry("https://hello.forumgratuit.org/page4"),
            entry("https://world.forumgratuit.org/page5"),
        ];
        assert!(is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_passes_diverse_serp() {
        // A normal SERP mixes domains; nothing flagged.
        let r = vec![
            entry("https://example.com/a"),
            entry("https://wikipedia.org/b"),
            entry("https://stackoverflow.com/c"),
            entry("https://reddit.com/d"),
            entry("https://example.com/e"),
        ];
        assert!(!is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_passes_short_result_set() {
        // Two results from the same domain isn't enough signal — false
        // positives on legitimate two-link answers (docs + homepage)
        // would hurt more than letting them through.
        let r = vec![
            entry("https://example.com/a"),
            entry("https://example.com/b"),
        ];
        assert!(!is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_threshold_is_sixty_percent() {
        // 3-of-5 same domain trips the 60% threshold.
        let r3of5 = vec![
            entry("https://spam.example.com/a"),
            entry("https://spam.example.com/b"),
            entry("https://spam.example.com/c"),
            entry("https://other.com/d"),
            entry("https://third.com/e"),
        ];
        assert!(is_likely_spam_results(&r3of5));
        // 2-of-5 does NOT trip the threshold.
        let r2of5 = vec![
            entry("https://spam.example.com/a"),
            entry("https://spam.example.com/b"),
            entry("https://other.com/c"),
            entry("https://third.com/d"),
            entry("https://fourth.com/e"),
        ];
        assert!(!is_likely_spam_results(&r2of5));
    }

    #[test]
    fn decode_html_entities_handles_named_entities() {
        assert_eq!(decode_html_entities("&amp;"), "&");
        assert_eq!(decode_html_entities("&lt;"), "<");
        assert_eq!(decode_html_entities("&gt;"), ">");
        assert_eq!(decode_html_entities("&quot;"), "\"");
        assert_eq!(decode_html_entities("&apos;"), "'");
        assert_eq!(decode_html_entities("&nbsp;"), " ");
        assert_eq!(decode_html_entities("&copy;"), "\u{00A9}");
        assert_eq!(decode_html_entities("&mdash;"), "\u{2014}");
    }

    #[test]
    fn decode_html_entities_handles_decimal_numeric_references() {
        assert_eq!(decode_html_entities("&#65;"), "A");
        assert_eq!(decode_html_entities("&#60;"), "<");
        assert_eq!(decode_html_entities("&#8211;"), "\u{2013}");
    }

    #[test]
    fn decode_html_entities_handles_hex_numeric_references() {
        assert_eq!(decode_html_entities("&#x41;"), "A");
        assert_eq!(decode_html_entities("&#x3C;"), "<");
        assert_eq!(decode_html_entities("&#x2014;"), "\u{2014}");
    }

    #[test]
    fn decode_html_entities_passthrough_unknown() {
        assert_eq!(decode_html_entities("&unknown;"), "&unknown;");
    }

    #[test]
    fn decode_html_entities_mixed_content() {
        let input = "Hello &amp; welcome to &quot;Rust&apos;s world&quot; &mdash; enjoy!";
        let expected = "Hello & welcome to \"Rust's world\" \u{2014} enjoy!";
        assert_eq!(decode_html_entities(input), expected);
    }

    #[test]
    fn extract_search_query_accepts_legacy_query() {
        let query =
            extract_search_query(&json!({"query": " deepseek v4 "})).expect("query should parse");
        assert_eq!(query, "deepseek v4");
    }

    #[test]
    fn extract_search_query_accepts_q_alias() {
        let query =
            extract_search_query(&json!({"q": "deepseek v4 pro"})).expect("q alias should parse");
        assert_eq!(query, "deepseek v4 pro");
    }

    #[test]
    fn extract_search_query_accepts_array_form() {
        let input = json!({"search_query": [{"q": "deepseek api", "max_results": 3}]});
        let query = extract_search_query(&input).expect("array form should parse");
        assert_eq!(query, "deepseek api");
        assert_eq!(optional_search_max_results(&input), 3);
    }

    #[test]
    fn extract_search_query_rejects_missing_query() {
        let err = extract_search_query(&json!({"max_results": 2}))
            .expect_err("missing query should fail");
        assert!(format!("{err}").contains("missing required field 'query'"));
    }

    #[test]
    fn optional_max_results_prefers_top_level_value() {
        // Top-level `max_results` wins over the array-form sibling
        // because callers using the array form usually copy-paste it
        // wholesale and then tweak the outer max_results afterwards.
        assert_eq!(
            optional_search_max_results(
                &json!({"query": "x", "max_results": 8, "search_query": [{"q": "y", "max_results": 2}]})
            ),
            8,
        );
    }

    #[test]
    fn optional_max_results_falls_back_to_array_form() {
        // When only the array form sets max_results, that value is the
        // one that should reach the caller. This is the path V4 uses
        // when it emits the structured `search_query: [{…}]` shape.
        assert_eq!(
            optional_search_max_results(&json!({"search_query": [{"q": "y", "max_results": 3}]})),
            3,
        );
    }

    #[test]
    fn optional_max_results_uses_default_when_neither_set() {
        // No explicit bound anywhere → the DEFAULT (currently 5)
        // applies, so the model can't accidentally pull MAX_RESULTS
        // worth of bandwidth just by omitting the field.
        assert_eq!(optional_search_max_results(&json!({"query": "x"})), 5);
        assert_eq!(
            optional_search_max_results(&json!({"search_query": [{"q": "y"}]})),
            5,
        );
    }

    #[test]
    fn optional_max_results_only_reads_first_array_entry() {
        // Sub-search support is a future feature; for now the array
        // entries beyond the first are ignored. Pin so a future
        // multi-query implementation has to update this test
        // intentionally rather than silently start fanning out.
        assert_eq!(
            optional_search_max_results(
                &json!({"search_query": [{"q": "first", "max_results": 1}, {"q": "second", "max_results": 9}]})
            ),
            1,
        );
    }

    #[test]
    fn extract_search_query_trims_whitespace_from_array_form_q_alias() {
        // The "trimmed" contract is part of the helper's invariant —
        // a model sometimes pads `q` with newlines from a heredoc.
        let q = extract_search_query(&json!({"search_query": [{"q": "  deepseek tui  "}]}))
            .expect("array form should parse with trim");
        assert_eq!(q, "deepseek tui");
    }

    #[test]
    fn extract_search_query_rejects_empty_query() {
        // A "" query lands in extract_search_query → propagates as
        // missing_field rather than a confusing engine error a few
        // layers down. Lock the failure mode.
        for body in [json!({"query": ""}), json!({"q": "   "}), json!({})] {
            let err = extract_search_query(&body).expect_err("empty query must reject");
            let msg = format!("{err}");
            assert!(
                msg.contains("missing required field 'query'") || msg.contains("Query"),
                "expected query-missing error, got `{msg}`"
            );
        }
    }

    #[test]
    fn truncate_error_body_truncates_long_body() {
        let body = "a".repeat(ERROR_BODY_PREVIEW_BYTES + 100);
        let truncated = truncate_error_body(&body);
        assert!(truncated.len() <= ERROR_BODY_PREVIEW_BYTES + 3);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn truncate_error_body_keeps_short_body_intact() {
        let body = "short error";
        assert_eq!(truncate_error_body(body), body);
    }

    #[test]
    fn sanitize_error_body_strips_html_and_control_chars() {
        let body = "<p>error</p>\x00\x01\x02";
        let sanitized = sanitize_error_body(body);
        assert_eq!(sanitized, "error");
    }

    #[tokio::test]
    async fn tavily_provider_without_api_key_surfaces_clear_error_not_silent_fallback() {
        // Trust-boundary pin: if a user has opted into Tavily but
        // forgot the api_key, the tool must NOT silently fall through
        // to DuckDuckGo (which would expose the query to a different
        // provider than the user authorised). Instead it returns a
        // ToolError that names the missing key explicitly.
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Tavily;
        ctx.search_api_key = None;
        let err = WebSearchTool
            .execute(json!({"query": "anything"}), &ctx)
            .await
            .expect_err("missing api_key must surface as ToolError");
        let msg = err.to_string();
        assert!(
            msg.contains("Tavily") && msg.contains("API key"),
            "error must name the provider and missing key; got `{msg}`"
        );
    }

    #[tokio::test]
    async fn bocha_provider_without_api_key_surfaces_clear_error_not_silent_fallback() {
        // Same trust-boundary pin for Bocha.
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Bocha;
        ctx.search_api_key = None;
        let err = WebSearchTool
            .execute(json!({"query": "anything"}), &ctx)
            .await
            .expect_err("missing api_key must surface as ToolError");
        let msg = err.to_string();
        assert!(
            msg.contains("Bocha") && msg.contains("API key"),
            "error must name the provider and missing key; got `{msg}`"
        );
    }
}
