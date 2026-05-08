//! Web browsing tool with multi-command support (search/open/click/find/screenshot).
//!
//! This mirrors the Codex harness `web.run` interface so models can use a single
//! tool call to perform multiple web actions and cite sources with ref_ids.

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_u64, required_str,
};
use crate::network_policy::{Decision, host_from_url};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_RESULTS: usize = 10;
const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const DEFAULT_OPEN_TIMEOUT_MS: u64 = 20_000;
const MAX_WEB_RUN_SESSIONS: usize = 64;
const MAX_PAGES_PER_SESSION: usize = 256;
const WEB_RUN_SESSION_TTL: Duration = Duration::from_secs(30 * 60);
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

static WEB_RUN_STATE: OnceLock<Mutex<WebRunState>> = OnceLock::new();

#[derive(Default)]
struct WebRunState {
    sessions: HashMap<String, WebRunSessionState>,
    pages: HashMap<String, StoredWebPage>,
}

struct WebRunSessionState {
    next_turn: u64,
    refs: VecDeque<String>,
    last_access: Instant,
}

impl Default for WebRunSessionState {
    fn default() -> Self {
        Self {
            next_turn: 0,
            refs: VecDeque::new(),
            last_access: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
struct StoredWebPage {
    namespace: String,
    page: WebPage,
}

impl WebRunState {
    fn cleanup(&mut self) {
        let now = Instant::now();
        let expired = self
            .sessions
            .iter()
            .filter_map(|(namespace, session)| {
                if now.duration_since(session.last_access) > WEB_RUN_SESSION_TTL {
                    Some(namespace.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for namespace in expired {
            self.remove_session(&namespace);
        }

        while self.sessions.len() > MAX_WEB_RUN_SESSIONS {
            let Some(oldest_namespace) = self
                .sessions
                .iter()
                .min_by_key(|(_, session)| session.last_access)
                .map(|(namespace, _)| namespace.clone())
            else {
                break;
            };
            self.remove_session(&oldest_namespace);
        }
    }

    fn remove_session(&mut self, namespace: &str) {
        if let Some(session) = self.sessions.remove(namespace) {
            for ref_id in session.refs {
                self.pages.remove(&ref_id);
            }
        }
    }

    fn touch_session(&mut self, namespace: &str) {
        self.cleanup();
        if !self.sessions.contains_key(namespace)
            && self.sessions.len() >= MAX_WEB_RUN_SESSIONS
            && let Some(oldest_namespace) = self
                .sessions
                .iter()
                .min_by_key(|(_, session)| session.last_access)
                .map(|(existing_namespace, _)| existing_namespace.clone())
        {
            self.remove_session(&oldest_namespace);
        }

        let session = self.sessions.entry(namespace.to_string()).or_default();
        session.last_access = Instant::now();
    }

    fn next_turn(&mut self, namespace: &str) -> u64 {
        self.touch_session(namespace);
        let session = self
            .sessions
            .get_mut(namespace)
            .expect("session should exist after touch");
        let current = session.next_turn;
        session.next_turn = session.next_turn.saturating_add(1);
        current
    }

    fn store_page(&mut self, namespace: &str, ref_id: &str, page: WebPage) {
        self.touch_session(namespace);
        let mut evicted_refs = Vec::new();
        {
            let session = self
                .sessions
                .get_mut(namespace)
                .expect("session should exist after touch");
            if let Some(existing_idx) = session.refs.iter().position(|existing| existing == ref_id)
            {
                session.refs.remove(existing_idx);
            }
            session.refs.push_back(ref_id.to_string());

            while session.refs.len() > MAX_PAGES_PER_SESSION {
                if let Some(evicted_ref) = session.refs.pop_front() {
                    evicted_refs.push(evicted_ref);
                }
            }
        }

        self.pages.insert(
            ref_id.to_string(),
            StoredWebPage {
                namespace: namespace.to_string(),
                page,
            },
        );
        for evicted_ref in evicted_refs {
            self.pages.remove(&evicted_ref);
        }
    }

    fn get_page(&mut self, ref_id: &str) -> Option<WebPage> {
        self.cleanup();
        let stored = self.pages.get(ref_id)?.clone();
        if let Some(session) = self.sessions.get_mut(&stored.namespace) {
            session.last_access = Instant::now();
        }
        Some(stored.page)
    }
}

#[derive(Debug, Clone, Serialize)]
struct WebLink {
    id: usize,
    url: String,
    text: String,
}

#[derive(Debug, Clone)]
struct WebPage {
    url: String,
    title: Option<String>,
    content_type: Option<String>,
    lines: Vec<String>,
    links: Vec<WebLink>,
    pdf_pages: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Clone, Copy)]
enum ResponseLength {
    Short,
    Medium,
    Long,
}

impl ResponseLength {
    fn from_input(input: Option<&Value>) -> Self {
        let raw = input.and_then(|v| v.as_str()).unwrap_or("medium");
        match raw.to_lowercase().as_str() {
            "short" => Self::Short,
            "long" => Self::Long,
            _ => Self::Medium,
        }
    }

    fn view_lines(self) -> usize {
        match self {
            Self::Short => 40,
            Self::Medium => 80,
            Self::Long => 160,
        }
    }

    fn wrap_width(self) -> usize {
        match self {
            Self::Short => 88,
            Self::Medium => 110,
            Self::Long => 140,
        }
    }

    fn max_results(self) -> usize {
        match self {
            Self::Short => 5,
            Self::Medium => 8,
            Self::Long => 10,
        }
    }

    fn max_find_matches(self) -> usize {
        match self {
            Self::Short => 8,
            Self::Medium => 15,
            Self::Long => 30,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SearchEntry {
    title: String,
    url: String,
    snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SearchResult {
    ref_id: String,
    query: String,
    source: String,
    count: usize,
    results: Vec<SearchEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PageViewResult {
    ref_id: String,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    line_start: usize,
    line_end: usize,
    total_lines: usize,
    content: String,
    links: Vec<WebLink>,
}

#[derive(Debug, Clone, Serialize)]
struct FindMatch {
    line: usize,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct FindResult {
    ref_id: String,
    pattern: String,
    count: usize,
    matches: Vec<FindMatch>,
}

#[derive(Debug, Clone, Serialize)]
struct ScreenshotResult {
    ref_id: String,
    pageno: usize,
    total_pages: usize,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
struct ImageResultEntry {
    image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumbnail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
struct ImageQueryResult {
    query: String,
    source: String,
    count: usize,
    results: Vec<ImageResultEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct WebRunOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    search_query: Option<Vec<SearchResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_query: Option<Vec<ImageQueryResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    open: Option<Vec<PageViewResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    click: Option<Vec<PageViewResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    find: Option<Vec<FindResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    screenshot: Option<Vec<ScreenshotResult>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    warnings: Vec<String>,
}

pub struct WebRunTool;

#[async_trait]
impl ToolSpec for WebRunTool {
    fn name(&self) -> &'static str {
        "web.run"
    }

    fn description(&self) -> &'static str {
        "Browse the web (search/open/click/find/screenshot/image_query) and return structured results with ref_ids for citations."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "search_query": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "q": { "type": "string" },
                            "recency": { "type": "integer" },
                            "max_results": { "type": "integer" },
                            "timeout_ms": { "type": "integer" },
                            "domains": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["q"]
                    }
                },
                "image_query": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "q": { "type": "string" },
                            "recency": { "type": "integer" },
                            "max_results": { "type": "integer" },
                            "timeout_ms": { "type": "integer" },
                            "domains": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["q"]
                    }
                },
                "open": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "ref_id": { "type": "string" },
                            "lineno": { "type": "integer" }
                        },
                        "required": ["ref_id"]
                    }
                },
                "click": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "ref_id": { "type": "string" },
                            "id": { "type": "integer" }
                        },
                        "required": ["ref_id", "id"]
                    }
                },
                "find": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "ref_id": { "type": "string" },
                            "pattern": { "type": "string" }
                        },
                        "required": ["ref_id", "pattern"]
                    }
                },
                "screenshot": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "ref_id": { "type": "string" },
                            "pageno": { "type": "integer" }
                        },
                        "required": ["ref_id", "pageno"]
                    }
                },
                "response_length": {
                    "type": "string",
                    "enum": ["short", "medium", "long"],
                    "description": "Controls result verbosity"
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
        let response_length = ResponseLength::from_input(input.get("response_length"));
        let mut output = WebRunOutput::default();
        let scope = scoped_ref_prefix(&context.state_namespace);
        let turn = with_state(|state| state.next_turn(&context.state_namespace));

        let mut search_counter = 0usize;
        let mut view_counter = 0usize;
        let mut click_counter = 0usize;

        if let Some(searches) = input.get("search_query").and_then(|v| v.as_array()) {
            let mut results = Vec::new();
            for search in searches {
                let query = required_str(search, "q")?.trim().to_string();
                if query.is_empty() {
                    continue;
                }
                let recency = optional_u64(search, "recency", 0);
                let max_results = usize::try_from(optional_u64(
                    search,
                    "max_results",
                    response_length.max_results() as u64,
                ))
                .unwrap_or(response_length.max_results())
                .clamp(1, MAX_RESULTS);
                let timeout_ms = optional_u64(search, "timeout_ms", DEFAULT_TIMEOUT_MS).min(60_000);

                let domains = search
                    .get("domains")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                let (entries, source, warning) =
                    run_search(&query, max_results, timeout_ms, &domains).await?;
                let mut warnings = Vec::new();
                if recency > 0 {
                    warnings.push(format!(
                        "Recency filter not enforced (requested last {recency} days)"
                    ));
                }
                if let Some(w) = warning {
                    warnings.push(w);
                }
                search_counter += 1;
                let ref_id = format!("{scope}turn{turn}search{search_counter}");

                let page = page_from_search(&query, &entries);
                store_page(&context.state_namespace, &ref_id, page);

                results.push(SearchResult {
                    ref_id,
                    query,
                    source,
                    count: entries.len(),
                    results: entries,
                    warning: if warnings.is_empty() {
                        None
                    } else {
                        Some(warnings.join("; "))
                    },
                });
            }
            if !results.is_empty() {
                output.search_query = Some(results);
            }
        }

        if let Some(images) = input.get("image_query").and_then(|v| v.as_array()) {
            let mut results = Vec::new();
            for image in images {
                let query = required_str(image, "q")?.trim().to_string();
                if query.is_empty() {
                    continue;
                }
                let recency = optional_u64(image, "recency", 0);
                let max_results = usize::try_from(optional_u64(
                    image,
                    "max_results",
                    response_length.max_results() as u64,
                ))
                .unwrap_or(response_length.max_results())
                .clamp(1, MAX_RESULTS);
                let timeout_ms = optional_u64(image, "timeout_ms", DEFAULT_TIMEOUT_MS).min(60_000);

                let domains = image
                    .get("domains")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                let (entries, warning) =
                    run_image_search(&query, max_results, timeout_ms, &domains).await?;

                let mut warnings = Vec::new();
                if recency > 0 {
                    warnings.push(format!(
                        "Recency filter not enforced (requested last {recency} days)"
                    ));
                }
                if let Some(w) = warning {
                    warnings.push(w);
                }

                results.push(ImageQueryResult {
                    query,
                    source: "duckduckgo_images".to_string(),
                    count: entries.len(),
                    results: entries,
                    warning: if warnings.is_empty() {
                        None
                    } else {
                        Some(warnings.join("; "))
                    },
                });
            }
            if !results.is_empty() {
                output.image_query = Some(results);
            }
        }

        if let Some(opens) = input.get("open").and_then(|v| v.as_array()) {
            let mut views = Vec::new();
            for open in opens {
                let ref_id = required_str(open, "ref_id")?.to_string();
                let lineno = optional_u64(open, "lineno", 1).max(1) as usize;

                let page = resolve_or_fetch_page(&ref_id, DEFAULT_OPEN_TIMEOUT_MS, context).await?;
                view_counter += 1;
                let view_ref = format!("{scope}turn{turn}view{view_counter}");
                store_page(&context.state_namespace, &view_ref, page.clone());

                let view = render_view(&view_ref, &page, lineno, response_length);
                views.push(view);
            }
            if !views.is_empty() {
                output.open = Some(views);
            }
        }

        if let Some(clicks) = input.get("click").and_then(|v| v.as_array()) {
            let mut views = Vec::new();
            for click in clicks {
                let ref_id = required_str(click, "ref_id")?.to_string();
                let link_id = optional_u64(click, "id", 0) as usize;
                if link_id == 0 {
                    return Err(ToolError::invalid_input("click.id must be >= 1"));
                }
                let page = get_page(&ref_id).ok_or_else(|| {
                    ToolError::invalid_input(format!("Unknown ref_id '{ref_id}'"))
                })?;
                let link = page.links.iter().find(|l| l.id == link_id).ok_or_else(|| {
                    ToolError::invalid_input(format!(
                        "Link id {link_id} not found for ref_id '{ref_id}'"
                    ))
                })?;
                let target = link.url.clone();
                let fetched =
                    resolve_or_fetch_page(&target, DEFAULT_OPEN_TIMEOUT_MS, context).await?;
                click_counter += 1;
                let click_ref = format!("{scope}turn{turn}click{click_counter}");
                store_page(&context.state_namespace, &click_ref, fetched.clone());
                let view = render_view(&click_ref, &fetched, 1, response_length);
                views.push(view);
            }
            if !views.is_empty() {
                output.click = Some(views);
            }
        }

        if let Some(find_requests) = input.get("find").and_then(|v| v.as_array()) {
            let mut finds = Vec::new();
            for find_req in find_requests {
                let ref_id = required_str(find_req, "ref_id")?.to_string();
                let pattern = required_str(find_req, "pattern")?.to_string();
                let page = get_page(&ref_id).ok_or_else(|| {
                    ToolError::invalid_input(format!("Unknown ref_id '{ref_id}'"))
                })?;
                let find_result = find_in_page(&ref_id, &pattern, &page, response_length);
                finds.push(find_result);
            }
            if !finds.is_empty() {
                output.find = Some(finds);
            }
        }

        if let Some(shots) = input.get("screenshot").and_then(|v| v.as_array()) {
            let mut screenshots = Vec::new();
            for shot in shots {
                let ref_id = required_str(shot, "ref_id")?.to_string();
                let pageno = optional_u64(shot, "pageno", 0) as usize;
                let page = get_page(&ref_id).ok_or_else(|| {
                    ToolError::invalid_input(format!("Unknown ref_id '{ref_id}'"))
                })?;
                let screenshot = screenshot_page(&ref_id, pageno, &page)?;
                screenshots.push(screenshot);
            }
            if !screenshots.is_empty() {
                output.screenshot = Some(screenshots);
            }
        }

        ToolResult::json(&output).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

fn with_state<T>(f: impl FnOnce(&mut WebRunState) -> T) -> T {
    let lock = WEB_RUN_STATE.get_or_init(|| Mutex::new(WebRunState::default()));
    let mut state = lock
        .lock()
        .expect("web run state mutex should not be poisoned");
    state.cleanup();
    f(&mut state)
}

fn scoped_ref_prefix(namespace: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    namespace.hash(&mut hasher);
    format!("s{:016x}_", hasher.finish())
}

fn store_page(namespace: &str, ref_id: &str, page: WebPage) {
    with_state(|state| {
        state.store_page(namespace, ref_id, page);
    });
}

fn get_page(ref_id: &str) -> Option<WebPage> {
    with_state(|state| state.get_page(ref_id))
}

#[cfg(test)]
fn reset_web_run_state() {
    with_state(|state| {
        *state = WebRunState::default();
    });
}

#[cfg(test)]
fn next_turn_for_namespace(namespace: &str) -> u64 {
    with_state(|state| state.next_turn(namespace))
}

async fn resolve_or_fetch_page(
    ref_id: &str,
    timeout_ms: u64,
    context: &ToolContext,
) -> Result<WebPage, ToolError> {
    if let Some(page) = get_page(ref_id) {
        return Ok(page);
    }
    if looks_like_url(ref_id) {
        check_network_policy(ref_id, context)?;
        return fetch_page(ref_id, timeout_ms).await;
    }
    Err(ToolError::invalid_input(format!(
        "Unknown ref_id '{ref_id}'"
    )))
}

fn looks_like_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

async fn run_search(
    query: &str,
    max_results: usize,
    timeout_ms: u64,
    domains: &[String],
) -> Result<(Vec<SearchEntry>, String, Option<String>), ToolError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ToolError::execution_failed(format!("Failed to build HTTP client: {e}")))?;

    let encoded = url_encode(query);
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
    let mut source = "duckduckgo".to_string();
    let mut warnings = Vec::new();

    if results.is_empty() {
        let duckduckgo_blocked = is_duckduckgo_challenge(&body);
        match run_bing_search(&client, query, max_results).await {
            Ok(fallback_results) if !fallback_results.is_empty() => {
                results = fallback_results;
                source = "bing".to_string();
                warnings.push(if duckduckgo_blocked {
                    "DuckDuckGo returned a bot challenge; used Bing fallback".to_string()
                } else {
                    "DuckDuckGo returned no parseable results; used Bing fallback".to_string()
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

    if !domains.is_empty() {
        let before = results.len();
        results.retain(|entry| domain_matches(&entry.url, domains));
        if before != results.len() {
            warnings.push("Filtered search results by domain list".to_string());
        }
    }

    Ok((
        results,
        source,
        if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("; "))
        },
    ))
}

async fn run_bing_search(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchEntry>, ToolError> {
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
        .map_err(|e| ToolError::execution_failed(format!("Bing fallback request failed: {e}")))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
        ToolError::execution_failed(format!("Failed to read Bing fallback response: {e}"))
    })?;

    if !status.is_success() {
        return Err(ToolError::execution_failed(format!(
            "Bing fallback failed: HTTP {}",
            status.as_u16()
        )));
    }

    Ok(parse_bing_results(&body, max_results))
}

fn domain_matches(url: &str, domains: &[String]) -> bool {
    if domains.is_empty() {
        return true;
    }
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    domains.iter().any(|domain| {
        let domain = domain.trim_start_matches("www.");
        host == domain || host.ends_with(&format!(".{domain}"))
    })
}

#[derive(Debug, Clone, Deserialize)]
struct DuckDuckGoImageResponse {
    #[serde(default)]
    results: Vec<DuckDuckGoImageResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct DuckDuckGoImageResult {
    image: String,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

fn extract_duckduckgo_vqd(html: &str) -> Option<String> {
    let html = html.trim();
    if html.is_empty() {
        return None;
    }

    for (prefix, suffix) in [("vqd='", "'"), ("vqd=\"", "\"")] {
        if let Some(start) = html.find(prefix) {
            let rest = &html[start + prefix.len()..];
            if let Some(end) = rest.find(suffix) {
                let token = rest[..end].trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    // Fallback: look for `vqd=` and accept a conservative token charset.
    if let Some(start) = html.find("vqd=") {
        let rest = &html[start + 4..];
        let mut token = String::new();
        for ch in rest.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                token.push(ch);
            } else {
                break;
            }
        }
        if !token.is_empty() {
            return Some(token);
        }
    }

    None
}

async fn run_image_search(
    query: &str,
    max_results: usize,
    timeout_ms: u64,
    domains: &[String],
) -> Result<(Vec<ImageResultEntry>, Option<String>), ToolError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ToolError::execution_failed(format!("Failed to build HTTP client: {e}")))?;

    // Step 1: fetch the HTML page to obtain the `vqd` token used by the images API.
    let encoded = url_encode(query);
    let seed_url = format!("https://duckduckgo.com/?q={encoded}&iax=images&ia=images");
    let seed_resp = client
        .get(&seed_url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.5")
        .send()
        .await
        .map_err(|e| {
            ToolError::execution_failed(format!("Image search seed request failed: {e}"))
        })?;

    let seed_status = seed_resp.status();
    let seed_body = seed_resp.text().await.map_err(|e| {
        ToolError::execution_failed(format!("Failed to read image seed response: {e}"))
    })?;

    if !seed_status.is_success() {
        return Err(ToolError::execution_failed(format!(
            "Image search seed request failed: HTTP {}",
            seed_status.as_u16()
        )));
    }

    let vqd = extract_duckduckgo_vqd(&seed_body).ok_or_else(|| {
        ToolError::execution_failed("Failed to extract DuckDuckGo image token (vqd)")
    })?;

    // Step 2: query the DuckDuckGo images JSON endpoint.
    let api_url = format!("https://duckduckgo.com/i.js?l=us-en&o=json&q={encoded}&vqd={vqd}&p=1");
    let api_resp = client
        .get(&api_url)
        .header("Accept", "application/json")
        .header("Referer", "https://duckduckgo.com/")
        .send()
        .await
        .map_err(|e| ToolError::execution_failed(format!("Image search request failed: {e}")))?;

    let api_status = api_resp.status();
    let api_body = api_resp
        .text()
        .await
        .map_err(|e| ToolError::execution_failed(format!("Failed to read image response: {e}")))?;

    if !api_status.is_success() {
        return Err(ToolError::execution_failed(format!(
            "Image search failed: HTTP {}",
            api_status.as_u16()
        )));
    }

    let parsed: DuckDuckGoImageResponse = serde_json::from_str(&api_body).map_err(|e| {
        ToolError::execution_failed(format!("Failed to parse image search JSON: {e}"))
    })?;

    let mut results = parsed
        .results
        .into_iter()
        .filter(|item| !item.image.trim().is_empty())
        .map(|item| ImageResultEntry {
            image: item.image,
            thumbnail: item.thumbnail,
            title: item.title,
            url: item.url,
            source: item.source,
            width: item.width,
            height: item.height,
        })
        .collect::<Vec<_>>();

    // Domain filter is applied to the source page URL when available.
    let warning = if !domains.is_empty() {
        let before = results.len();
        results.retain(|entry| match entry.url.as_deref() {
            Some(url) => domain_matches(url, domains),
            None => true,
        });
        if before != results.len() {
            Some("Filtered image results by domain list".to_string())
        } else {
            None
        }
    } else {
        None
    };

    results.truncate(max_results);
    Ok((results, warning))
}

fn page_from_search(query: &str, results: &[SearchEntry]) -> WebPage {
    let mut lines = Vec::new();
    let mut links = Vec::new();

    lines.push(format!("Search results for: {query}"));
    for (idx, entry) in results.iter().enumerate() {
        let id = idx + 1;
        links.push(WebLink {
            id,
            url: entry.url.clone(),
            text: entry.title.clone(),
        });
        lines.push(format!("{}. [{}] {}", id, id, entry.title));
        if let Some(snippet) = entry.snippet.as_ref()
            && !snippet.trim().is_empty()
        {
            lines.push(format!("    {snippet}"));
        }
        lines.push(format!("    {url}", url = entry.url));
    }

    WebPage {
        url: "https://html.duckduckgo.com/html/".to_string(),
        title: Some("Search Results".to_string()),
        content_type: Some("text/html".to_string()),
        lines,
        links,
        pdf_pages: None,
    }
}

/// Check network policy for a URL before fetching.
/// Returns an error if the policy denies access.
fn check_network_policy(url: &str, context: &ToolContext) -> Result<(), ToolError> {
    let Some(decider) = context.network_policy.as_ref() else {
        return Ok(());
    };
    let Some(host) = host_from_url(url) else {
        return Ok(());
    };
    match decider.evaluate(&host, "web_run") {
        Decision::Allow => Ok(()),
        Decision::Deny => Err(ToolError::permission_denied(format!(
            "network call to '{host}' blocked by network policy"
        ))),
        Decision::Prompt => Err(ToolError::permission_denied(format!(
            "network call to '{host}' requires approval; \
             re-run after `/network allow {host}` or set network.default = \"allow\" in config"
        ))),
    }
}

async fn fetch_page(url: &str, timeout_ms: u64) -> Result<WebPage, ToolError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ToolError::execution_failed(format!("Failed to build HTTP client: {e}")))?;

    let resp = client
        .get(url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.5")
        .send()
        .await
        .map_err(|e| ToolError::execution_failed(format!("Web request failed: {e}")))?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ToolError::execution_failed(format!("Failed to read response: {e}")))?;

    if !status.is_success() {
        return Err(ToolError::execution_failed(format!(
            "Web request failed: HTTP {}",
            status.as_u16()
        )));
    }

    if is_pdf(&content_type, url) {
        return parse_pdf_page(url, content_type, &bytes);
    }

    let body = String::from_utf8_lossy(&bytes).to_string();
    let (lines, links, title) = parse_html(&body, url);

    Ok(WebPage {
        url: url.to_string(),
        title,
        content_type,
        lines,
        links,
        pdf_pages: None,
    })
}

fn is_pdf(content_type: &Option<String>, url: &str) -> bool {
    if let Some(ct) = content_type
        && ct.to_lowercase().contains("application/pdf")
    {
        return true;
    }
    url.to_lowercase().ends_with(".pdf")
}

fn parse_pdf_page(
    url: &str,
    content_type: Option<String>,
    bytes: &[u8],
) -> Result<WebPage, ToolError> {
    let text = pdf_extract_text(bytes)?;
    let pages = split_pdf_pages(&text);
    let lines = pages.first().cloned().unwrap_or_default();

    Ok(WebPage {
        url: url.to_string(),
        title: Some("PDF Document".to_string()),
        content_type,
        lines,
        links: Vec::new(),
        pdf_pages: Some(pages),
    })
}

fn pdf_extract_text(bytes: &[u8]) -> Result<String, ToolError> {
    pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| ToolError::execution_failed(format!("PDF extract failed: {e}")))
}

