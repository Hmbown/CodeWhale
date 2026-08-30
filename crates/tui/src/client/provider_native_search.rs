//! Narrow provider-native web-search client.
//!
//! This adapter reuses the active route's authenticated HTTP client without
//! exposing credentials to tool code. Route capability facts decide whether
//! the adapter is attached; this module only speaks the three documented
//! first-party wire contracts.

use anyhow::{Context, Result, bail};
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{Value, json};

use crate::config::ApiProvider;

use super::{DeepSeekClient, api_url, responses_api_url};

mod zai;

mod kimi;

const MAX_NATIVE_ANSWER_CHARS: usize = 4_000;

#[derive(Clone)]
pub(crate) struct ProviderNativeSearchClient {
    pub(super) inner: DeepSeekClient,
}

#[derive(Clone)]
pub(crate) struct ProviderNativeSearchRequest {
    pub(crate) query: String,
    pub(crate) max_results: u8,
    pub(crate) domains: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderNativeCitation {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) snippet: Option<String>,
    pub(crate) published: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderNativeSearchResponse {
    pub(crate) answer: Option<String>,
    pub(crate) citations: Vec<ProviderNativeCitation>,
}

impl ProviderNativeSearchClient {
    #[must_use]
    pub(crate) fn new(inner: DeepSeekClient) -> Option<Self> {
        matches!(
            inner.api_provider,
            ApiProvider::Openai
                | ApiProvider::Anthropic
                | ApiProvider::Xai
                | ApiProvider::XiaomiMimo
                | ApiProvider::Zai
                | ApiProvider::ModelstudioTokenPlan
                | ApiProvider::Deepseek
                | ApiProvider::DeepseekCN
                | ApiProvider::Moonshot
        )
        .then_some(Self { inner })
    }

    #[must_use]
    pub(crate) fn provider(&self) -> ApiProvider {
        self.inner.api_provider
    }

    #[must_use]
    pub(crate) fn model(&self) -> &str {
        &self.inner.default_model
    }

