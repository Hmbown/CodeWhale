//! HTTP client for DeepSeek's OpenAI-compatible Chat Completions API.
//!
//! DeepSeek documents `/chat/completions` as the primary endpoint, and this
//! client now routes all normal traffic through that surface.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose};
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

use codewhale_config::catalog::{
    CatalogOffering, CatalogRefreshError, CatalogSnapshot, CatalogSource, CatalogStatus,
    ProviderCatalogCache, ProviderCatalogDelta, base_url_fingerprint, now_unix,
};
use codewhale_config::route::ReadyRouteCandidate;
use codewhale_config::{auth_mode_disables_api_key, is_upstream_auth_header};

use crate::config::{
    ApiProvider, Config, RetryPolicy, validate_route, wire_model_for_provider_route,
};
use crate::llm_client::{
    LlmClient, LlmError, RetryConfig as LlmRetryConfig, extract_retry_after,
    sanitize_http_error_body, with_retry,
};
use crate::logging;
use crate::models::{
    ContentBlock, Message, MessageRequest, MessageResponse, ServerToolUsage, SystemPrompt, Usage,
};

pub(super) fn to_api_tool_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else if ch == '-' {
            out.push_str("--");
        } else {
            out.push_str("-x");
            out.push_str(&format!("{:06X}", ch as u32));
            out.push('-');
        }
    }
    out
}

pub(super) fn from_api_tool_name(name: &str) -> String {
    let mut out = String::new();
    let mut iter = name.chars().peekable();
    while let Some(ch) = iter.next() {
        if ch != '-' {
            out.push(ch);
            continue;
        }
        if let Some('-') = iter.peek().copied() {
            iter.next();
            out.push('-');
            continue;
        }
        if iter.peek().copied() == Some('x') {
            iter.next();
            let mut hex = String::new();
            for _ in 0..6 {
                if let Some(h) = iter.next() {
                    hex.push(h);
                } else {
                    break;
                }
            }
            // Only decode if we got exactly 6 hex digits (matching encoder output).
            // Fewer digits means a truncated/malformed sequence — pass through as-is.
            if hex.len() == 6
                && let Ok(code) = u32::from_str_radix(&hex, 16)
                && let Some(decoded) = std::char::from_u32(code)
            {
                if let Some('-') = iter.peek().copied() {
                    iter.next();
                }
                out.push(decoded);
                continue;
            }
            out.push('-');
            out.push('x');
            out.push_str(&hex);
            continue;
        }
        out.push('-');
    }

    // Second pass: decode bare hex escapes (e.g. `x00002E`) that the model
    // may produce when it mangles the `-x00002E-` delimiter form.  Only
    // decode when the resulting character is one that `to_api_tool_name`
    // would have encoded (not alphanumeric, not `_`, not `-`).
    decode_bare_hex_escapes(&out)
}

/// Decode bare `x[0-9A-Fa-f]{6}` sequences (optionally followed by `-`)
/// that survive the standard delimiter-based pass.  This handles cases
/// where the model strips or replaces the leading `-` of `-x00002E-`.
pub(super) fn decode_bare_hex_escapes(input: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"x([0-9A-Fa-f]{6})-?").unwrap());

    let result = re.replace_all(input, |caps: &regex::Captures| {
        let hex = &caps[1];
        if let Ok(code) = u32::from_str_radix(hex, 16)
            && let Some(decoded) = std::char::from_u32(code)
        {
            // Only decode characters that to_api_tool_name would have encoded
            if !decoded.is_ascii_alphanumeric() && decoded != '_' && decoded != '-' {
                return decoded.to_string();
            }
        }
        // Not a character we'd encode — leave as-is
        caps[0].to_string()
    });
    result.into_owned()
}

// === Types ===

/// Model descriptor returned by the provider's `/v1/models` endpoint.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AvailableModel {
    pub id: String,
    pub owned_by: Option<String>,
    pub created: Option<u64>,
}

/// Request payload for Xiaomi MiMo speech synthesis models.
///
/// MiMo-V2.5-TTS / MiMo-V2-TTS use the OpenAI-compatible
/// `/v1/chat/completions` endpoint: the optional style/voice instruction is
/// sent as a `user` message, while the text to synthesize is sent as an
/// `assistant` message.
#[derive(Debug, Clone)]
pub struct SpeechSynthesisRequest {
    pub model: String,
    pub text: String,
    pub instruction: Option<String>,
    pub audio_format: String,
    pub voice: Option<String>,
}

/// Decoded speech synthesis result.
#[derive(Debug, Clone)]
pub struct SpeechSynthesisResponse {
    pub model: String,
    pub audio_format: String,
    pub audio_bytes: Vec<u8>,
    pub transcript: Option<String>,
    pub voice: Option<String>,
}

/// Client for DeepSeek's OpenAI-compatible APIs.
#[must_use]
pub struct DeepSeekClient {
    pub(super) http_client: reqwest::Client,
    api_key: String,
    /// Exact configured credential values removed from model-bound tool
    /// results. Structural redaction handles config/JSON assignments, while
    /// this list closes the gap for bare provider tokens with no recognizable
    /// prefix (for example token-plan and provider-specific keys).
    model_bound_secret_values: Arc<Vec<String>>,
    pub(super) base_url: String,
    pub(super) api_provider: ApiProvider,
    retry: RetryPolicy,
    default_model: String,
    connection_health: Arc<AsyncMutex<ConnectionHealth>>,
    rate_limiter: Arc<AsyncMutex<TokenBucket>>,
    request_concurrency: Option<ProviderConcurrencyLimiter>,
    path_suffix: Option<String>,
    pub(super) reasoning_stream_style: Option<String>,
    pub(super) stream_idle_timeout: Duration,
}

const CONNECTION_FAILURE_THRESHOLD: u32 = 2;
const RECOVERY_PROBE_COOLDOWN: Duration = Duration::from_secs(15);

const DEFAULT_CLIENT_RATE_LIMIT_RPS: f64 = 8.0;
const DEFAULT_CLIENT_RATE_LIMIT_BURST: f64 = 16.0;
const ALLOW_INSECURE_HTTP_ENV: &str = "DEEPSEEK_ALLOW_INSECURE_HTTP";

/// Upper bound on a single sleep inside the provider-wide rate-limit pause
/// loop in `send_with_retry`. The pause window lives in process-global state
/// (`retry_status`), so waiting requests re-poll it on this cadence instead
/// of committing to the full remaining window up front.
const RATE_LIMIT_PAUSE_RECHECK_INTERVAL: Duration = Duration::from_millis(250);

pub(super) const SSE_BACKPRESSURE_HIGH_WATERMARK: usize = 1024 * 1024; // 1 MB
pub(super) const SSE_BACKPRESSURE_SLEEP_MS: u64 = 10;
pub(super) const SSE_MAX_LINES_PER_CHUNK: usize = 256;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Healthy,
    Degraded,
    Recovering,
}

#[derive(Debug)]
struct ConnectionHealth {
    state: ConnectionState,
    consecutive_failures: u32,
    last_failure: Option<Instant>,
    last_success: Option<Instant>,
    last_probe: Option<Instant>,
}

impl Default for ConnectionHealth {
    fn default() -> Self {
        Self {
            state: ConnectionState::Healthy,
            consecutive_failures: 0,
            last_failure: None,
            last_success: None,
            last_probe: None,
        }
    }
}

#[derive(Debug)]
struct TokenBucket {
    enabled: bool,
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last_refill: Instant,
}

#[derive(Debug, Clone)]
struct ProviderConcurrencyLimiter {
    semaphore: Arc<Semaphore>,
    active: Arc<AtomicUsize>,
    limit: usize,
}

struct ProviderRequestPermit {
    _permit: OwnedSemaphorePermit,
    active: Arc<AtomicUsize>,
}

impl ProviderConcurrencyLimiter {
    fn new(limit: usize) -> Self {
        let limit = limit.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(limit)),
            active: Arc::new(AtomicUsize::new(0)),
            limit,
        }
    }

    async fn acquire(&self) -> Option<ProviderRequestPermit> {
        let permit = Arc::clone(&self.semaphore).acquire_owned().await.ok()?;
        self.active.fetch_add(1, Ordering::AcqRel);
        Some(ProviderRequestPermit {
            _permit: permit,
            active: Arc::clone(&self.active),
        })
    }

    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    fn limit(&self) -> usize {
        self.limit
    }
}

impl Drop for ProviderRequestPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl TokenBucket {
    fn from_env() -> Self {
        let rps = std::env::var("DEEPSEEK_RATE_LIMIT_RPS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(DEFAULT_CLIENT_RATE_LIMIT_RPS)
            .max(0.0);
        let burst = std::env::var("DEEPSEEK_RATE_LIMIT_BURST")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(DEFAULT_CLIENT_RATE_LIMIT_BURST)
            .max(1.0);
        let enabled = rps > 0.0;
        Self {
            enabled,
            capacity: burst,
            tokens: burst,
            refill_per_sec: rps,
            last_refill: Instant::now(),
        }
    }

    fn refill(&mut self, now: Instant) {
        if !self.enabled {
            return;
        }
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
    }

    fn delay_until_available(&mut self, tokens: f64) -> Option<Duration> {
        if !self.enabled {
            return None;
        }
        let now = Instant::now();
        self.refill(now);
        if self.tokens >= tokens {
            self.tokens -= tokens;
            return None;
        }
        let needed = tokens - self.tokens;
        self.tokens = 0.0;
        if self.refill_per_sec <= 0.0 {
            return Some(Duration::from_secs(1));
        }
        Some(Duration::from_secs_f64(needed / self.refill_per_sec))
    }
}

fn apply_request_success(health: &mut ConnectionHealth, now: Instant) -> bool {
    let recovered = health.state != ConnectionState::Healthy;
    health.state = ConnectionState::Healthy;
    health.consecutive_failures = 0;
    health.last_success = Some(now);
    recovered
}

fn apply_request_failure(health: &mut ConnectionHealth, now: Instant) {
    health.consecutive_failures = health.consecutive_failures.saturating_add(1);
    health.last_failure = Some(now);
    if health.consecutive_failures >= CONNECTION_FAILURE_THRESHOLD {
        health.state = ConnectionState::Degraded;
    }
}

fn mark_recovery_probe_if_due(health: &mut ConnectionHealth, now: Instant) -> bool {
    if health.state == ConnectionState::Healthy {
        return false;
    }
    if health
        .last_probe
        .is_some_and(|last| now.duration_since(last) < RECOVERY_PROBE_COOLDOWN)
    {
        return false;
    }
    health.last_probe = Some(now);
    health.state = ConnectionState::Recovering;
    true
}

fn buffer_pool() -> &'static StdMutex<Vec<Vec<u8>>> {
    static POOL: OnceLock<StdMutex<Vec<Vec<u8>>>> = OnceLock::new();
    POOL.get_or_init(|| StdMutex::new(Vec::new()))
}

fn acquire_stream_buffer() -> Vec<u8> {
    if let Ok(mut pool) = buffer_pool().lock() {
        pool.pop().unwrap_or_else(|| Vec::with_capacity(8192))
    } else {
        Vec::with_capacity(8192)
    }
}

fn release_stream_buffer(mut buf: Vec<u8>) {
    buf.clear();
    if buf.capacity() > 256 * 1024 {
        buf.shrink_to(256 * 1024);
    }
    if let Ok(mut pool) = buffer_pool().lock()
        && pool.len() < 8
    {
        pool.push(buf);
    }
}

impl Clone for DeepSeekClient {
    fn clone(&self) -> Self {
        Self {
            http_client: self.http_client.clone(),
            api_key: self.api_key.clone(),
            model_bound_secret_values: Arc::clone(&self.model_bound_secret_values),
            base_url: self.base_url.clone(),
            api_provider: self.api_provider,
            retry: self.retry.clone(),
            default_model: self.default_model.clone(),
            connection_health: self.connection_health.clone(),
            rate_limiter: self.rate_limiter.clone(),
            request_concurrency: self.request_concurrency.clone(),
            path_suffix: self.path_suffix.clone(),
            reasoning_stream_style: self.reasoning_stream_style.clone(),
            stream_idle_timeout: self.stream_idle_timeout,
        }
    }
}

const MIN_EXACT_SECRET_CHARS: usize = 8;

fn push_model_bound_secret(values: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = value
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() >= MIN_EXACT_SECRET_CHARS)
    else {
        return;
    };
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn model_bound_secret_store_slot(provider: ApiProvider) -> Option<&'static str> {
    match provider {
        ApiProvider::DeepseekCN => Some("deepseek"),
        ApiProvider::SiliconflowCn => Some("siliconflow"),
        ApiProvider::Custom => None,
        _ => Some(provider.as_str()),
    }
}

fn push_file_backed_model_bound_secrets(values: &mut Vec<String>) {
    // Unit tests must never inspect the developer's real credential store.
    // The isolated regression below opts in with a temporary CODEWHALE_HOME,
    // matching Config's existing secret-store test discipline.
    #[cfg(test)]
    if std::env::var_os("CODEWHALE_HOME").is_none()
        || std::env::var_os("CODEWHALE_SECRET_BACKEND").is_none()
    {
        return;
    }

    // Read the file backend directly. `Secrets::auto_detect()` could select
    // the opt-in OS keychain, where probing every inactive provider would
    // cause a burst of user-visible credential prompts. The active credential
    // is already covered by `deepseek_api_key()` regardless of backend.
    let secrets = codewhale_secrets::Secrets::file_backed();
    let mut slots = Vec::new();
    for provider in ApiProvider::all()
        .iter()
        .copied()
        .chain(std::iter::once(ApiProvider::DeepseekCN))
    {
        let Some(slot) = model_bound_secret_store_slot(provider) else {
            continue;
        };
        if !slots.contains(&slot) {
            slots.push(slot);
        }
    }
    // The legacy literal `provider = "custom"` route owns this durable slot.
    slots.push("custom");

    for slot in slots {
        if let Ok(Some(secret)) = secrets.get(slot) {
            push_model_bound_secret(values, Some(&secret));
        }
    }
}

fn configured_model_bound_secret_values(config: &Config, active_api_key: &str) -> Vec<String> {
    let mut values = Vec::new();
    push_model_bound_secret(&mut values, Some(active_api_key));
    push_model_bound_secret(&mut values, config.api_key.as_deref());
    push_model_bound_secret(&mut values, config.sandbox_api_key.as_deref());
    push_model_bound_secret(
        &mut values,
        config
            .search
            .as_ref()
            .and_then(|search| search.api_key.as_deref()),
    );
    push_model_bound_secret(
        &mut values,
        config
            .vision_model
            .as_ref()
            .and_then(|vision| vision.api_key.as_deref()),
    );

    if let Some(headers) = config.http_headers.as_ref() {
        for (name, value) in headers {
            if is_upstream_auth_header(name) {
                push_model_bound_secret(&mut values, Some(value));
            }
        }
    }

    for provider in ApiProvider::all()
        .iter()
        .copied()
        .chain(std::iter::once(ApiProvider::DeepseekCN))
        .filter(|provider| *provider != ApiProvider::Custom)
    {
        for env_name in provider.env_vars() {
            if let Ok(value) = std::env::var(env_name) {
                push_model_bound_secret(&mut values, Some(&value));
            }
        }
        let Some(provider_config) = config.provider_config_for(provider) else {
            continue;
        };
        push_model_bound_secret(&mut values, provider_config.api_key.as_deref());
        if let Some(headers) = provider_config.http_headers.as_ref() {
            for (name, value) in headers {
                if is_upstream_auth_header(name) {
                    push_model_bound_secret(&mut values, Some(value));
                }
            }
        }
    }

    if let Some(providers) = config.providers.as_ref() {
        for provider_config in providers.custom.values() {
            push_model_bound_secret(&mut values, provider_config.api_key.as_deref());
            if let Some(env_name) = provider_config
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                && let Ok(value) = std::env::var(env_name)
            {
                push_model_bound_secret(&mut values, Some(&value));
            }
            if let Some(headers) = provider_config.http_headers.as_ref() {
                for (name, value) in headers {
                    if is_upstream_auth_header(name) {
                        push_model_bound_secret(&mut values, Some(value));
                    }
                }
            }
        }
    }

    push_file_backed_model_bound_secrets(&mut values);

    // Replace longer values first in case one credential happens to contain
    // another as a prefix.
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values
}

fn redact_model_bound_text(text: &str, exact_secret_values: &[String]) -> String {
    let mut redacted = text.to_string();
    for secret in exact_secret_values {
        redacted = redacted.replace(secret, codewhale_config::persistence::REDACTED);
    }
    codewhale_config::persistence::redact_secrets(&redacted)
}

// === Helpers ===

/// Maximum bytes to read from an error response body (64 KB).
pub(super) const ERROR_BODY_MAX_BYTES: usize = 64 * 1024;

/// Read an error response body with a size limit to prevent unbounded allocation.
pub(super) async fn bounded_error_text(response: reqwest::Response, max_bytes: usize) -> String {
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut buf = Vec::with_capacity(max_bytes.min(8192));
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        let remaining = max_bytes.saturating_sub(buf.len());
        if remaining == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn validate_base_url_security(base_url: &str) -> Result<()> {
    let display_base_url = redact_url_for_display(base_url);
    if base_url.starts_with("https://")
        || base_url.starts_with("http://localhost")
        || base_url.starts_with("http://127.0.0.1")
        || base_url.starts_with("http://[::1]")
    {
        return Ok(());
    }

    if base_url.starts_with("http://")
        && std::env::var(ALLOW_INSECURE_HTTP_ENV)
            .ok()
            .as_deref()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    {
        logging::warn(format!(
            "Using insecure HTTP base URL because {ALLOW_INSECURE_HTTP_ENV} is set"
        ));
        return Ok(());
    }

    if base_url.starts_with("http://") {
        anyhow::bail!(
            "Refusing insecure base URL '{display_base_url}'.\n\
             \n\
             Loopback hosts (localhost, 127.0.0.1, [::1]) are auto-allowed.\n\
             For other trusted local hosts (LAN, llama.cpp on a private IP, etc.)\n\
             set the env var `{ALLOW_INSECURE_HTTP_ENV}=1` in the shell that runs deepseek and re-run.\n\
             \n\
             Example: `{ALLOW_INSECURE_HTTP_ENV}=1 deepseek` (note the underscores).",
        );
    }

    anyhow::bail!(
        "Refusing base URL '{display_base_url}': only HTTPS (or explicitly allowed HTTP) URLs are supported.",
    )
}

pub(crate) fn redact_url_for_display(url: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        let _ = parsed.set_username("***");
        let _ = parsed.set_password(Some("***"));
    }
    if parsed.query().is_none() {
        return parsed.to_string();
    }
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(key, value)| {
            let value = if is_sensitive_url_query_key(&key) {
                "***".to_string()
            } else {
                value.into_owned()
            };
            (key.into_owned(), value)
        })
        .collect();
    parsed.set_query(None);
    let mut query = parsed.query_pairs_mut();
    for (key, value) in pairs {
        query.append_pair(&key, &value);
    }
    drop(query);
    parsed.to_string()
}

fn is_sensitive_url_query_key(key: &str) -> bool {
    let normalized = key.trim().replace(['-', '.'], "_").to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "api_key"
            | "apikey"
            | "access_token"
            | "auth_token"
            | "authorization"
            | "bearer"
            | "client_secret"
            | "credential"
            | "id_token"
            | "password"
            | "refresh_token"
            | "secret"
            | "token"
    ) || normalized.ends_with("_api_key")
        || normalized.ends_with("_authorization")
        || normalized.ends_with("_password")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_token")
}

pub(super) fn versioned_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if base_url_has_version_suffix(trimmed) {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

fn unversioned_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    trimmed
        .rsplit_once('/')
        .filter(|(_, segment)| is_version_segment(segment))
        .map(|(base, _)| base)
        .unwrap_or(trimmed)
        .to_string()
}

fn base_url_has_version_suffix(trimmed: &str) -> bool {
    trimmed.rsplit('/').next().is_some_and(is_version_segment)
}

fn is_version_segment(segment: &str) -> bool {
    segment.eq_ignore_ascii_case("beta")
        || segment
            .strip_prefix('v')
            .or_else(|| segment.strip_prefix('V'))
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()))
}

pub(super) fn api_url(base_url: &str, path: &str) -> String {
    api_url_with_suffix(base_url, path, None)
}

pub(super) fn api_url_with_suffix(base_url: &str, path: &str, path_suffix: Option<&str>) -> String {
    let path = path.trim_start_matches('/');
    if path.starts_with("beta/") {
        return format!("{}/{}", unversioned_base_url(base_url), path);
    }
    if let ("chat/completions", Some(suffix)) = (path, path_suffix) {
        return format!(
            "{}/{}",
            unversioned_base_url(base_url),
            suffix.trim_start_matches('/')
        );
    }
    let mut versioned = versioned_base_url(base_url);
    // The /beta suffix is not a real API version — it is an
    // opt-in surface for beta features.  Only paths with an
    // explicit `beta/` prefix should hit the beta surface;
    // everything else (models, chat/completions, health, …)
    // must go to the standard /v1 surface.
    if versioned.ends_with("beta") {
        versioned = format!("{}/v1", unversioned_base_url(base_url));
    }
    format!("{}/{}", versioned.trim_end_matches('/'), path)
}

fn normalize_audio_format(format: &str) -> String {
    let normalized = format.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        "wav".to_string()
    } else {
        normalized
    }
}

fn parse_speech_audio_response(payload: &Value) -> Result<(Vec<u8>, Option<String>)> {
    let audio = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| {
            choice
                .get("message")
                .and_then(|message| message.get("audio"))
                .or_else(|| choice.get("delta").and_then(|delta| delta.get("audio")))
        })
        .or_else(|| payload.get("audio"))
        .context("Speech synthesis response did not include choices[0].message.audio")?;

    let data = audio
        .get("data")
        .and_then(Value::as_str)
        .context("Speech synthesis response did not include audio.data")?
        .trim();
    let data = data
        .split_once(',')
        .map(|(_, base64)| base64.trim())
        .unwrap_or(data);
    let audio_bytes = general_purpose::STANDARD
        .decode(data)
        .context("Failed to decode speech audio base64 data")?;
    let transcript = audio
        .get("transcript")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok((audio_bytes, transcript))
}

fn build_speech_synthesis_body(
    model: &str,
    text: &str,
    instruction: Option<&str>,
    audio: Value,
) -> Value {
    let mut messages = Vec::new();
    if let Some(instruction) = instruction.map(str::trim).filter(|value| !value.is_empty()) {
        messages.push(json!({
            "role": "user",
            "content": instruction,
        }));
    }
    messages.push(json!({
        "role": "assistant",
        "content": text,
    }));

    json!({
        "model": model,
        "messages": messages,
        "audio": audio,
    })
}