fn split_pdf_pages(text: &str) -> Vec<Vec<String>> {
    let raw_pages: Vec<&str> = text.split('\x0C').collect();
    raw_pages
        .iter()
        .map(|page| {
            page.lines()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn render_view(
    ref_id: &str,
    page: &WebPage,
    lineno: usize,
    response: ResponseLength,
) -> PageViewResult {
    let total = page.lines.len();
    let view_lines = response.view_lines();
    let start = if total == 0 {
        1
    } else if lineno > total {
        total.saturating_sub(view_lines.saturating_sub(1)).max(1)
    } else {
        lineno
    };
    let end = if total == 0 {
        0
    } else {
        (start + view_lines - 1).min(total)
    };

    let content = if total == 0 {
        "(no content)".to_string()
    } else {
        render_lines(&page.lines, start, end)
    };

    PageViewResult {
        ref_id: ref_id.to_string(),
        url: page.url.clone(),
        title: page.title.clone(),
        content_type: page.content_type.clone(),
        line_start: start,
        line_end: end,
        total_lines: total,
        content,
        links: page.links.clone(),
    }
}

fn render_lines(lines: &[String], start: usize, end: usize) -> String {
    lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| {
            let line_no = idx + 1;
            if line_no < start || line_no > end {
                return None;
            }
            Some(format!("{:>4} {}", line_no, line))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_in_page(
    ref_id: &str,
    pattern: &str,
    page: &WebPage,
    response: ResponseLength,
) -> FindResult {
    let needle = pattern.to_lowercase();
    let mut matches = Vec::new();
    for (idx, line) in page.lines.iter().enumerate() {
        if line.to_lowercase().contains(&needle) {
            matches.push(FindMatch {
                line: idx + 1,
                text: line.clone(),
            });
        }
        if matches.len() >= response.max_find_matches() {
            break;
        }
    }

    FindResult {
        ref_id: ref_id.to_string(),
        pattern: pattern.to_string(),
        count: matches.len(),
        matches,
    }
}

fn screenshot_page(
    ref_id: &str,
    pageno: usize,
    page: &WebPage,
) -> Result<ScreenshotResult, ToolError> {
    let pages = page
        .pdf_pages
        .as_ref()
        .ok_or_else(|| ToolError::invalid_input("screenshot is only supported for PDF pages"))?;
    if pages.is_empty() {
        return Err(ToolError::execution_failed("PDF has no pages"));
    }
    if pageno >= pages.len() {
        return Err(ToolError::invalid_input(format!(
            "pageno {pageno} out of range (0..{max})",
            max = pages.len().saturating_sub(1)
        )));
    }
    let content = pages[pageno].join("\n");
    Ok(ScreenshotResult {
        ref_id: ref_id.to_string(),
        pageno,
        total_pages: pages.len(),
        content,
    })
}

// === HTML Parsing ===

static ANCHOR_RE: OnceLock<Regex> = OnceLock::new();
static TAG_RE: OnceLock<Regex> = OnceLock::new();
static BLOCK_RE: OnceLock<Regex> = OnceLock::new();
static SCRIPT_RE: OnceLock<Regex> = OnceLock::new();
static STYLE_RE: OnceLock<Regex> = OnceLock::new();
static TITLE_RE: OnceLock<Regex> = OnceLock::new();
static SNIPPET_RE: OnceLock<Regex> = OnceLock::new();
static SEARCH_TITLE_RE: OnceLock<Regex> = OnceLock::new();
static BING_RESULT_RE: OnceLock<Regex> = OnceLock::new();
static BING_TITLE_RE: OnceLock<Regex> = OnceLock::new();
static BING_SNIPPET_RE: OnceLock<Regex> = OnceLock::new();

fn get_anchor_re() -> &'static Regex {
    ANCHOR_RE.get_or_init(|| {
        Regex::new(r#"(?is)<a\s+[^>]*href\s*=\s*['\"]([^'\"]+)['\"][^>]*>(.*?)</a>"#)
            .expect("anchor regex")
    })
}

fn get_tag_re() -> &'static Regex {
    TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").expect("tag regex"))
}

fn get_block_re() -> &'static Regex {
    BLOCK_RE.get_or_init(|| {
        Regex::new(r"(?is)</?(p|div|li|ul|ol|br|h[1-6]|tr|td|th|table|section|article)[^>]*>")
            .expect("block regex")
    })
}

fn get_script_re() -> &'static Regex {
    SCRIPT_RE.get_or_init(|| Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap())
}

fn get_style_re() -> &'static Regex {
    STYLE_RE.get_or_init(|| Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap())
}

fn get_title_re() -> &'static Regex {
    TITLE_RE.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap())
}