    #[must_use]
    pub(crate) fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    #[must_use]
    pub(crate) fn host(&self) -> Option<String> {
        reqwest::Url::parse(&self.inner.base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    }

    #[must_use]
    pub(crate) fn cache_identity(&self) -> String {
        format!(
            "provider-native://{}/{}/{}",
            self.inner.api_provider.as_str(),
            self.host().as_deref().unwrap_or("unknown-host"),
            self.inner.default_model
        )
    }

    #[must_use]
    pub(crate) const fn maximum_domain_count(&self) -> Option<usize> {
        match self.inner.api_provider {
            ApiProvider::Xai => Some(5),
            ApiProvider::Openai => Some(100),
            ApiProvider::Anthropic => None,
            _ => Some(0),
        }
    }

    pub(crate) async fn search(
        &self,
        request: &ProviderNativeSearchRequest,
    ) -> Result<ProviderNativeSearchResponse> {
        // This adapter performs model-backed inference directly instead of
        // calling `DeepSeekClient::create_message*`. It must therefore join
        // the same attached-run ownership boundary explicitly. The guard is
        // retained through response decode so a relay writer cannot start
        // while this result is still able to feed the interactive turn.
        let _inference = self.inner.acquire_remote_control_inference_permit().await;
        if self.inner.api_provider == ApiProvider::Moonshot {
            // Kimi/Moonshot runs a bounded multi-round agentic search with its
            // own request/reply loop, so it cannot share the single-shot body
            // dispatch below. It still runs under the inference permit above.
            let mut parsed = kimi::search(self, request).await?;
            parsed.citations.truncate(usize::from(request.max_results));
            return Ok(parsed);
        }
        let body = match self.inner.api_provider {
            ApiProvider::Openai => build_responses_search_body(
                &self.inner.default_model,
                request,
                ResponsesSearchDialect::Openai,
            ),
            ApiProvider::Xai => build_responses_search_body(
                &self.inner.default_model,
                request,
                ResponsesSearchDialect::Xai,
            ),
            ApiProvider::ModelstudioTokenPlan => build_responses_search_body(
                &self.inner.default_model,
                request,
                ResponsesSearchDialect::ModelStudio,
            ),
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => build_responses_search_body(
                &self.inner.default_model,
                request,
                ResponsesSearchDialect::Deepseek,
            ),
            ApiProvider::Anthropic => {
                let route_cap = self
                    .inner
                    .effective_max_output_tokens(&self.inner.default_model);
                build_anthropic_search_body(
                    &self.inner.default_model,
                    request,
                    2_048_u32.min(route_cap),
                )
            }
            ApiProvider::XiaomiMimo => build_mimo_search_body(&self.inner.default_model, request),
            ApiProvider::Zai => zai::build_body(request, &self.inner.base_url)?,
            _ => bail!("active provider has no native web-search adapter"),
        };
        let url = match self.inner.api_provider {
            ApiProvider::Openai | ApiProvider::Xai | ApiProvider::ModelstudioTokenPlan => {
                api_url(&self.inner.base_url, "responses")
            }
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
                responses_api_url(&self.inner.base_url, self.inner.api_provider)
            }
            ApiProvider::Anthropic => anthropic_messages_url(&self.inner.base_url),
            ApiProvider::XiaomiMimo => api_url(&self.inner.base_url, "chat/completions"),
            ApiProvider::Zai => api_url(&self.inner.base_url, "web_search"),
            _ => unreachable!("provider checked above"),
        };
        let body_bytes = serde_json::to_vec(&body)
            .context("failed to serialize provider-native web-search request")?;
        let response = self
            .inner
            .send_with_retry(|| {
                self.inner
                    .http_client
                    .post(&url)
                    .header("Accept", "application/json")
                    .body(body_bytes.clone())
            })
            .await
            .context("provider-native web search request failed")?;
        let payload = response
            .json::<Value>()
            .await
            .context("provider-native web search returned invalid JSON")?;
        let mut parsed = match self.inner.api_provider {
            ApiProvider::Openai
            | ApiProvider::Xai
            | ApiProvider::ModelstudioTokenPlan
            | ApiProvider::Deepseek
            | ApiProvider::DeepseekCN => parse_responses_search(&payload),
            ApiProvider::Anthropic => parse_anthropic_search(&payload),
            ApiProvider::XiaomiMimo => parse_mimo_search(&payload),
            ApiProvider::Zai => zai::parse(&payload),
            _ => unreachable!("provider checked above"),
        };
        parsed.citations.truncate(usize::from(request.max_results));
        Ok(parsed)
    }

    pub(super) async fn post_json(
        &self,
        url: &str,
        body: &Value,
        headers: &[(HeaderName, HeaderValue)],
    ) -> Result<Value> {
        let body_bytes = serde_json::to_vec(&body)
            .context("failed to serialize provider-native web-search request")?;
        let headers = headers.to_vec();
        let response = self
            .inner
            .send_with_retry(|| {
                let mut request = self
                    .inner
                    .http_client
                    .post(url)
                    .header("Accept", "application/json")
                    .body(body_bytes.clone());
                for (name, value) in &headers {
                    request = request.header(name, value);
                }
                request
            })
            .await
            .context("provider-native web search request failed")?;
        response
            .json::<Value>()
            .await
            .context("provider-native web search returned invalid JSON")
    }