// === DeepSeekClient ===

/// Returns true when DEEPSEEK_FORCE_HTTP1 is set to a truthy value
/// (`1`, `true`, `yes`, `on`, case-insensitive). Used by `build_http_client`
/// to opt out of HTTP/2 entirely when DeepSeek's edge mishandles long-lived H2
/// streams (#103). Anything else (unset, `0`, `false`, ...) leaves HTTP/2 on.
fn force_http1_from_env() -> bool {
    std::env::var("DEEPSEEK_FORCE_HTTP1")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

/// Read `SSL_CERT_FILE` and add its contents as extra root
/// certificates on the reqwest builder (#418). Tries the PEM-bundle
/// parser first (covers single-cert files too), then falls back to
/// DER. All failures log a warning and return the builder unchanged
/// so a malformed env var degrades gracefully.
fn add_extra_root_certs(
    mut builder: reqwest::ClientBuilder,
    cert_path: &str,
) -> reqwest::ClientBuilder {
    let bytes = match std::fs::read(cert_path) {
        Ok(b) => b,
        Err(err) => {
            logging::warn(format!(
                "SSL_CERT_FILE={cert_path} could not be read: {err}"
            ));
            return builder;
        }
    };

    if let Ok(certs) = reqwest::Certificate::from_pem_bundle(&bytes) {
        let added = certs.len();
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
        logging::info(format!(
            "SSL_CERT_FILE={cert_path} loaded ({added} cert(s))"
        ));
        return builder;
    }

    match reqwest::Certificate::from_der(&bytes) {
        Ok(cert) => {
            builder = builder.add_root_certificate(cert);
            logging::info(format!("SSL_CERT_FILE={cert_path} loaded (1 DER cert)"));
        }
        Err(err) => {
            logging::warn(format!(
                "SSL_CERT_FILE={cert_path} could not be parsed as PEM bundle or DER: {err}"
            ));
        }
    }
    builder
}

impl DeepSeekClient {
    /// Create a DeepSeek client from CLI configuration.
    pub fn new(config: &Config) -> Result<Self> {
        Self::from_parts(config.deepseek_base_url(), config.default_model(), config)
    }

    /// Create a DeepSeek client whose transport is bound to a runtime-resolved
    /// route (#3384).
    ///
    /// The base URL and default model come from the executable `candidate`, so
    /// the client talks to exactly the endpoint and wire model the resolver
    /// chose instead of re-deriving them from `Config`. Secrets stay in
    /// `Config`: `ReadyRouteCandidate` is secret-free by design (it carries only
    /// an auth-source *class*), so the API key and provider are still read from
    /// `config`.
    pub fn from_candidate(config: &Config, candidate: &ReadyRouteCandidate) -> Result<Self> {
        Self::from_parts(
            candidate.endpoint.base_url.clone(),
            candidate.wire_model_id.as_str().to_string(),
            config,
        )
    }

    /// Shared constructor body for [`Self::new`] and [`Self::from_candidate`].
    ///
    /// `base_url` and `default_model` are the only inputs that differ between
    /// the two entry points; everything else (auth, provider, retry, headers,
    /// timeouts) is derived from `config` so the two paths cannot drift.
    fn from_parts(base_url: String, default_model: String, config: &Config) -> Result<Self> {
        let api_provider = config.api_provider();
        if api_provider == ApiProvider::OpencodeGo {
            validate_route(api_provider, &default_model).map_err(anyhow::Error::msg)?;
        }
        let api_key = config.deepseek_api_key()?;
        let model_bound_secret_values =
            Arc::new(configured_model_bound_secret_values(config, &api_key));
        validate_base_url_security(&base_url)?;
        let retry = config.retry_policy();
        let stream_idle_timeout = Duration::from_secs(config.stream_chunk_timeout_secs());
        let http_headers = config.http_headers();
        let auth_disabled =
            auth_mode_disables_api_key(config.auth_mode_for_provider(api_provider).as_deref());
        let insecure_skip_tls_verify = config.insecure_skip_tls_verify();
        let path_suffix = config
            .provider_config_for(api_provider)
            .and_then(|p| p.path_suffix.clone());
        let reasoning_stream_style = config
            .provider_config_for(api_provider)
            .and_then(|p| p.reasoning_stream_style.clone());
        let request_concurrency_limit = config.provider_max_concurrency(api_provider);

        logging::info(format!("API provider: {}", api_provider.as_str()));
        logging::info(format!(
            "API base URL: {}",
            redact_url_for_display(&base_url)
        ));
        if let Some(suffix) = &path_suffix {
            logging::info(format!("API path suffix override: {suffix}"));
        }
        if !http_headers.is_empty() {
            logging::info(format!(
                "{} custom HTTP header(s) configured",
                http_headers.len()
            ));
        }
        if insecure_skip_tls_verify {
            logging::warn(format!(
                "TLS certificate verification cannot be disabled for provider {}; use SSL_CERT_FILE with a trusted custom CA bundle instead",
                api_provider.as_str()
            ));
            bail!(
                "TLS certificate verification cannot be disabled for provider {}; configure SSL_CERT_FILE with a trusted custom CA bundle instead",
                api_provider.as_str()
            );
        }
        logging::info(format!(
            "Retry policy: enabled={}, max_retries={}, initial_delay={}s, max_delay={}s",
            retry.enabled, retry.max_retries, retry.initial_delay, retry.max_delay
        ));
        if let Some(limit) = request_concurrency_limit {
            logging::info(format!(
                "Provider request concurrency cap: {} in-flight request(s)",
                limit
            ));
        }

        let http_client = Self::build_http_client_with_auth_mode(
            &api_key,
            &http_headers,
            api_provider,
            &base_url,
            auth_disabled,
        )?;

        Ok(Self {
            http_client,
            api_key,
            model_bound_secret_values,
            base_url,
            api_provider,
            retry,
            default_model,
            connection_health: Arc::new(AsyncMutex::new(ConnectionHealth::default())),
            rate_limiter: Arc::new(AsyncMutex::new(TokenBucket::from_env())),
            request_concurrency: request_concurrency_limit.map(ProviderConcurrencyLimiter::new),
            path_suffix,
            reasoning_stream_style,
            stream_idle_timeout,
        })
    }

    /// Return a request whose tool results are safe to send to an upstream
    /// model provider.
    ///
    /// Tool output is untrusted model-bound data: it can contain a whole
    /// config file, a bare credential emitted by a shell command, or a
    /// spillover receipt whose backing content is later persisted by the chat
    /// adapter. Keep this boundary above all protocol adapters so Chat,
    /// Anthropic Messages, and OpenAI Responses — streaming and non-streaming
    /// alike — receive the same sanitized payload.
    fn prepare_model_bound_request(&self, mut request: MessageRequest) -> MessageRequest {
        for message in &mut request.messages {
            for block in &mut message.content {
                if let ContentBlock::ToolResult { content, .. } = block {
                    *content = redact_model_bound_text(content, &self.model_bound_secret_values);
                }
            }
        }
        request
    }

    #[cfg(test)]
    fn build_http_client(
        api_key: &str,
        extra_headers: &HashMap<String, String>,
        api_provider: ApiProvider,
        base_url: &str,
    ) -> Result<reqwest::Client> {
        Self::build_http_client_with_auth_mode(
            api_key,
            extra_headers,
            api_provider,
            base_url,
            false,
        )
    }

    fn build_http_client_with_auth_mode(
        api_key: &str,
        extra_headers: &HashMap<String, String>,
        api_provider: ApiProvider,
        base_url: &str,
        auth_disabled: bool,
    ) -> Result<reqwest::Client> {
        let headers = build_default_headers(
            api_key,
            extra_headers,
            api_provider,
            base_url,
            auth_disabled,
        )?;
        // The ChatGPT Codex backend sits behind Cloudflare bot protection that
        // only admits the Codex CLI's user agent; present a codex_cli_rs UA on
        // that path so the request is handled like the official client.
        let user_agent: &str = if api_provider == ApiProvider::OpenaiCodex {
            concat!(
                "codex_cli_rs/0.137.0 (CodeWhale ",
                env!("CARGO_PKG_VERSION"),
                ")"
            )
        } else {
            concat!(
                "Mozilla/5.0 (compatible; codewhale/",
                env!("CARGO_PKG_VERSION"),
                "; +https://github.com/Hmbown/CodeWhale)"
            )
        };
        let mut builder = crate::tls::reqwest_client_builder()
            .default_headers(headers)
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(30))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .http2_keep_alive_interval(Some(Duration::from_secs(15)))
            .http2_keep_alive_timeout(Duration::from_secs(20))
            .min_tls_version(reqwest::tls::Version::TLS_1_2);
        if force_http1_from_env() {
            logging::info("DEEPSEEK_FORCE_HTTP1=1 — pinning HTTP client to HTTP/1.1");
            builder = builder.http1_only();
        }
        if let Ok(cert_path) = std::env::var("SSL_CERT_FILE")
            && !cert_path.is_empty()
        {
            builder = add_extra_root_certs(builder, &cert_path);
        }
        builder.build().map_err(Into::into)
    }

    #[cfg(test)]
    fn default_headers(
        api_key: &str,
        extra_headers: &HashMap<String, String>,
    ) -> Result<HeaderMap> {
        build_default_headers(
            api_key,
            extra_headers,
            ApiProvider::Deepseek,
            crate::config::DEFAULT_DEEPSEEK_BASE_URL,
            false,
        )
    }

    #[cfg(test)]
    fn default_headers_for_provider(
        api_key: &str,
        extra_headers: &HashMap<String, String>,
        api_provider: ApiProvider,
        base_url: &str,
    ) -> Result<HeaderMap> {
        build_default_headers(api_key, extra_headers, api_provider, base_url, false)
    }

    #[cfg(test)]
    fn default_headers_for_provider_with_auth_disabled(
        api_key: &str,
        extra_headers: &HashMap<String, String>,
        api_provider: ApiProvider,
        base_url: &str,
    ) -> Result<HeaderMap> {
        build_default_headers(api_key, extra_headers, api_provider, base_url, true)
    }
}

fn build_default_headers(
    api_key: &str,
    extra_headers: &HashMap<String, String>,
    api_provider: ApiProvider,
    base_url: &str,
    auth_disabled: bool,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let api_key = api_key.trim();
    if api_provider_uses_anthropic_messages(api_provider) {
        // #3014: most Messages API routes authenticate with `x-api-key`.
        // OpenModel also supports Bearer auth for Messages, and its `/models`
        // endpoint requires it, so the header chooser below keeps OpenModel on
        // Bearer while still pinning the Anthropic wire contract here.
        headers.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static("2023-06-01"),
        );
    }
    let auth_header_name = if auth_disabled {
        None
    } else if !api_key.is_empty()
        && api_provider_uses_anthropic_messages(api_provider)
        && api_provider != ApiProvider::Openmodel
    {
        Some(HeaderName::from_static("x-api-key"))
    } else if !api_key.is_empty()
        && api_provider == ApiProvider::XiaomiMimo
        && (xiaomi_mimo_base_url_uses_token_plan(base_url)
            || xiaomi_mimo_api_key_uses_token_plan(api_key))
    {
        Some(HeaderName::from_static("api-key"))
    } else if !api_key.is_empty() {
        Some(AUTHORIZATION)
    } else {
        None
    };
    if let Some(header_name) = auth_header_name.as_ref() {
        let header_value = if *header_name == AUTHORIZATION {
            HeaderValue::from_str(&format!("Bearer {api_key}"))?
        } else {
            HeaderValue::from_str(api_key)?
        };
        headers.insert(header_name.clone(), header_value);
    }
    for (name, value) in extra_headers {
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if auth_disabled && is_upstream_auth_header(name) {
            continue;
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())?;
        if header_name == AUTHORIZATION
            || header_name == CONTENT_TYPE
            || auth_header_name.as_ref() == Some(&header_name)
            || (auth_header_name.is_some() && is_auth_dialect_header(&header_name))
        {
            continue;
        }
        headers.insert(header_name, HeaderValue::from_str(value)?);
    }
    Ok(headers)
}

fn is_auth_dialect_header(header_name: &HeaderName) -> bool {
    header_name == AUTHORIZATION
        || header_name == HeaderName::from_static("api-key")
        || header_name == HeaderName::from_static("x-api-key")
}

fn api_provider_uses_anthropic_messages(api_provider: ApiProvider) -> bool {
    matches!(
        api_provider,
        ApiProvider::Anthropic
            | ApiProvider::DeepseekAnthropic
            | ApiProvider::MinimaxAnthropic
            | ApiProvider::Openmodel
    )
}

fn api_provider_skips_models_probe(api_provider: ApiProvider) -> bool {
    matches!(api_provider, ApiProvider::DeepseekAnthropic)
}

#[must_use]
#[cfg(test)]
pub(crate) fn provider_api_key_verification_is_observed(api_provider: ApiProvider) -> bool {
    !api_provider_skips_models_probe(api_provider)
}

/// Verify a provider API key by hitting the `/models` endpoint
/// (#3875). Builds a minimal HTTP client with the canonical auth
/// headers for `provider`, issues a single GET, and returns
/// `Ok(())` on a 2xx response or `Err(reason)` on any failure.
///
/// This is intentionally a one-shot call — no retry, no rate-limit
/// wait — so a bad key is surfaced immediately.
pub async fn verify_provider_api_key(
    provider: ApiProvider,
    api_key: &str,
    base_url: &str,
) -> Result<(), String> {
    if api_provider_skips_models_probe(provider) {
        // Providers without a /models endpoint can't be verified this
        // way; accept the key optimistically (same as health_check).
        return Ok(());
    }
    let headers = build_default_headers(api_key, &Default::default(), provider, base_url, false)
        .map_err(|err| format!("failed to build auth headers: {err:#}"))?;
    let client = crate::tls::reqwest_client_builder()
        .default_headers(headers)
        .user_agent(concat!(
            "Mozilla/5.0 (compatible; codewhale/",
            env!("CARGO_PKG_VERSION"),
            "; +https://github.com/Hmbown/CodeWhale)"
        ))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|err| format!("failed to build HTTP client: {err:#}"))?;
    let url = api_url(base_url, "models");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("request failed: {err:#}"))?;
    let status = response.status();
    if status.is_success() {
        // Consume the body so the connection returns to the pool.
        let _ = response.text().await;
        Ok(())
    } else {
        let body = response.text().await.unwrap_or_default();
        let summary = if body.chars().count() > 200 {
            format!("{}...", body.chars().take(200).collect::<String>())
        } else {
            body
        };
        Err(format!("HTTP {status}: {summary}"))
    }
}

fn translation_system_prompt(target_language: &str) -> String {
    format!(
        "You are a professional translator. Your ONLY task is to translate text to {target_language}. \
         Rules:\n\
         1. Output ONLY the translation, nothing else — no explanations, no notes, no quotes.\n\
         2. Preserve all code blocks (```...```), URLs, file paths, command names, \
         and technical terms like API names, function names, and library names untranslated.\n\
         3. Keep Markdown formatting (headings, lists, bold, italics, links) intact.\n\
         4. Translate all natural-language prose naturally and professionally.\n\
         5. Do NOT add any prefix, suffix, or commentary.\n\
         6. If the input is already in {target_language} or contains no prose to translate, \
         return it as-is."
    )
}

fn translation_message_request(text: &str, model: String, target_language: &str) -> MessageRequest {
    MessageRequest {
        model,
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }],
        max_tokens: 4096,
        system: Some(SystemPrompt::Text(translation_system_prompt(
            target_language,
        ))),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: Some("off".to_string()),
        stream: Some(false),
        temperature: Some(0.1),
        top_p: None,
    }
}

fn translation_text_from_response(response: &MessageResponse) -> Result<String> {
    let translated = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    if translated.is_empty() {
        bail!("translate: Anthropic Messages response did not contain text content");
    }
    Ok(translated)
}

fn xiaomi_mimo_base_url_uses_token_plan(base_url: &str) -> bool {
    let normalized = base_url.trim().to_ascii_lowercase();
    let without_scheme = normalized
        .strip_prefix("https://")
        .or_else(|| normalized.strip_prefix("http://"))
        .unwrap_or(&normalized);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = host.split(':').next().unwrap_or(host);
    host.starts_with("token-plan-") && host.ends_with(".xiaomimimo.com")
}

fn xiaomi_mimo_api_key_uses_token_plan(api_key: &str) -> bool {
    api_key.trim_start().starts_with("tp-")
}

impl DeepSeekClient {
    /// Returns the API base URL used by this client.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the active API provider for this client.
    pub fn api_provider(&self) -> ApiProvider {
        self.api_provider
    }

    /// Resolved in-flight provider request cap, if one is active.
    #[must_use]
    pub fn provider_request_concurrency_limit(&self) -> Option<usize> {
        self.request_concurrency
            .as_ref()
            .map(ProviderConcurrencyLimiter::limit)
    }

    /// Number of currently active requests held by this client's shared
    /// provider request limiter.
    #[must_use]
    pub fn active_provider_requests(&self) -> usize {
        self.request_concurrency
            .as_ref()
            .map_or(0, ProviderConcurrencyLimiter::active)
    }

    async fn acquire_provider_request_permit(&self) -> Option<ProviderRequestPermit> {
        match self.request_concurrency.as_ref() {
            Some(limiter) => limiter.acquire().await,
            None => None,
        }
    }

    fn hold_provider_request_permit_for_stream(
        stream: crate::llm_client::StreamEventBox,
        permit: Option<ProviderRequestPermit>,
    ) -> crate::llm_client::StreamEventBox {
        Box::pin(async_stream::stream! {
            let _permit = permit;
            let mut stream = stream;
            while let Some(event) = stream.next().await {
                yield event;
            }
        })
    }