fn get_search_title_re() -> &'static Regex {
    SEARCH_TITLE_RE.get_or_init(|| {
        Regex::new(r#"<a[^>]*class=\"result__a\"[^>]*href=\"([^\"]+)\"[^>]*>(.*?)</a>"#)
            .expect("title regex pattern is valid")
    })
}

fn get_search_snippet_re() -> &'static Regex {
    SNIPPET_RE.get_or_init(|| {
        Regex::new(
            r#"<a[^>]*class=\"result__snippet\"[^>]*>(.*?)</a>|<div[^>]*class=\"result__snippet\"[^>]*>(.*?)</div>"#,
        )
        .expect("snippet regex pattern is valid")
    })
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

fn parse_html(html: &str, base_url: &str) -> (Vec<String>, Vec<WebLink>, Option<String>) {
    let title = extract_title(html);
    let without_scripts = get_script_re().replace_all(html, "").to_string();
    let without_styles = get_style_re().replace_all(&without_scripts, "").to_string();

    let (with_links, links) = replace_links(&without_styles, base_url);
    let with_breaks = get_block_re().replace_all(&with_links, "\n").to_string();
    let without_tags = get_tag_re().replace_all(&with_breaks, "").to_string();
    let decoded = decode_html_entities(&without_tags);

    let mut lines = Vec::new();
    for line in decoded.lines() {
        let trimmed = normalize_whitespace(line);
        if trimmed.is_empty() {
            continue;
        }
        for wrapped in wrap_line(&trimmed, ResponseLength::Medium.wrap_width()) {
            lines.push(wrapped);
        }
    }

    (lines, links, title)
}