    pub(super) async fn get_json(&self, url: &str) -> Result<Value> {
        let response = self
            .inner
            .send_with_retry(|| {
                self.inner
                    .http_client
                    .get(url)
                    .header("Accept", "application/json")
            })
            .await
            .context("provider-native web search request failed")?;
        response
            .json::<Value>()
            .await
            .context("provider-native web search returned invalid JSON")
    }
}

#[derive(Clone, Copy)]
enum ResponsesSearchDialect {
    Openai,
    Xai,
    ModelStudio,
    Deepseek,
}

fn search_prompt(request: &ProviderNativeSearchRequest) -> String {
    format!(
        "Search the web for the following query and answer only from web sources. \
         Use concise prose with citations and prefer at most {} distinct sources.\n\n{}",
        request.max_results, request.query
    )
}

fn build_responses_search_body(
    model: &str,
    request: &ProviderNativeSearchRequest,
    dialect: ResponsesSearchDialect,
) -> Value {
    let mut tool = json!({ "type": "web_search" });
    if !request.domains.is_empty()
        && matches!(
            dialect,
            ResponsesSearchDialect::Openai | ResponsesSearchDialect::Xai
        )
    {
        tool["filters"] = json!({ "allowed_domains": request.domains });
    }
    let mut body = json!({
        "model": model,
        "input": search_prompt(request),
        "tools": [tool],
    });
    match dialect {
        ResponsesSearchDialect::Openai => {
            body["tool_choice"] = json!("required");
            body["store"] = json!(false);
            body["include"] = json!(["web_search_call.action.sources"]);
        }
        ResponsesSearchDialect::Xai => {
            body["tool_choice"] = json!("required");
        }
        ResponsesSearchDialect::ModelStudio => {
            body["tool_choice"] = json!("required");
        }
        ResponsesSearchDialect::Deepseek => {
            body["tool_choice"] = json!({ "type": "web_search" });
        }
    }
    body
}

fn build_anthropic_search_body(
    model: &str,
    request: &ProviderNativeSearchRequest,
    max_tokens: u32,
) -> Value {
    let mut tool = json!({
        "type": "web_search_20250305",
        "name": "web_search",
        "max_uses": 1,
    });
    if !request.domains.is_empty() {
        tool["allowed_domains"] = json!(request.domains);
    }
    json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{ "role": "user", "content": search_prompt(request) }],
        "tools": [tool],
    })
}

fn build_mimo_search_body(model: &str, request: &ProviderNativeSearchRequest) -> Value {
    json!({
        "model": model,
        "messages": [{ "role": "user", "content": search_prompt(request) }],
        "tools": [{
            "type": "web_search",
            "max_keyword": 1,
            "force_search": true,
            "limit": request.max_results,
        }],
        "tool_choice": "auto",
        "max_completion_tokens": 2_048,
        "stream": false,
        "thinking": { "type": "disabled" },
    })
}

fn anthropic_messages_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

fn parse_responses_search(payload: &Value) -> ProviderNativeSearchResponse {
    let mut answer_parts = Vec::new();
    let mut citations = Vec::new();
    if let Some(output) = payload.get("output").and_then(Value::as_array) {
        for item in output {
            let item_type = item.get("type").and_then(Value::as_str);
            if item_type == Some("web_search_call")
                && let Some(action) = item.get("action")
            {
                if let Some(sources) = action.get("sources").and_then(Value::as_array) {
                    for source in sources {
                        push_citation(&mut citations, citation_from_value(source, None, None));
                    }
                }
                push_citation(&mut citations, citation_from_value(action, None, None));
            }

            if item_type == Some("message")
                && let Some(content) = item.get("content").and_then(Value::as_array)
            {
                for block in content {
                    if matches!(
                        block.get("type").and_then(Value::as_str),
                        Some("output_text" | "text")
                    ) && let Some(text) = block.get("text").and_then(Value::as_str)
                        && !text.trim().is_empty()
                    {
                        answer_parts.push(text.trim().to_string());
                    }
                    if let Some(annotations) = block.get("annotations").and_then(Value::as_array) {
                        for annotation in annotations {
                            push_citation(
                                &mut citations,
                                citation_from_value(annotation, None, None),
                            );
                        }
                    }
                }
            }
        }
    }
    if answer_parts.is_empty()
        && let Some(output_text) = payload.get("output_text").and_then(Value::as_str)
        && !output_text.trim().is_empty()
    {
        answer_parts.push(output_text.trim().to_string());
    }
    for answer in &answer_parts {
        for citation in citations_from_text(answer) {
            push_citation(&mut citations, Some(citation));
        }
    }
    if let Some(top_level) = payload.get("citations").and_then(Value::as_array) {
        for citation in top_level {
            let parsed = citation
                .as_str()
                .and_then(|url| citation_from_url(url, None, None, None))
                .or_else(|| citation_from_value(citation, None, None));
            push_citation(&mut citations, parsed);
        }
    }
    ProviderNativeSearchResponse {
        answer: bounded_answer(answer_parts),
        citations,
    }
}