    /// Translate text to the requested target language using a focused
    /// non-streaming chat completion call on the supplied model.
    ///
    /// This is a lightweight translation service — no tool calls, no
    /// streaming, no conversation history. The dedicated translation agent
    /// receives the source text and returns only the translated result.
    pub async fn translate(
        &self,
        text: &str,
        model: &str,
        target_language: &str,
    ) -> Result<String> {
        let model = wire_model_for_provider_route(self.api_provider, &self.base_url, model);
        if api_provider_uses_anthropic_messages(self.api_provider) {
            let response = self
                .handle_anthropic_message(translation_message_request(text, model, target_language))
                .await?;
            return translation_text_from_response(&response);
        }

        let url = api_url_with_suffix(
            &self.base_url,
            "chat/completions",
            self.path_suffix.as_deref(),
        );
        let mut body = serde_json::json!({
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": translation_system_prompt(target_language)
                },
                {
                    "role": "user",
                    "content": text
                }
            ],
            "max_tokens": 4096,
            "temperature": 0.1,
            "stream": false
        });
        apply_reasoning_effort(&mut body, Some("off"), self.api_provider);

        let response = self.send_json_with_retry(&url, &body).await?;

        let value: serde_json::Value = response.json().await?;
        let translated = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("translate: unexpected API response shape"))?
            .trim()
            .to_string();

        Ok(translated)
    }

    /// List available models from the provider.
    pub async fn list_models(&self) -> Result<Vec<AvailableModel>> {
        let url = api_url(&self.base_url, "models");
        let response = self.send_with_retry(|| self.http_client.get(&url)).await?;

        let status = response.status();
        if !status.is_success() {
            let raw_error_text = bounded_error_text(response, ERROR_BODY_MAX_BYTES).await;
            let error_text = sanitize_http_error_body(
                Some(self.api_provider.display_name()),
                status.as_u16(),
                &raw_error_text,
            );
            anyhow::bail!("Failed to list models: HTTP {status}: {error_text}");
        }
        let response_text = response
            .text()
            .await
            .context("Failed to read models response body")?;

        parse_models_response(&response_text)
            .map(|models| apply_provider_model_cutline(self.api_provider, models))
    }

    /// The catalog provider id for this client (the `ProviderKind` slug, falling
    /// back to the `ApiProvider` slug for legacy variants without a kind). This
    /// is the id used as the cache scope and `CatalogOffering.provider`.
    fn catalog_provider_id(&self) -> String {
        self.api_provider
            .kind()
            .map(|kind| kind.as_str().to_string())
            .unwrap_or_else(|| self.api_provider.as_str().to_string())
    }

    /// Fetch the provider's live `/models` listing as a secret-free
    /// [`ProviderCatalogDelta`] (#3385).
    ///
    /// Uses the same URL construction and auth client as [`Self::list_models`],
    /// but issues a single request without `send_with_retry` so a refresh
    /// failure stays typed and non-fatal — bundled / saved / static rows are
    /// untouched. The delta is scoped to the base-URL fingerprint and stamped
    /// with the fetch time; the API key authorizes the request but is **never**
    /// persisted into the delta or cache. Unknown live rows carry no canonical
    /// model, capabilities, or pricing, per the #3385 contract.
    pub async fn fetch_catalog_delta(&self) -> Result<ProviderCatalogDelta, CatalogRefreshError> {
        let url = api_url(&self.base_url, "models");
        // A catalog refresh is non-fatal and must produce a *typed* outcome, so
        // it issues a single request and maps the raw status. This intentionally
        // does NOT route through `send_with_retry` like `list_models` does: that
        // path erases the HTTP status into a generic error and retries
        // non-retryable auth failures, neither of which suits a typed refresh.
        // Auth headers are baked into `http_client` (the key is used but never
        // persisted into the delta or cache).
        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|_| CatalogRefreshError::Network)?;

        let status = response.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 => CatalogRefreshError::Unauthorized,
                403 => CatalogRefreshError::Forbidden,
                404 => CatalogRefreshError::NotFound,
                429 => CatalogRefreshError::RateLimited,
                // Any other non-success (5xx, unexpected) is treated as a
                // transient transport-class failure.
                _ => CatalogRefreshError::Network,
            });
        }

        let body = response
            .text()
            .await
            .map_err(|_| CatalogRefreshError::Network)?;

        let provider = self.catalog_provider_id();
        let fingerprint = base_url_fingerprint(&self.base_url);
        let fetched_at = now_unix();

        // OpenRouter returns extended capability metadata in its /models
        // response (#3385). Capture limits, pricing, reasoning, and modalities
        // from the live API instead of leaving them unknown.
        let offerings: Vec<CatalogOffering> = if provider == "openrouter" {
            let or_models = parse_openrouter_models_response(&body)?;
            if or_models.is_empty() {
                return Err(CatalogRefreshError::EmptyList);
            }
            or_models
                .iter()
                .map(|item| {
                    openrouter_to_catalog_offering(item, &provider, &fingerprint, fetched_at)
                })
                .collect()
        } else {
            let models = apply_provider_model_cutline(
                self.api_provider,
                parse_models_response(&body).map_err(|_| CatalogRefreshError::InvalidResponse)?,
            );
            if models.is_empty() {
                return Err(CatalogRefreshError::EmptyList);
            }
            models
                .into_iter()
                .map(|model| CatalogOffering {
                    provider: provider.clone(),
                    wire_model_id: model.id,
                    canonical_model: None,
                    endpoint_key: "chat".to_string(),
                    default_for_provider: false,
                    family: None,
                    limit: None,
                    cost: None,
                    modalities: None,
                    reasoning: None,
                    tool_call: None,
                    reasoning_options: Vec::new(),
                    source: CatalogSource::Live {
                        base_url_fingerprint: fingerprint.clone(),
                        fetched_at,
                    },
                })
                .collect()
        };

        Ok(ProviderCatalogDelta {
            provider,
            base_url_fingerprint: fingerprint,
            fetched_at,
            offerings,
        })
    }

    /// Refresh `cache` for this client's provider + base URL, recording either a
    /// success or a typed failure (#3385). Returns the resulting status so the UI
    /// can surface a visible "fresh / failed(reason)" chip without inspecting the
    /// cache internals. A failed refresh preserves any previously cached rows.
    pub async fn refresh_catalog_cache(
        &self,
        cache: &mut ProviderCatalogCache,
        ttl_secs: u64,
    ) -> CatalogStatus {
        match self.fetch_catalog_delta().await {
            Ok(delta) => {
                cache.record_success(delta, ttl_secs);
                publish_provider_lake_snapshot(cache);
                CatalogStatus::Fresh
            }
            Err(reason) => {
                cache.record_failure(
                    &self.catalog_provider_id(),
                    &base_url_fingerprint(&self.base_url),
                    reason,
                );
                publish_provider_lake_snapshot(cache);
                CatalogStatus::Failed { reason }
            }
        }
    }

    /// Generate speech with Xiaomi MiMo TTS models.
    ///
    /// The spoken text is placed in an `assistant` message because Xiaomi
    /// MiMo's TTS chat-completions surface expects that shape. The optional
    /// `instruction` is a `user` message that controls style, voice design, or
    /// voice-clone performance and is not spoken verbatim.
    pub async fn synthesize_speech(
        &self,
        request: SpeechSynthesisRequest,
    ) -> Result<SpeechSynthesisResponse> {
        if self.api_provider != crate::config::ApiProvider::XiaomiMimo {
            anyhow::bail!(
                "speech synthesis requires provider 'xiaomi-mimo' (current: {})",
                self.api_provider.as_str()
            );
        }

        let model = request.model.trim().to_string();
        if model.is_empty() {
            anyhow::bail!("Speech model cannot be empty");
        }
        let text = request.text.trim().to_string();
        if text.is_empty() {
            anyhow::bail!("Speech text cannot be empty");
        }

        let audio_format = normalize_audio_format(&request.audio_format);
        let model = wire_model_for_provider_route(self.api_provider, &self.base_url, &model);
        let model_lower = model.to_ascii_lowercase();
        let instruction = request
            .instruction
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let voice = request
            .voice
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        if model_lower.contains("voicedesign") && instruction.is_none() {
            anyhow::bail!(
                "Model '{model}' requires a voice design prompt. Pass --voice-prompt or --instruction."
            );
        }
        if model_lower.contains("voiceclone") && voice.is_none() {
            anyhow::bail!(
                "Model '{model}' requires cloned voice data. Pass --clone-voice <mp3|wav> or --voice <data-uri>."
            );
        }

        let mut audio = json!({
            "format": audio_format.clone(),
        });
        if let Some(voice) = voice.as_deref() {
            audio["voice"] = json!(voice);
        }

        let body = build_speech_synthesis_body(&model, &text, instruction, audio);

        let url = api_url(&self.base_url, "chat/completions");
        let response = self.send_json_with_retry(&url, &body).await?;
        let status = response.status();
        if !status.is_success() {
            let raw_error_text = bounded_error_text(response, ERROR_BODY_MAX_BYTES).await;
            let error_text = sanitize_http_error_body(
                Some(self.api_provider.display_name()),
                status.as_u16(),
                &raw_error_text,
            );
            anyhow::bail!("Speech synthesis failed: HTTP {status}: {error_text}");
        }

        let response_text = response
            .text()
            .await
            .context("Failed to read speech synthesis response body")?;
        let payload: Value = serde_json::from_str(&response_text)
            .context("Failed to parse speech synthesis response JSON")?;
        let (audio_bytes, transcript) = parse_speech_audio_response(&payload)?;

        Ok(SpeechSynthesisResponse {
            model,
            audio_format,
            audio_bytes,
            transcript,
            voice,
        })
    }

    async fn wait_for_rate_limit(&self) {
        let maybe_delay = {
            let mut limiter = self.rate_limiter.lock().await;
            limiter.delay_until_available(1.0)
        };
        if let Some(delay) = maybe_delay {
            tokio::time::sleep(delay).await;
        }
    }

    async fn mark_request_success(&self) {
        let mut health = self.connection_health.lock().await;
        if apply_request_success(&mut health, Instant::now()) {
            logging::info("Connection recovered");
        }
    }

    async fn mark_request_failure(&self, reason: &str) {
        let mut health = self.connection_health.lock().await;
        apply_request_failure(&mut health, Instant::now());
        logging::warn(format!(
            "Connection degraded (failures={}): {}",
            health.consecutive_failures, reason
        ));
    }

    async fn maybe_probe_recovery(&self) {
        let should_probe = {
            let mut health = self.connection_health.lock().await;
            mark_recovery_probe_if_due(&mut health, Instant::now())
        };
        if !should_probe {
            return;
        }
        if api_provider_skips_models_probe(self.api_provider) {
            self.mark_request_success().await;
            logging::info("Skipping /models recovery probe for provider without a models endpoint");
            return;
        }
        let health_url = api_url(&self.base_url, "models");
        let probe = self.http_client.get(health_url).send().await;
        match probe {
            Ok(resp) if resp.status().is_success() => {
                // Consume the response body so the connection can be returned to the pool.
                let _ = resp.text().await;
                self.mark_request_success().await;
                logging::info("Recovery probe succeeded");
            }
            Ok(resp) => {
                self.mark_request_failure(&format!("probe status={}", resp.status()))
                    .await;
            }
            Err(err) => {
                self.mark_request_failure(&format!("probe error={err}"))
                    .await;
            }
        }
    }

    pub(super) async fn send_with_retry<F>(&self, mut build: F) -> Result<reqwest::Response>
    where
        F: FnMut() -> reqwest::RequestBuilder,
    {
        let retry_cfg: LlmRetryConfig = self.retry.clone().into();
        let request_result = with_retry(
            &retry_cfg,
            || {
                let request = build();
                async move {
                    // Sleep in bounded slices rather than the full remaining
                    // window: the pause is process-global, so a concurrent
                    // `clear_rate_limit()` (or a shortened deadline) must
                    // release requests that are already waiting instead of
                    // stranding them for the whole original window.
                    while let Some(delay) = crate::retry_status::rate_limit_remaining() {
                        tokio::time::sleep(delay.min(RATE_LIMIT_PAUSE_RECHECK_INTERVAL)).await;
                    }
                    self.wait_for_rate_limit().await;
                    let response = request
                        .send()
                        .await
                        .map_err(|err| LlmError::from_reqwest(&err))?;
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    }
                    let retry_after = extract_retry_after(response.headers());
                    let body = bounded_error_text(response, ERROR_BODY_MAX_BYTES).await;
                    let body = sanitize_http_error_body(
                        Some(self.api_provider.display_name()),
                        status.as_u16(),
                        &body,
                    );
                    Err(LlmError::from_http_response_with_retry_after(
                        status.as_u16(),
                        &body,
                        retry_after,
                    ))
                }
            },
            Some(Box::new(|err, attempt, delay| {
                let (reason_label, human_reason) = retry_reason_label_and_human(err);
                logging::warn(format!(
                    "HTTP retry reason={} attempt={} delay={:.2}s",
                    reason_label,
                    attempt + 1,
                    delay.as_secs_f64(),
                ));
                if matches!(err, LlmError::RateLimited { .. }) {
                    crate::retry_status::note_rate_limit(delay);
                }
                crate::retry_status::start(attempt + 1, delay, human_reason);
            })),
        )
        .await;

        match request_result {
            Ok(response) => {
                crate::retry_status::succeeded();
                self.mark_request_success().await;
                Ok(response)
            }
            Err(err) => {
                if let LlmError::RateLimited { retry_after, .. } = &err.last_error {
                    crate::retry_status::note_rate_limit(
                        retry_after
                            .unwrap_or_else(|| retry_cfg.delay_for_attempt(retry_cfg.max_retries)),
                    );
                }
                let last = err.last_error.to_string();
                if err.attempts > 1 {
                    crate::retry_status::failed(last.clone());
                } else {
                    crate::retry_status::clear();
                }
                self.mark_request_failure(&last).await;
                self.maybe_probe_recovery().await;
                // Keep the structured `LlmError` downcastable so failure
                // surfaces can classify auth/rate-limit/invalid-request
                // instead of reporting an opaque string (#3884).
                Err(anyhow::Error::new(err.last_error))
            }
        }
    }

    pub(super) async fn send_json_with_retry(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response> {
        let request_body =
            serde_json::to_vec(body).context("Failed to serialize JSON request body")?;
        self.send_with_retry(|| {
            self.http_client
                .post(url)
                .header(CONTENT_TYPE, "application/json")
                .body(request_body.clone())
        })
        .await
    }
}

/// Translate the structured `LlmError` into both a categorical label
/// (for structured logs / metrics) and a short human reason string
/// (for the retry banner). Returning both from one match avoids the
/// double-classification we had before.
fn retry_reason_label_and_human(err: &LlmError) -> (&'static str, String) {
    match err {
        LlmError::RateLimited { retry_after, .. } => {
            let human = if let Some(after) = retry_after {
                format!("rate limited (Retry-After {}s)", after.as_secs())
            } else {
                "rate limited".to_string()
            };
            ("rate_limited", human)
        }
        LlmError::ServerError { status, .. } => ("server_error", format!("upstream {status}")),
        LlmError::NetworkError(_) => ("network_error", "network error".to_string()),
        LlmError::Timeout(_) => ("timeout", "timeout".to_string()),
        _ => ("other", "other".to_string()),
    }
}