fn extract_title(html: &str) -> Option<String> {
    let re = get_title_re();
    let cap = re.captures(html)?;
    let raw = cap.get(1)?.as_str();
    let cleaned = normalize_whitespace(&decode_html_entities(raw));
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn replace_links(html: &str, base_url: &str) -> (String, Vec<WebLink>) {
    let re = get_anchor_re();
    let mut links = Vec::new();
    let mut output = String::with_capacity(html.len());
    let mut last = 0;

    for cap in re.captures_iter(html) {
        let Some(full) = cap.get(0) else { continue };
        let Some(href) = cap.get(1) else { continue };
        let Some(text_match) = cap.get(2) else {
            continue;
        };

        output.push_str(&html[last..full.start()]);
        let text = normalize_whitespace(&strip_tags(text_match.as_str()));
        let resolved = resolve_url(base_url, href.as_str());
        if !text.is_empty() {
            let id = links.len() + 1;
            links.push(WebLink {
                id,
                url: resolved.clone(),
                text: text.clone(),
            });
            output.push_str(&format!("[{}] {}", id, text));
        } else {
            output.push_str(&resolved);
        }
        last = full.end();
    }

    output.push_str(&html[last..]);
    (output, links)
}

fn resolve_url(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    if let Ok(base_url) = reqwest::Url::parse(base)
        && let Ok(joined) = base_url.join(href)
    {
        return joined.to_string();
    }
    href.to_string()
}

fn strip_tags(text: &str) -> String {
    get_tag_re().replace_all(text, "").to_string()
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wrap_line(text: &str, width: usize) -> Vec<String> {
    if text.len() <= width {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + word.len() < width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn decode_html_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

fn parse_duckduckgo_results(html: &str, max_results: usize) -> Vec<SearchEntry> {
    let title_re = get_search_title_re();
    let snippet_re = get_search_snippet_re();
    let snippets: Vec<String> = snippet_re
        .captures_iter(html)
        .filter_map(|cap| cap.get(1).or_else(|| cap.get(2)))
        .map(|m| normalize_whitespace(&decode_html_entities(&strip_tags(m.as_str()))))
        .collect();

    let mut results = Vec::new();
    for (idx, cap) in title_re.captures_iter(html).enumerate() {
        if results.len() >= max_results {
            break;
        }
        let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_raw = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = normalize_whitespace(&decode_html_entities(&strip_tags(title_raw)));
        if title.is_empty() {
            continue;
        }
        let url = normalize_search_url(href);
        let snippet = snippets
            .get(idx)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        results.push(SearchEntry {
            title,
            url,
            snippet,
        });
    }

    results
}

fn is_duckduckgo_challenge(html: &str) -> bool {
    html.contains("anomaly-modal") || html.contains("Unfortunately, bots use DuckDuckGo too")
}

fn parse_bing_results(html: &str, max_results: usize) -> Vec<SearchEntry> {
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
        let title = normalize_whitespace(&decode_html_entities(&strip_tags(title_raw)));
        if title.is_empty() {
            continue;
        }
        let snippet = get_bing_snippet_re()
            .captures(block)
            .and_then(|snippet_cap| snippet_cap.get(1))
            .map(|m| normalize_whitespace(&decode_html_entities(&strip_tags(m.as_str()))))
            .filter(|s| !s.is_empty());

        results.push(SearchEntry {
            title,
            url: normalize_bing_url(href),
            snippet,
        });
    }

    results
}

fn normalize_search_url(href: &str) -> String {
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
            && looks_like_url(&url)
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

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let query_start = url.find('?')?;
    let query = &url[query_start + 1..];
    for part in query.split('&') {
        let (k, v) = part.split_once('=')?;
        if k == key {
            return Some(v.to_string());
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%'
            && idx + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[idx + 1..idx + 3])
            && let Ok(val) = u8::from_str_radix(hex, 16)
        {
            out.push(val);
            idx += 3;
            continue;
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn url_encode(input: &str) -> String {
    crate::utils::url_encode(input)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_page(url: &str) -> WebPage {
        WebPage {
            url: url.to_string(),
            title: Some("Example".to_string()),
            content_type: Some("text/html".to_string()),
            lines: vec!["example line".to_string()],
            links: Vec::new(),
            pdf_pages: None,
        }
    }

    #[test]
    fn html_link_parsing_extracts_links() {
        let html = r#"
            <html><body>
            <p>Hello <a href="https://example.com">Example</a> world.</p>
            </body></html>
        "#;
        let (lines, links, title) = parse_html(html, "https://example.com");
        assert!(title.is_none());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://example.com");
        assert!(lines.iter().any(|line| line.contains("Example")));
    }

    #[test]
    fn wrap_line_splits_long_lines() {
        let line = "This is a long line that should wrap cleanly at word boundaries";
        let wrapped = wrap_line(line, 20);
        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().all(|l| l.len() <= 20));
    }

    #[test]
    fn extracts_duckduckgo_vqd_token() {
        let html_single = "<script>var x = {vqd='3-1234567890'};</script>";
        assert_eq!(
            extract_duckduckgo_vqd(html_single),
            Some("3-1234567890".to_string())
        );

        let html_double = "<script>var x = {vqd=\"3-abcdef\"};</script>";
        assert_eq!(
            extract_duckduckgo_vqd(html_double),
            Some("3-abcdef".to_string())
        );

        let html_plain = "https://duckduckgo.com/?q=test&vqd=3-xyz_123&ia=images";
        assert_eq!(
            extract_duckduckgo_vqd(html_plain),
            Some("3-xyz_123".to_string())
        );
    }

    #[test]
    fn parses_bing_results_and_decodes_redirect_url() {
        let html = r#"
            <ol>
              <li class="b_algo">
                <h2><a href="https://www.bing.com/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9wYXRoP3E9MQ">Example &amp; Result</a></h2>
                <div class="b_caption"><p>A <strong>useful</strong> snippet.</p></div>
              </li>
            </ol>
        "#;

        let results = parse_bing_results(html, 5);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example & Result");
        assert_eq!(results[0].url, "https://example.com/path?q=1");
        assert_eq!(results[0].snippet.as_deref(), Some("A useful snippet."));
    }

    #[test]
    fn percent_decode_handles_utf8_multibyte_sequences() {
        // Percent-encoded CJK: %E4%B8%AA%E4%BA%BA = 个人 (each glyph is 3 UTF-8 bytes).
        assert_eq!(percent_decode("Hello %E4%B8%AA%E4%BA%BA"), "Hello 个人");
        assert_eq!(percent_decode("%E7%B4%A0%E6%9D%90"), "素材");
        // Percent-encoded UTF-8 inside a URL path (DuckDuckGo `uddg=` redirect shape).
        assert_eq!(
            percent_decode("https://example.com/%E9%A1%B5%E9%9D%A2"),
            "https://example.com/页面"
        );
        // Raw UTF-8 in the input passes through unchanged.
        assert_eq!(percent_decode("查询 keyword"), "查询 keyword");
        // ASCII-only inputs preserve existing behavior; `+` stays literal.
        assert_eq!(percent_decode("foo+bar%20baz"), "foo+bar baz");
    }

    #[test]
    fn scoped_ref_prefix_is_session_specific() {
        reset_web_run_state();
        let alpha = scoped_ref_prefix("session-alpha");
        let beta = scoped_ref_prefix("session-beta");

        assert_ne!(alpha, beta);
        assert!(alpha.starts_with('s'));
        assert!(alpha.ends_with('_'));
        assert_eq!(alpha.len(), 18);
    }

    #[test]
    fn stored_pages_do_not_cross_scoped_sessions() {
        reset_web_run_state();
        let shared_suffix = "turn1search1";
        let ref_alpha = format!("{}{}", scoped_ref_prefix("session-alpha"), shared_suffix);
        let ref_beta = format!("{}{}", scoped_ref_prefix("session-beta"), shared_suffix);

        store_page(
            "session-alpha",
            &ref_alpha,
            sample_page("https://example.com/alpha"),
        );

        assert!(get_page(&ref_alpha).is_some());
        assert!(get_page(&ref_beta).is_none());
    }

    #[test]
    fn turn_counters_are_scoped_per_session() {
        reset_web_run_state();

        assert_eq!(next_turn_for_namespace("session-alpha"), 0);
        assert_eq!(next_turn_for_namespace("session-alpha"), 1);
        assert_eq!(next_turn_for_namespace("session-beta"), 0);
    }

    #[test]
    fn stale_session_pages_are_evicted() {
        reset_web_run_state();
        let namespace = "session-alpha";
        let ref_id = format!("{}turn0search1", scoped_ref_prefix(namespace));
        store_page(namespace, &ref_id, sample_page("https://example.com/alpha"));

        // On Windows, Instant's epoch is system boot.  If the CI runner has
        // been up for less than WEB_RUN_SESSION_TTL the subtraction would
        // underflow, so we skip the test in that case.
        let stale = WEB_RUN_SESSION_TTL + Duration::from_secs(1);
        let can_test = with_state(|state| {
            let session = state
                .sessions
                .get_mut(namespace)
                .expect("session should exist");
            match Instant::now().checked_sub(stale) {
                Some(past) => {
                    session.last_access = past;
                    true
                }
                None => false,
            }
        });
        if !can_test {
            // System uptime shorter than session TTL; can't test eviction.
            return;
        }

        let _ = next_turn_for_namespace("session-beta");

        assert!(get_page(&ref_id).is_none());
    }

    #[test]
    fn direct_urls_remain_compatible_open_refs() {
        assert!(looks_like_url("https://example.com"));
        assert!(looks_like_url("http://example.com"));
        assert!(!looks_like_url("turn0search0"));
    }

    #[test]
    fn network_policy_denies_direct_open_url() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Deny.into(),
            allow: vec!["api.deepseek.com".to_string()],
            deny: vec![],
            proxy: Vec::new(),
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ctx = ToolContext::new(PathBuf::from(".")).with_network_policy(decider);

        let err = check_network_policy("https://example.com/private", &ctx)
            .expect_err("blocked host should fail");
        assert!(format!("{err}").contains("blocked by network policy"));
    }
}