fn parse_anthropic_search(payload: &Value) -> ProviderNativeSearchResponse {
    let mut answer_parts = Vec::new();
    let mut citations = Vec::new();
    if let Some(content) = payload.get("content").and_then(Value::as_array) {
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("web_search_tool_result") => {
                    if let Some(results) = block.get("content").and_then(Value::as_array) {
                        for result in results {
                            let published = result
                                .get("page_age")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            push_citation(
                                &mut citations,
                                citation_from_value(result, None, published),
                            );
                        }
                    }
                }
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str)
                        && !text.trim().is_empty()
                    {
                        answer_parts.push(text.trim().to_string());
                    }
                    if let Some(block_citations) = block.get("citations").and_then(Value::as_array)
                    {
                        for citation in block_citations {
                            let snippet = citation
                                .get("cited_text")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            push_citation(
                                &mut citations,
                                citation_from_value(citation, snippet, None),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    ProviderNativeSearchResponse {
        answer: bounded_answer(answer_parts),
        citations,
    }
}

fn parse_mimo_search(payload: &Value) -> ProviderNativeSearchResponse {
    let message = payload.pointer("/choices/0/message");
    let answer = message
        .and_then(|value| value.get("content"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let mut citations = Vec::new();
    if let Some(annotations) = message
        .and_then(|value| value.get("annotations"))
        .and_then(Value::as_array)
        .or_else(|| payload.get("annotations").and_then(Value::as_array))
    {
        for annotation in annotations {
            let Some(url) = annotation.get("url").and_then(Value::as_str) else {
                continue;
            };
            let title = annotation
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string);
            let snippet = annotation
                .get("summary")
                .and_then(Value::as_str)
                .map(str::to_string);
            let published = annotation
                .get("publish_time")
                .and_then(Value::as_str)
                .map(str::to_string);
            push_citation(
                &mut citations,
                citation_from_url(url, title, snippet, published),
            );
        }
    }
    ProviderNativeSearchResponse {
        answer: bounded_answer(answer.into_iter().collect()),
        citations,
    }
}

fn citation_from_value(
    value: &Value,
    snippet: Option<String>,
    published: Option<String>,
) -> Option<ProviderNativeCitation> {
    let url = value.get("url").and_then(Value::as_str)?.trim();
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string);
    citation_from_url(url, title, snippet, published)
}

fn citation_from_url(
    url: &str,
    title: Option<String>,
    snippet: Option<String>,
    published: Option<String>,
) -> Option<ProviderNativeCitation> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    Some(ProviderNativeCitation {
        url: url.to_string(),
        title: title.unwrap_or_else(|| fallback_title(url)),
        snippet,
        published,
    })
}

fn push_citation(
    citations: &mut Vec<ProviderNativeCitation>,
    candidate: Option<ProviderNativeCitation>,
) {
    let Some(candidate) = candidate else {
        return;
    };
    if let Some(existing) = citations
        .iter_mut()
        .find(|existing| existing.url == candidate.url)
    {
        if existing.title == fallback_title(&existing.url)
            && candidate.title != fallback_title(&candidate.url)
        {
            existing.title = candidate.title;
        }
        if existing.snippet.is_none() {
            existing.snippet = candidate.snippet;
        }
        if existing.published.is_none() {
            existing.published = candidate.published;
        }
        return;
    }
    citations.push(candidate);
}

fn citations_from_text(text: &str) -> Vec<ProviderNativeCitation> {
    let mut citations = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let remaining = &text[offset..];
        let relative_start = match (remaining.find("https://"), remaining.find("http://")) {
            (Some(https), Some(http)) => Some(https.min(http)),
            (Some(https), None) => Some(https),
            (None, Some(http)) => Some(http),
            (None, None) => None,
        };
        let Some(relative_start) = relative_start else {
            break;
        };
        let start = offset + relative_start;
        let tail = &text[start..];
        // Balanced parentheses belong to the URL (Wikipedia titles such as
        // `Foo_(bar)` keep their closing paren); an unmatched closer ends it.
        let mut open_parens = 0_usize;
        let end = tail
            .char_indices()
            .find_map(|(index, ch)| match ch {
                '(' => {
                    open_parens += 1;
                    None
                }
                ')' if open_parens > 0 => {
                    open_parens -= 1;
                    None
                }
                _ => (index > 0
                    && (ch.is_whitespace()
                        || matches!(ch, ')' | ']' | '}' | '>' | '"' | '\'' | '`')))
                .then_some(index),
            })
            .unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        push_citation(&mut citations, citation_from_url(url, None, None, None));
        offset = start + end.max(1);
    }
    citations
}

fn fallback_title(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| "Web source".to_string())
}