impl LlmClient for DeepSeekClient {
    fn provider_name(&self) -> &'static str {
        self.api_provider.as_str()
    }

    fn model(&self) -> &str {
        &self.default_model
    }

    async fn health_check(&self) -> Result<bool> {
        if api_provider_skips_models_probe(self.api_provider) {
            self.mark_request_success().await;
            return Ok(true);
        }
        let health_url = api_url(&self.base_url, "models");
        self.wait_for_rate_limit().await;
        let response = self.http_client.get(health_url).send().await;
        match response {
            Ok(resp) if resp.status().is_success() => {
                // Consume the response body so the connection can be returned to the pool.
                let _ = resp.text().await;
                self.mark_request_success().await;
                Ok(true)
            }
            Ok(resp) => {
                self.mark_request_failure(&format!("health status={}", resp.status()))
                    .await;
                Ok(false)
            }
            Err(err) => {
                self.mark_request_failure(&format!("health error={err}"))
                    .await;
                Ok(false)
            }
        }
    }

    async fn create_message(&self, request: MessageRequest) -> Result<MessageResponse> {
        let _permit = self.acquire_provider_request_permit().await;
        let request = self.prepare_model_bound_request(request);
        if self.api_provider == ApiProvider::OpenaiCodex {
            return self.handle_responses_message(request).await;
        }
        if api_provider_uses_anthropic_messages(self.api_provider) {
            return self.handle_anthropic_message(request).await;
        }
        self.create_message_chat(&request).await
    }

    async fn create_message_stream(
        &self,
        request: MessageRequest,
    ) -> Result<crate::llm_client::StreamEventBox> {
        let permit = self.acquire_provider_request_permit().await;
        let request = self.prepare_model_bound_request(request);
        if self.api_provider == ApiProvider::OpenaiCodex {
            let stream = self.handle_responses_stream(request).await?;
            return Ok(Self::hold_provider_request_permit_for_stream(
                stream, permit,
            ));
        }
        if api_provider_uses_anthropic_messages(self.api_provider) {
            let stream = self.handle_anthropic_stream(request).await?;
            return Ok(Self::hold_provider_request_permit_for_stream(
                stream, permit,
            ));
        }
        let stream = self.handle_chat_completion_stream(request).await?;
        Ok(Self::hold_provider_request_permit_for_stream(
            stream, permit,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelListItem>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModelItem>,
}

#[derive(Debug, Deserialize)]
struct ModelListItem {
    id: String,
    #[serde(default)]
    owned_by: Option<String>,
    #[serde(default)]
    created: Option<u64>,
}

/// OpenRouter `/models` response item with full capability metadata (#3385).
#[derive(Debug, Deserialize)]
struct OpenRouterModelItem {
    id: String,
    // Captured from OpenRouter for future display/deprecation surfaces. The
    // current CatalogOffering shape has no honest fields for these yet.
    #[allow(dead_code)]
    #[serde(default)]
    name: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    created: Option<u64>,
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    pricing: Option<OpenRouterPricing>,
    #[serde(default)]
    top_provider: Option<OpenRouterTopProvider>,
    #[serde(default)]
    supported_parameters: Option<Vec<String>>,
    #[serde(default)]
    architecture: Option<OpenRouterArchitecture>,
    #[allow(dead_code)]
    #[serde(default)]
    expiration_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterPricing {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
    #[serde(default)]
    input_cache_read: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterTopProvider {
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    max_completion_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    modality: Option<String>,
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    output_modalities: Option<Vec<String>>,
}

pub(super) fn parse_models_response(payload: &str) -> Result<Vec<AvailableModel>> {
    let parsed: ModelsListResponse =
        serde_json::from_str(payload).context("Failed to parse model list JSON")?;

    let mut models = parsed
        .data
        .into_iter()
        .map(|item| AvailableModel {
            id: item.id,
            owned_by: item.owned_by,
            created: item.created,
        })
        .collect::<Vec<_>>();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

/// Apply provider-owned protocol cutlines to a live `/models` response.
///
/// OpenCode Go mixes OpenAI Chat Completions and Anthropic Messages models in
/// one roster. Codewhale's `OpencodeGo` route is intentionally Chat-only, so
/// both `/models` consumers must share this filter before publishing choices.
fn apply_provider_model_cutline(
    provider: ApiProvider,
    models: Vec<AvailableModel>,
) -> Vec<AvailableModel> {
    if provider != ApiProvider::OpencodeGo {
        return models;
    }

    let mut models: Vec<_> = models
        .into_iter()
        .filter_map(|mut model| {
            let canonical = crate::config::opencode_go_chat_model_id(&model.id)?;
            model.id = canonical.to_string();
            Some(model)
        })
        .collect();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    models
}

/// Parse an OpenRouter `/models` response, preserving server-side ordering and
/// capturing full capability metadata (#3385).
fn parse_openrouter_models_response(
    payload: &str,
) -> Result<Vec<OpenRouterModelItem>, CatalogRefreshError> {
    let parsed: OpenRouterModelsResponse =
        serde_json::from_str(payload).map_err(|_| CatalogRefreshError::InvalidResponse)?;
    let mut seen = std::collections::HashSet::new();
    let models: Vec<_> = parsed
        .data
        .into_iter()
        .filter(|item| seen.insert(item.id.clone()))
        .collect();
    Ok(models)
}

fn publish_provider_lake_snapshot(cache: &ProviderCatalogCache) {
    // Publish fresh *and* stale/prior rows so pickers keep live catalog coverage
    // after TTL expiry or a failed refresh (#4139). Empty caches clear the live
    // layer and fall back to the bundled snapshot.
    let offerings = cache.all_visible_offerings(now_unix());
    if offerings.is_empty() {
        crate::provider_lake::clear_live_snapshot();
    } else {
        crate::provider_lake::set_live_snapshot(CatalogSnapshot { offerings });
    }
}

/// Convert an OpenRouter model item into a [`CatalogOffering`] with live-sourced
/// limits, pricing, reasoning, and modalities (#3385).
fn openrouter_to_catalog_offering(
    item: &OpenRouterModelItem,
    provider: &str,
    base_url_fingerprint: &str,
    fetched_at: u64,
) -> CatalogOffering {
    use codewhale_config::models_dev::{ModelsDevCost, ModelsDevLimit, ModelsDevModalities};

    let context_length = item
        .top_provider
        .as_ref()
        .and_then(|tp| tp.context_length)
        .or(item.context_length);

    let max_output = item
        .top_provider
        .as_ref()
        .and_then(|tp| tp.max_completion_tokens);

    let limit = if context_length.is_some() || max_output.is_some() {
        Some(ModelsDevLimit {
            context: context_length.map(u64::from),
            input: context_length.map(u64::from),
            output: max_output.map(u64::from),
        })
    } else {
        None
    };

    let cost = item.pricing.as_ref().map(|p| {
        let parse_price = |s: &Option<String>| -> Option<f64> {
            s.as_ref()
                .and_then(|v| v.parse::<f64>().ok())
                .map(|price_per_token| price_per_token * 1_000_000.0)
        };
        ModelsDevCost {
            input: parse_price(&p.prompt),
            output: parse_price(&p.completion),
            cache_read: parse_price(&p.input_cache_read),
            cache_write: None,
        }
    });

    let reasoning = item.supported_parameters.as_ref().map(|params| {
        params
            .iter()
            .any(|p| p == "reasoning" || p == "include_reasoning" || p.contains("reasoning"))
    });

    let tool_call = item.supported_parameters.as_ref().map(|params| {
        params
            .iter()
            .any(|p| p == "tools" || p == "tool_choice" || p == "functions" || p.contains("tool"))
    });

    let modalities = item.architecture.as_ref().map(|arch| {
        let mut input = arch.input_modalities.clone().unwrap_or_default();
        let mut output = arch.output_modalities.clone().unwrap_or_default();
        if input.is_empty()
            && output.is_empty()
            && let Some((left, right)) = arch
                .modality
                .as_deref()
                .and_then(|value| value.split_once("->"))
        {
            input.extend(
                left.split('+')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            );
            output.extend(
                right
                    .split('+')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            );
        }
        ModelsDevModalities { input, output }
    });

    CatalogOffering {
        provider: provider.to_string(),
        wire_model_id: item.id.clone(),
        canonical_model: None,
        endpoint_key: "chat".to_string(),
        default_for_provider: false,
        family: None,
        limit,
        cost,
        modalities,
        reasoning,
        tool_call,
        reasoning_options: Vec::new(),
        source: CatalogSource::Live {
            base_url_fingerprint: base_url_fingerprint.to_string(),
            fetched_at,
        },
    }
}

pub(super) fn system_to_instructions(system: Option<SystemPrompt>) -> Option<String> {
    match system {
        Some(SystemPrompt::Text(text)) => Some(text),
        Some(SystemPrompt::Blocks(blocks)) => {
            let joined = blocks
                .into_iter()
                .map(|b| b.text)
                .collect::<Vec<_>>()
                .join("\n\n---\n\n");
            if joined.trim().is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        None => None,
    }
}

pub(super) fn apply_reasoning_effort(
    body: &mut Value,
    effort: Option<&str>,
    provider: ApiProvider,
) {
    if matches!(provider, ApiProvider::Minimax) {
        // MiniMax's OpenAI-compatible API keeps thinking inside `content`
        // unless reasoning_split is enabled. Always request the split shape
        // so private thinking renders as Thinking cells rather than answer
        // prose.
        body["reasoning_split"] = json!(true);
    }
    let Some(effort) = effort else {
        return;
    };
    let normalized = effort.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "off" | "disabled" | "none" | "false" => match provider {
            ApiProvider::Deepseek
            | ApiProvider::DeepseekCN
            | ApiProvider::Openrouter
            | ApiProvider::XiaomiMimo
            | ApiProvider::Novita
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Sglang
            | ApiProvider::Volcengine
            | ApiProvider::Deepinfra
            | ApiProvider::Together
            | ApiProvider::Atlascloud
            | ApiProvider::Zai => {
                body["thinking"] = json!({ "type": "disabled" });
            }
            ApiProvider::OpenaiCodex => {
                // OpenAI Codex uses Responses API — thinking handled differently
            }
            ApiProvider::Fireworks => {}
            // vLLM is an OpenAI-protocol server, not an Anthropic-protocol one.
            // For Qwen3 / DeepSeek-R1 / other reasoning models hosted via vLLM,
            // the canonical OpenAI extension to disable thinking is
            // `chat_template_kwargs.enable_thinking`. The old
            // `thinking: {type: disabled}` field is Anthropic-native and
            // silently ignored by vLLM — the model still emits a full
            // reasoning trace into the `reasoning` field (which this client
            // doesn't surface), causing 10+ seconds of perceived "freeze"
            // before the first content token (PR #1480 by @h3c-hexin).
            ApiProvider::Vllm => {
                body["chat_template_kwargs"] = json!({
                    "enable_thinking": false,
                });
            }
            ApiProvider::Openai
            | ApiProvider::WanjieArk
            | ApiProvider::Qianfan
            | ApiProvider::Arcee
            | ApiProvider::Huggingface
            | ApiProvider::Custom => {}
            ApiProvider::Moonshot => {
                // #3024: Kimi models accept thinking enable/disable.
                body["thinking"] = json!({ "type": "disabled" });
            }
            ApiProvider::Ollama => {
                // #3024: Ollama OpenAI-compat endpoint accepts think param.
                body["think"] = json!(false);
            }
            ApiProvider::Anthropic
            | ApiProvider::DeepseekAnthropic
            | ApiProvider::MinimaxAnthropic
            | ApiProvider::Openmodel => {
                // Thinking shaping happens in the Messages adapter, which
                // applies each provider's supported control fields.
            }
            ApiProvider::NvidiaNim => {
                body["chat_template_kwargs"] = json!({
                    "thinking": false,
                });
            }
            ApiProvider::Minimax => {
                body["thinking"] = json!({ "type": "disabled" });
            }
            ApiProvider::Stepfun => {}
            ApiProvider::Sakana => {}
            ApiProvider::LongCat => {}
            ApiProvider::OpencodeGo | ApiProvider::OpencodeZen => {}
            ApiProvider::Meta => {}
            ApiProvider::Xai => {}
        },
        "low" | "minimal" | "medium" | "mid" | "high" | "" => match provider {
            // DeepSeek compatibility: low/medium both map to high
            ApiProvider::Deepseek
            | ApiProvider::DeepseekCN
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Sglang
            | ApiProvider::Volcengine
            | ApiProvider::Deepinfra
            | ApiProvider::Atlascloud => {
                body["reasoning_effort"] = json!("high");
                body["thinking"] = json!({ "type": "enabled" });
            }
            // OpenRouter/Novita/Together: pass through the actual user-chosen value.
            // OpenRouter's unified scale is none/minimal/low/medium/high/xhigh;
            // DeepSeek models hosted there accept those directly.
            ApiProvider::Openrouter | ApiProvider::Novita | ApiProvider::Together => {
                let value = match normalized.as_str() {
                    "low" | "minimal" => "low",
                    "medium" | "mid" => "medium",
                    _ => "high",
                };
                body["reasoning_effort"] = json!(value);
                body["thinking"] = json!({ "type": "enabled" });
            }
            ApiProvider::XiaomiMimo => {
                body["thinking"] = json!({ "type": "enabled" });
            }
            ApiProvider::Arcee | ApiProvider::Huggingface => {
                let value = match normalized.as_str() {
                    "minimal" => "minimal",
                    "low" => "low",
                    "medium" | "mid" => "medium",
                    _ => "high",
                };
                body["reasoning_effort"] = json!(value);
            }
            ApiProvider::Fireworks => {
                body["reasoning_effort"] = json!("high");
            }
            ApiProvider::Vllm => {
                body["chat_template_kwargs"] = json!({
                    "enable_thinking": true,
                });
                // vLLM supports low/medium/high natively — pass through the
                // user-chosen value instead of hard-coding "high".
                let value = match normalized.as_str() {
                    "low" | "minimal" => "low",
                    "medium" | "mid" => "medium",
                    _ => "high",
                };
                body["reasoning_effort"] = json!(value);
            }
            ApiProvider::Openai
            | ApiProvider::WanjieArk
            | ApiProvider::Qianfan
            | ApiProvider::OpenaiCodex
            | ApiProvider::Custom => {}
            ApiProvider::Moonshot => {
                // #3024: Kimi models accept thinking enable.
                body["thinking"] = json!({ "type": "enabled" });
            }
            ApiProvider::Ollama => {
                // #3024: Ollama think param.
                body["think"] = json!(true);
            }
            ApiProvider::Anthropic
            | ApiProvider::DeepseekAnthropic
            | ApiProvider::MinimaxAnthropic
            | ApiProvider::Openmodel => {
                // Thinking shaping happens in the Messages adapter, which
                // applies each provider's supported control fields.
            }
            ApiProvider::NvidiaNim => {
                body["chat_template_kwargs"] = json!({
                    "thinking": true,
                    "reasoning_effort": "high",
                });
            }
            ApiProvider::Minimax => {
                body["thinking"] = json!({ "type": "adaptive" });
            }
            ApiProvider::Zai => {
                body["thinking"] = json!({
                    "type": "enabled",
                    "clear_thinking": false,
                });
            }
            ApiProvider::Stepfun => {}
            ApiProvider::Sakana => {}
            ApiProvider::LongCat => {}
            ApiProvider::OpencodeGo | ApiProvider::OpencodeZen => {}
            ApiProvider::Meta => {}
            ApiProvider::Xai => {}
        },
        "xhigh" | "max" | "highest" | "ultracode" => match provider {
            ApiProvider::Deepseek
            | ApiProvider::DeepseekCN
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Sglang
            | ApiProvider::Volcengine
            | ApiProvider::Deepinfra
            | ApiProvider::Atlascloud => {
                body["reasoning_effort"] = json!("max");
                body["thinking"] = json!({ "type": "enabled" });
            }
            ApiProvider::Openrouter | ApiProvider::Novita | ApiProvider::Together => {
                body["reasoning_effort"] = json!("xhigh");
                body["thinking"] = json!({ "type": "enabled" });
            }
            ApiProvider::XiaomiMimo => {
                body["thinking"] = json!({ "type": "enabled" });
            }
            ApiProvider::Arcee | ApiProvider::Huggingface => {
                body["reasoning_effort"] = json!("high");
            }
            ApiProvider::Fireworks => {
                body["reasoning_effort"] = json!("max");
            }
            ApiProvider::Vllm => {
                body["chat_template_kwargs"] = json!({
                    "enable_thinking": true,
                });
                // vLLM only supports none/low/medium/high — downgrade
                // "max" to "high" instead of sending an invalid value.
                body["reasoning_effort"] = json!("high");
            }
            ApiProvider::Openai
            | ApiProvider::WanjieArk
            | ApiProvider::Qianfan
            | ApiProvider::OpenaiCodex
            | ApiProvider::Custom => {}
            ApiProvider::Moonshot => {
                // #3024: Kimi models accept thinking enable.
                body["thinking"] = json!({ "type": "enabled" });
            }
            ApiProvider::Ollama => {
                // #3024: Ollama think param.
                body["think"] = json!(true);
            }
            ApiProvider::Anthropic
            | ApiProvider::DeepseekAnthropic
            | ApiProvider::MinimaxAnthropic
            | ApiProvider::Openmodel => {
                // Thinking shaping happens in the Messages adapter, which
                // applies each provider's supported control fields.
            }
            ApiProvider::NvidiaNim => {
                body["chat_template_kwargs"] = json!({
                    "thinking": true,
                    "reasoning_effort": "max",
                });
            }
            ApiProvider::Minimax => {
                body["thinking"] = json!({ "type": "adaptive" });
            }
            ApiProvider::Zai => {
                body["thinking"] = json!({
                    "type": "enabled",
                    "clear_thinking": false,
                });
            }
            ApiProvider::Stepfun => {}
            ApiProvider::Sakana => {}
            ApiProvider::LongCat => {}
            ApiProvider::OpencodeGo | ApiProvider::OpencodeZen => {}
            ApiProvider::Meta => {}
            ApiProvider::Xai => {}
        },
        _ => {}
    }
}

pub(super) fn parse_usage(usage: Option<&Value>) -> Usage {
    let input_tokens = usage
        .and_then(|u| u.get("input_tokens").or_else(|| u.get("prompt_tokens")))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut output_tokens = usage
        .and_then(|u| {
            u.get("output_tokens")
                .or_else(|| u.get("completion_tokens"))
        })
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .and_then(|u| u.get("total_tokens"))
        .and_then(Value::as_u64);
    let reasoning_tokens_raw = usage
        .and_then(|u| u.get("completion_tokens_details"))
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64);
    if output_tokens == 0
        && let Some(reasoning_tokens) = reasoning_tokens_raw
    {
        output_tokens = reasoning_tokens;
    } else if output_tokens == 0
        && let Some(total_tokens) = total_tokens
    {
        output_tokens = total_tokens.saturating_sub(input_tokens);
    }
    let cached_tokens = usage
        .and_then(|u| u.get("prompt_tokens_details"))
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64);
    let prompt_cache_hit_tokens = usage
        .and_then(|u| u.get("prompt_cache_hit_tokens"))
        .and_then(Value::as_u64)
        .or(cached_tokens)
        .map(|v| v as u32);
    let prompt_cache_miss_tokens = usage
        .and_then(|u| u.get("prompt_cache_miss_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| prompt_cache_hit_tokens.map(|hit| input_tokens.saturating_sub(u64::from(hit))))
        .map(|v| v as u32);
    let reasoning_tokens = reasoning_tokens_raw.map(|v| v as u32);

    let server_tool_use = usage.and_then(|u| u.get("server_tool_use")).map(|server| {
        let code_execution_requests = server
            .get("code_execution_requests")
            .and_then(Value::as_u64)
            .map(|v| v as u32);
        let tool_search_requests = server
            .get("tool_search_requests")
            .and_then(Value::as_u64)
            .map(|v| v as u32);
        ServerToolUsage {
            code_execution_requests,
            tool_search_requests,
        }
    });

    Usage {
        input_tokens: input_tokens.min(u64::from(u32::MAX)) as u32,
        output_tokens: output_tokens.min(u64::from(u32::MAX)) as u32,
        prompt_cache_hit_tokens,
        prompt_cache_miss_tokens,
        prompt_cache_write_tokens: None,
        reasoning_tokens,
        reasoning_replay_tokens: None,
        server_tool_use,
    }
}

impl DeepSeekClient {
    /// Call the DeepSeek `/beta/completions` FIM endpoint.
    pub async fn fim_completion(
        &self,
        model: &str,
        prompt: &str,
        suffix: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        if api_provider_uses_anthropic_messages(self.api_provider) {
            bail!(
                "FIM completion is not supported for {} because it uses the Anthropic Messages protocol",
                self.api_provider.display_name()
            );
        }
        let url = api_url_with_suffix(&self.base_url, "beta/completions", None);
        let model = wire_model_for_provider_route(self.api_provider, &self.base_url, model);
        let body = json!({
            "model": model,
            "prompt": prompt,
            "suffix": suffix,
            "max_tokens": max_tokens,
        });
        let response = self.send_json_with_retry(&url, &body).await?;
        let status = response.status();
        if !status.is_success() {
            let raw_error_text = bounded_error_text(response, ERROR_BODY_MAX_BYTES).await;
            let error_text = sanitize_http_error_body(
                Some(self.api_provider.display_name()),
                status.as_u16(),
                &raw_error_text,
            );
            anyhow::bail!("FIM API error: HTTP {status}: {error_text}");
        }
        let response_text = response
            .text()
            .await
            .context("Failed to read FIM API response body")?;
        let value: serde_json::Value =
            serde_json::from_str(&response_text).context("Failed to parse FIM API response")?;
        let text = value
            .pointer("/choices/0/text")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("FIM response missing choices[0].text"))?;
        Ok(text.to_string())
    }
}

mod anthropic;
mod chat;
mod responses;

fn extract_sse_data_value(line: &str) -> Option<&str> {
    line.strip_prefix("data:")
        .map(|value| value.strip_prefix(' ').unwrap_or(value))
}

/// Take the next COMPLETE line (up to the first `\n`) off a raw byte buffer,
/// draining it, and return it trimmed. Returns `None` when no full line is
/// buffered yet. Decoding only complete lines (never an arbitrary network-read
/// boundary) means a multi-byte UTF-8 char — CJK, emoji, accented letter —
/// split across two reads is never corrupted to U+FFFD, since the `\n`
/// delimiter is ASCII and can never fall inside a multi-byte sequence.
fn take_sse_line(buffer: &mut Vec<u8>) -> Option<String> {
    let line_end = buffer.iter().position(|&b| b == b'\n')?;
    let line = String::from_utf8_lossy(&buffer[..line_end])
        .trim()
        .to_string();
    buffer.drain(..=line_end);
    Some(line)
}

pub(crate) use chat::{CacheWarmupKey, PromptInspection};

pub(crate) fn inspect_prompt_for_request(request: &MessageRequest) -> PromptInspection {
    chat::inspect_prompt_for_request(request)
}

pub(crate) fn build_cache_warmup_request(request: &MessageRequest) -> MessageRequest {
    chat::build_cache_warmup_request(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::chat::{
        build_chat_messages, build_chat_messages_for_request,
        build_chat_messages_for_request_and_provider, count_reasoning_replay_chars,
        parse_chat_message, parse_sse_chunk, sanitize_thinking_mode_messages, tool_to_chat,
        tool_to_chat_for_base_url,
    };
    use crate::client::responses::build_responses_body;
    use crate::config::{ProviderConfig, ProvidersConfig};
    use crate::models::{
        ContentBlock, ContentBlockStart, Delta, Message, MessageRequest, StreamEvent, Tool,
    };
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_tool(name: &str) -> Tool {
        Tool {
            tool_type: None,
            name: name.to_string(),
            description: format!("{name} test tool"),
            input_schema: json!({
                "type": "object",
                "properties": {},
            }),
            allowed_callers: None,
            defer_loading: Some(false),
            input_examples: None,
            strict: Some(true),
            cache_control: None,
        }
    }

    const CONFIG_SECRET_SENTINELS: [&str; 8] = [
        "deepseek-config-secret-001",
        "arcee-config-secret-002",
        "moonshot-config-secret-003",
        "openrouter-config-secret-004",
        "together-config-secret-005",
        "xiaomi-config-secret-006",
        "zai-active-config-secret-007",
        "sakana-config-secret-008",
    ];

    fn client_with_config_secret_sentinels() -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        DeepSeekClient::new(&Config {
            provider: Some("zai".to_string()),
            api_key: Some(CONFIG_SECRET_SENTINELS[0].to_string()),
            providers: Some(ProvidersConfig {
                arcee: ProviderConfig {
                    api_key: Some(CONFIG_SECRET_SENTINELS[1].to_string()),
                    ..ProviderConfig::default()
                },
                moonshot: ProviderConfig {
                    api_key: Some(CONFIG_SECRET_SENTINELS[2].to_string()),
                    ..ProviderConfig::default()
                },
                openrouter: ProviderConfig {
                    api_key: Some(CONFIG_SECRET_SENTINELS[3].to_string()),
                    ..ProviderConfig::default()
                },
                together: ProviderConfig {
                    api_key: Some(CONFIG_SECRET_SENTINELS[4].to_string()),
                    ..ProviderConfig::default()
                },
                xiaomi_mimo: ProviderConfig {
                    api_key: Some(CONFIG_SECRET_SENTINELS[5].to_string()),
                    ..ProviderConfig::default()
                },
                zai: ProviderConfig {
                    api_key: Some(CONFIG_SECRET_SENTINELS[6].to_string()),
                    ..ProviderConfig::default()
                },
                sakana: ProviderConfig {
                    api_key: Some(CONFIG_SECRET_SENTINELS[7].to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("client with secret sentinels")
    }

    fn request_with_tool_result(content: impl Into<String>) -> MessageRequest {
        MessageRequest {
            model: "glm-5.2".to_string(),
            messages: vec![
                Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "call-secret-test".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "config.toml"}),
                        caller: None,
                    }],
                },
                Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "call-secret-test".to_string(),
                        content: content.into(),
                        is_error: None,
                        content_blocks: None,
                    }],
                },
            ],
            max_tokens: 128,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: None,
            temperature: None,
            top_p: None,
        }
    }

    fn tool_result_content(request: &MessageRequest) -> &str {
        request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .find_map(|block| match block {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .expect("tool result content")
    }

    #[test]
    fn model_bound_request_redacts_configured_secrets_and_bare_active_key() {
        let client = client_with_config_secret_sentinels();
        let config_dump = format!(
            "api_key = \"{}\"\n[providers.arcee]\napi_key = \"{}\"\n\
             ordinary_setting = \"keep-me\"\nall bare values: {}",
            CONFIG_SECRET_SENTINELS[0],
            CONFIG_SECRET_SENTINELS[1],
            CONFIG_SECRET_SENTINELS.join(" ")
        );

        let prepared = client.prepare_model_bound_request(request_with_tool_result(config_dump));
        let content = tool_result_content(&prepared);

        for secret in CONFIG_SECRET_SENTINELS {
            assert!(!content.contains(secret), "secret survived redaction");
        }
        assert!(content.contains(codewhale_config::persistence::REDACTED));
        assert!(content.contains("ordinary_setting"));
        assert!(content.contains("keep-me"));
    }

    #[test]
    fn model_bound_request_redacts_inactive_file_store_and_environment_secrets() {
        const FILE_STORED_INACTIVE: &str = "inactive-arcee-file-secret-901";
        const BUILTIN_ENV_SECRET: &str = "inactive-arcee-env-secret-902";
        const CUSTOM_ENV_NAME: &str = "CW_TEST_CUSTOM_PROVIDER_API_KEY";
        const CUSTOM_ENV_SECRET: &str = "inactive-custom-env-secret-903";

        let _env_lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        let codewhale_home = tmp.path().join("codewhale-home");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("create isolated home");
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _secret_backend =
            crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &home);
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", &home);
        let builtin_env_name = ApiProvider::Arcee
            .env_vars()
            .first()
            .copied()
            .expect("Arcee API-key environment variable");
        let _builtin_env =
            crate::test_support::EnvVarGuard::set(builtin_env_name, BUILTIN_ENV_SECRET);
        let _custom_env = crate::test_support::EnvVarGuard::set(CUSTOM_ENV_NAME, CUSTOM_ENV_SECRET);

        codewhale_secrets::Secrets::file_backed()
            .set("arcee", FILE_STORED_INACTIVE)
            .expect("write isolated inactive provider credential");

        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = DeepSeekClient::new(&Config {
            provider: Some("zai".to_string()),
            providers: Some(ProvidersConfig {
                zai: ProviderConfig {
                    api_key: Some("active-zai-secret-900".to_string()),
                    ..ProviderConfig::default()
                },
                custom: HashMap::from([(
                    "example-custom".to_string(),
                    ProviderConfig {
                        kind: Some("openai-compatible".to_string()),
                        api_key_env: Some(CUSTOM_ENV_NAME.to_string()),
                        ..ProviderConfig::default()
                    },
                )]),
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("client with inactive file-store credential");
        let prepared = client.prepare_model_bound_request(request_with_tool_result(format!(
            "retrieved values: {FILE_STORED_INACTIVE} {BUILTIN_ENV_SECRET} {CUSTOM_ENV_SECRET}\nordinary output survives"
        )));
        let content = tool_result_content(&prepared);

        for secret in [FILE_STORED_INACTIVE, BUILTIN_ENV_SECRET, CUSTOM_ENV_SECRET] {
            assert!(
                !content.contains(secret),
                "inactive secret survived: {secret}"
            );
        }
        assert!(content.contains(codewhale_config::persistence::REDACTED));
        assert!(content.contains("ordinary output survives"));
    }

    #[test]
    fn model_bound_request_leaves_ordinary_tool_output_unchanged() {
        let client = client_with_config_secret_sentinels();
        let ordinary = "tests passed: 42\nREADME.md updated\n";
        let prepared =
            client.prepare_model_bound_request(request_with_tool_result(ordinary.to_string()));
        assert_eq!(tool_result_content(&prepared), ordinary);
    }

    #[test]
    fn short_chat_tool_payload_is_redacted_before_wire_serialization() {
        let client = client_with_config_secret_sentinels();
        let prepared = client.prepare_model_bound_request(request_with_tool_result(format!(
            "active token: {}",
            CONFIG_SECRET_SENTINELS[6]
        )));
        let wire = build_chat_messages_for_request(&prepared);
        let serialized = serde_json::to_string(&wire).expect("serialize chat wire messages");

        assert!(!serialized.contains(CONFIG_SECRET_SENTINELS[6]));
        assert!(serialized.contains(codewhale_config::persistence::REDACTED));
    }

    #[test]
    fn configured_secret_redaction_reaches_all_protocol_bodies() {
        let client = client_with_config_secret_sentinels();
        let prepared = client.prepare_model_bound_request(request_with_tool_result(format!(
            "safe output then {}",
            CONFIG_SECRET_SENTINELS[6]
        )));

        let chat = serde_json::to_string(&build_chat_messages_for_request(&prepared))
            .expect("serialize Chat Completions body");
        let anthropic = client.build_anthropic_body(&prepared, false).to_string();
        let responses = build_responses_body(&prepared).to_string();

        for (route, body) in [
            ("chat", chat.as_str()),
            ("anthropic", anthropic.as_str()),
            ("responses", responses.as_str()),
        ] {
            assert!(
                !body.contains(CONFIG_SECRET_SENTINELS[6]),
                "{route} body retained the configured credential"
            );
            assert!(
                body.contains(codewhale_config::persistence::REDACTED),
                "{route} body lost the redaction marker"
            );
        }
    }

    // This test deliberately serializes access to process-global spillover
    // state while awaiting the retrieval path.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn retrieved_turn_loop_spillover_is_sanitized_before_model_wire() {
        let _guard = crate::tools::truncate::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        let spillover_root = tmp.path().join(".codewhale").join("tool_outputs");
        let prior = crate::tools::truncate::set_test_spillover_root(Some(spillover_root.clone()));
        struct Restore(Option<std::path::PathBuf>);
        impl Drop for Restore {
            fn drop(&mut self) {
                crate::tools::truncate::set_test_spillover_root(self.0.take());
            }
        }
        let _restore = Restore(prior);

        let head = (0..40)
            .map(|_| format!("{}\n", "safe-head".repeat(100)))
            .collect::<String>();
        let tail = (0..80)
            .map(|_| format!("{}\n", "safe-tail".repeat(100)))
            .collect::<String>();
        let raw = format!("{head}\n{}\n{tail}", CONFIG_SECRET_SENTINELS[6]);
        assert!(
            raw.len() > crate::tools::truncate::SPILLOVER_THRESHOLD_BYTES,
            "fixture must enter turn-loop spillover"
        );

        let mut spilled = crate::tools::spec::ToolResult::success(raw.clone());
        let path = crate::tools::truncate::apply_spillover(&mut spilled, "call-local-secret")
            .expect("turn-loop spillover");
        assert_eq!(path.parent(), Some(spillover_root.as_path()));
        assert!(
            std::fs::read_to_string(&path)
                .expect("read local spillover")
                .contains(CONFIG_SECRET_SENTINELS[6]),
            "the full raw result remains available only in the local spillover store"
        );
        assert!(
            !spilled.content.contains(CONFIG_SECRET_SENTINELS[6]),
            "middle-only secret should not be present in retained head/tail"
        );

        let context = crate::tools::spec::ToolContext::new(tmp.path().to_path_buf());
        let retrieved = crate::tools::spec::ToolSpec::execute(
            &crate::tools::tool_result_retrieval::RetrieveToolResultTool,
            json!({
                "ref": "call-local-secret",
                "mode": "query",
                "query": CONFIG_SECRET_SENTINELS[6],
            }),
            &context,
        )
        .await
        .expect("retrieve secret-bearing local spillover slice");
        assert!(retrieved.content.contains(CONFIG_SECRET_SENTINELS[6]));

        let client = client_with_config_secret_sentinels();
        let prepared =
            client.prepare_model_bound_request(request_with_tool_result(retrieved.content));
        let wire = serde_json::to_string(&build_chat_messages_for_request(&prepared))
            .expect("serialize sanitized retrieval result");
        assert!(!wire.contains(CONFIG_SECRET_SENTINELS[6]));
        assert!(wire.contains(codewhale_config::persistence::REDACTED));
    }

    #[test]
    fn sha_spillover_persists_only_sanitized_tool_result() {
        let _guard = crate::tools::truncate::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        let prior = crate::tools::truncate::set_test_spillover_root(Some(
            tmp.path().join(".codewhale").join("tool_outputs"),
        ));
        struct Restore(Option<std::path::PathBuf>);
        impl Drop for Restore {
            fn drop(&mut self) {
                crate::tools::truncate::set_test_spillover_root(self.0.take());
            }
        }
        let _restore = Restore(prior);

        let client = client_with_config_secret_sentinels();
        let raw = format!(
            "{}\ncredential={}\n{}",
            "ordinary output ".repeat(80),
            CONFIG_SECRET_SENTINELS[6],
            "tail ".repeat(80)
        );
        assert!(raw.len() > 1024, "fixture must enter SHA persistence path");
        let raw_sha = crate::hashing::sha256_hex(raw.as_bytes());
        let prepared = client.prepare_model_bound_request(request_with_tool_result(raw));
        let sanitized = tool_result_content(&prepared).to_string();
        let sanitized_sha = crate::hashing::sha256_hex(sanitized.as_bytes());

        let wire = build_chat_messages_for_request(&prepared);
        let serialized = serde_json::to_string(&wire).expect("serialize chat wire messages");
        assert!(!serialized.contains(CONFIG_SECRET_SENTINELS[6]));

        let sanitized_path = crate::tools::truncate::sha_spillover_path(&sanitized_sha)
            .expect("sanitized spillover path");
        let persisted = std::fs::read_to_string(&sanitized_path)
            .expect("sanitized tool output should be persisted");
        assert_eq!(persisted, sanitized);
        assert!(!persisted.contains(CONFIG_SECRET_SENTINELS[6]));
        assert!(persisted.contains(codewhale_config::persistence::REDACTED));

        let raw_path =
            crate::tools::truncate::sha_spillover_path(&raw_sha).expect("raw spillover path");
        assert!(
            !raw_path.exists(),
            "unsanitized tool output must never be persisted by the wire adapter"
        );
    }

    fn deepseek_anthropic_client(server: &MockServer) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let providers = ProvidersConfig {
            deepseek_anthropic: ProviderConfig {
                api_key: Some("ds-test".to_string()),
                base_url: Some(server.uri()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        };
        DeepSeekClient::new(&Config {
            provider: Some("deepseek-anthropic".to_string()),
            providers: Some(providers),
            ..Config::default()
        })
        .expect("deepseek anthropic client")
    }

    fn minimax_anthropic_client_with_base_url(base_url: String) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let providers = ProvidersConfig {
            minimax_anthropic: ProviderConfig {
                api_key: Some("minimax-test".to_string()),
                base_url: Some(base_url),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        };
        DeepSeekClient::new(&Config {
            provider: Some("minimax-anthropic".to_string()),
            providers: Some(providers),
            ..Config::default()
        })
        .expect("minimax anthropic client")
    }

    fn zai_client_for_test() -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let providers = ProvidersConfig {
            zai: ProviderConfig {
                api_key: Some("zai-test".to_string()),
                base_url: Some("https://api.z.ai/api/coding/paas/v4".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        };
        DeepSeekClient::new(&Config {
            provider: Some("zai".to_string()),
            providers: Some(providers),
            ..Config::default()
        })
        .expect("zai client")
    }

    #[tokio::test]
    async fn provider_request_concurrency_limiter_is_shared_across_client_clones() {
        let client = zai_client_for_test();
        assert_eq!(
            client.provider_request_concurrency_limit(),
            Some(crate::config::DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY)
        );

        let clone = client.clone();
        let permit = client
            .acquire_provider_request_permit()
            .await
            .expect("zai default should install provider request limiter");

        assert_eq!(client.active_provider_requests(), 1);
        assert_eq!(clone.active_provider_requests(), 1);

        drop(permit);

        assert_eq!(client.active_provider_requests(), 0);
        assert_eq!(clone.active_provider_requests(), 0);
    }

    #[tokio::test]
    async fn provider_request_permit_lives_until_stream_is_consumed() {
        let client = zai_client_for_test();
        let permit = client
            .acquire_provider_request_permit()
            .await
            .expect("zai default should install provider request limiter");
        let stream: crate::llm_client::StreamEventBox =
            Box::pin(futures_util::stream::iter(vec![Ok(
                StreamEvent::MessageStop,
            )]));
        let mut wrapped =
            DeepSeekClient::hold_provider_request_permit_for_stream(stream, Some(permit));

        assert_eq!(client.active_provider_requests(), 1);
        assert!(wrapped.next().await.is_some());
        assert!(wrapped.next().await.is_none());
        assert_eq!(client.active_provider_requests(), 0);
    }

    #[test]
    fn parse_speech_audio_response_accepts_message_audio() {
        let encoded = general_purpose::STANDARD.encode(b"hi");
        let payload = json!({
            "choices": [{
                "message": {
                    "audio": {
                        "data": encoded,
                        "transcript": "hi"
                    }
                }
            }]
        });

        let (audio, transcript) = parse_speech_audio_response(&payload).unwrap();
        assert_eq!(audio, b"hi");
        assert_eq!(transcript.as_deref(), Some("hi"));
    }

    #[test]
    fn parse_speech_audio_response_accepts_data_uri() {
        let encoded = general_purpose::STANDARD.encode(b"wav");
        let payload = json!({
            "audio": {
                "data": format!("data:audio/wav;base64,{encoded}")
            }
        });

        let (audio, transcript) = parse_speech_audio_response(&payload).unwrap();
        assert_eq!(audio, b"wav");
        assert_eq!(transcript, None);
    }

    #[test]
    fn speech_synthesis_body_omits_user_message_without_instruction() {
        let body =
            build_speech_synthesis_body("mimo-v2.5-tts", "hello", None, json!({"format": "wav"}));
        let messages = body["messages"].as_array().expect("messages array");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["content"], "hello");
        assert!(
            messages
                .iter()
                .all(|message| message["content"].as_str() != Some(""))
        );
    }

    #[test]
    fn speech_synthesis_body_ignores_blank_instruction() {
        let body = build_speech_synthesis_body(
            "mimo-v2.5-tts",
            "hello",
            Some("  \t\n  "),
            json!({"format": "wav"}),
        );
        let messages = body["messages"].as_array().expect("messages array");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "assistant");
    }

    #[test]
    fn speech_synthesis_body_includes_non_empty_instruction_first() {
        let body = build_speech_synthesis_body(
            "mimo-v2.5-tts-voicedesign",
            "hello",
            Some("warm and calm"),
            json!({"format": "wav"}),
        );
        let messages = body["messages"].as_array().expect("messages array");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "warm and calm");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "hello");
    }

    #[test]
    fn tool_name_roundtrip_dot() {
        let original = "multi_tool_use.parallel";
        let encoded = to_api_tool_name(original);
        assert_eq!(encoded, "multi_tool_use-x00002E-parallel");
        let decoded = from_api_tool_name(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn tool_name_decode_mangled_dot_prefix() {
        let mangled = "multi_tool_use.x00002E-parallel";
        let decoded = from_api_tool_name(mangled);
        assert_eq!(decoded, "multi_tool_use..parallel");
    }

    #[test]
    fn tool_name_decode_bare_hex_no_trailing_dash() {
        let mangled = "foo_x00002Ebar";
        let decoded = from_api_tool_name(mangled);
        assert_eq!(decoded, "foo_.bar");
    }

    #[test]
    fn tool_name_bare_hex_preserves_alnum() {
        let input = "foox000041bar";
        let decoded = from_api_tool_name(input);
        assert_eq!(decoded, input);
    }

    #[test]
    fn tool_name_bare_hex_preserves_underscore() {
        let input = "foox00005Fbar";
        let decoded = from_api_tool_name(input);
        assert_eq!(decoded, input);
    }

    #[test]
    fn tool_name_roundtrip_colon() {
        let original = "mcp__server:tool_name";
        let encoded = to_api_tool_name(original);
        let decoded = from_api_tool_name(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn api_url_handles_default_v1_and_beta_base_urls() {
        assert_eq!(
            api_url("https://api.deepseek.com", "chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            api_url("https://api.deepseek.com/v1", "chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // Non-beta paths from a /beta base URL route to /v1.
        // Only paths with an explicit beta/ prefix use the beta surface.
        assert_eq!(
            api_url("https://api.deepseek.com/beta", "chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            api_url(
                "https://openai-compatible.example/api/coding/paas/v4",
                "chat/completions"
            ),
            "https://openai-compatible.example/api/coding/paas/v4/chat/completions"
        );
    }

    #[test]
    fn api_url_routes_beta_paths_from_any_deepseek_base() {
        assert_eq!(
            api_url("https://api.deepseek.com", "beta/completions"),
            "https://api.deepseek.com/beta/completions"
        );
        assert_eq!(
            api_url("https://api.deepseek.com/v1", "beta/completions"),
            "https://api.deepseek.com/beta/completions"
        );
        assert_eq!(
            api_url("https://api.deepseek.com/beta", "beta/completions"),
            "https://api.deepseek.com/beta/completions"
        );
    }

    #[test]
    fn api_url_routes_models_and_non_beta_paths_to_v1() {
        // The /models endpoint only exists at /v1/models, never at
        // /beta/models. Non-beta paths from a /beta base URL must
        // still route to /v1.
        assert_eq!(
            api_url("https://api.deepseek.com", "models"),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            api_url("https://api.deepseek.com/v1", "models"),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            api_url("https://api.deepseek.com/beta", "models"),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            api_url("https://api.minimax.io/anthropic", "models"),
            "https://api.minimax.io/anthropic/v1/models"
        );
        assert_eq!(
            api_url("https://api.minimaxi.com/anthropic", "models"),
            "https://api.minimaxi.com/anthropic/v1/models"
        );
        // explicit v<N> versions other than /v1 should be preserved
        assert_eq!(
            api_url(
                "https://openai-compatible.example/api/coding/paas/v4",
                "models"
            ),
            "https://openai-compatible.example/api/coding/paas/v4/models"
        );
    }

    #[test]
    fn default_headers_include_custom_headers_when_configured() {
        let mut extra = HashMap::new();
        extra.insert("X-Model-Provider-Id".to_string(), "tongyi".to_string());
        let headers = DeepSeekClient::default_headers("sk-test", &extra).expect("headers");
        assert_eq!(
            headers
                .get("x-model-provider-id")
                .and_then(|value| value.to_str().ok()),
            Some("tongyi")
        );
    }

    #[test]
    fn default_headers_ignore_blank_custom_headers() {
        let mut extra = HashMap::new();
        extra.insert("X-Blank".to_string(), "   ".to_string());
        let headers = DeepSeekClient::default_headers("sk-test", &extra).expect("headers");
        assert!(headers.get("x-blank").is_none());
    }

    #[test]
    fn disabled_auth_strips_every_auth_header_dialect_at_client_sink() {
        let mut extra = HashMap::new();
        extra.insert(
            "aUtHoRiZaTiOn".to_string(),
            "Bearer configured-secret".to_string(),
        );
        extra.insert("X-API-Key".to_string(), "configured-x-key".to_string());
        extra.insert("Api-Key".to_string(), "configured-key".to_string());
        extra.insert(
            "Proxy-Authorization".to_string(),
            "Basic configured-proxy-secret".to_string(),
        );
        extra.insert(
            "X-Auth-Token".to_string(),
            "configured-auth-token".to_string(),
        );
        extra.insert(
            "X-Access-Token".to_string(),
            "configured-access-token".to_string(),
        );
        extra.insert(
            "X-Goog-Api-Key".to_string(),
            "configured-google-key".to_string(),
        );
        extra.insert("Cookie".to_string(), "session=secret".to_string());
        extra.insert("X-Route-Metadata".to_string(), "safe".to_string());

        let headers = DeepSeekClient::default_headers_for_provider_with_auth_disabled(
            "generated-secret",
            &extra,
            ApiProvider::Deepseek,
            crate::config::DEFAULT_DEEPSEEK_BASE_URL,
        )
        .expect("headers");

        for name in [
            "authorization",
            "x-api-key",
            "api-key",
            "proxy-authorization",
            "x-auth-token",
            "x-access-token",
            "x-goog-api-key",
            "cookie",
        ] {
            assert!(headers.get(name).is_none(), "disabled auth leaked {name}");
        }
        assert_eq!(
            headers
                .get("x-route-metadata")
                .and_then(|value| value.to_str().ok()),
            Some("safe")
        );
    }

    #[test]
    fn build_http_client_accepts_default_tls_verification() {
        let client = DeepSeekClient::build_http_client(
            "sk-test",
            &HashMap::new(),
            ApiProvider::Deepseek,
            crate::config::DEFAULT_DEEPSEEK_BASE_URL,
        );

        assert!(client.is_ok());
    }

    #[test]
    fn client_new_rejects_provider_scoped_tls_skip_verify() {
        let mut providers = crate::config::ProvidersConfig::default();
        providers.openai.api_key = Some("sk-test".to_string());
        providers.openai.base_url = Some(crate::config::DEFAULT_OPENAI_BASE_URL.to_string());
        providers.openai.insecure_skip_tls_verify = Some(true);
        let config = Config {
            provider: Some("openai".to_string()),
            providers: Some(providers),
            ..Config::default()
        };
        assert!(config.insecure_skip_tls_verify());

        let err = match DeepSeekClient::new(&config) {
            Ok(_) => panic!("tls skip verify should be rejected"),
            Err(err) => err,
        };
        let message = err.to_string();
        assert!(message.contains("cannot be disabled"));
        assert!(message.contains("SSL_CERT_FILE"));
    }

    #[test]
    fn client_stream_idle_timeout_uses_tui_config() {
        let client = DeepSeekClient::new(&Config {
            api_key: Some("sk-test".to_string()),
            tui: Some(crate::config::TuiConfig {
                stream_chunk_timeout_secs: Some(777),
                ..crate::config::TuiConfig::default()
            }),
            ..Config::default()
        })
        .expect("client");

        assert_eq!(client.stream_idle_timeout, Duration::from_secs(777));
    }

    #[test]
    fn xiaomi_mimo_token_plan_endpoint_uses_api_key_header() {
        let headers = DeepSeekClient::default_headers_for_provider(
            "tp-test",
            &HashMap::new(),
            ApiProvider::XiaomiMimo,
            crate::config::DEFAULT_XIAOMI_MIMO_BASE_URL,
        )
        .expect("headers");

        assert_eq!(
            headers.get("api-key").and_then(|value| value.to_str().ok()),
            Some("tp-test")
        );
        assert!(
            headers.get(AUTHORIZATION).is_none(),
            "Token Plan requires api-key instead of Authorization Bearer"
        );
    }

    #[test]
    fn xiaomi_mimo_tp_key_uses_api_key_header_with_custom_base_url() {
        let mut extra = HashMap::new();
        extra.insert("api-key".to_string(), "wrong".to_string());
        extra.insert("Authorization".to_string(), "Bearer wrong".to_string());
        let headers = DeepSeekClient::default_headers_for_provider(
            "tp-custom",
            &extra,
            ApiProvider::XiaomiMimo,
            "https://proxy.example.test/mimo/v1",
        )
        .expect("headers");

        assert_eq!(
            headers.get("api-key").and_then(|value| value.to_str().ok()),
            Some("tp-custom")
        );
        assert!(
            headers.get(AUTHORIZATION).is_none(),
            "tp-* Token Plan keys should use api-key auth even through custom gateways"
        );
    }

    #[test]
    fn openrouter_uses_bearer_header_after_mimo_token_plan_context() {
        let mut extra = HashMap::new();
        extra.insert("api-key".to_string(), "wrong".to_string());
        let headers = DeepSeekClient::default_headers_for_provider(
            "sk-or-test",
            &extra,
            ApiProvider::Openrouter,
            crate::config::DEFAULT_OPENROUTER_BASE_URL,
        )
        .expect("headers");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer sk-or-test")
        );
        assert!(
            headers.get("api-key").is_none(),
            "OpenRouter must not inherit Xiaomi MiMo's api-key header dialect"
        );
    }

    #[test]
    fn siliconflow_cn_uses_bearer_header_and_pins_content_type() {
        let mut extra = HashMap::new();
        extra.insert("Authorization".to_string(), "Bearer wrong".to_string());
        extra.insert("Content-Type".to_string(), "text/plain".to_string());
        let headers = DeepSeekClient::default_headers_for_provider(
            "sf-cn-test",
            &extra,
            ApiProvider::SiliconflowCn,
            crate::config::DEFAULT_SILICONFLOW_CN_BASE_URL,
        )
        .expect("headers");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer sf-cn-test")
        );
        assert_eq!(
            headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert!(headers.get("api-key").is_none());
    }

    #[test]
    fn tokenhub_openai_compatible_route_uses_bearer_header() {
        let mut extra = HashMap::new();
        extra.insert("api-key".to_string(), "wrong".to_string());
        extra.insert("x-api-key".to_string(), "wrong".to_string());
        let headers = DeepSeekClient::default_headers_for_provider(
            "tokenhub-test",
            &extra,
            ApiProvider::Openai,
            "https://tokenhub.tencentmaas.com/v1",
        )
        .expect("headers");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer tokenhub-test")
        );
        assert!(headers.get("api-key").is_none());
        assert!(headers.get("x-api-key").is_none());
    }

    #[test]
    fn deepseek_anthropic_uses_anthropic_header_dialect() {
        let mut extra = HashMap::new();
        extra.insert("Authorization".to_string(), "Bearer wrong".to_string());
        extra.insert("api-key".to_string(), "wrong".to_string());
        let headers = DeepSeekClient::default_headers_for_provider(
            "ds-test",
            &extra,
            ApiProvider::DeepseekAnthropic,
            crate::config::DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL,
        )
        .expect("headers");

        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("ds-test")
        );
        assert_eq!(
            headers
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some("2023-06-01")
        );
        assert!(
            headers.get(AUTHORIZATION).is_none(),
            "Anthropic-compatible DeepSeek route must not use Bearer auth"
        );
        assert!(
            headers.get("api-key").is_none(),
            "Anthropic-compatible DeepSeek route must not inherit MiMo auth headers"
        );
    }

    #[test]
    fn minimax_anthropic_uses_anthropic_header_dialect() {
        let headers = DeepSeekClient::default_headers_for_provider(
            "minimax-test",
            &HashMap::new(),
            ApiProvider::MinimaxAnthropic,
            crate::config::DEFAULT_MINIMAX_ANTHROPIC_BASE_URL,
        )
        .expect("headers");

        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("minimax-test")
        );
        assert_eq!(
            headers
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some("2023-06-01")
        );
        assert!(headers.get(AUTHORIZATION).is_none());
    }

    #[test]
    fn openmodel_uses_bearer_auth_with_anthropic_version() {
        let mut extra = HashMap::new();
        extra.insert("Authorization".to_string(), "Bearer wrong".to_string());
        extra.insert("api-key".to_string(), "wrong".to_string());
        extra.insert("x-api-key".to_string(), "wrong".to_string());
        let headers = DeepSeekClient::default_headers_for_provider(
            "om-test",
            &extra,
            ApiProvider::Openmodel,
            crate::config::DEFAULT_OPENMODEL_BASE_URL,
        )
        .expect("headers");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer om-test")
        );
        assert_eq!(
            headers
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some("2023-06-01")
        );
        assert!(
            headers.get("x-api-key").is_none(),
            "OpenModel uses Bearer auth so /v1/models and /v1/messages share one client"
        );
        assert!(
            headers.get("api-key").is_none(),
            "OpenModel Messages route must not inherit MiMo auth headers"
        );
    }

    #[tokio::test]
    async fn deepseek_anthropic_translate_uses_messages_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Hola"}],
                "model": "deepseek-chat",
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 3, "output_tokens": 1}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = deepseek_anthropic_client(&server);
        let translated = client
            .translate("Hello", "deepseek-chat", "Spanish")
            .await
            .expect("translation succeeds");

        assert_eq!(translated, "Hola");
        let requests = server.received_requests().await.expect("recorded requests");
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).expect("json body");
        assert_eq!(
            body.get("model").and_then(Value::as_str),
            Some("deepseek-chat"),
            "custom Messages endpoints own their model ids: {body}"
        );
        assert_eq!(
            body.pointer("/messages/0/role").and_then(Value::as_str),
            Some("user")
        );
        assert_eq!(
            body.pointer("/messages/0/content/0/text")
                .and_then(Value::as_str),
            Some("Hello")
        );
        assert!(
            body.get("thinking").is_none(),
            "translation disables thinking: {body}"
        );
        assert!(
            body.get("system")
                .and_then(Value::as_str)
                .is_some_and(|system| system.contains("Spanish")),
            "target language should be in system prompt: {body}"
        );
    }

    #[tokio::test]
    async fn deepseek_anthropic_health_check_skips_models_probe() {
        let server = MockServer::start().await;
        let client = deepseek_anthropic_client(&server);

        assert!(client.health_check().await.expect("health check"));
        assert!(!provider_api_key_verification_is_observed(
            ApiProvider::DeepseekAnthropic
        ));
        let requests = server.received_requests().await.expect("recorded requests");
        assert!(
            requests.is_empty(),
            "DeepSeek Anthropic-compatible route must not probe /models"
        );
    }

    #[tokio::test]
    async fn minimax_anthropic_health_check_uses_models_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/anthropic/v1/models"))
            .and(header("x-api-key", "minimax-test"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
            .expect(1)
            .mount(&server)
            .await;
        let client = minimax_anthropic_client_with_base_url(format!("{}/anthropic", server.uri()));

        assert!(client.health_check().await.expect("health check"));
    }

    #[tokio::test]
    async fn minimax_anthropic_request_uses_messages_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/anthropic/v1/messages"))
            .and(header("x-api-key", "minimax-test"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ok"}],
                "model": "MiniMax-M3",
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 3, "output_tokens": 1}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = minimax_anthropic_client_with_base_url(format!("{}/anthropic", server.uri()));
        let response = client
            .create_message(MessageRequest {
                model: "MiniMax-M3".to_string(),
                messages: vec![Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "hello".to_string(),
                        cache_control: None,
                    }],
                }],
                max_tokens: 32,
                system: None,
                tools: None,
                tool_choice: None,
                metadata: None,
                thinking: None,
                reasoning_effort: Some("off".to_string()),
                stream: Some(false),
                temperature: None,
                top_p: None,
            })
            .await
            .expect("message succeeds");

        assert_eq!(response.content.len(), 1);
        let requests = server.received_requests().await.expect("recorded requests");
        let body: Value = serde_json::from_slice(&requests[0].body).expect("request JSON");
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("disabled")
        );
        assert!(body.get("output_config").is_none(), "{body}");
    }

    #[tokio::test]
    async fn deepseek_anthropic_fim_fails_without_http_request() {
        let server = MockServer::start().await;
        let client = deepseek_anthropic_client(&server);

        let err = client
            .fim_completion("deepseek-chat", "fn main() {", "}", 16)
            .await
            .expect_err("FIM is unsupported");
        let message = err.to_string();
        assert!(
            message.contains("FIM completion is not supported"),
            "{message}"
        );
        assert!(message.contains("Anthropic Messages protocol"), "{message}");
        let requests = server.received_requests().await.expect("recorded requests");
        assert!(
            requests.is_empty(),
            "unsupported FIM should fail locally before any HTTP call"
        );
    }

    #[test]
    fn custom_api_key_header_is_allowed_without_primary_provider_key() {
        let mut extra = HashMap::new();
        extra.insert("api-key".to_string(), "gateway-key".to_string());
        let headers = DeepSeekClient::default_headers_for_provider(
            "",
            &extra,
            ApiProvider::Openai,
            "https://gateway.example.test/v1",
        )
        .expect("headers");

        assert_eq!(
            headers.get("api-key").and_then(|value| value.to_str().ok()),
            Some("gateway-key")
        );
        assert!(headers.get(AUTHORIZATION).is_none());
    }

    #[test]
    fn xiaomi_mimo_pay_as_you_go_endpoint_keeps_bearer_header() {
        let headers = DeepSeekClient::default_headers_for_provider(
            "sk-test",
            &HashMap::new(),
            ApiProvider::XiaomiMimo,
            crate::config::XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL,
        )
        .expect("headers");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer sk-test")
        );
        assert!(headers.get("api-key").is_none());
    }

    #[test]
    fn chat_messages_keep_current_turn_reasoning_content() {
        let message = Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Thinking {
                    signature: None,
                    thinking: "plan".to_string(),
                },
                ContentBlock::Text {
                    text: "done".to_string(),
                    cache_control: None,
                },
            ],
        };
        let out = build_chat_messages(None, &[message], "deepseek-v4-pro");
        let assistant = out
            .iter()
            .find(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
            .expect("assistant message");
        assert_eq!(
            assistant.get("content").and_then(Value::as_str),
            Some("done")
        );
        assert_eq!(
            assistant.get("reasoning_content").and_then(Value::as_str),
            Some("plan"),
            "thinking-mode models keep reasoning_content while still in the current turn"
        );
    }

    #[test]
    fn generic_openai_provider_drops_reasoning_content_for_non_deepseek_models() {
        // #1542 intent (narrowed by #1739/#1694): a *genuine non-DeepSeek*
        // model on the generic openai provider must not carry DeepSeek-only
        // `reasoning_content`. A DeepSeek reasoning model on the openai
        // provider (DeepSeek-compatible endpoint) is now covered separately
        // and DOES replay reasoning_content — see
        // `deepseek_model_on_openai_provider_still_replays_reasoning_content`.
        let request = MessageRequest {
            model: "qwen3-coder".to_string(),
            messages: vec![Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        thinking: "plan".to_string(),
                    },
                    ContentBlock::Text {
                        text: "done".to_string(),
                        cache_control: None,
                    },
                ],
            }],
            max_tokens: 16,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("max".to_string()),
            stream: None,
            temperature: None,
            top_p: None,
        };

        let openai = build_chat_messages_for_request_and_provider(&request, ApiProvider::Openai);
        let generic_assistant = openai
            .iter()
            .find(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
            .expect("assistant message");
        assert_eq!(
            generic_assistant.get("content").and_then(Value::as_str),
            Some("done")
        );
        assert!(
            generic_assistant.get("reasoning_content").is_none(),
            "generic OpenAI-compatible providers reject DeepSeek-only reasoning_content (#1542)"
        );
    }

    #[test]
    fn chat_messages_replay_tool_round_reasoning_before_new_user_turn() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Need the date".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        thinking: "Need to call a tool".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "get_date".to_string(),
                        input: json!({}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "2026-04-23".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let out = build_chat_messages(None, &messages, "deepseek-v4-pro");
        let tool_assistant = out
            .iter()
            .find(|value| {
                value.get("role").and_then(Value::as_str) == Some("assistant")
                    && value.get("tool_calls").is_some()
            })
            .expect("tool-call assistant message");
        assert_eq!(
            tool_assistant
                .get("reasoning_content")
                .and_then(Value::as_str),
            Some("Need to call a tool"),
            "thinking-mode tool sub-turns must replay reasoning_content until the tool chain finishes"
        );
    }

    #[test]
    fn chat_messages_replay_prior_tool_round_reasoning_after_new_user_turn() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Need the date".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        thinking: "Need to call a tool".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "get_date".to_string(),
                        input: json!({}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "2026-04-23".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "It is 2026-04-23.".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Thanks. Next question.".to_string(),
                    cache_control: None,
                }],
            },
        ];
        let out = build_chat_messages(None, &messages, "deepseek-v4-pro");
        let tool_assistant = out
            .iter()
            .find(|value| {
                value.get("role").and_then(Value::as_str) == Some("assistant")
                    && value.get("tool_calls").is_some()
            })
            .expect("tool-call assistant message");
        assert_eq!(
            tool_assistant
                .get("reasoning_content")
                .and_then(Value::as_str),
            Some("Need to call a tool"),
            "tool-call reasoning_content must be replayed across later user turns"
        );
    }

    #[test]
    fn chat_messages_keep_prior_non_tool_reasoning_after_new_user_turn() {
        // The serialized JSON for a stored assistant message MUST be a pure
        // function of that message — never of what comes after it. DeepSeek's
        // prompt cache hashes the leading bytes of every request; flipping
        // `reasoning_content` on/off across turns rewrites historical bytes
        // and busts the prefix cache from that message onwards. (#583)
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Explain it".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        thinking: "Internal explanation plan".to_string(),
                    },
                    ContentBlock::Text {
                        text: "Final answer".to_string(),
                        cache_control: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Next question".to_string(),
                    cache_control: None,
                }],
            },
        ];

        let out = build_chat_messages(None, &messages, "deepseek-v4-pro");
        let assistant = out
            .iter()
            .find(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
            .expect("assistant message");

        assert_eq!(
            assistant.get("content").and_then(Value::as_str),
            Some("Final answer")
        );
        assert_eq!(
            assistant.get("reasoning_content").and_then(Value::as_str),
            Some("Internal explanation plan"),
            "reasoning_content must be preserved across follow-up user turns to keep DeepSeek's prefix cache warm"
        );
    }

    #[test]
    fn chat_messages_assistant_json_is_byte_stable_across_follow_up_user_turn() {
        // Direct prefix-cache regression: the JSON for the assistant message
        // built on turn N must equal the JSON for the same assistant message
        // built on turn N+1, after a new user message has been appended.
        let assistant = Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Thinking {
                    signature: None,
                    thinking: "I should explain step by step.".to_string(),
                },
                ContentBlock::Text {
                    text: "Here is the explanation.".to_string(),
                    cache_control: None,
                },
            ],
        };
        let user_initial = Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "Explain it".to_string(),
                cache_control: None,
            }],
        };
        let user_follow_up = Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "Next question".to_string(),
                cache_control: None,
            }],
        };

        let turn_n = build_chat_messages(
            None,
            &[user_initial.clone(), assistant.clone()],
            "deepseek-v4-pro",
        );
        let turn_n_plus_1 = build_chat_messages(
            None,
            &[user_initial, assistant, user_follow_up],
            "deepseek-v4-pro",
        );

        let assistant_n = turn_n
            .iter()
            .find(|v| v.get("role").and_then(Value::as_str) == Some("assistant"))
            .expect("assistant present in turn N");
        let assistant_n1 = turn_n_plus_1
            .iter()
            .find(|v| v.get("role").and_then(Value::as_str) == Some("assistant"))
            .expect("assistant present in turn N+1");

        assert_eq!(
            assistant_n, assistant_n1,
            "assistant message JSON must be byte-identical across turns or DeepSeek's prefix cache breaks"
        );
    }

    #[test]
    fn chat_messages_allow_tool_round_without_reasoning_when_thinking_disabled() {
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![
                Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "call-no-thinking".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "Cargo.toml"}),
                        caller: None,
                    }],
                },
                Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "call-no-thinking".to_string(),
                        content: "workspace manifest".to_string(),
                        is_error: None,
                        content_blocks: None,
                    }],
                },
            ],
            max_tokens: 1024,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("off".to_string()),
            stream: None,
            temperature: None,
            top_p: None,
        };

        let out = build_chat_messages_for_request(&request);
        assert!(
            out.iter().any(
                |value| value.get("role").and_then(Value::as_str) == Some("assistant")
                    && value.get("tool_calls").is_some()
            ),
            "tool calls remain valid when thinking mode is disabled"
        );
        assert!(
            out.iter()
                .any(|value| value.get("role").and_then(Value::as_str) == Some("tool")),
            "matching tool result should remain"
        );
    }

    #[test]
    fn prompt_builder_keeps_system_first_and_current_user_input_last() {
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![
                Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Previous answer".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: "user".to_string(),
                    content: vec![
                        ContentBlock::Text {
                            text: "<turn_meta>\nCurrent local date: 2026-05-08\n</turn_meta>"
                                .to_string(),
                            cache_control: None,
                        },
                        ContentBlock::Text {
                            text: "Current user question".to_string(),
                            cache_control: None,
                        },
                    ],
                },
            ],
            max_tokens: 1024,
            system: Some(SystemPrompt::Text(
                "Stable mode, project rules, and tool policy".to_string(),
            )),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("max".to_string()),
            stream: None,
            temperature: None,
            top_p: None,
        };

        let out = build_chat_messages_for_request(&request);

        assert_eq!(out[0].get("role").and_then(Value::as_str), Some("system"));
        assert_eq!(
            out[0].get("content").and_then(Value::as_str),
            Some("Stable mode, project rules, and tool policy")
        );
        let last = out.last().expect("latest user message");
        assert_eq!(last.get("role").and_then(Value::as_str), Some("user"));
        assert!(
            last.get("content")
                .and_then(Value::as_str)
                .is_some_and(|content| content.ends_with("Current user question")),
            "current-turn user input must be at the tail of the wire prompt: {last:?}"
        );
    }

    #[test]
    fn prompt_inspect_reports_stable_layers_and_dynamic_user_task() {
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![
                Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Prior answer".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Current task".to_string(),
                        cache_control: None,
                    }],
                },
            ],
            max_tokens: 1024,
            system: Some(SystemPrompt::Text(
                "Base policy\n\n<project_instructions source=\"AGENTS.md\">\nRules\n</project_instructions>\n\n## Project Context Pack\n\n<project_context_pack>\n{}\n</project_context_pack>\n\n## Environment\n\n- lang: en"
                    .to_string(),
            )),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("max".to_string()),
            stream: None,
            temperature: None,
            top_p: None,
        };

        let inspection = inspect_prompt_for_request(&request);

        assert_eq!(inspection.base_static_prefix_hash.len(), 64);
        assert_eq!(inspection.full_request_prefix_hash.len(), 64);
        assert!(inspection.layers.iter().any(|layer| {
            layer.name == "Global system prefix"
                && layer.stability.label() == "static"
                && layer.char_len == "Base policy".chars().count()
                && layer.sha256.len() == 64
        }));
        assert!(inspection.layers.iter().any(|layer| {
            layer.name == "Project context" && layer.stability.label() == "static"
        }));
        assert!(inspection.layers.iter().any(|layer| {
            layer.name == "Project context pack" && layer.stability.label() == "static"
        }));
        assert!(inspection.layers.iter().any(|layer| {
            layer.name == "Message #1 assistant" && layer.stability.label() == "history"
        }));
        assert!(
            inspection.layers.last().is_some_and(
                |layer| layer.name == "User task" && layer.stability.label() == "dynamic"
            )
        );
    }

    #[test]
    fn prompt_inspect_keeps_static_base_hash_across_different_user_tasks() {
        fn request_with_user_task(task: &str) -> MessageRequest {
            MessageRequest {
                model: "deepseek-v4-pro".to_string(),
                messages: vec![
                    Message {
                        role: "assistant".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "Prior answer".to_string(),
                            cache_control: None,
                        }],
                    },
                    Message {
                        role: "user".to_string(),
                        content: vec![ContentBlock::Text {
                            text: task.to_string(),
                            cache_control: None,
                        }],
                    },
                ],
                max_tokens: 1024,
                system: Some(SystemPrompt::Text(
                    "Base policy\n\n## Environment\n\n- shell: powershell\n\n## Skills\n\n- rust\n\n## Context Management\n\nKeep concise\n\n## Compact\n\nTemplate"
                        .to_string(),
                )),
                tools: None,
                tool_choice: None,
                metadata: None,
                thinking: None,
                reasoning_effort: Some("max".to_string()),
                stream: None,
                temperature: None,
                top_p: None,
            }
        }

        let first = inspect_prompt_for_request(&request_with_user_task("First task"));
        let second = inspect_prompt_for_request(&request_with_user_task("Second task"));
        let mut changed_history_request = request_with_user_task("Second task");
        changed_history_request.messages[0] = Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Different prior answer".to_string(),
                cache_control: None,
            }],
        };
        let changed_history = inspect_prompt_for_request(&changed_history_request);

        assert_eq!(
            first.base_static_prefix_hash,
            second.base_static_prefix_hash
        );
        assert_eq!(
            first.full_request_prefix_hash, second.full_request_prefix_hash,
            "full request prefix excludes the final dynamic user task"
        );
        assert_ne!(
            second.full_request_prefix_hash, changed_history.full_request_prefix_hash,
            "full request prefix can change when session history changes"
        );
        assert!(
            second.layers.last().is_some_and(
                |layer| layer.name == "User task" && layer.stability.label() == "dynamic"
            ),
            "current user task must remain the final layer"
        );
        assert!(second.layers.iter().any(|layer| {
            layer.name == "Message #1 assistant" && layer.stability.label() == "history"
        }));
        assert!(!second.layers.iter().any(
            |layer| layer.name.starts_with("Message #") && layer.stability.label() == "static"
        ));
    }

    #[test]
    fn prompt_inspect_tracks_tool_catalog_in_static_prefix_hash() {
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Current task".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 1024,
            system: Some(SystemPrompt::Text("Base policy".to_string())),
            tools: Some(vec![test_tool("read_file")]),
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("max".to_string()),
            stream: None,
            temperature: None,
            top_p: None,
        };

        let first = inspect_prompt_for_request(&request);
        let mut changed_tools = request.clone();
        changed_tools.tools = Some(vec![test_tool("read_file"), test_tool("grep_files")]);
        let second = inspect_prompt_for_request(&changed_tools);

        assert!(
            first.layers.iter().any(|layer| {
                layer.name == "Tool catalog" && layer.stability.label() == "static"
            })
        );
        assert_ne!(
            first.base_static_prefix_hash, second.base_static_prefix_hash,
            "tool schema changes must be visible to cache-inspect base prefix diagnostics"
        );
        assert_ne!(
            first.full_request_prefix_hash, second.full_request_prefix_hash,
            "tool schema changes must be visible to full reusable-prefix diagnostics"
        );
    }

    #[test]
    fn cache_warmup_request_reuses_stable_prefix_and_fixed_user_tail() {
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![
                Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Stable prior answer".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Dynamic latest user task".to_string(),
                        cache_control: None,
                    }],
                },
            ],
            max_tokens: 1024,
            system: Some(SystemPrompt::Text(
                "Base policy\n\n<project_instructions source=\"AGENTS.md\">\nStable project rules\n</project_instructions>\n\n## Previous Session Relay\n\nDynamic relay"
                    .to_string(),
            )),
            tools: Some(vec![test_tool("read_file")]),
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("max".to_string()),
            stream: Some(true),
            temperature: Some(0.7),
            top_p: None,
        };

        let warmup = build_cache_warmup_request(&request);

        assert_eq!(warmup.max_tokens, 8);
        assert_eq!(warmup.temperature, Some(0.0));
        assert_eq!(warmup.reasoning_effort.as_deref(), Some("max"));
        assert_eq!(warmup.tools.as_ref().map(Vec::len), Some(1));
        assert_eq!(warmup.tool_choice, Some(json!("none")));
        assert_eq!(warmup.messages.len(), 2);
        assert_eq!(warmup.messages[0].role, "assistant");
        assert_eq!(warmup.messages[1].role, "user");
        assert_eq!(
            warmup.messages[1].content,
            vec![ContentBlock::Text {
                text: "请只回复 OK".to_string(),
                cache_control: None,
            }]
        );

        let wire = build_chat_messages_for_request(&warmup);
        let system = wire
            .first()
            .and_then(|value| value.get("content"))
            .and_then(Value::as_str)
            .expect("warmup system prompt");
        assert!(system.contains("Stable project rules"));
        assert!(!system.contains("Dynamic relay"));
        assert!(
            !wire
                .iter()
                .any(|value| value.to_string().contains("Dynamic latest user task")),
            "warmup must not include the dynamic latest user task"
        );
    }

    #[test]
    fn reasoning_effort_uses_deepseek_top_level_thinking_parameter() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("max"), ApiProvider::Deepseek);

        assert_eq!(
            body.get("reasoning_effort").and_then(Value::as_str),
            Some("max")
        );
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("enabled")
        );
        assert!(body.get("extra_body").is_none());
    }

    #[test]
    fn reasoning_effort_off_disables_top_level_thinking() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Deepseek);

        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("disabled")
        );
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("extra_body").is_none());
    }

    #[test]
    fn reasoning_effort_off_is_omitted_for_strict_openai_like_providers() {
        for provider in [
            ApiProvider::Openai,
            ApiProvider::WanjieArk,
            ApiProvider::Qianfan,
            ApiProvider::Arcee,
            ApiProvider::Huggingface,
            ApiProvider::Fireworks,
        ] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some("off"), provider);

            assert_eq!(
                body,
                json!({}),
                "provider {provider:?} should not receive unsupported reasoning-off fields"
            );
        }
    }

    #[test]
    fn reasoning_effort_atlascloud_speaks_deepseek_dialect() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("high"), ApiProvider::Atlascloud);
        assert_eq!(
            body,
            json!({ "reasoning_effort": "high", "thinking": { "type": "enabled" } })
        );

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("max"), ApiProvider::Atlascloud);
        assert_eq!(
            body,
            json!({ "reasoning_effort": "max", "thinking": { "type": "enabled" } })
        );

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Atlascloud);
        assert_eq!(body, json!({ "thinking": { "type": "disabled" } }));
    }

    #[test]
    fn reasoning_effort_moonshot_toggles_thinking() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("high"), ApiProvider::Moonshot);
        assert_eq!(body, json!({ "thinking": { "type": "enabled" } }));

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Moonshot);
        assert_eq!(body, json!({ "thinking": { "type": "disabled" } }));
    }

    #[test]
    fn reasoning_effort_ollama_toggles_think_flag() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("high"), ApiProvider::Ollama);
        assert_eq!(body, json!({ "think": true }));

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Ollama);
        assert_eq!(body, json!({ "think": false }));
    }

    #[test]
    fn reasoning_effort_uses_nvidia_nim_chat_template_kwargs() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("max"), ApiProvider::NvidiaNim);

        assert_eq!(
            body.pointer("/chat_template_kwargs/thinking")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            body.pointer("/chat_template_kwargs/reasoning_effort")
                .and_then(Value::as_str),
            Some("max")
        );
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn reasoning_effort_off_disables_nvidia_nim_thinking() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("off"), ApiProvider::NvidiaNim);

        assert_eq!(
            body.pointer("/chat_template_kwargs/thinking")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            body.pointer("/chat_template_kwargs/reasoning_effort")
                .is_none()
        );
    }

    #[test]
    fn reasoning_effort_uses_openai_compatible_shape_for_fireworks() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("max"), ApiProvider::Fireworks);

        assert_eq!(
            body.get("reasoning_effort").and_then(Value::as_str),
            Some("max")
        );
        assert!(
            body.get("thinking").is_none(),
            "Fireworks strict-validates OpenAI-compatible requests and rejects top-level thinking"
        );
    }

    #[test]
    fn reasoning_effort_uses_arcee_reasoning_effort_without_thinking_object() {
        for (input, expected) in [
            ("minimal", "minimal"),
            ("low", "low"),
            ("mid", "medium"),
            ("medium", "medium"),
            ("high", "high"),
            ("max", "high"),
        ] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some(input), ApiProvider::Arcee);

            assert_eq!(
                body.get("reasoning_effort").and_then(Value::as_str),
                Some(expected)
            );
            assert!(
                body.get("thinking").is_none(),
                "Arcee documents reasoning_effort rather than a DeepSeek thinking object"
            );
        }
    }

    #[test]
    fn reasoning_effort_maps_openrouter_scale_without_deepseek_max_label() {
        for (input, expected) in [
            ("low", "low"),
            ("minimal", "low"),
            ("medium", "medium"),
            ("mid", "medium"),
            ("high", "high"),
            ("max", "xhigh"),
            ("xhigh", "xhigh"),
        ] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some(input), ApiProvider::Openrouter);

            assert_eq!(
                body.get("reasoning_effort").and_then(Value::as_str),
                Some(expected),
                "OpenRouter effort mapping for {input}"
            );
            assert_eq!(
                body.pointer("/thinking/type").and_then(Value::as_str),
                Some("enabled")
            );
        }
    }

    #[test]
    fn reasoning_effort_uses_xiaomi_mimo_thinking_parameter_only() {
        for input in ["low", "medium", "max", "xhigh"] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some(input), ApiProvider::XiaomiMimo);

            assert_eq!(
                body.pointer("/thinking/type").and_then(Value::as_str),
                Some("enabled"),
                "MiMo thinking mapping for {input}"
            );
            assert!(body.get("reasoning_effort").is_none());
        }

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("off"), ApiProvider::XiaomiMimo);
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("disabled")
        );
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn reasoning_effort_minimax_splits_reasoning_from_content() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("high"), ApiProvider::Minimax);
        assert_eq!(
            body.get("reasoning_split").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("adaptive")
        );
        assert!(body.get("reasoning_effort").is_none());

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Minimax);
        assert_eq!(
            body.get("reasoning_split").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("disabled")
        );

        let mut body = json!({});
        apply_reasoning_effort(&mut body, None, ApiProvider::Minimax);
        assert_eq!(body, json!({ "reasoning_split": true }));
    }

    #[test]
    fn reasoning_effort_zai_uses_documented_thinking_shape() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("high"), ApiProvider::Zai);
        assert_eq!(
            body,
            json!({ "thinking": { "type": "enabled", "clear_thinking": false } })
        );

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("max"), ApiProvider::Zai);
        assert_eq!(
            body,
            json!({ "thinking": { "type": "enabled", "clear_thinking": false } })
        );

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("ultracode"), ApiProvider::Zai);
        assert_eq!(
            body,
            json!({ "thinking": { "type": "enabled", "clear_thinking": false } })
        );

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Zai);
        assert_eq!(body, json!({ "thinking": { "type": "disabled" } }));
    }

    #[test]
    fn chat_parser_accepts_nvidia_nim_reasoning_field() -> Result<()> {
        let response = parse_chat_message(&json!({
            "id": "chatcmpl-test",
            "model": "deepseek-ai/deepseek-v4-pro",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning": "thinking via NIM",
                    "content": "final answer"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 3
            }
        }))?;

        assert!(matches!(
            response.content.first(),
            Some(ContentBlock::Thinking { thinking, .. }) if thinking == "thinking via NIM"
        ));
        assert!(matches!(
            response.content.get(1),
            Some(ContentBlock::Text { text, .. }) if text == "final answer"
        ));
        Ok(())
    }

    #[test]
    fn sse_parser_accepts_nvidia_nim_reasoning_delta() {
        let mut content_index = 0;
        let mut text_started = false;
        let mut thinking_started = false;
        let mut tool_indices = std::collections::HashMap::new();
        let mut reasoning_detail_buffers = std::collections::HashMap::new();
        let events = parse_sse_chunk(
            &json!({
                "choices": [{
                    "delta": {
                        "reasoning": "nim thought"
                    }
                }]
            }),
            &mut content_index,
            &mut text_started,
            &mut thinking_started,
            &mut tool_indices,
            &mut reasoning_detail_buffers,
            true,
        );

        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::ContentBlockDelta {
                delta: Delta::ThinkingDelta { thinking },
                ..
            } if thinking == "nim thought"
        )));
    }

    #[test]
    fn chat_tool_strict_flag_is_nested_under_function() {
        let tool = Tool {
            tool_type: Some("function".to_string()),
            name: "emit_json".to_string(),
            description: "Emit JSON".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: Some(true),
            cache_control: None,
        };
        let encoded = tool_to_chat(&tool);
        assert_eq!(
            encoded
                .get("function")
                .and_then(|function| function.get("strict"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(encoded.get("strict").is_none());
    }

    #[test]
    fn deepseek_non_beta_base_url_strips_strict_tool_flag() {
        let tool = Tool {
            tool_type: Some("function".to_string()),
            name: "emit_json".to_string(),
            description: "Emit JSON".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: Some(true),
            cache_control: None,
        };

        let encoded = tool_to_chat_for_base_url(&tool, "https://api.deepseek.com/v1");

        assert!(
            encoded
                .get("function")
                .and_then(|function| function.get("strict"))
                .is_none()
        );
    }

    #[test]
    fn deepseek_beta_and_custom_base_urls_keep_strict_tool_flag() {
        let tool = Tool {
            tool_type: Some("function".to_string()),
            name: "emit_json".to_string(),
            description: "Emit JSON".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: Some(true),
            cache_control: None,
        };

        for base_url in [
            "https://api.deepseek.com/beta",
            "https://example.com/openai/v1",
        ] {
            let encoded = tool_to_chat_for_base_url(&tool, base_url);
            assert_eq!(
                encoded
                    .get("function")
                    .and_then(|function| function.get("strict"))
                    .and_then(Value::as_bool),
                Some(true)
            );
        }
    }

    #[test]
    fn chat_tool_wire_shape_omits_anthropic_only_metadata() {
        let tool = Tool {
            tool_type: Some("function".to_string()),
            name: "mcp_read_resource".to_string(),
            description: "Read resource".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: Some(vec![json!({"uri": "file://example"})]),
            strict: None,
            cache_control: None,
        };

        let encoded = tool_to_chat_for_base_url(&tool, "https://api.fireworks.ai/inference/v1");

        assert!(encoded.get("allowed_callers").is_none());
        assert!(encoded.get("defer_loading").is_none());
        assert!(encoded.get("input_examples").is_none());
    }

    #[test]
    fn chat_messages_drop_thinking_only_assistant_for_non_reasoning_model() {
        let message = Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Thinking {
                signature: None,
                thinking: "plan".to_string(),
            }],
        };
        let out = build_chat_messages(None, &[message], "some-non-deepseek-model");
        assert!(
            !out.iter()
                .any(|value| value.get("role").and_then(Value::as_str) == Some("assistant")),
            "non-reasoning model should drop thinking-only assistant"
        );
    }

    #[test]
    fn parse_sse_chunk_closes_each_tool_block_with_matching_index() {
        let chunk = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_0",
                            "function": {"name": "read_file", "arguments": "{\"path\":\"a\"}"}
                        },
                        {
                            "index": 1,
                            "id": "call_1",
                            "function": {"name": "read_file", "arguments": "{\"path\":\"b\"}"}
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let mut content_index = 0;
        let mut text_started = false;
        let mut thinking_started = false;
        let mut tool_indices: std::collections::HashMap<u32, u32> =
            std::collections::HashMap::new();
        let mut reasoning_detail_buffers = std::collections::HashMap::new();
        let events = parse_sse_chunk(
            &chunk,
            &mut content_index,
            &mut text_started,
            &mut thinking_started,
            &mut tool_indices,
            &mut reasoning_detail_buffers,
            false,
        );

        let starts: Vec<u32> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlockStart::ToolUse { .. },
                } => Some(*index),
                _ => None,
            })
            .collect();
        let stops: Vec<u32> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ContentBlockStop { index } => Some(*index),
                _ => None,
            })
            .collect();
        let deltas: Vec<u32> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ContentBlockDelta {
                    index,
                    delta: Delta::InputJsonDelta { .. },
                } => Some(*index),
                _ => None,
            })
            .collect();

        assert_eq!(starts, vec![0, 1]);
        assert_eq!(stops, vec![0, 1]);
        assert_eq!(deltas, vec![0, 1]);
    }

    #[test]
    fn parse_sse_chunk_handles_empty_choices_usage_chunk() {
        let chunk = json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "prompt_cache_hit_tokens": 70,
                "prompt_cache_miss_tokens": 30
            }
        });

        let mut content_index = 0;
        let mut text_started = false;
        let mut thinking_started = false;
        let mut tool_indices: std::collections::HashMap<u32, u32> =
            std::collections::HashMap::new();
        let mut reasoning_detail_buffers = std::collections::HashMap::new();
        let events = parse_sse_chunk(
            &chunk,
            &mut content_index,
            &mut text_started,
            &mut thinking_started,
            &mut tool_indices,
            &mut reasoning_detail_buffers,
            false,
        );

        let StreamEvent::MessageDelta {
            usage: Some(usage), ..
        } = &events[0]
        else {
            panic!("expected usage delta");
        };
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(70));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(30));
    }

    #[test]
    fn chat_messages_drop_orphan_tool_results() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "ok".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }];

        let out = build_chat_messages(None, &messages, "deepseek-v4-flash");
        assert!(
            !out.iter()
                .any(|value| { value.get("role").and_then(Value::as_str) == Some("tool") })
        );
    }

    #[test]
    fn chat_messages_include_tool_results_when_call_present() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        thinking: "Need to inspect the directory".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "list_dir".to_string(),
                        input: json!({}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "ok".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];

        let out = build_chat_messages(None, &messages, "deepseek-v4-flash");
        assert!(
            out.iter()
                .any(|value| { value.get("role").and_then(Value::as_str) == Some("tool") })
        );
        let assistant = out
            .iter()
            .find(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
            .expect("assistant message");
        assert!(assistant.get("tool_calls").is_some());
    }

    #[test]
    fn chat_messages_encode_tool_call_names() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        thinking: "Need to search".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "web.run".to_string(),
                        input: json!({}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "ok".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];

        let out = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let assistant = out
            .iter()
            .find(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
            .expect("assistant message");
        let tool_calls = assistant
            .get("tool_calls")
            .and_then(Value::as_array)
            .expect("tool_calls array");
        let function_name = tool_calls
            .first()
            .and_then(|call| call.get("function"))
            .and_then(|func| func.get("name"))
            .and_then(Value::as_str)
            .expect("tool call function name");

        assert_eq!(function_name, to_api_tool_name("web.run"));
    }

    #[test]
    fn chat_messages_strips_orphaned_tool_calls_after_compaction() {
        // Simulates post-compaction state: assistant has tool_calls but the
        // tool result messages were summarized away.
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "tool-orphan".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "src/main.rs"}),
                    caller: None,
                }],
            },
            // No tool result follows — it was removed by compaction.
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "continue".to_string(),
                    cache_control: None,
                }],
            },
        ];

        let out = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let assistant = out
            .iter()
            .find(|value| value.get("role").and_then(Value::as_str) == Some("assistant"));
        // The safety net may drop the assistant message entirely if it only
        // contained orphaned tool_calls and no text content.
        assert!(
            assistant.is_none(),
            "assistant without content/tool_calls should be removed"
        );
        assert!(
            !out.iter()
                .any(|v| v.get("role").and_then(Value::as_str) == Some("tool")),
            "orphaned tool results should also be removed"
        );
    }

    #[test]
    fn chat_messages_keeps_valid_tool_calls_intact() {
        // Complete call+result pair should NOT be stripped.
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        thinking: "Need to list files".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-ok".to_string(),
                        name: "list_dir".to_string(),
                        input: json!({}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-ok".to_string(),
                    content: "files".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];

        let out = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let assistant = out
            .iter()
            .find(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
            .expect("assistant message");
        assert!(
            assistant.get("tool_calls").is_some(),
            "valid tool_calls should remain intact"
        );
        assert!(
            out.iter()
                .any(|value| value.get("role").and_then(Value::as_str) == Some("tool")),
            "tool result should remain"
        );
    }

    #[test]
    fn chat_messages_strips_partial_tool_results() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "a.rs"}),
                        caller: None,
                    },
                    ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "b.rs"}),
                        caller: None,
                    },
                    ContentBlock::ToolUse {
                        id: "t3".to_string(),
                        name: "shell".to_string(),
                        input: json!({"cmd": "ls"}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "content a".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t2".to_string(),
                    content: "content b".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            // No result for t3
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "continue".to_string(),
                    cache_control: None,
                }],
            },
        ];

        let out = build_chat_messages(None, &messages, "deepseek-v4-flash");
        let assistant = out
            .iter()
            .find(|v| v.get("role").and_then(Value::as_str) == Some("assistant"));
        assert!(
            assistant.is_none(),
            "assistant with only partial tool_calls should be removed"
        );
        assert!(
            !out.iter()
                .any(|v| v.get("role").and_then(Value::as_str) == Some("tool")),
            "all orphaned tool results should be removed"
        );
    }

    #[test]
    fn parse_models_response_parses_and_deduplicates() {
        let payload = r#"{
            "object": "list",
            "data": [
                {"id": "deepseek-v4-pro", "object": "model", "owned_by": "deepseek", "created": 1},
                {"id": "deepseek-v4-flash", "object": "model"},
                {"id": "deepseek-v4-pro", "object": "model", "owned_by": "deepseek", "created": 1}
            ]
        }"#;

        let models = parse_models_response(payload).expect("parse models");
        assert_eq!(
            models,
            vec![
                AvailableModel {
                    id: "deepseek-v4-flash".to_string(),
                    owned_by: None,
                    created: None
                },
                AvailableModel {
                    id: "deepseek-v4-pro".to_string(),
                    owned_by: Some("deepseek".to_string()),
                    created: Some(1)
                }
            ]
        );
    }

    #[test]
    fn parse_models_response_accepts_ollama_tag_ids() {
        let payload = r#"{
            "object": "list",
            "data": [
                {"id": "qwen2.5-coder:7b", "object": "model", "owned_by": "library"},
                {"id": "deepseek-coder-v2:16b", "object": "model"}
            ]
        }"#;

        let models = parse_models_response(payload).expect("parse models");
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["deepseek-coder-v2:16b", "qwen2.5-coder:7b"]
        );
    }

    // === #3385: provider live /models fetch + secret-free cache ==============
    //
    // All model ids below are SYNTHETIC (never real vendor model names), per the
    // issue's anti-hardcoding rule.

    /// Build a client whose OpenRouter base URL points at a mock server.
    fn openrouter_client_for(server: &MockServer) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        DeepSeekClient::new(&Config {
            provider: Some("openrouter".to_string()),
            providers: Some(ProvidersConfig {
                openrouter: ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    base_url: Some(server.uri()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("openrouter client")
    }

    fn opencode_go_client_for(server: &MockServer) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        DeepSeekClient::new(&Config {
            provider: Some("opencode-go".to_string()),
            providers: Some(ProvidersConfig {
                opencode_go: ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    base_url: Some(server.uri()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("OpenCode Go client")
    }

    async fn mount_models_json(server: &MockServer, status: u16, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn verify_provider_api_key_accepts_mocked_models_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
            .mount(&server)
            .await;

        verify_provider_api_key(ApiProvider::Openrouter, "test-key", &server.uri())
            .await
            .expect("mocked /models success should verify");
    }

    #[tokio::test]
    async fn verify_provider_api_key_returns_status_and_unicode_body_without_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401).set_body_string("密钥无效"))
            .mount(&server)
            .await;

        let err = verify_provider_api_key(ApiProvider::Openrouter, "bad-key", &server.uri())
            .await
            .expect_err("mocked /models failure should be reported");

        assert!(err.contains("HTTP 401"), "status is preserved: {err}");
        assert!(err.contains("密钥无效"), "unicode body is preserved: {err}");
    }

    #[test]
    fn opencode_go_client_rejects_messages_only_config_models() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        for model in [
            "minimax-m3",
            "minimax-m2.7",
            "minimax-m2.5",
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.6-plus",
        ] {
            let config = Config {
                provider: Some("opencode-go".to_string()),
                providers: Some(ProvidersConfig {
                    opencode_go: ProviderConfig {
                        api_key: Some("test-key".to_string()),
                        model: Some(model.to_string()),
                        ..ProviderConfig::default()
                    },
                    ..ProvidersConfig::default()
                }),
                ..Config::default()
            };
            let err = DeepSeekClient::new(&config)
                .err()
                .expect("Messages-only model must fail before client construction");
            assert!(err.to_string().contains("Chat Completions"), "{err:#}");
        }
    }

    #[tokio::test]
    async fn opencode_go_live_model_paths_keep_only_chat_completions_rows() {
        let server = MockServer::start().await;
        let mut rows: Vec<_> = crate::config::OPENCODE_GO_CHAT_MODELS
            .iter()
            .map(|id| json!({"id": id}))
            .collect();
        rows.extend([
            json!({"id": "minimax-m3"}),
            json!({"id": "minimax-m2.7"}),
            json!({"id": "minimax-m2.5"}),
            json!({"id": "qwen3.7-max"}),
            json!({"id": "qwen3.7-plus"}),
            json!({"id": "qwen3.6-plus"}),
        ]);
        mount_models_json(&server, 200, json!({"data": rows})).await;
        let client = opencode_go_client_for(&server);

        let listed = client.list_models().await.expect("filtered model list");
        let listed: std::collections::BTreeSet<_> =
            listed.into_iter().map(|model| model.id).collect();
        let expected: std::collections::BTreeSet<_> = crate::config::OPENCODE_GO_CHAT_MODELS
            .iter()
            .map(|model| (*model).to_string())
            .collect();
        assert_eq!(listed, expected);

        let delta = client.fetch_catalog_delta().await.expect("filtered delta");
        assert_eq!(delta.provider, "opencode-go");
        let delta_ids: std::collections::BTreeSet<_> = delta
            .offerings
            .iter()
            .map(|offering| offering.wire_model_id.clone())
            .collect();
        assert_eq!(delta_ids, expected);
        assert!(
            delta
                .offerings
                .iter()
                .all(|offering| offering.endpoint_key == "chat")
        );
    }

    #[tokio::test]
    async fn fetch_catalog_delta_success_builds_scoped_secret_free_live_delta() {
        let server = MockServer::start().await;
        mount_models_json(
            &server,
            200,
            json!({"data": [
                {"id": "synthetic-model-alpha", "owned_by": "synthetic-owner"},
                {"id": "synthetic-model-beta"}
            ]}),
        )
        .await;
        let client = openrouter_client_for(&server);

        let delta = client.fetch_catalog_delta().await.expect("delta");
        assert_eq!(delta.provider, "openrouter");
        assert_eq!(
            delta.base_url_fingerprint,
            base_url_fingerprint(&server.uri()),
            "delta is scoped to the base-URL fingerprint"
        );
        let ids: Vec<&str> = delta
            .offerings
            .iter()
            .map(|offering| offering.wire_model_id.as_str())
            .collect();
        assert!(ids.contains(&"synthetic-model-alpha"), "ids: {ids:?}");
        assert!(ids.contains(&"synthetic-model-beta"), "ids: {ids:?}");
        for offering in &delta.offerings {
            // Live rows carry honest provenance and no inferred facts/secrets.
            assert!(matches!(offering.source, CatalogSource::Live { .. }));
            assert_eq!(offering.canonical_model, None);
            assert_eq!(offering.cost, None);
            assert!(offering.reasoning.is_none());
        }
    }

    #[tokio::test]
    async fn fetch_catalog_delta_maps_http_statuses_to_typed_errors() {
        for (status, expected) in [
            (401u16, CatalogRefreshError::Unauthorized),
            (403, CatalogRefreshError::Forbidden),
            (404, CatalogRefreshError::NotFound),
            (429, CatalogRefreshError::RateLimited),
            (500, CatalogRefreshError::Network),
        ] {
            let server = MockServer::start().await;
            mount_models_json(&server, status, json!({"error": "nope"})).await;
            let client = openrouter_client_for(&server);
            let err = client.fetch_catalog_delta().await.expect_err("should fail");
            assert_eq!(err, expected, "status {status} should map to {expected:?}");
        }
    }

    #[tokio::test]
    async fn fetch_catalog_delta_maps_invalid_json_and_empty_list() {
        // Invalid JSON -> InvalidResponse.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        let client = openrouter_client_for(&server);
        assert_eq!(
            client
                .fetch_catalog_delta()
                .await
                .expect_err("invalid json"),
            CatalogRefreshError::InvalidResponse
        );

        // Empty list -> EmptyList.
        let server = MockServer::start().await;
        mount_models_json(&server, 200, json!({"data": []})).await;
        let client = openrouter_client_for(&server);
        assert_eq!(
            client.fetch_catalog_delta().await.expect_err("empty list"),
            CatalogRefreshError::EmptyList
        );
    }

    #[tokio::test]
    async fn refresh_catalog_cache_records_success_then_preserves_rows_on_failure() {
        // First refresh succeeds and caches live rows.
        let server = MockServer::start().await;
        mount_models_json(
            &server,
            200,
            json!({"data": [{"id": "synthetic-model-gamma"}]}),
        )
        .await;
        let client = openrouter_client_for(&server);
        let mut cache = ProviderCatalogCache::new();

        let status = client.refresh_catalog_cache(&mut cache, 3600).await;
        assert_eq!(status, CatalogStatus::Fresh);
        let fp = base_url_fingerprint(&server.uri());
        let cached = cache.get("openrouter", &fp).expect("cached entry");
        assert_eq!(cached.offerings.len(), 1);
        assert_eq!(cached.offerings[0].wire_model_id, "synthetic-model-gamma");

        // A later failing refresh on the same base URL flips status to Failed
        // but PRESERVES the rows.
        server.reset().await;
        mount_models_json(&server, 401, json!({"error": "denied"})).await;
        let status = client.refresh_catalog_cache(&mut cache, 3600).await;
        assert!(matches!(
            status,
            CatalogStatus::Failed {
                reason: CatalogRefreshError::Unauthorized,
                ..
            }
        ));
        let cached = cache.get("openrouter", &fp).expect("entry still present");
        assert_eq!(
            cached.offerings.len(),
            1,
            "rows from the prior success must survive a failed refresh"
        );
        assert!(matches!(cached.status, CatalogStatus::Failed { .. }));

        // #4139: failed/stale rows must still publish into ProviderLake so
        // pickers keep live coverage instead of dropping back to bundled-only.
        let visible = cache.all_visible_offerings(now_unix());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].wire_model_id, "synthetic-model-gamma");
        assert!(
            cache.all_fresh_offerings(now_unix()).is_empty(),
            "Failed entries are not fresh, but they remain visible"
        );
    }

    #[tokio::test]
    async fn live_catalog_is_scoped_by_base_url_fingerprint() {
        // Same provider, two different base URLs -> two distinct cache scopes.
        let server_a = MockServer::start().await;
        mount_models_json(&server_a, 200, json!({"data": [{"id": "synthetic-a"}]})).await;
        let server_b = MockServer::start().await;
        mount_models_json(&server_b, 200, json!({"data": [{"id": "synthetic-b"}]})).await;

        let mut cache = ProviderCatalogCache::new();
        openrouter_client_for(&server_a)
            .refresh_catalog_cache(&mut cache, 3600)
            .await;
        openrouter_client_for(&server_b)
            .refresh_catalog_cache(&mut cache, 3600)
            .await;

        let fp_a = base_url_fingerprint(&server_a.uri());
        let fp_b = base_url_fingerprint(&server_b.uri());
        assert_ne!(
            fp_a, fp_b,
            "different base URLs must fingerprint differently"
        );
        assert_eq!(
            cache.get("openrouter", &fp_a).expect("a").offerings[0].wire_model_id,
            "synthetic-a"
        );
        assert_eq!(
            cache.get("openrouter", &fp_b).expect("b").offerings[0].wire_model_id,
            "synthetic-b"
        );
    }

    #[tokio::test]
    async fn static_rows_survive_a_live_refresh_failure() {
        // Bundled/static rows compile through even when the live layer is empty
        // (the state after a failed refresh with no prior success).
        let server = MockServer::start().await;
        mount_models_json(&server, 503, json!({"error": "down"})).await;
        let client = openrouter_client_for(&server);
        let mut cache = ProviderCatalogCache::new();
        let status = client.refresh_catalog_cache(&mut cache, 3600).await;
        assert!(matches!(status, CatalogStatus::Failed { .. }));

        let static_row = CatalogOffering {
            provider: "openrouter".to_string(),
            wire_model_id: "synthetic-static".to_string(),
            endpoint_key: "chat".to_string(),
            ..CatalogOffering::default()
        };
        let fp = base_url_fingerprint(&server.uri());
        let fresh_live: Vec<CatalogOffering> = cache
            .get("openrouter", &fp)
            .filter(|entry| entry.is_fresh(now_unix()))
            .map(|entry| entry.offerings.clone())
            .unwrap_or_default();
        let snapshot = codewhale_config::catalog::CatalogCompiler::new()
            .with_bundled(vec![static_row])
            .with_live(fresh_live)
            .compile();
        assert!(
            snapshot
                .offerings
                .iter()
                .any(|offering| offering.wire_model_id == "synthetic-static"),
            "static fallback row must remain available after a failed refresh"
        );
    }

    #[test]
    fn parse_usage_reads_deepseek_cache_and_reasoning_tokens() {
        let usage = parse_usage(Some(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "prompt_cache_hit_tokens": 70,
            "prompt_cache_miss_tokens": 30,
            "completion_tokens_details": {
                "reasoning_tokens": 12
            }
        })));

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(70));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(30));
        assert_eq!(usage.reasoning_tokens, Some(12));
    }

    #[test]
    fn parse_usage_counts_reasoning_tokens_when_completion_tokens_are_zero() {
        let usage = parse_usage(Some(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 0,
            "completion_tokens_details": {
                "reasoning_tokens": 12
            }
        })));

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.reasoning_tokens, Some(12));
        assert!(
            crate::pricing::calculate_turn_cost_from_usage("deepseek-v4-pro", &usage)
                .expect("DeepSeek V4 Pro pricing should apply")
                > 0.0
        );
    }

    #[test]
    fn parse_usage_derives_completion_tokens_from_total_tokens_when_needed() {
        let usage = parse_usage(Some(&json!({
            "prompt_tokens": 100,
            "total_tokens": 125,
            "prompt_cache_hit_tokens": 70,
            "prompt_cache_miss_tokens": 30
        })));

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 25);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(70));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(30));
    }

    #[test]
    fn parse_usage_reads_v4_prompt_tokens_details_cached_tokens() {
        let usage = parse_usage(Some(&json!({
            "prompt_tokens": 4000,
            "completion_tokens": 20,
            "prompt_tokens_details": {
                "cached_tokens": 3000
            }
        })));

        assert_eq!(usage.input_tokens, 4000);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(3000));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(1000));
    }

    #[test]
    fn parse_usage_infers_cache_miss_from_selected_hit_source() {
        let usage = parse_usage(Some(&json!({
            "prompt_tokens": 4000,
            "completion_tokens": 20,
            "prompt_cache_hit_tokens": 3000,
            "prompt_tokens_details": {
                "cached_tokens": 1000
            }
        })));

        assert_eq!(usage.input_tokens, 4000);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(3000));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(1000));
    }

    #[test]
    fn sanitize_thinking_mode_counts_reasoning_replay_across_assistant_turns() {
        // Multi-turn body that mimics two prior tool-calling rounds: each
        // assistant message carries its `reasoning_content`. The sanitizer
        // should keep all of them and the count helper should tally bytes
        // across every assistant message.
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "messages": [
                { "role": "system", "content": "you are helpful" },
                { "role": "user", "content": "step 1" },
                {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "I need to call tool A first.",
                    "tool_calls": [{ "id": "1", "type": "function" }]
                },
                { "role": "tool", "tool_call_id": "1", "content": "ok" },
                {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "Now I call tool B.",
                    "tool_calls": [{ "id": "2", "type": "function" }]
                },
                { "role": "tool", "tool_call_id": "2", "content": "ok" },
                { "role": "user", "content": "step 2" }
            ]
        });

        let approx_tokens = sanitize_thinking_mode_messages(
            &mut body,
            "deepseek-v4-pro",
            Some("max"),
            ApiProvider::Deepseek,
        )
        .expect("multi-turn thinking-mode conversation should report replay tokens");
        // ~4 chars/token; 46 bytes of reasoning -> 11 tokens.
        assert_eq!(approx_tokens, 11);

        let chars = count_reasoning_replay_chars(&body);
        // "I need to call tool A first." (28) + "Now I call tool B." (18) = 46
        assert_eq!(chars, 46);

        // No assistant messages should have lost or had their reasoning_content blanked.
        let messages = body["messages"].as_array().unwrap();
        let assistant_with_reasoning: usize = messages
            .iter()
            .filter(|m| m["role"] == "assistant")
            .filter(|m| {
                m["reasoning_content"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            })
            .count();
        assert_eq!(assistant_with_reasoning, 2);
    }

    /// Issue #30: when no thinking-mode replay applies (non-thinking model or
    /// empty conversation), the sanitizer returns `None` so the footer chip
    /// stays hidden.
    #[test]
    fn sanitize_thinking_mode_returns_none_for_non_thinking_model() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "user", "content": "hi" }
            ]
        });
        let result = sanitize_thinking_mode_messages(
            &mut body,
            "deepseek-v4-flash",
            None,
            ApiProvider::Deepseek,
        );
        // reasoning_effort is None → no thinking injection, result is None
        assert!(result.is_none());
    }

    #[test]
    fn sanitize_thinking_mode_counts_substituted_placeholder() {
        // An assistant tool-call message is missing reasoning_content; the
        // sanitizer must inject the placeholder, and the count helper must
        // include the placeholder in the total (since it's in the wire
        // payload that ships to DeepSeek).
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "messages": [
                { "role": "user", "content": "hi" },
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{ "id": "1", "type": "function" }]
                }
            ]
        });

        sanitize_thinking_mode_messages(
            &mut body,
            "deepseek-v4-pro",
            Some("max"),
            ApiProvider::Deepseek,
        );

        let chars = count_reasoning_replay_chars(&body);
        // "(reasoning omitted)" is 19 bytes.
        assert_eq!(chars, 19);
    }

    #[test]
    fn sanitize_thinking_mode_skips_generic_openai_provider() {
        // #1542 intent (narrowed by #1739/#1694): the sanitizer only skips for
        // a *genuine non-DeepSeek* model on the generic openai provider. A
        // DeepSeek reasoning model on the openai provider still gets sanitized
        // (see chat.rs `deepseek_model_on_openai_provider_still_replays_*`).
        let mut body = json!({
            "model": "qwen3-coder",
            "messages": [
                { "role": "user", "content": "hi" },
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{ "id": "1", "type": "function" }]
                }
            ]
        });

        let result = sanitize_thinking_mode_messages(
            &mut body,
            "qwen3-coder",
            Some("max"),
            ApiProvider::Openai,
        );

        assert!(result.is_none());
        let assistant = body["messages"]
            .as_array()
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message["role"] == "assistant")
            })
            .expect("assistant message");
        assert!(
            assistant.get("reasoning_content").is_none(),
            "generic OpenAI-compatible provider payload must not get reasoning_content (#1542)"
        );
    }

    #[test]
    fn sanitize_thinking_mode_keeps_tool_call_placeholder_after_new_user_turn() {
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "messages": [
                { "role": "user", "content": "step 1" },
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{ "id": "1", "type": "function" }]
                },
                { "role": "tool", "tool_call_id": "1", "content": "ok" },
                { "role": "user", "content": "step 2" }
            ]
        });

        sanitize_thinking_mode_messages(
            &mut body,
            "deepseek-v4-pro",
            Some("max"),
            ApiProvider::Deepseek,
        );

        let messages = body["messages"].as_array().unwrap();
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant tool-call message");
        assert_eq!(
            assistant.get("reasoning_content").and_then(Value::as_str),
            Some("(reasoning omitted)")
        );
    }

    #[test]
    fn token_bucket_enforces_delay_when_empty() {
        let now = Instant::now();
        let mut bucket = TokenBucket {
            enabled: true,
            capacity: 1.0,
            tokens: 1.0,
            refill_per_sec: 2.0,
            last_refill: now,
        };

        assert!(bucket.delay_until_available(1.0).is_none());
        let delay = bucket
            .delay_until_available(1.0)
            .expect("bucket should require refill delay");
        assert!(
            delay >= Duration::from_millis(400) && delay <= Duration::from_millis(600),
            "unexpected refill delay: {delay:?}"
        );
    }

    #[test]
    fn stream_buffer_pool_reuses_released_buffers() {
        let mut first = acquire_stream_buffer();
        first.extend_from_slice(b"hello");
        let released_capacity = first.capacity();
        release_stream_buffer(first);

        let second = acquire_stream_buffer();
        assert!(second.is_empty());
        assert!(
            second.capacity() >= released_capacity,
            "pooled buffer capacity should be reused"
        );
    }

    #[test]
    fn base_url_security_rejects_insecure_non_local_http() {
        let _lock = ALLOW_INSECURE_HTTP_ENV_LOCK.lock().unwrap();
        let _guard = AllowInsecureHttpEnvGuard::capture();
        unsafe { std::env::remove_var(ALLOW_INSECURE_HTTP_ENV) };

        let err = validate_base_url_security("http://api.deepseek.com")
            .expect_err("non-local insecure HTTP should be rejected");
        assert!(err.to_string().contains("Refusing insecure base URL"));
    }

    #[test]
    fn base_url_security_errors_redact_sensitive_url_parts() {
        let _lock = ALLOW_INSECURE_HTTP_ENV_LOCK.lock().unwrap();
        let _guard = AllowInsecureHttpEnvGuard::capture();
        unsafe { std::env::remove_var(ALLOW_INSECURE_HTTP_ENV) };

        let err =
            validate_base_url_security("http://user:secret@example.com/v1?api_key=sk-test&ok=1")
                .expect_err("non-local insecure HTTP should be rejected");
        let message = err.to_string();

        assert!(message.contains("http://***:***@example.com/v1?api_key=***&ok=1"));
        assert!(!message.contains("user:secret"));
        assert!(!message.contains("sk-test"));
    }

    #[test]
    fn base_url_security_allows_localhost_http() {
        let _lock = ALLOW_INSECURE_HTTP_ENV_LOCK.lock().unwrap();
        let _guard = AllowInsecureHttpEnvGuard::capture();
        unsafe { std::env::remove_var(ALLOW_INSECURE_HTTP_ENV) };

        assert!(validate_base_url_security("http://localhost:8080").is_ok());
        assert!(validate_base_url_security("http://127.0.0.1:8080").is_ok());
    }

    #[test]
    fn base_url_security_allows_non_local_http_with_explicit_opt_in() {
        let _lock = ALLOW_INSECURE_HTTP_ENV_LOCK.lock().unwrap();
        let _guard = AllowInsecureHttpEnvGuard::capture();
        unsafe { std::env::set_var(ALLOW_INSECURE_HTTP_ENV, "1") };

        assert!(validate_base_url_security("http://192.168.0.110:8000/v1").is_ok());
    }

    /// Serialize tests that mutate `DEEPSEEK_ALLOW_INSECURE_HTTP`; env vars are
    /// process-global and would otherwise leak across security checks.
    static ALLOW_INSECURE_HTTP_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct AllowInsecureHttpEnvGuard {
        prior: Option<std::ffi::OsString>,
    }
    impl AllowInsecureHttpEnvGuard {
        fn capture() -> Self {
            Self {
                prior: std::env::var_os(ALLOW_INSECURE_HTTP_ENV),
            }
        }
    }
    impl Drop for AllowInsecureHttpEnvGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(v) => unsafe { std::env::set_var(ALLOW_INSECURE_HTTP_ENV, v) },
                None => unsafe { std::env::remove_var(ALLOW_INSECURE_HTTP_ENV) },
            }
        }
    }

    #[test]
    fn connection_health_degrades_and_recovers() {
        let now = Instant::now();
        let mut health = ConnectionHealth::default();
        assert_eq!(health.state, ConnectionState::Healthy);

        apply_request_failure(&mut health, now);
        assert_eq!(health.state, ConnectionState::Healthy);

        apply_request_failure(&mut health, now + Duration::from_millis(1));
        assert_eq!(health.state, ConnectionState::Degraded);
        assert_eq!(health.consecutive_failures, 2);

        let recovered = apply_request_success(&mut health, now + Duration::from_secs(1));
        assert!(recovered);
        assert_eq!(health.state, ConnectionState::Healthy);
        assert_eq!(health.consecutive_failures, 0);
    }

    #[test]
    fn recovery_probe_respects_cooldown() {
        let now = Instant::now();
        let mut health = ConnectionHealth {
            state: ConnectionState::Degraded,
            ..ConnectionHealth::default()
        };

        assert!(mark_recovery_probe_if_due(&mut health, now));
        assert_eq!(health.state, ConnectionState::Recovering);
        assert!(!mark_recovery_probe_if_due(
            &mut health,
            now + Duration::from_secs(1)
        ));
        assert!(mark_recovery_probe_if_due(
            &mut health,
            now + RECOVERY_PROBE_COOLDOWN + Duration::from_millis(1)
        ));
    }

    // === #103 Phase 2: HTTP/1 escape hatch ===================================

    /// Serialize tests that mutate `DEEPSEEK_FORCE_HTTP1` so they don't race
    /// against each other — env vars are process-global.
    static FORCE_HTTP1_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct ForceHttp1EnvGuard {
        prior: Option<std::ffi::OsString>,
    }
    impl ForceHttp1EnvGuard {
        fn capture() -> Self {
            Self {
                prior: std::env::var_os("DEEPSEEK_FORCE_HTTP1"),
            }
        }
    }
    impl Drop for ForceHttp1EnvGuard {
        fn drop(&mut self) {
            // Safety: scoped to test process; reverts to the captured value.
            match &self.prior {
                Some(v) => unsafe { std::env::set_var("DEEPSEEK_FORCE_HTTP1", v) },
                None => unsafe { std::env::remove_var("DEEPSEEK_FORCE_HTTP1") },
            }
        }
    }

    #[test]
    fn force_http1_unset_is_false() {
        let _lock = FORCE_HTTP1_ENV_LOCK.lock().unwrap();
        let _guard = ForceHttp1EnvGuard::capture();
        unsafe { std::env::remove_var("DEEPSEEK_FORCE_HTTP1") };
        assert!(!force_http1_from_env());
    }

    #[test]
    fn force_http1_truthy_values() {
        let _lock = FORCE_HTTP1_ENV_LOCK.lock().unwrap();
        let _guard = ForceHttp1EnvGuard::capture();
        for value in ["1", "true", "True", "YES", "on", " 1 "] {
            // Safety: serialized by FORCE_HTTP1_ENV_LOCK; reverted by guard.
            unsafe { std::env::set_var("DEEPSEEK_FORCE_HTTP1", value) };
            assert!(
                force_http1_from_env(),
                "{value:?} should be parsed as truthy",
            );
        }
    }

    #[test]
    fn force_http1_falsy_values() {
        let _lock = FORCE_HTTP1_ENV_LOCK.lock().unwrap();
        let _guard = ForceHttp1EnvGuard::capture();
        for value in ["0", "false", "no", "off", "", "garbage", "2"] {
            unsafe { std::env::set_var("DEEPSEEK_FORCE_HTTP1", value) };
            assert!(
                !force_http1_from_env(),
                "{value:?} should NOT be parsed as truthy"
            );
        }
    }

    #[test]
    fn api_url_with_suffix_strips_version_before_chat_suffix() {
        assert_eq!(
            api_url_with_suffix(
                "https://api.example.com/v1",
                "chat/completions",
                Some("/chat/completions")
            ),
            "https://api.example.com/chat/completions"
        );
        assert_eq!(
            api_url_with_suffix(
                "https://api.example.com/beta",
                "chat/completions",
                Some("/chat/completions")
            ),
            "https://api.example.com/chat/completions"
        );
    }

    #[test]
    fn api_url_with_suffix_handles_leading_slash() {
        assert_eq!(
            api_url_with_suffix(
                "https://api.example.com/v1",
                "chat/completions",
                Some("chat/completions")
            ),
            "https://api.example.com/chat/completions"
        );
    }

    #[test]
    fn api_url_with_suffix_ignores_suffix_for_models() {
        assert_eq!(
            api_url_with_suffix(
                "https://api.example.com/v1",
                "models",
                Some("/chat/completions")
            ),
            "https://api.example.com/v1/models"
        );
    }

    #[test]
    fn api_url_with_suffix_ignores_suffix_for_beta_paths() {
        assert_eq!(
            api_url_with_suffix(
                "https://api.example.com/v1",
                "beta/completions",
                Some("/chat/completions")
            ),
            "https://api.example.com/beta/completions"
        );
    }

    #[test]
    fn api_url_with_suffix_default_behavior_without_suffix() {
        assert_eq!(
            api_url_with_suffix("https://api.deepseek.com", "chat/completions", None),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn redact_url_for_display_masks_userinfo_and_sensitive_query_values() {
        let redacted = redact_url_for_display(
            "https://user:secret@example.com/v1?api_key=sk-test&region=us&refresh-token=abc",
        );

        assert_eq!(
            redacted,
            "https://***:***@example.com/v1?api_key=***&region=us&refresh-token=***"
        );
    }

    #[test]
    fn take_sse_line_preserves_multibyte_split_across_reads() {
        // "你好" streamed so the 3-byte '好' straddles a read boundary.
        let full = "data: 你好\n";
        let bytes = full.as_bytes();
        let split = bytes.len() - 2; // mid '好'
        let mut buffer: Vec<u8> = Vec::new();
        // First read: no complete line yet.
        buffer.extend_from_slice(&bytes[..split]);
        assert_eq!(take_sse_line(&mut buffer), None);
        // Second read completes the line; '好' must be intact, not U+FFFD.
        buffer.extend_from_slice(&bytes[split..]);
        let line = take_sse_line(&mut buffer).expect("a complete line");
        assert_eq!(line, "data: 你好");
        assert!(!line.contains('\u{FFFD}'), "multibyte char was corrupted");
        assert_eq!(extract_sse_data_value(&line), Some("你好"));
        // Buffer fully drained.
        assert!(buffer.is_empty());
    }

    #[test]
    fn take_sse_line_returns_none_without_newline() {
        let mut buffer = b"data: partial".to_vec();
        assert_eq!(take_sse_line(&mut buffer), None);
        assert_eq!(buffer, b"data: partial");
    }

    #[test]
    fn extract_sse_data_value_accepts_optional_space() {
        assert_eq!(
            extract_sse_data_value("data: {\"ok\":true}"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            extract_sse_data_value("data:{\"ok\":true}"),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn extract_sse_data_value_handles_done_marker() {
        assert_eq!(extract_sse_data_value("data: [DONE]"), Some("[DONE]"));
        assert_eq!(extract_sse_data_value("data:[DONE]"), Some("[DONE]"));
    }

    #[test]
    fn extract_sse_data_value_rejects_non_data_lines() {
        assert_eq!(extract_sse_data_value("event: message"), None);
        assert_eq!(extract_sse_data_value(": heartbeat"), None);
    }

    /// Build a DeepSeek config with an inline key/base URL plus the resolved
    /// runtime route for it. `RouteResolver` (reached through
    /// `resolve_runtime_route`) is the only producer of `ReadyRouteCandidate`,
    /// so we mint candidates the same way the engine does at switch time.
    fn deepseek_route_for_test(
        base_url: &str,
        model: &str,
    ) -> (Config, crate::route_runtime::ResolvedRuntimeRoute) {
        let config = Config {
            provider: Some("deepseek".to_string()),
            api_key: Some("ds-test".to_string()),
            base_url: Some(base_url.to_string()),
            default_text_model: Some(model.to_string()),
            ..Config::default()
        };
        let route = crate::route_runtime::resolve_runtime_route(
            &config,
            ApiProvider::Deepseek,
            Some(model),
        )
        .expect("deepseek route should resolve");
        (config, route)
    }

    #[test]
    fn from_candidate_uses_candidate_base_url_and_wire_model() {
        let (_config, route) =
            deepseek_route_for_test("https://route.example.com/v1", "deepseek-v4-pro");

        let client = DeepSeekClient::from_candidate(&route.config, &route.candidate)
            .expect("client should construct from candidate");

        // The transport is bound to the candidate, not re-derived from Config.
        assert_eq!(client.base_url, route.candidate.endpoint.base_url);
        assert_eq!(client.default_model, route.candidate.wire_model_id.as_str());
    }

    #[test]
    fn from_candidate_matches_new_when_config_agrees() {
        // For a normal route, the resolver writes the candidate's wire model and
        // endpoint back into `route.config`, so constructing from the candidate
        // must be byte-identical to constructing from that config. This pins the
        // "no behavior change today" guarantee for Slice A.
        let (_config, route) =
            deepseek_route_for_test("https://api.deepseek.com/v1", "deepseek-v4-pro");

        let from_new = DeepSeekClient::new(&route.config).expect("new client");
        let from_candidate = DeepSeekClient::from_candidate(&route.config, &route.candidate)
            .expect("candidate client");

        assert_eq!(from_candidate.base_url, from_new.base_url);
        assert_eq!(from_candidate.default_model, from_new.default_model);
        assert_eq!(from_candidate.api_provider, from_new.api_provider);
    }

    #[test]
    fn from_candidate_binds_custom_provider_base_url_and_model() {
        // #1519: a custom OpenAI-compatible provider resolves to a candidate
        // whose endpoint/model come from the named `[providers.<name>]` table,
        // and `from_candidate` must bind that verbatim base URL + wire model.
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "my_thing".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://api.example.com/v1".to_string()),
                model: Some("custom-model-v1".to_string()),
                api_key_env: Some("EXAMPLE_API_KEY_FROM_CANDIDATE_TEST".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("my_thing".to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        // The config names a custom provider, so it must resolve as Custom.
        assert_eq!(config.api_provider(), ApiProvider::Custom);

        let route = crate::route_runtime::resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom route should resolve");

        // Provide the key the route's auth path will read.
        // SAFETY: single-threaded unit test mutating a uniquely-named var.
        unsafe {
            std::env::set_var("EXAMPLE_API_KEY_FROM_CANDIDATE_TEST", "sk-custom");
        }
        let client = DeepSeekClient::from_candidate(&route.config, &route.candidate)
            .expect("client should construct from custom candidate");
        unsafe {
            std::env::remove_var("EXAMPLE_API_KEY_FROM_CANDIDATE_TEST");
        }

        assert_eq!(client.base_url, "https://api.example.com/v1");
        assert_eq!(client.default_model, "custom-model-v1");
        assert_eq!(client.api_provider, ApiProvider::Custom);
        // The candidate carried the custom endpoint + verbatim wire model.
        assert_eq!(
            route.candidate.endpoint.base_url,
            "https://api.example.com/v1"
        );
        assert_eq!(route.candidate.wire_model_id.as_str(), "custom-model-v1");
    }
}