fn bounded_answer(parts: Vec<String>) -> Option<String> {
    let joined = parts.join("\n\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= MAX_NATIVE_ANSWER_CHARS {
        return Some(trimmed.to_string());
    }
    let mut bounded = trimmed
        .chars()
        .take(MAX_NATIVE_ANSWER_CHARS.saturating_sub(1))
        .collect::<String>();
    bounded.push('…');
    Some(bounded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProviderConfig, ProvidersConfig};
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request() -> ProviderNativeSearchRequest {
        ProviderNativeSearchRequest {
            query: "current release".to_string(),
            max_results: 3,
            domains: vec!["example.com".to_string()],
        }
    }

    fn xai_client_with_boundary(
        server: &MockServer,
        isolated: bool,
        unrelated: bool,
    ) -> ProviderNativeSearchClient {
        let config = Config {
            provider: Some("xai".to_string()),
            providers: Some(ProvidersConfig {
                xai: ProviderConfig {
                    api_key: Some("xai-test-key".to_string()),
                    base_url: Some(format!("{}/v1", server.uri())),
                    model: Some("grok-4.5".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            runtime_chat_isolated: isolated,
            runtime_thread_inference_unrelated: unrelated,
            ..Config::default()
        };
        ProviderNativeSearchClient::new(DeepSeekClient::new(&config).expect("test xAI client"))
            .expect("xAI native adapter")
    }

    #[test]
    fn responses_payload_requires_search_and_keeps_domains_provider_side() {
        let body =
            build_responses_search_body("gpt-5.6", &request(), ResponsesSearchDialect::Openai);
        assert_eq!(body["tools"][0]["type"], "web_search");
        assert_eq!(
            body["tools"][0]["filters"]["allowed_domains"][0],
            "example.com"
        );
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["include"][0], "web_search_call.action.sources");
    }

    #[test]
    fn modelstudio_payload_uses_required_harness_search_without_filters() {
        let body = build_responses_search_body(
            "qwen3.8-max",
            &request(),
            ResponsesSearchDialect::ModelStudio,
        );
        assert_eq!(body["tools"][0]["type"], "web_search");
        assert!(body["tools"][0].get("filters").is_none());
        assert_eq!(body["tool_choice"], "required");
        assert!(body.get("include").is_none());
        assert!(body.get("store").is_none());
    }

    #[test]
    fn deepseek_payload_uses_its_responses_search_contract() {
        let body = build_responses_search_body(
            "deepseek-v4-flash",
            &request(),
            ResponsesSearchDialect::Deepseek,
        );
        assert_eq!(body["tools"][0]["type"], "web_search");
        assert!(body["tools"][0].get("filters").is_none());
        assert_eq!(body["tool_choice"]["type"], "web_search");
        assert!(body.get("include").is_none());
        assert!(body.get("store").is_none());
    }

    #[test]
    fn anthropic_payload_uses_basic_direct_search_contract() {
        let body = build_anthropic_search_body("claude-opus-4-8", &request(), 2_048);
        assert_eq!(body["tools"][0]["type"], "web_search_20250305");
        assert_eq!(body["tools"][0]["max_uses"], 1);
        assert_eq!(body["tools"][0]["allowed_domains"][0], "example.com");
        assert_eq!(body["max_tokens"], 2_048);

        let tiny_route = build_anthropic_search_body("claude-opus-4-8", &request(), 128);
        assert_eq!(tiny_route["max_tokens"], 128);
    }

    #[test]
    fn mimo_payload_forces_bounded_web_search_plugin() {
        let body = build_mimo_search_body("mimo-v2.5-pro", &request());
        assert_eq!(body["tools"][0]["type"], "web_search");
        assert_eq!(body["tools"][0]["force_search"], true);
        assert_eq!(body["tools"][0]["limit"], 3);
        assert_eq!(body["max_completion_tokens"], 2_048);
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn responses_parser_separates_answer_and_deduplicated_citations() {
        let payload = json!({
            "output": [
                {
                    "type": "web_search_call",
                    "action": { "sources": [
                        { "url": "https://example.com/a", "title": "Source A" }
                    ] }
                },
                {
                    "type": "message",
                    "content": [{
                        "type": "output_text",
                        "text": "Grounded answer.",
                        "annotations": [
                            { "type": "url_citation", "url": "https://example.com/a", "title": "Source A" },
                            { "type": "url_citation", "url": "https://example.org/b", "title": "Source B" }
                        ]
                    }]
                }
            ]
        });
        let parsed = parse_responses_search(&payload);
        assert_eq!(parsed.answer.as_deref(), Some("Grounded answer."));
        assert_eq!(parsed.citations.len(), 2);
        assert_eq!(parsed.citations[0].title, "Source A");
        assert_eq!(parsed.citations[1].url, "https://example.org/b");
    }

    #[test]
    fn responses_parser_keeps_final_message_and_opened_pages_only() {
        let payload = json!({
            "output": [
                {
                    "type": "reasoning",
                    "content": [{
                        "type": "reasoning_text",
                        "text": "private analysis https://reasoning.example/ must stay hidden"
                    }]
                },
                {
                    "type": "web_search_call",
                    "action": {
                        "type": "open_page",
                        "url": "https://github.com/Hmbown/CodeWhale"
                    }
                },
                {
                    "type": "message",
                    "content": [{
                        "type": "output_text",
                        "text": "Official repository: https://github.com/Hmbown/CodeWhale",
                        "annotations": []
                    }]
                }
            ]
        });

        let parsed = parse_responses_search(&payload);

        assert_eq!(
            parsed.answer.as_deref(),
            Some("Official repository: https://github.com/Hmbown/CodeWhale")
        );
        assert_eq!(parsed.citations.len(), 1);
        assert_eq!(
            parsed.citations[0].url,
            "https://github.com/Hmbown/CodeWhale"
        );
    }

    #[test]
    fn anthropic_parser_keeps_result_metadata_and_cited_text_separate() {
        let payload = json!({
            "content": [
                {
                    "type": "web_search_tool_result",
                    "content": [{
                        "type": "web_search_result",
                        "url": "https://example.com/a",
                        "title": "Source A",
                        "page_age": "July 18, 2026"
                    }]
                },
                {
                    "type": "text",
                    "text": "Grounded answer.",
                    "citations": [{
                        "type": "web_search_result_location",
                        "url": "https://example.com/a",
                        "title": "Source A",
                        "cited_text": "Supporting passage"
                    }]
                }
            ]
        });
        let parsed = parse_anthropic_search(&payload);
        assert_eq!(parsed.answer.as_deref(), Some("Grounded answer."));
        assert_eq!(parsed.citations.len(), 1);
        assert_eq!(
            parsed.citations[0].published.as_deref(),
            Some("July 18, 2026")
        );
        assert_eq!(
            parsed.citations[0].snippet.as_deref(),
            Some("Supporting passage")
        );
    }

    #[test]
    fn mimo_parser_keeps_non_streaming_annotations() {
        let parsed = parse_mimo_search(&json!({
            "choices": [{
                "message": {
                    "content": "Grounded answer.",
                    "annotations": [{
                        "type": "url_citation",
                        "url": "https://example.com/weather",
                        "title": "Weather",
                        "summary": "Forecast",
                        "publish_time": "2026-08-28"
                    }]
                }
            }]
        }));
        assert_eq!(parsed.answer.as_deref(), Some("Grounded answer."));
        assert_eq!(parsed.citations.len(), 1);
        assert_eq!(parsed.citations[0].snippet.as_deref(), Some("Forecast"));
        assert_eq!(parsed.citations[0].published.as_deref(), Some("2026-08-28"));
    }

    #[test]
    fn non_http_citations_are_rejected() {
        let payload = json!({ "citations": ["javascript:alert(1)"] });
        assert!(parse_responses_search(&payload).citations.is_empty());
    }

    #[test]
    fn answer_links_preserve_mixed_scheme_source_order() {
        let citations =
            citations_from_text("First http://legacy.example/a, then https://secure.example/b.");
        assert_eq!(citations.len(), 2);
        assert_eq!(citations[0].url, "http://legacy.example/a");
        assert_eq!(citations[1].url, "https://secure.example/b");
    }

    #[test]
    fn answer_links_keep_balanced_parenthesis_segments() {
        let citations = citations_from_text(
            "See https://en.wikipedia.org/wiki/Foo_(bar) and https://en.wikipedia.org/wiki/Baz_(qux_(nested)) for details.",
        );
        assert_eq!(citations.len(), 2);
        assert_eq!(
            citations[0].url, "https://en.wikipedia.org/wiki/Foo_(bar)",
            "a balanced closing paren is part of the URL"
        );
        assert_eq!(
            citations[1].url, "https://en.wikipedia.org/wiki/Baz_(qux_(nested))",
            "nested balanced parens stay intact"
        );

        let unbalanced = citations_from_text("Broken https://en.wikipedia.org/wiki/Foo_(bar here.");
        assert_eq!(
            unbalanced[0].url, "https://en.wikipedia.org/wiki/Foo_(bar",
            "an unclosed paren cannot extend past the next whitespace"
        );
    }

    #[tokio::test]
    async fn xai_adapter_reuses_active_authenticated_transport() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer xai-test-key"))
            .and(body_partial_json(json!({
                "model": "grok-4.5",
                "tools": [{
                    "type": "web_search",
                    "filters": { "allowed_domains": ["example.com"] }
                }],
                "tool_choice": "required"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output_text": "Grounded answer.",
                "citations": ["https://example.com/source"]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let config = Config {
            provider: Some("xai".to_string()),
            providers: Some(ProvidersConfig {
                xai: ProviderConfig {
                    api_key: Some("xai-test-key".to_string()),
                    base_url: Some(format!("{}/v1", server.uri())),
                    model: Some("grok-4.5".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        let inner = DeepSeekClient::new(&config).expect("test xAI client");
        let client = ProviderNativeSearchClient::new(inner).expect("xAI native adapter");
        let cache_identity = client.cache_identity();
        assert!(cache_identity.contains("provider-native://xai/"));
        assert!(cache_identity.ends_with("/grok-4.5"));
        assert!(!cache_identity.contains("xai-test-key"));

        let response = client.search(&request()).await.expect("native search");

        assert_eq!(response.answer.as_deref(), Some("Grounded answer."));
        assert_eq!(response.citations.len(), 1);
        assert_eq!(response.citations[0].url, "https://example.com/source");
    }

    #[tokio::test]
    async fn modelstudio_adapter_uses_token_plan_responses_contract() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer modelstudio-test-key"))
            .and(body_partial_json(json!({
                "model": "qwen3.8-max",
                "tools": [{ "type": "web_search" }],
                "tool_choice": "required"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": [{
                    "type": "web_search_call",
                    "action": {
                        "sources": [{
                            "url": "https://example.com/qwen",
                            "title": "Qwen source"
                        }]
                    }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let config = Config {
            provider: Some("modelstudio-token-plan".to_string()),
            providers: Some(ProvidersConfig {
                modelstudio_token_plan: ProviderConfig {
                    api_key: Some("modelstudio-test-key".to_string()),
                    base_url: Some(format!("{}/v1", server.uri())),
                    model: Some("qwen3.8-max".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        let inner = DeepSeekClient::new(&config).expect("test ModelStudio client");
        let client = ProviderNativeSearchClient::new(inner).expect("Qwen native adapter");

        let response = client.search(&request()).await.expect("native search");

        assert_eq!(response.citations.len(), 1);
        assert_eq!(response.citations[0].url, "https://example.com/qwen");
    }
    #[tokio::test]
    async fn deepseek_adapter_uses_authenticated_responses_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer deepseek-test-key"))
            .and(body_partial_json(json!({
                "model": "deepseek-v4-flash",
                "tools": [{ "type": "web_search" }],
                "tool_choice": { "type": "web_search" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": [
                    {
                        "type": "web_search_call",
                        "action": {
                            "type": "open_page",
                            "url": "https://example.com/deepseek"
                        }
                    },
                    {
                        "type": "message",
                        "content": [{
                            "type": "output_text",
                            "text": "Grounded answer.",
                            "annotations": []
                        }]
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(ProvidersConfig {
                deepseek: ProviderConfig {
                    api_key: Some("deepseek-test-key".to_string()),
                    base_url: Some(format!("{}/v1", server.uri())),
                    model: Some("deepseek-v4-flash".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        let inner = DeepSeekClient::new(&config).expect("test DeepSeek client");
        let client = ProviderNativeSearchClient::new(inner).expect("DeepSeek native adapter");

        let response = client.search(&request()).await.expect("native search");

        assert_eq!(response.answer.as_deref(), Some("Grounded answer."));
        assert_eq!(response.citations.len(), 1);
        assert_eq!(response.citations[0].url, "https://example.com/deepseek");
    }

    #[tokio::test]
    async fn native_search_obeys_attached_run_ownership_without_blocking_unrelated_runtime() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output_text": "Grounded answer.",
                "citations": ["https://example.com/source"]
            })))
            .expect(3)
            .mount(&server)
            .await;
        let participant = xai_client_with_boundary(&server, false, false);
        let isolated = xai_client_with_boundary(&server, true, false);
        let unrelated = xai_client_with_boundary(&server, false, true);

        let ownership = crate::client::acquire_runtime_chat_inference_ownership().await;
        let participant_request = request();
        let mut waiting =
            tokio::spawn(async move { participant.search(&participant_request).await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(40), &mut waiting)
                .await
                .is_err(),
            "provider-native inference from the attached run must wait behind Runtime Chat"
        );

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            isolated.search(&request()),
        )
        .await
        .expect("isolated relay request must not self-deadlock")
        .expect("isolated native search fixture");
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            unrelated.search(&request()),
        )
        .await
        .expect("unrelated Runtime manager stays concurrent")
        .expect("unrelated native search fixture");

        drop(ownership);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("attached participant resumes after relay settlement")
            .expect("participant task")
            .expect("participant native search fixture");
    }
}
