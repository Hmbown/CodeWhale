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
use tokio::sync::{
    Mutex as AsyncMutex, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, OwnedSemaphorePermit, RwLock,
    Semaphore,
};

use codewhale_config::catalog::{
    CatalogOffering, CatalogRefreshError, CatalogSnapshot, CatalogSource, CatalogStatus,
    ProviderCatalogCache, ProviderCatalogDelta, base_url_fingerprint, now_unix,
};
use codewhale_config::provider::WireFormat;
use codewhale_config::route::{
    LogicalModelRef, ReadyRouteCandidate, RouteLimits, RouteRequest, RouteResolver,
};
use codewhale_config::{auth_mode_disables_api_key, is_upstream_auth_header};

use crate::config::{
    ApiProvider, Config, RetryPolicy, validate_route, wire_model_for_provider_route,
};
use crate::llm_client::{
    LlmClient, LlmError, RetryConfig as LlmRetryConfig, extract_retry_after,
    sanitize_http_error_body, with_retry,
};
use crate::logging;
use crate::models::Role;
use crate::models::{
    ContentBlock, Message, MessageRequest, MessageResponse, ServerToolUsage, SystemPrompt, Usage,
};

/// Every provider request that can feed the interactive TUI's attached CWC run
/// takes a shared permit at this lowest common dispatch seam. Runtime Chat holds
/// the exclusive permit from native admission through durable terminal
/// acknowledgement. This covers ordinary turns, auto-route classification,
/// advisor calls, detached subagents, compaction, purge, and streaming without
/// serializing independent RuntimeThreadManager stores.
#[cfg(not(test))]
static RUNTIME_CHAT_INFERENCE_GATE: OnceLock<Arc<RwLock<()>>> = OnceLock::new();

/// Unit tests run many independent Tokio runtimes in one process. Keying the
/// otherwise-identical gate by runtime keeps unrelated libtest cases from
/// manufacturing contention while preserving exact read/write behavior among
/// tasks on the same current-thread or multi-thread runtime.
#[cfg(test)]
static RUNTIME_CHAT_INFERENCE_TEST_GATES: OnceLock<
    StdMutex<HashMap<RuntimeChatInferenceTestScope, std::sync::Weak<RwLock<()>>>>,
> = OnceLock::new();

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum RuntimeChatInferenceTestScope {
    Runtime(tokio::runtime::Id),
    Thread(std::thread::ThreadId),
}

#[cfg(not(test))]
fn runtime_chat_inference_gate() -> Arc<RwLock<()>> {
    Arc::clone(RUNTIME_CHAT_INFERENCE_GATE.get_or_init(|| Arc::new(RwLock::new(()))))
}

#[cfg(test)]
fn runtime_chat_inference_gate() -> Arc<RwLock<()>> {
    let scope = tokio::runtime::Handle::try_current()
        .map(|handle| RuntimeChatInferenceTestScope::Runtime(handle.id()))
        .unwrap_or_else(|_| RuntimeChatInferenceTestScope::Thread(std::thread::current().id()));
    let mut gates = RUNTIME_CHAT_INFERENCE_TEST_GATES
        .get_or_init(|| StdMutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(gate) = gates.get(&scope).and_then(std::sync::Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(RwLock::new(()));
    gates.insert(scope, Arc::downgrade(&gate));
    gate
}

/// Exclusive ownership retained by isolated Runtime Chat.
pub(crate) struct RuntimeChatInferenceOwnership {
    _gate: OwnedRwLockWriteGuard<()>,
}

/// Shared participation retained for the full provider-output lifecycle.
pub(crate) struct RemoteControlInferencePermit {
    _gate: OwnedRwLockReadGuard<()>,
}

#[cfg(test)]
pub(crate) async fn acquire_runtime_chat_inference_ownership() -> RuntimeChatInferenceOwnership {
    let gate = runtime_chat_inference_gate().write_owned().await;
    RuntimeChatInferenceOwnership { _gate: gate }
}

pub(crate) fn try_acquire_runtime_chat_inference_ownership() -> Option<RuntimeChatInferenceOwnership>
{
    let gate = runtime_chat_inference_gate().try_write_owned().ok()?;
    Some(RuntimeChatInferenceOwnership { _gate: gate })
}

/// Join the attached interactive CWC run as a provider-output participant.
/// Standalone background adapters that bypass `DeepSeekClient::create_message`
/// use this guard and retain it through response decoding.
pub(crate) async fn acquire_remote_control_inference_participant() -> RemoteControlInferencePermit {
    let gate = runtime_chat_inference_gate().read_owned().await;
    RemoteControlInferencePermit { _gate: gate }
}

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
    /// HTTP/1.1-only twin of [`Self::http_client`], used for automatic
    /// stream-header fallback when H2 stalls. Same auth and headers.
    pub(super) http1_client: reqwest::Client,
    api_key: String,
    /// Exact configured credential values removed from model-bound tool
    /// results. Structural redaction handles config/JSON assignments, while
    /// this list closes the gap for bare provider tokens with no recognizable
    /// prefix (for example token-plan and provider-specific keys).
    model_bound_secret_values: Arc<Vec<String>>,
    pub(super) base_url: String,
    pub(super) api_provider: ApiProvider,
    /// Exact configured provider identity and billing mode frozen when this
    /// client is built. Child/tool calls only carry the client at dispatch, so
    /// these route facts must travel with it instead of being reconstructed
    /// from the mutable parent session at completion time.
    provider_identity: String,
    billing_surface: Option<String>,
    billing_mode: crate::cost_status::RouteBillingMode,
    /// Non-secret limits frozen from the same resolved candidate as the
    /// endpoint and wire model. Auxiliary calls carry only this client, so
    /// they must not reconstruct output caps with `None` and discard a custom
    /// route's context window.
    route_limits: Option<RouteLimits>,
    /// ChatGPT account id captured through the same consent-gated credential
    /// resolution as the Codex bearer token.
    pub(super) codex_account_id: Option<String>,
    wire_format: WireFormat,
    retry: RetryPolicy,
    /// Auxiliary inspection calls use the normal bounded retry schedule but
    /// never publish retry/rate-limit state into process-global UI cells.
    isolated_request_state: bool,
    /// Whether this concrete client can contribute provider output to the
    /// interactive TUI's attached CWC run. Isolated Runtime Chat executes under
    /// the host's exclusive permit; independent RuntimeThreadManager stores do
    /// not share that run and remain concurrent.
    remote_control_inference_participant: bool,
    default_model: String,
    connection_health: Arc<AsyncMutex<ConnectionHealth>>,
    rate_limiter: Arc<AsyncMutex<TokenBucket>>,
    request_concurrency: Option<ProviderConcurrencyLimiter>,
    path_suffix: Option<String>,
    /// Unit tests keep the semantic route exact while sending the actual
    /// production request through a local capture server. This field is
    /// compiled out of release builds.
    #[cfg(test)]
    test_chat_transport_base_url: Option<String>,
    /// Messages equivalent of `test_chat_transport_base_url`; keeps exact
    /// route shaping bound to the semantic endpoint while tests capture on a
    /// local server.
    #[cfg(test)]
    test_messages_transport_base_url: Option<String>,
    pub(super) reasoning_stream_style: Option<String>,
    pub(super) stream_idle_timeout: Duration,
}

const CONNECTION_FAILURE_THRESHOLD: u32 = 2;
const RECOVERY_PROBE_COOLDOWN: Duration = Duration::from_secs(15);

const DEFAULT_CLIENT_RATE_LIMIT_RPS: f64 = 8.0;
const DEFAULT_CLIENT_RATE_LIMIT_BURST: f64 = 16.0;
const ALLOW_INSECURE_HTTP_ENV: &str = "CODEWHALE_ALLOW_INSECURE_HTTP";
/// Legacy alias for [`ALLOW_INSECURE_HTTP_ENV`].
const LEGACY_ALLOW_INSECURE_HTTP_ENV: &str = "DEEPSEEK_ALLOW_INSECURE_HTTP";

fn client_user_agent(api_provider: ApiProvider) -> &'static str {
    // The ChatGPT Codex backend is the sole route with a documented
    // compatibility exception. Kimi Code, including K3, must keep the normal
    // Codewhale identity rather than impersonating a Kimi CLI.
    if api_provider == ApiProvider::OpenaiCodex {
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
    }
}

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
        let rps = std::env::var("CODEWHALE_RATE_LIMIT_RPS")
            .or_else(|_| std::env::var("DEEPSEEK_RATE_LIMIT_RPS"))
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(DEFAULT_CLIENT_RATE_LIMIT_RPS)
            .max(0.0);
        let burst = std::env::var("CODEWHALE_RATE_LIMIT_BURST")
            .or_else(|_| std::env::var("DEEPSEEK_RATE_LIMIT_BURST"))
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

    /// Reserve `tokens` and report how long the caller must sleep first.
    ///
    /// The debt is *kept* (the balance is allowed to go negative) rather than
    /// floored at zero. Callers release the bucket lock before sleeping, so a
    /// floored balance would hand every queued waiter the same short delay and
    /// they would all wake — and fire — at the same instant, which is the
    /// burst the configured limit exists to prevent. Carrying the deficit
    /// spaces successive waiters one refill interval apart, and `refill`'s
    /// clamp to `capacity` still caps how much credit an idle bucket banks.
    fn delay_until_available(&mut self, tokens: f64) -> Option<Duration> {
        if !self.enabled {
            return None;
        }
        let now = Instant::now();
        self.refill(now);
        self.tokens -= tokens;
        if self.tokens >= 0.0 {
            return None;
        }
        if self.refill_per_sec <= 0.0 {
            return Some(Duration::from_secs(1));
        }
        Some(Duration::from_secs_f64(-self.tokens / self.refill_per_sec))
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
            http1_client: self.http1_client.clone(),
            api_key: self.api_key.clone(),
            model_bound_secret_values: Arc::clone(&self.model_bound_secret_values),
            base_url: self.base_url.clone(),
            api_provider: self.api_provider,
            provider_identity: self.provider_identity.clone(),
            billing_surface: self.billing_surface.clone(),
            billing_mode: self.billing_mode,
            route_limits: self.route_limits,
            codex_account_id: self.codex_account_id.clone(),
            wire_format: self.wire_format,
            retry: self.retry.clone(),
            isolated_request_state: self.isolated_request_state,
            remote_control_inference_participant: self.remote_control_inference_participant,
            default_model: self.default_model.clone(),
            connection_health: self.connection_health.clone(),
            rate_limiter: self.rate_limiter.clone(),
            request_concurrency: self.request_concurrency.clone(),
            path_suffix: self.path_suffix.clone(),
            #[cfg(test)]
            test_chat_transport_base_url: self.test_chat_transport_base_url.clone(),
            #[cfg(test)]
            test_messages_transport_base_url: self.test_messages_transport_base_url.clone(),
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
    if !codewhale_paths::codewhale_home_is_explicit()
        || std::env::var_os("CODEWHALE_SECRET_BACKEND").is_none()
    {
        return;
    }

    // Redaction needs only a best-effort view of inactive file-backed
    // credentials. It must not cause a legacy-store migration merely because a
    // client is being constructed (notably for `doctor`'s live probe). Keep
    // this file-only to avoid a burst of OS-keychain prompts for inactive
    // providers; the active credential is already supplied by the route
    // resolver.
    let secrets = codewhale_secrets::Secrets::file_backed_read_only();
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
    // Tool results feed exact-match edits, so only credential-shaped values
    // are masked here; key-only hits (`password: credentials?.password`) stay
    // byte-exact. Logs and previews keep the broad key-based scrubber.
    codewhale_config::persistence::redact_model_bound_secrets(&redacted)
}

// === Helpers ===

/// Maximum bytes to read from an error response body (64 KB).
pub(super) const ERROR_BODY_MAX_BYTES: usize = 64 * 1024;

/// Read/overall timeout for the shared client's non-streaming requests
/// (`/models` listing, catalog refresh, health probes). Streaming requests
/// keep their own idle-timeout envelope; without this, a provider that accepts
/// the connection and never answers hangs model-list, catalog refresh, and
/// health checks forever (ops R4).
pub(super) const NON_STREAMING_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

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
            .or_else(|_| std::env::var(LEGACY_ALLOW_INSECURE_HTTP_ENV))
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
             set the env var `{ALLOW_INSECURE_HTTP_ENV}=1` in the shell that runs codewhale and re-run.\n\
             \n\
             Example: `{ALLOW_INSECURE_HTTP_ENV}=1 codewhale` (note the underscores).",
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

pub(crate) fn api_url(base_url: &str, path: &str) -> String {
    api_url_with_suffix(base_url, path, None)
}

fn responses_api_url(base_url: &str, provider: ApiProvider) -> String {
    let normalized = base_url.trim_end_matches('/').to_ascii_lowercase();
    let official_deepseek = matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && matches!(
            normalized.as_str(),
            "https://api.deepseek.com"
                | "https://api.deepseek.com/v1"
                | "https://api.deepseek.com/beta"
                | "https://api.deepseeki.com"
                | "https://api.deepseeki.com/v1"
                | "https://api.deepseeki.com/beta"
        );
    if official_deepseek {
        format!("{}/responses", unversioned_base_url(base_url))
    } else {
        api_url(base_url, "responses")
    }
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

/// Route strict DeepSeek tool requests through the beta Chat Completions
/// surface while keeping every ordinary request on the canonical `/v1` path.
///
/// DeepSeek requires its `/beta` base URL when a function opts into
/// `strict: true`. The configured route URL remains semantic here because
/// unit tests may replace only the transport origin with a local capture
/// server.
///
/// Source: <https://api-docs.deepseek.com/guides/tool_calls/> (verified 2026-07-22).
fn chat_completions_url(
    transport_base_url: &str,
    route_base_url: &str,
    provider: ApiProvider,
    path_suffix: Option<&str>,
    body: &Value,
) -> String {
    let uses_deepseek_beta = matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && is_official_deepseek_beta_base_url(route_base_url)
        && body_uses_strict_tools(body)
        && path_suffix.is_none();
    let path = if uses_deepseek_beta {
        "beta/chat/completions"
    } else {
        "chat/completions"
    };
    api_url_with_suffix(transport_base_url, path, path_suffix)
}

fn is_official_deepseek_beta_base_url(base_url: &str) -> bool {
    matches!(
        base_url.trim_end_matches('/').to_ascii_lowercase().as_str(),
        "https://api.deepseek.com/beta" | "https://api.deepseeki.com/beta"
    )
}

fn body_uses_strict_tools(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.pointer("/function/strict").and_then(Value::as_bool) == Some(true))
        })
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

/// Returns true when CODEWHALE_FORCE_HTTP1 (legacy alias: DEEPSEEK_FORCE_HTTP1)
/// is set to a truthy value (`1`, `true`, `yes`, `on`, case-insensitive). Used
/// by `build_http_client` to opt out of HTTP/2 entirely when a provider's edge
/// mishandles long-lived H2 streams (#103). Anything else (unset, `0`,
/// `false`, ...) leaves HTTP/2 on.
pub(crate) fn force_http1_from_env() -> bool {
    std::env::var("CODEWHALE_FORCE_HTTP1")
        .or_else(|_| std::env::var("DEEPSEEK_FORCE_HTTP1"))
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
    fn is_local_ds4_model(&self, model: &str) -> bool {
        self.api_provider == ApiProvider::Custom
            && self.provider_identity.eq_ignore_ascii_case("ds4")
            && crate::config::base_url_uses_local_host(&self.base_url)
            && matches!(
                model.trim().to_ascii_lowercase().as_str(),
                "deepseek-v4-flash" | "deepseek-v4-pro"
            )
    }

    /// DS4 is configured as a named custom route so its endpoint and billing
    /// identity remain exact, but its chat payload deliberately speaks the
    /// first-party DeepSeek reasoning/tool dialect that DS4 implements.
    fn chat_shape_provider(&self, model: &str) -> ApiProvider {
        if self.is_local_ds4_model(model) {
            ApiProvider::Deepseek
        } else {
            self.api_provider
        }
    }

    /// Create a DeepSeek client from CLI configuration.
    pub fn new(config: &Config) -> Result<Self> {
        let api_provider = config.api_provider();
        let model_aware = api_provider.metadata().is_some_and(|provider| {
            provider.wire_policy() == codewhale_config::provider::WirePolicy::ModelAware
        });
        if model_aware {
            let route = crate::route_runtime::resolve_runtime_route(config, api_provider, None)
                .map_err(anyhow::Error::msg)?;
            return Self::from_candidate(&route.config, &route.candidate);
        }
        let default_model = config.default_model();
        let route_limits =
            crate::route_runtime::resolve_runtime_route(config, api_provider, Some(&default_model))
                .ok()
                .and_then(|route| {
                    crate::route_budget::known_route_limits(route.candidate.limits())
                });
        Self::from_parts(
            config.deepseek_base_url(),
            default_model,
            provider_wire_format_for_config(api_provider, Some(config)),
            route_limits,
            config,
        )
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
            candidate.endpoint().base_url.clone(),
            candidate.wire_model_id().as_str().to_string(),
            candidate.protocol(),
            crate::route_budget::known_route_limits(candidate.limits()),
            config,
        )
    }

    /// Shared constructor body for [`Self::new`] and [`Self::from_candidate`].
    ///
    /// `base_url` and `default_model` are the only inputs that differ between
    /// the two entry points; everything else (auth, provider, retry, headers,
    /// timeouts) is derived from `config` so the two paths cannot drift.
    fn from_parts(
        base_url: String,
        default_model: String,
        wire_format: WireFormat,
        route_limits: Option<RouteLimits>,
        config: &Config,
    ) -> Result<Self> {
        let api_provider = config.api_provider();
        let provider_identity = config.provider_identity_for(api_provider);
        let billing_surface = crate::route_billing::billing_surface_for_dispatch(
            Some(config),
            api_provider,
            Some(&base_url),
        )
        .map(str::to_string);
        let billing_mode = crate::route_billing::for_route(config, api_provider).into();
        if api_provider == ApiProvider::OpencodeGo {
            validate_route(api_provider, &default_model).map_err(anyhow::Error::msg)?;
        }
        let (api_key, codex_account_id) = if api_provider == ApiProvider::OpenaiCodex {
            // The official endpoint requires Codex OAuth credentials. A custom
            // endpoint prefers its own configured key, but an explicit
            // `OPENAI_CODEX_ACCESS_TOKEN` still wins (`codex_credentials`
            // checks env before enforcing the official-endpoint consent
            // grant), so existing token-plus-custom-base-url setups keep
            // working. Only when no env token exists does the custom endpoint
            // fall back to the generic provider-scoped key resolver.
            match config.codex_credentials() {
                Ok(credentials) => (credentials.access_token, credentials.account_id),
                Err(error) => {
                    if config.provider_uses_custom_endpoint(ApiProvider::OpenaiCodex) {
                        (config.deepseek_api_key()?, None)
                    } else {
                        return Err(error);
                    }
                }
            }
        } else {
            (config.deepseek_api_key()?, None)
        };
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
            wire_format,
            auth_disabled,
            false,
        )?;
        // Always keep an HTTP/1.1 twin for automatic stream-header fallback
        // when H2 stalls. When CODEWHALE_FORCE_HTTP1 is set, both clients are
        // HTTP/1.1 and the fallback is a no-op retry path.
        let http1_client = Self::build_http_client_with_auth_mode(
            &api_key,
            &http_headers,
            api_provider,
            &base_url,
            wire_format,
            auth_disabled,
            true,
        )?;

        Ok(Self {
            http_client,
            http1_client,
            api_key,
            model_bound_secret_values,
            base_url,
            api_provider,
            provider_identity,
            billing_surface,
            billing_mode,
            route_limits,
            codex_account_id,
            wire_format,
            retry,
            isolated_request_state: false,
            remote_control_inference_participant: !config.runtime_chat_isolated
                && !config.runtime_thread_inference_unrelated,
            default_model,
            connection_health: Arc::new(AsyncMutex::new(ConnectionHealth::default())),
            rate_limiter: Arc::new(AsyncMutex::new(TokenBucket::from_env())),
            request_concurrency: request_concurrency_limit.map(ProviderConcurrencyLimiter::new),
            path_suffix,
            #[cfg(test)]
            test_chat_transport_base_url: None,
            #[cfg(test)]
            test_messages_transport_base_url: None,
            reasoning_stream_style,
            stream_idle_timeout,
        })
    }

    /// Transport destination for Chat Completions requests.
    ///
    /// Production always uses the semantic route base URL. Unit tests may
    /// substitute a local capture server without changing the endpoint/model
    /// identity used by exact-route request shaping.
    pub(super) fn chat_transport_base_url(&self) -> &str {
        #[cfg(test)]
        if let Some(base_url) = self.test_chat_transport_base_url.as_deref() {
            return base_url;
        }
        &self.base_url
    }

    /// Redirect Chat Completions *transport* to a local capture server while
    /// the semantic route (`base_url`, model, endpoint identity) stays exact.
    ///
    /// Test-only, and compiled out of release builds. Route shaping reads
    /// [`Self::base_url`], so an exact-route matrix can capture the real
    /// first-turn body for `api.z.ai`, `api.moonshot.ai`, `api.kimi.com`, or
    /// `api.minimax.io` without ever making a live provider call.
    #[cfg(test)]
    pub(crate) fn set_test_chat_transport_base_url(&mut self, base_url: String) {
        self.test_chat_transport_base_url = Some(base_url);
    }

    /// Transport destination for a prepared Anthropic-compatible request.
    /// Production sends the exact prepared endpoint; tests may redirect the
    /// transport while preserving that immutable endpoint for route shaping.
    pub(super) fn messages_transport_url(&self, prepared_url: &str) -> String {
        #[cfg(test)]
        if let Some(base_url) = self.test_messages_transport_base_url.as_deref() {
            return anthropic::anthropic_messages_url(base_url);
        }
        prepared_url.to_string()
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
        let repair =
            crate::tool_history_repair::repair_tool_call_pairs_for_provider(&mut request.messages);
        if !repair.is_empty() {
            tracing::warn!(
                repaired_call_ids = ?repair.repaired_call_ids,
                duplicate_result_ids = ?repair.duplicate_result_ids,
                orphan_result_ids = ?repair.orphan_result_ids,
                "repaired tool call/result history before provider projection"
            );
        }
        for message in &mut request.messages {
            for block in &mut message.content {
                if let ContentBlock::ToolResult { content, .. } = block {
                    *content = redact_model_bound_text(content, &self.model_bound_secret_values);
                }
            }
        }
        request
    }

    /// Redact configured credentials from text that has been flattened into a
    /// normal model-bound text block. Most requests preserve tool results as
    /// structured blocks and are sanitized by `prepare_model_bound_request`,
    /// but routing/classification prompts intentionally summarize them first.
    pub(crate) fn redact_model_bound_text(&self, text: &str) -> String {
        redact_model_bound_text(text, &self.model_bound_secret_values)
    }

    /// Resolve `model` through the central route resolver and rebuild this
    /// client whenever its exact wire identity, limits, or protocol differs
    /// from the route bound at construction (#5042). `Ok(None)` means the
    /// existing binding is already exact for `model`. This includes
    /// same-protocol switches: an OpenAI-compatible client for model A must not
    /// carry A's route limits into model B merely because both speak Chat.
    pub(crate) fn rebound_for_model_protocol(
        &self,
        config: Option<&Config>,
        model: &str,
    ) -> Result<Option<Self>> {
        static RESOLVER: OnceLock<RouteResolver> = OnceLock::new();
        let candidate = RESOLVER
            .get_or_init(RouteResolver::new)
            .resolve(&RouteRequest {
                explicit_provider: self.api_provider.kind(),
                model_selector: Some(LogicalModelRef::from(model)),
                saved_provider_model: None,
                base_url_override: Some(self.base_url.clone()),
                limit_overrides: Vec::new(),
            })
            .map_err(anyhow::Error::msg)?;
        let candidate_limits = crate::route_budget::known_route_limits(candidate.limits());
        if candidate.protocol() == self.wire_format
            && candidate.wire_model_id().as_str() == self.default_model
            && candidate_limits == self.route_limits
        {
            return Ok(None);
        }
        if candidate.protocol() == self.wire_format {
            // Same-protocol model switches keep the already-authenticated,
            // endpoint-bound transport but must freeze the alternate model's
            // exact wire identity and limits. This path is also what makes a
            // model-aware client usable in embedded/test runtimes that do not
            // retain the original Config after construction.
            let mut rebound = self.clone();
            rebound.default_model = candidate.wire_model_id().as_str().to_string();
            rebound.route_limits = candidate_limits;
            return Ok(Some(rebound));
        }
        let config = config.ok_or_else(|| {
            anyhow::anyhow!(
                "{} model {:?} uses {:?}, but this client is bound to {:?} and no configuration is available to rebuild it",
                self.api_provider.display_name(),
                model,
                candidate.protocol(),
                self.wire_format
            )
        })?;
        Self::from_candidate(config, &candidate).map(Some)
    }

    fn bind_request_to_protocol(
        &self,
        mut request: MessageRequest,
    ) -> Result<(MessageRequest, Option<RouteLimits>)> {
        let model_aware = self.api_provider.metadata().is_some_and(|provider| {
            provider.wire_policy() == codewhale_config::provider::WirePolicy::ModelAware
        });
        if !model_aware && request.model.trim() == self.default_model {
            return Ok((request, self.route_limits));
        }

        static RESOLVER: OnceLock<RouteResolver> = OnceLock::new();
        let candidate = match RESOLVER
            .get_or_init(RouteResolver::new)
            .resolve(&RouteRequest {
                explicit_provider: self.api_provider.kind(),
                model_selector: Some(LogicalModelRef::from(request.model.as_str())),
                saved_provider_model: None,
                base_url_override: Some(self.base_url.clone()),
                limit_overrides: Vec::new(),
            }) {
            Ok(candidate) => candidate,
            Err(error) if model_aware => return Err(anyhow::Error::msg(error)),
            Err(_) => {
                // A fixed-protocol gateway may legitimately accept an id that
                // is newer than our offline catalog. Preserve the caller's
                // model, but fail closed on limits instead of reusing the
                // bound default model's envelope.
                return Ok((request, None));
            }
        };
        if candidate.protocol() != self.wire_format {
            bail!(
                "{} model {:?} uses {:?}, but this client is bound to {:?}; resolve a new model route before sending",
                self.api_provider.display_name(),
                request.model,
                candidate.protocol(),
                self.wire_format
            );
        }
        request.model = candidate.wire_model_id().as_str().to_string();
        let route_limits = crate::route_budget::known_route_limits(candidate.limits());
        Ok((request, route_limits))
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
            provider_default_wire_format(api_provider),
            false,
            false,
        )
    }

    fn build_http_client_with_auth_mode(
        api_key: &str,
        extra_headers: &HashMap<String, String>,
        api_provider: ApiProvider,
        base_url: &str,
        wire_format: WireFormat,
        auth_disabled: bool,
        force_http1: bool,
    ) -> Result<reqwest::Client> {
        let headers = build_default_headers(
            api_key,
            extra_headers,
            api_provider,
            base_url,
            wire_format,
            auth_disabled,
        )?;
        let mut builder = crate::tls::reqwest_client_builder()
            .default_headers(headers)
            .user_agent(client_user_agent(api_provider))
            .connect_timeout(Duration::from_secs(30))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .http2_keep_alive_interval(Some(Duration::from_secs(15)))
            .http2_keep_alive_timeout(Duration::from_secs(20))
            .min_tls_version(reqwest::tls::Version::TLS_1_2);
        let pin_http1 = force_http1 || force_http1_from_env();
        if pin_http1 {
            if force_http1_from_env() && !force_http1 {
                logging::info("CODEWHALE_FORCE_HTTP1=1 — pinning HTTP client to HTTP/1.1");
            }
            builder = builder.http1_only();
        }
        if let Ok(cert_path) = std::env::var("SSL_CERT_FILE")
            && !cert_path.is_empty()
        {
            builder = add_extra_root_certs(builder, &cert_path);
        }
        builder.build().map_err(Into::into)
    }

    /// HTTP/1.1 client for automatic stream-header fallback.
    #[must_use]
    pub(crate) fn http1_fallback_client(&self) -> &reqwest::Client {
        &self.http1_client
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
            WireFormat::ChatCompletions,
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
        build_default_headers(
            api_key,
            extra_headers,
            api_provider,
            base_url,
            provider_default_wire_format(api_provider),
            false,
        )
    }

    #[cfg(test)]
    fn default_headers_for_provider_with_auth_disabled(
        api_key: &str,
        extra_headers: &HashMap<String, String>,
        api_provider: ApiProvider,
        base_url: &str,
    ) -> Result<HeaderMap> {
        build_default_headers(
            api_key,
            extra_headers,
            api_provider,
            base_url,
            provider_default_wire_format(api_provider),
            true,
        )
    }
}

fn build_default_headers(
    api_key: &str,
    extra_headers: &HashMap<String, String>,
    api_provider: ApiProvider,
    base_url: &str,
    wire_format: WireFormat,
    auth_disabled: bool,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let api_key = api_key.trim();
    let uses_anthropic_messages = wire_format == WireFormat::AnthropicMessages;
    if uses_anthropic_messages {
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
        && uses_anthropic_messages
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
    // OpenRouter app attribution: these two headers are how apps appear on
    // openrouter.ai's app rankings — there is no manual submission. They
    // identify the app, never the user, and a user-configured header of the
    // same name below still wins (the extra-header loop overwrites).
    if api_provider == ApiProvider::Openrouter {
        headers.insert(
            HeaderName::from_static("http-referer"),
            HeaderValue::from_static("https://codewhale.net"),
        );
        headers.insert(
            HeaderName::from_static("x-title"),
            HeaderValue::from_static("Codewhale"),
        );
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

fn provider_default_wire_format(api_provider: ApiProvider) -> WireFormat {
    provider_wire_format_for_config(api_provider, None)
}

/// Resolve the wire dialect for a dual-protocol vendor.
///
/// Power-user toggle: `providers.<id>.wire = "openai" | "anthropic" | "responses"`.
/// Legacy dialect kinds (`*Anthropic`) still force Messages. Custom providers
/// honor `wire = "responses" | "anthropic" | "chat"` per-config (see
/// `crates/config/src/provider.rs:Custom`). Everyone else keeps the descriptor's
/// fixed policy (or Chat Completions).
fn provider_wire_format_for_config(
    api_provider: ApiProvider,
    config: Option<&crate::config::Config>,
) -> WireFormat {
    let catalog = api_provider.catalog_identity();
    let wire = config.and_then(|cfg| cfg.provider_wire_dialect(catalog));
    let prefers_anthropic = matches!(
        api_provider,
        ApiProvider::DeepseekAnthropic
            | ApiProvider::MinimaxAnthropic
            | ApiProvider::ModelstudioTokenPlanAnthropic
            | ApiProvider::ModelstudioCodingPlanAnthropic
    ) || wire_config_prefers_anthropic(wire);

    if prefers_anthropic
        && matches!(
            catalog,
            ApiProvider::Deepseek
                | ApiProvider::Minimax
                | ApiProvider::ModelstudioTokenPlan
                | ApiProvider::DeepseekAnthropic
                | ApiProvider::MinimaxAnthropic
                | ApiProvider::ModelstudioTokenPlanAnthropic
                | ApiProvider::ModelstudioCodingPlan
                | ApiProvider::ModelstudioCodingPlanAnthropic
        )
    {
        return WireFormat::AnthropicMessages;
    }

    // Custom providers honor `wire = "anthropic"` / `wire = "responses"` explicitly.
    // The static `Custom::wire_policy()` remains `Chat` as a safe default; the
    // per-config override lives here (and in `provider_capability`) so existing
    // `[providers.<name>]` tables gain the three-way switch without changing the
    // provider registry trait. Supported aliases:
    //   anthropic: "anthropic" | "messages" | "claude" | "anthropic-messages" | ...
    //   responses: "responses" | "responses-api" | "openai-responses" | "openai_responses" | ...
    if api_provider == ApiProvider::Custom {
        if wire_config_prefers_anthropic(wire) {
            return WireFormat::AnthropicMessages;
        }
        if wire_config_prefers_responses(wire) {
            return WireFormat::Responses;
        }
    }

    api_provider
        .kind()
        .and_then(|kind| {
            codewhale_config::provider::provider_for_kind(kind)
                .wire_policy()
                .fixed()
        })
        .unwrap_or_else(|| {
            if api_provider == ApiProvider::OpencodeZen {
                WireFormat::Responses
            } else {
                WireFormat::ChatCompletions
            }
        })
}

fn wire_config_prefers_anthropic(wire: Option<&str>) -> bool {
    let Some(raw) = wire.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let normalized = raw.to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(
        normalized.as_str(),
        "anthropic"
            | "anthropic-messages"
            | "messages"
            | "claude"
            | "anthropic-compatible"
            | "anthropic-compat"
    )
}

fn wire_config_prefers_responses(wire: Option<&str>) -> bool {
    let Some(raw) = wire.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let normalized = raw.to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(
        normalized.as_str(),
        "responses"
            | "responses-api"
            | "openai-responses"
            | "openai-responses-api"
            | "response"
            | "response-api"
            | "openai-responses-compat"
            | "responses-compat"
    )
}

fn api_provider_skips_models_probe(api_provider: ApiProvider) -> bool {
    // Concentrate's `GET /v1/models` is explicitly unauthenticated
    // (docs/PROVIDERS.md): a 2xx proves nothing about the key, so guided
    // setup must not count it as verification — the key is observed on the
    // first authenticated call instead.
    matches!(
        api_provider,
        ApiProvider::DeepseekAnthropic | ApiProvider::Concentrate
    )
}

#[must_use]
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
    let headers = build_default_headers(
        api_key,
        &Default::default(),
        provider,
        base_url,
        provider_default_wire_format(provider),
        false,
    )
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
        // TelecomJS verification already returns the key-scoped model roster.
        // Publish it before returning so the guided model picker can render the
        // live choices in this session instead of requiring a restart. A valid
        // 2xx response remains sufficient to verify the key even if the body is
        // malformed; in that case failure-preserving catalog semantics keep the
        // existing/static rows.
        let body = response.text().await.unwrap_or_default();
        if matches!(provider, ApiProvider::Telecomjs | ApiProvider::Edenai)
            && let Some(kind) = provider.kind()
            && let Ok(offerings) = named_gateway_catalog_offerings_from_body(
                &body,
                kind,
                provider.as_str(),
                &base_url_fingerprint(base_url),
                now_unix(),
            )
        {
            crate::provider_lake::merge_live_offerings(offerings);
        }
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

fn translation_message_request(
    text: &str,
    model: String,
    target_language: &str,
    max_tokens: u32,
) -> MessageRequest {
    MessageRequest {
        model,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }],
        max_tokens,
        system: Some(SystemPrompt::Text(translation_system_prompt(
            target_language,
        ))),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: Some("off".to_string()),
        stream: Some(false),
        temperature: None,
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

    /// Prepare — but do not send — the exact outbound request for `request`.
    ///
    /// This is *the* outbound seam (#1004). Production dispatch
    /// (`create_message`, `create_message_stream`) and `/preview-request` both
    /// call it, so a preview cannot describe a request different from the one
    /// a turn would send.
    ///
    /// It runs, in production order:
    ///
    /// 1. tool-history repair and model-bound secret redaction
    ///    ([`Self::prepare_model_bound_request`]);
    /// 2. protocol binding and route model re-resolution
    ///    ([`Self::bind_request_to_protocol`]);
    /// 3. the dialect's own body builder — Chat Completions, Anthropic
    ///    Messages, or OpenAI Responses — including every provider-specific
    ///    sanitizer and reasoning shaper;
    /// 4. exact endpoint resolution for that dialect and route shape.
    ///
    /// It performs no I/O and mutates no client state.
    pub(crate) fn prepare_outbound_request(
        &self,
        request: MessageRequest,
        stream: bool,
    ) -> Result<PreparedOutboundRequest> {
        // Step 0: refuse role/dialect pairs this wire cannot represent, before
        // any dialect builds a body. Doing it here rather than inside each
        // adapter is what stops an unrepresentable role from being discovered
        // as an opaque provider 400 (Anthropic) or from vanishing silently
        // (the OpenAI-shaped dialects) depending on which adapter ran.
        let outbound_dialect = if self.api_provider == crate::config::ApiProvider::Antigravity {
            WireDialect::GoogleCloudCode
        } else {
            WireDialect::from_wire_format(self.wire_format)
        };
        role_placement::reject_unsupported_roles(&request.messages, outbound_dialect)?;
        let clamp_output_cap = |mut request: MessageRequest, route_limits: Option<RouteLimits>| {
            let route_cap =
                self.effective_max_output_tokens_with_limits(&request.model, route_limits);
            if request.max_tokens > route_cap {
                tracing::debug!(
                    requested_max_tokens = request.max_tokens,
                    route_max_tokens = route_cap,
                    model = %request.model,
                    "clamped outbound max_tokens to the resolved route envelope"
                );
                request.max_tokens = route_cap;
            }
            request
        };
        if self.api_provider == crate::config::ApiProvider::Antigravity {
            let request =
                clamp_output_cap(self.prepare_model_bound_request(request), self.route_limits);
            let body = cloud_code::build_generate_content_body(&request)?;
            let url = cloud_code::stream_generate_content_url(&self.base_url);
            return Ok(PreparedOutboundRequest::new(
                WireDialect::GoogleCloudCode,
                self.endpoint_identity(url, RouteShape::CloudCode),
                request.model.clone(),
                body,
                request.reasoning_effort.clone(),
                None,
                CallerStreamMode::from_stream_flag(stream),
            ));
        }
        let (request, request_route_limits) =
            self.bind_request_to_protocol(self.prepare_model_bound_request(request))?;
        let mut request = clamp_output_cap(request, request_route_limits);
        if self.is_local_ds4_model(&request.model)
            && let Some(tools) = request.tools.as_mut()
        {
            for tool in tools {
                tool.strict = None;
            }
        }
        let requested_effort = request.reasoning_effort.clone();
        // Same value computed for the seam above; Antigravity already returned.
        let dialect = outbound_dialect;
        // `stream` is the caller's entry point, not a wire fact: each dialect
        // decides for itself what the body's `stream` field says.
        let entrypoint = CallerStreamMode::from_stream_flag(stream);

        match self.wire_format {
            WireFormat::ChatCompletions => {
                let chat_shape_provider = self.chat_shape_provider(&request.model);
                let wire = chat::build_chat_wire_body(
                    &request,
                    chat_shape_provider,
                    &self.base_url,
                    stream,
                )?;
                let url = chat_completions_url(
                    self.chat_transport_base_url(),
                    &self.base_url,
                    self.api_provider,
                    self.path_suffix.as_deref(),
                    &wire.body,
                );
                let shape = prepared::chat_route_shape(
                    self.api_provider,
                    &self.base_url,
                    &wire.model,
                    &url,
                );
                Ok(PreparedOutboundRequest::new(
                    dialect,
                    self.endpoint_identity(url, shape),
                    wire.model,
                    wire.body,
                    requested_effort,
                    wire.replay_input_tokens,
                    entrypoint,
                )
                .with_omitted_tool_names(wire.omitted_tool_names))
            }
            WireFormat::AnthropicMessages => {
                let body = self.build_anthropic_body(&request, stream);
                let url = anthropic::anthropic_messages_url(&self.base_url);
                let shape = if self.api_provider == ApiProvider::OpencodeZen {
                    RouteShape::OpencodeZen
                } else if self.api_provider == ApiProvider::Custom {
                    RouteShape::CustomCompatible
                } else {
                    RouteShape::Standard
                };
                let wire_model = body
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or(request.model.as_str())
                    .to_string();
                Ok(PreparedOutboundRequest::new(
                    dialect,
                    self.endpoint_identity(url, shape),
                    wire_model,
                    body,
                    requested_effort,
                    None,
                    entrypoint,
                ))
            }
            WireFormat::Responses => {
                let body =
                    responses::build_responses_body_for_provider(&request, self.api_provider);
                let is_codex = self.api_provider == ApiProvider::OpenaiCodex;
                let url = if is_codex {
                    format!("{}{}", self.base_url, responses::CODEX_RESPONSES_PATH)
                } else {
                    responses_api_url(&self.base_url, self.api_provider)
                };
                let shape = if is_codex {
                    RouteShape::CodexResponses
                } else if self.api_provider == ApiProvider::OpencodeZen {
                    RouteShape::OpencodeZen
                } else if self.api_provider == ApiProvider::Custom {
                    RouteShape::CustomCompatible
                } else {
                    RouteShape::Standard
                };
                let wire_model = body
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or(request.model.as_str())
                    .to_string();
                Ok(PreparedOutboundRequest::new(
                    dialect,
                    self.endpoint_identity(url, shape),
                    wire_model,
                    body,
                    requested_effort,
                    None,
                    entrypoint,
                ))
            }
        }
    }

    /// Typed identity of the endpoint this client would POST to.
    ///
    /// `route_id` is left empty here on purpose: the client knows the provider
    /// and the URL, but only the caller's resolved turn plan knows whether the
    /// user reached this route through a named custom-provider entry. The
    /// engine attaches it with [`PreparedOutboundRequest::with_route_id`].
    fn endpoint_identity(&self, url: String, shape: RouteShape) -> EndpointIdentity {
        EndpointIdentity {
            provider_id: self.api_provider.as_str().to_string(),
            provider_display: self.api_provider.display_name().to_string(),
            route_id: None,
            url,
            shape,
        }
    }

    /// Returns the active API provider for this client.
    pub fn api_provider(&self) -> ApiProvider {
        self.api_provider
    }

    /// Route limits frozen with this client at resolution time.
    #[must_use]
    pub fn route_limits(&self) -> Option<RouteLimits> {
        self.route_limits
    }

    /// Output cap for a request dispatched by this exact client route.
    #[must_use]
    pub fn effective_max_output_tokens(&self, requested_model: &str) -> u32 {
        let route_limits = if requested_model.trim() == self.default_model {
            self.route_limits
        } else {
            static RESOLVER: OnceLock<RouteResolver> = OnceLock::new();
            RESOLVER
                .get_or_init(RouteResolver::new)
                .resolve(&RouteRequest {
                    explicit_provider: self.api_provider.kind(),
                    model_selector: Some(LogicalModelRef::from(requested_model)),
                    saved_provider_model: None,
                    base_url_override: Some(self.base_url.clone()),
                    limit_overrides: Vec::new(),
                })
                .ok()
                .and_then(|candidate| crate::route_budget::known_route_limits(candidate.limits()))
        };
        self.effective_max_output_tokens_with_limits(requested_model, route_limits)
    }

    #[must_use]
    fn effective_max_output_tokens_with_limits(
        &self,
        requested_model: &str,
        route_limits: Option<RouteLimits>,
    ) -> u32 {
        let wire_model =
            wire_model_for_provider_route(self.api_provider, &self.base_url, requested_model);
        crate::route_budget::effective_max_output_tokens_for_route(
            self.api_provider,
            &wire_model,
            route_limits,
        )
    }

    /// Secret-free receipt for the exact base endpoint and credential
    /// generation this client was constructed with.
    ///
    /// This is the only way the API key leaves `client.rs`, and it leaves as a
    /// one-way digest. Minting the receipt here — rather than re-reading config
    /// at some later lifecycle point — is what makes it immutable proof of the
    /// route that was actually installed for the turn.
    #[must_use]
    pub fn turn_route_receipt(
        &self,
        provider_identity: &str,
    ) -> crate::route_receipt::TurnRouteReceipt {
        crate::route_receipt::TurnRouteReceipt::new(
            self.api_provider,
            provider_identity,
            &self.default_model,
            &self.base_url,
            &self.api_key,
        )
    }

    /// Capture the immutable, redacted route envelope for a request immediately
    /// before it is dispatched. The wire model is normalized exactly as the
    /// transport will normalize it; a provider-returned alias must never replace
    /// this billing identity later.
    #[must_use]
    pub fn effective_route_envelope(
        &self,
        requested_model: &str,
        dispatched_at: chrono::DateTime<chrono::Utc>,
    ) -> crate::cost_status::EffectiveRouteEnvelope {
        let model =
            wire_model_for_provider_route(self.api_provider, &self.base_url, requested_model);
        crate::cost_status::EffectiveRouteEnvelope {
            provider: self.api_provider,
            provider_identity: self.provider_identity.clone(),
            model,
            billing_surface: self.billing_surface.clone(),
            endpoint_fingerprint: crate::cost_status::endpoint_fingerprint(&self.base_url),
            billing_mode: self.billing_mode,
            dispatched_at,
        }
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

    pub(crate) async fn acquire_remote_control_inference_permit(
        &self,
    ) -> Option<RemoteControlInferencePermit> {
        if !self.remote_control_inference_participant {
            return None;
        }
        Some(acquire_remote_control_inference_participant().await)
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

    fn prepend_tool_projection_warning(
        stream: crate::llm_client::StreamEventBox,
        provider: String,
        omitted_tool_names: Vec<String>,
        omitted_tool_count: usize,
    ) -> crate::llm_client::StreamEventBox {
        Box::pin(async_stream::stream! {
            yield Ok(crate::models::StreamEvent::ToolProjectionWarning {
                provider,
                omitted_tool_names,
                omitted_tool_count,
            });
            let mut stream = stream;
            while let Some(event) = stream.next().await {
                yield event;
            }
        })
    }

    fn hold_remote_control_inference_permit_for_stream(
        stream: crate::llm_client::StreamEventBox,
        permit: Option<RemoteControlInferencePermit>,
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
        let _inference = self.acquire_remote_control_inference_permit().await;
        let _permit = self.acquire_provider_request_permit().await;
        let model = wire_model_for_provider_route(self.api_provider, &self.base_url, model);
        let max_tokens = self.effective_max_output_tokens(&model);
        if self.wire_format != WireFormat::ChatCompletions {
            // Non-Chat dialects reuse the prepared-request seam so translation
            // cannot drift from production shaping. Translation is still an
            // *auxiliary* call, not a primary agent turn: the Chat dialect
            // below builds its own small fixed body, and `/preview-request`
            // deliberately does not claim to describe either
            // (see `docs/PREVIEW_REQUEST.md`).
            let prepared = self.prepare_outbound_request(
                translation_message_request(text, model, target_language, max_tokens),
                false,
            )?;
            let response = match prepared.dialect {
                WireDialect::OpenAiResponses => self.handle_responses_message(&prepared).await?,
                WireDialect::AnthropicMessages => self.handle_anthropic_message(&prepared).await?,
                WireDialect::ChatCompletions | WireDialect::GoogleCloudCode => unreachable!(),
            };
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
            "max_tokens": max_tokens,
            "stream": false
        });
        chat::apply_route_reasoning_controls(
            &mut body,
            self.api_provider,
            &self.base_url,
            &model,
            Some("off"),
        );

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
        let response = self
            .send_with_retry(|| {
                self.http_client
                    .get(&url)
                    .timeout(NON_STREAMING_HTTP_TIMEOUT)
            })
            .await?;

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
            .timeout(NON_STREAMING_HTTP_TIMEOUT)
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
        } else if provider == "telecomjs" {
            named_gateway_catalog_offerings_from_body(
                &body,
                codewhale_config::ProviderKind::Telecomjs,
                &provider,
                &fingerprint,
                fetched_at,
            )?
        } else if provider == "edenai" {
            named_gateway_catalog_offerings_from_body(
                &body,
                codewhale_config::ProviderKind::Edenai,
                &provider,
                &fingerprint,
                fetched_at,
            )?
        } else if provider == "concentrate" {
            // Concentrate's unauthenticated `GET /v1/models` is the same
            // OpenAI list shape (`{"object":"list","data":[{"id":..}]}`);
            // rows stay unclaimed unless a same-provider bundled row exists.
            named_gateway_catalog_offerings_from_body(
                &body,
                codewhale_config::ProviderKind::Concentrate,
                &provider,
                &fingerprint,
                fetched_at,
            )?
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
                    attachment: None,
                    reasoning: None,
                    tool_call: None,
                    structured_output: None,
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

    /// Best-effort background refresh of the active provider's own `/v1/models`
    /// catalog, merging results into the provider lake (#3385).
    ///
    /// Unlike `models_dev_live::spawn_background_refresh` (which fetches the
    /// cross-provider Models.dev catalog), this calls the provider's own
    /// `/v1/models` endpoint and merges the results into the existing live
    /// snapshot via `provider_lake::merge_live_offerings`, preserving rows
    /// from other sources.
    ///
    /// Activated for providers whose model list is not covered by the
    /// Models.dev catalog (TelecomJS TokenHub, Eden AI, Concentrate, local
    /// Ollama).
    /// Ollama's OpenAI-compat `GET /v1/models` returns the same tags as
    /// native `GET /api/tags`. The refresh is non-fatal: on failure,
    /// existing/bundled rows remain available.
    pub fn spawn_active_provider_catalog_refresh(config: &Config) {
        let provider = config.api_provider();
        // Only refresh for providers that serve their own model list and are
        // not already covered by the Models.dev catalog.
        if !matches!(
            provider,
            ApiProvider::Telecomjs
                | ApiProvider::Edenai
                | ApiProvider::Concentrate
                | ApiProvider::Ollama
        ) {
            return;
        }

        let client = match DeepSeekClient::new(config) {
            Ok(client) => client,
            Err(err) => {
                tracing::debug!(
                    target: "provider_catalog",
                    error = %err,
                    "skipping provider catalog refresh: client creation failed"
                );
                return;
            }
        };

        tokio::spawn(async move {
            match client.fetch_catalog_delta().await {
                Ok(delta) => {
                    let count = delta.offerings.len();
                    crate::provider_lake::merge_live_offerings(delta.offerings);
                    tracing::debug!(
                        target: "provider_catalog",
                        offering_count = count,
                        "provider catalog refresh merged {count} offerings into provider lake"
                    );
                }
                Err(err) => {
                    tracing::debug!(
                        target: "provider_catalog",
                        error = ?err,
                        "provider catalog refresh failed; keeping existing rows"
                    );
                }
            }
        });
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
        let _inference = self.acquire_remote_control_inference_permit().await;
        let _permit = self.acquire_provider_request_permit().await;
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
        let probe = self
            .http_client
            .get(health_url)
            .timeout(NON_STREAMING_HTTP_TIMEOUT)
            .send()
            .await;
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
        if self.isolated_request_state {
            return self.send_with_isolated_retry(build).await;
        }
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

    /// The same bounded transport retry policy without process-global retry
    /// banners, provider-wide pause cells, or shared connection-health writes.
    /// Used only by the Auto classifier during read-only request inspection.
    async fn send_with_isolated_retry<F>(&self, mut build: F) -> Result<reqwest::Response>
    where
        F: FnMut() -> reqwest::RequestBuilder,
    {
        let retry_cfg: LlmRetryConfig = self.retry.clone().into();
        let request_result = with_retry(
            &retry_cfg,
            || {
                let request = build();
                async move {
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
                let (reason_label, _) = retry_reason_label_and_human(err);
                logging::warn(format!(
                    "Isolated HTTP retry reason={} attempt={} delay={:.2}s",
                    reason_label,
                    attempt + 1,
                    delay.as_secs_f64(),
                ));
            })),
        )
        .await;

        request_result.map_err(|err| anyhow::Error::new(err.last_error))
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

/// Record that a request was routed to `provider` and came back with `status`.
///
/// Called at every provider response site, **before** the error is built: an
/// `LlmError` carries the raw provider body verbatim, so the status class has
/// to be taken from the response itself.
///
/// The provider is recorded as a `ProviderKind` by value. Every accessor that
/// looks like the natural seam here — the persistence identity, the stream
/// meta's `provider_id`, the planned route's effective label — returns the
/// customer's own `[providers.<name>]` table key when the route is custom.
/// `ProviderKind::Custom` yields the literal `"custom"` and nothing else, and
/// no model id is sent for any provider.
pub(crate) fn record_provider_response(provider: crate::config::ApiProvider, status: u16) {
    let counters = codewhale_telemetry::session_counters();
    if let Some(kind) = provider.kind() {
        counters.record_provider(kind);
    }
    if let Some(counter) = codewhale_telemetry::counters::http_status_counter(status) {
        counters.bump_error(counter);
    }
}

/// Translate the structured `LlmError` into both a categorical label
/// (for structured logs / metrics) and a short human reason string
/// (for the retry banner). Returning both from one match avoids the
/// double-classification we had before.
fn retry_reason_label_and_human(err: &LlmError) -> (&'static str, String) {
    // The variant, never the payload. Every `LlmError` variant carries the raw
    // provider HTTP body verbatim, and a 400 from a content filter routinely
    // echoes the prompt.
    if matches!(err, LlmError::NetworkError(_) | LlmError::Timeout(_)) {
        codewhale_telemetry::session_counters()
            .bump_error(codewhale_telemetry::ErrorCounter::NetworkError);
    }
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

impl DeepSeekClient {
    /// Execute a non-streaming request without consulting or updating the
    /// process-global response cache.
    ///
    /// Request previews use this only for Auto's auxiliary router classifier:
    /// the classifier may call its configured provider, but an inspection must
    /// not perturb later production routing through shared cache state.
    pub(crate) async fn create_message_without_response_cache(
        &self,
        request: MessageRequest,
    ) -> Result<MessageResponse> {
        let mut isolated = self.clone();
        isolated.isolated_request_state = true;
        // The ordinary clone shares its provider token bucket so concurrent
        // production calls observe one rate budget. Request inspection is an
        // auxiliary classifier call, however: it must neither consume nor
        // inherit that mutable foreground state.
        isolated.rate_limiter = Arc::new(AsyncMutex::new(TokenBucket::from_env()));
        let _inference = isolated.acquire_remote_control_inference_permit().await;
        let _permit = isolated.acquire_provider_request_permit().await;
        let prepared = isolated.prepare_outbound_request(request, false)?;
        match prepared.dialect {
            WireDialect::OpenAiResponses => isolated.handle_responses_message(&prepared).await,
            WireDialect::AnthropicMessages => isolated.handle_anthropic_message(&prepared).await,
            WireDialect::ChatCompletions => isolated.create_message_chat(&prepared, false).await,
            WireDialect::GoogleCloudCode => anyhow::bail!(
                "Antigravity cloud-code is stream-only; blocking create_message is not implemented"
            ),
        }
    }
}

impl LlmClient for DeepSeekClient {
    fn provider_name(&self) -> &'static str {
        self.api_provider.as_str()
    }

    fn model(&self) -> &str {
        &self.default_model
    }

    fn billing_base_url(&self) -> Option<&str> {
        Some(&self.base_url)
    }

    fn route_limits(&self) -> Option<RouteLimits> {
        DeepSeekClient::route_limits(self)
    }

    fn effective_max_output_tokens(&self, requested_model: &str) -> u32 {
        DeepSeekClient::effective_max_output_tokens(self, requested_model)
    }

    fn effective_route_envelope(
        &self,
        requested_model: &str,
        dispatched_at: chrono::DateTime<chrono::Utc>,
    ) -> crate::cost_status::EffectiveRouteEnvelope {
        DeepSeekClient::effective_route_envelope(self, requested_model, dispatched_at)
    }

    async fn health_check(&self) -> Result<bool> {
        if api_provider_skips_models_probe(self.api_provider) {
            self.mark_request_success().await;
            return Ok(true);
        }
        let health_url = api_url(&self.base_url, "models");
        self.wait_for_rate_limit().await;
        let response = self
            .http_client
            .get(health_url)
            .timeout(NON_STREAMING_HTTP_TIMEOUT)
            .send()
            .await;
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
        let _inference = self.acquire_remote_control_inference_permit().await;
        let _permit = self.acquire_provider_request_permit().await;
        // Cacheability is a property of the caller's request, not of the wire
        // body, so it is read before the request is consumed by the seam.
        let cacheable = crate::llm_response_cache::request_is_cacheable(&request);
        let prepared = self.prepare_outbound_request(request, false)?;
        match prepared.dialect {
            WireDialect::OpenAiResponses => self.handle_responses_message(&prepared).await,
            WireDialect::AnthropicMessages => self.handle_anthropic_message(&prepared).await,
            WireDialect::ChatCompletions => self.create_message_chat(&prepared, cacheable).await,
            WireDialect::GoogleCloudCode => anyhow::bail!(
                "Antigravity cloud-code is stream-only; blocking create_message is not implemented"
            ),
        }
    }

    async fn create_message_stream(
        &self,
        request: MessageRequest,
    ) -> Result<crate::llm_client::StreamEventBox> {
        let inference = self.acquire_remote_control_inference_permit().await;
        let permit = self.acquire_provider_request_permit().await;
        let prepared = self.prepare_outbound_request(request, true)?;
        if self.api_provider == crate::config::ApiProvider::Antigravity {
            let stream = Self::hold_provider_request_permit_for_stream(
                self.handle_cloud_code_stream(&prepared).await?,
                permit,
            );
            return Ok(Self::hold_remote_control_inference_permit_for_stream(
                stream, inference,
            ));
        }
        let projection_warning = (!prepared.omitted_tool_names.is_empty()).then(|| {
            let omitted_tool_count = prepared.omitted_tool_names.len();
            (
                prepared.endpoint.provider_display.clone(),
                crate::core::events::bounded_tool_projection_warning_names(
                    &prepared.omitted_tool_names,
                ),
                omitted_tool_count,
            )
        });
        let stream = match prepared.dialect {
            WireDialect::OpenAiResponses => self.handle_responses_stream(&prepared).await?,
            WireDialect::AnthropicMessages => self.handle_anthropic_stream(&prepared).await?,
            WireDialect::ChatCompletions => self.handle_chat_completion_stream(prepared).await?,
            WireDialect::GoogleCloudCode => {
                unreachable!("Antigravity streams before dialect match")
            }
        };
        let stream = match projection_warning {
            Some((provider, omitted_tool_names, omitted_tool_count)) => {
                Self::prepend_tool_projection_warning(
                    stream,
                    provider,
                    omitted_tool_names,
                    omitted_tool_count,
                )
            }
            None => stream,
        };
        let stream = Self::hold_provider_request_permit_for_stream(stream, permit);
        Ok(Self::hold_remote_control_inference_permit_for_stream(
            stream, inference,
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
    /// Per-token cache-write (cache-creation) price. OpenRouter publishes this
    /// for the upstreams that charge a write premium (Anthropic, Qwen, …);
    /// dropping it undercounted every cache-creation turn on those routes.
    #[serde(default)]
    input_cache_write: Option<String>,
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

/// Convert a named gateway's `/models` response into truthful provider-scoped
/// catalog rows. Matching model ids on other providers prove no capabilities,
/// limits, or prices; only an explicit same-provider bundled row may enrich a
/// live offering.
fn named_gateway_catalog_offerings_from_body(
    body: &str,
    kind: codewhale_config::ProviderKind,
    provider: &str,
    fingerprint: &str,
    fetched_at: u64,
) -> Result<Vec<CatalogOffering>, CatalogRefreshError> {
    let models = parse_models_response(body).map_err(|_| CatalogRefreshError::InvalidResponse)?;
    if models.is_empty() {
        return Err(CatalogRefreshError::EmptyList);
    }

    let bundled = codewhale_config::catalog::bundled_catalog_offerings();
    let default_model_id = kind.provider().default_model();
    Ok(models
        .into_iter()
        .map(|model| {
            let is_default = model.id.eq_ignore_ascii_case(default_model_id);
            let same_provider_match = bundled.iter().find(|offering| {
                offering.provider.eq_ignore_ascii_case(provider)
                    && offering.wire_model_id.eq_ignore_ascii_case(&model.id)
            });
            if let Some(matched) = same_provider_match {
                CatalogOffering {
                    provider: provider.to_string(),
                    wire_model_id: model.id,
                    canonical_model: matched.canonical_model.clone(),
                    endpoint_key: "chat".to_string(),
                    default_for_provider: is_default,
                    family: matched.family.clone(),
                    limit: matched.limit.clone(),
                    cost: matched.cost.clone(),
                    modalities: matched.modalities.clone(),
                    attachment: matched.attachment,
                    reasoning: matched.reasoning,
                    tool_call: matched.tool_call,
                    structured_output: matched.structured_output,
                    reasoning_options: matched.reasoning_options.clone(),
                    source: CatalogSource::Live {
                        base_url_fingerprint: fingerprint.to_string(),
                        fetched_at,
                    },
                }
            } else {
                CatalogOffering {
                    provider: provider.to_string(),
                    wire_model_id: model.id,
                    canonical_model: None,
                    endpoint_key: "chat".to_string(),
                    default_for_provider: is_default,
                    family: None,
                    limit: None,
                    cost: None,
                    modalities: None,
                    attachment: None,
                    reasoning: None,
                    tool_call: None,
                    structured_output: None,
                    reasoning_options: Vec::new(),
                    source: CatalogSource::Live {
                        base_url_fingerprint: fingerprint.to_string(),
                        fetched_at,
                    },
                }
            }
        })
        .collect())
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
    // after TTL expiry or a failed refresh (#4139). An empty cache publishes
    // nothing: it must not erase a provider-scoped layer populated by another
    // refresh path.
    let offerings = cache.all_visible_offerings(now_unix());
    if !offerings.is_empty() {
        crate::provider_lake::set_live_snapshot(
            CatalogSnapshot { offerings },
            crate::provider_lake::LiveSource::PerProvider,
        );
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
        // OpenRouter quotes per-token USD strings; ModelsDevCost is per million.
        let parse_price = |s: &Option<String>| -> Option<f64> {
            s.as_ref()
                .and_then(|v| v.parse::<f64>().ok())
                .map(|price_per_token| price_per_token * 1_000_000.0)
        };
        ModelsDevCost {
            input: parse_price(&p.prompt),
            output: parse_price(&p.completion),
            cache_read: parse_price(&p.input_cache_read),
            cache_write: parse_price(&p.input_cache_write),
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
        attachment: None,
        reasoning,
        tool_call,
        structured_output: None,
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

/// Write DeepSeek's Chat Completions thinking controls from the shared tier
/// table.
///
/// The table (`client::deepseek_effort`) is the only place the tier ladder is
/// written down; this function is just the Chat wire's spelling of it. An
/// effort string the table does not name is not a DeepSeek tier request, so
/// nothing is written rather than guessing a field the user did not ask for.
fn apply_deepseek_chat_reasoning_effort(body: &mut Value, normalized: &str) {
    let Some(tier) = deepseek_effort::deepseek_effort_tier(normalized) else {
        return;
    };
    if let Some(value) = tier.chat_reasoning_effort() {
        body["reasoning_effort"] = json!(value);
    }
    body["thinking"] = json!({
        "type": if tier.chat_thinking_enabled() { "enabled" } else { "disabled" },
    });
}

pub(super) fn apply_reasoning_effort(
    body: &mut Value,
    effort: Option<&str>,
    provider: ApiProvider,
) {
    let Some(effort) = effort else {
        return;
    };
    let normalized = effort.trim().to_ascii_lowercase();
    // DeepSeek's first-party routes read their tier ladder from the one
    // annotated table (`client::deepseek_effort`), shared with the Responses
    // wire, so a documented mapping change is a single edit. Every other
    // provider keeps its own dialect below.
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        apply_deepseek_chat_reasoning_effort(body, &normalized);
        return;
    }
    match normalized.as_str() {
        "off" | "disabled" | "none" | "false" => match provider {
            // Handled by the shared DeepSeek table above, before this match.
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {}
            ApiProvider::Openrouter
            | ApiProvider::Orcarouter
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
            // TelecomJS TokenHub: the gateway's OpenAI Chat Completions API
            // (POST /v1/chat/completions) does not document `reasoning_effort`
            // or `thinking` as supported parameters. The `thinking` field is
            // only available on the Anthropic Messages API (POST /v1/messages)
            // with a different shape ({"type":"enabled","budget_tokens":N}).
            // Since CodeWhale routes TelecomJS through the Chat Completions
            // path, we must NOT inject these fields — the gateway may silently
            // ignore them or reject the request, and not every gateway model
            // (qwen-max, deepseek-chat, gpt-4o, claude, etc.) accepts the same
            // reasoning dialect (#4188 review: verify against actual behavior).
            ApiProvider::Telecomjs => {}
            // Eden AI documents `thinking` only for Anthropic Claude models.
            // This gateway can route unrelated model families, so the generic
            // provider must not inject a model-specific reasoning dialect.
            ApiProvider::Edenai => {}
            // Concentrate rides the Responses wire (`reasoning.effort`), never
            // these Chat Completions controls.
            ApiProvider::Concentrate => {}
            // Model Studio (DashScope): its top-level controls are route- AND
            // model-specific, so the provider enum alone cannot decide them —
            // a custom `base_url` on the same identity is an arbitrary
            // gateway. `apply_modelstudio_route_reasoning_controls` in
            // client::chat is the sole writer; it strips these fields for all
            // four variants and re-adds them only on a verified Alibaba host.
            // Source: <https://www.alibabacloud.com/help/en/model-studio/deep-thinking>
            ApiProvider::ModelstudioTokenPlan
            | ApiProvider::ModelstudioTokenPlanAnthropic
            | ApiProvider::ModelstudioCodingPlan
            | ApiProvider::ModelstudioCodingPlanAnthropic => {}
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
            ApiProvider::OllamaCloud => {
                // Ollama Cloud stays on the documented OpenAI-compatible
                // `/v1/chat/completions` wire. Native `/api/chat` uses
                // `think`; this wire uses `reasoning_effort`.
                body["reasoning_effort"] = json!("none");
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
            ApiProvider::Minimax => {}
            ApiProvider::Stepfun => {}
            ApiProvider::Sakana => {}
            ApiProvider::LongCat => {}
            ApiProvider::OpencodeGo | ApiProvider::OpencodeZen => {}
            ApiProvider::Meta => {}
            ApiProvider::Xai => {}
            ApiProvider::Mistral => {}
            ApiProvider::Google => {}
            ApiProvider::Antigravity => {}
        },
        "low" | "minimal" | "medium" | "mid" | "high" | "" => match provider {
            // Handled by the shared DeepSeek table above, before this match.
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {}
            // DeepSeek-compatible hosted routes: low/medium both map to high.
            // Their own wire contracts are not verified here, so the historic
            // collapse stays rather than inventing unsupported wire values.
            ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Sglang
            | ApiProvider::Volcengine
            | ApiProvider::Deepinfra
            | ApiProvider::Atlascloud => {
                body["reasoning_effort"] = json!("high");
                body["thinking"] = json!({ "type": "enabled" });
            }
            // TelecomJS: see comment in the "off" branch above — the gateway's
            // Chat Completions API does not support reasoning_effort or thinking.
            ApiProvider::Telecomjs => {}
            ApiProvider::Edenai => {}
            // Concentrate rides the Responses wire (`reasoning.effort`), never
            // these Chat Completions controls.
            ApiProvider::Concentrate => {}
            // Model Studio: see the "off" branch — the route- and model-aware
            // shaper in client::chat is the sole writer of these fields.
            ApiProvider::ModelstudioTokenPlan
            | ApiProvider::ModelstudioTokenPlanAnthropic
            | ApiProvider::ModelstudioCodingPlan
            | ApiProvider::ModelstudioCodingPlanAnthropic => {}
            // OpenRouter/OrcaRouter/Novita/Together: pass through the actual
            // user-chosen value. OpenRouter's unified scale is
            // none/minimal/low/medium/high/xhigh; DeepSeek models hosted there
            // accept those directly.
            ApiProvider::Openrouter
            | ApiProvider::Orcarouter
            | ApiProvider::Novita
            | ApiProvider::Together => {
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
            ApiProvider::OllamaCloud => {
                let value = match normalized.as_str() {
                    "low" | "minimal" => "low",
                    "medium" | "mid" => "medium",
                    _ => "high",
                };
                body["reasoning_effort"] = json!(value);
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
            ApiProvider::Minimax => {}
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
            ApiProvider::Mistral => {}
            ApiProvider::Google => {}
            ApiProvider::Antigravity => {}
        },
        "xhigh" | "max" | "highest" | "ultra" | "ultracode" => match provider {
            // Handled by the shared DeepSeek table above, before this match.
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {}
            ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Sglang
            | ApiProvider::Volcengine
            | ApiProvider::Deepinfra
            | ApiProvider::Atlascloud => {
                body["reasoning_effort"] = json!("max");
                body["thinking"] = json!({ "type": "enabled" });
            }
            // TelecomJS: see comment in the "off" branch above — the gateway's
            // Chat Completions API does not support reasoning_effort or thinking.
            ApiProvider::Telecomjs => {}
            ApiProvider::Edenai => {}
            // Concentrate rides the Responses wire (`reasoning.effort`), never
            // these Chat Completions controls.
            ApiProvider::Concentrate => {}
            // Model Studio: see the "off" branch — the route- and model-aware
            // shaper in client::chat is the sole writer of these fields.
            ApiProvider::ModelstudioTokenPlan
            | ApiProvider::ModelstudioTokenPlanAnthropic
            | ApiProvider::ModelstudioCodingPlan
            | ApiProvider::ModelstudioCodingPlanAnthropic => {}
            ApiProvider::Openrouter
            | ApiProvider::Orcarouter
            | ApiProvider::Novita
            | ApiProvider::Together => {
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
            ApiProvider::OllamaCloud => {
                body["reasoning_effort"] = json!("max");
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
            ApiProvider::Minimax => {}
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
            ApiProvider::Mistral => {}
            ApiProvider::Google => {}
            ApiProvider::Antigravity => {}
        },
        _ => {}
    }
}

pub(super) fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
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
        .map(saturating_u32);
    let prompt_cache_miss_tokens = usage
        .and_then(|u| u.get("prompt_cache_miss_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| prompt_cache_hit_tokens.map(|hit| input_tokens.saturating_sub(u64::from(hit))))
        .map(saturating_u32);
    // Reasoning tokens are a *subset* of the completion count every provider
    // bills, so they are never added to output. A payload claiming more
    // reasoning than output contradicts that invariant, which makes the figure
    // invalid telemetry rather than extra billable output: drop it instead of
    // letting a bad number reach the cost surfaces (#4318).
    let reasoning_tokens = reasoning_tokens_raw
        .filter(|reasoning| *reasoning <= output_tokens)
        .map(saturating_u32);

    let server_tool_use = usage.and_then(|u| u.get("server_tool_use")).map(|server| {
        let code_execution_requests = server
            .get("code_execution_requests")
            .and_then(Value::as_u64)
            .map(saturating_u32);
        let tool_search_requests = server
            .get("tool_search_requests")
            .and_then(Value::as_u64)
            .map(saturating_u32);
        ServerToolUsage {
            code_execution_requests,
            tool_search_requests,
        }
    });

    Usage {
        input_tokens: saturating_u32(input_tokens),
        output_tokens: saturating_u32(output_tokens),
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
        let _inference = self.acquire_remote_control_inference_permit().await;
        let _permit = self.acquire_provider_request_permit().await;
        if self.api_provider == ApiProvider::OpencodeZen
            || self.wire_format != WireFormat::ChatCompletions
        {
            bail!(
                "FIM completion is not supported for {} because the route has no proven FIM wire contract ({:?})",
                self.api_provider.display_name(),
                self.wire_format
            );
        }
        let url = api_url_with_suffix(&self.base_url, "beta/completions", None);
        let model = wire_model_for_provider_route(self.api_provider, &self.base_url, model);
        let max_tokens = max_tokens.min(self.effective_max_output_tokens(&model));
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
pub(crate) mod cloud_code;
mod deepseek_effort;
#[cfg(test)]
mod ds4_tests;
mod prepared;
mod provider_native_search;
mod responses;
mod role_placement;
mod stream_entry;

#[cfg(test)]
pub(crate) fn anthropic_tool_result_content_for_test(
    content: &str,
    content_blocks: Option<&[Value]>,
) -> Value {
    anthropic::anthropic_tool_result_content(content, content_blocks)
}

#[cfg(test)]
pub(crate) fn responses_tool_output_for_test(
    content: &str,
    content_blocks: Option<&[Value]>,
) -> Value {
    responses::responses_tool_output(content, content_blocks)
}

#[cfg(test)]
pub(crate) fn chat_messages_for_test(messages: &[crate::models::Message]) -> Vec<Value> {
    chat::build_chat_messages(None, messages, "gpt-4o")
}

fn extract_sse_data_value(line: &str) -> Option<&str> {
    line.strip_prefix("data:")
        .map(|value| value.strip_prefix(' ').unwrap_or(value))
}

/// Genuine invalid UTF-8 in an SSE line (or an unterminated flush).
///
/// HTTP/2 DATA and other transports may split a multi-byte character across
/// chunks. That is not this error: callers must buffer raw bytes until a
/// complete line (or stream end) before decoding. This type is only returned
/// when `str::from_utf8` rejects the assembled bytes. We never substitute
/// U+FFFD — fail closed so garbled CJK cannot enter the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InvalidSseUtf8 {
    valid_up_to: usize,
}

impl std::fmt::Display for InvalidSseUtf8 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid UTF-8 in SSE stream at byte {}",
            self.valid_up_to
        )
    }
}

impl std::error::Error for InvalidSseUtf8 {}

/// Decode one assembled SSE line (or stream-end tail) with `str::from_utf8`.
/// Does not substitute U+FFFD.
fn decode_sse_line_bytes(bytes: &[u8]) -> Result<&str, InvalidSseUtf8> {
    std::str::from_utf8(bytes).map_err(|err| InvalidSseUtf8 {
        valid_up_to: err.valid_up_to(),
    })
}

/// Take the next COMPLETE line (up to the first `\n`) off a raw byte buffer,
/// draining it, and return it trimmed. Returns `Ok(None)` when no full line is
/// buffered yet. Decoding only complete lines (never an arbitrary network-read
/// boundary) means a multi-byte UTF-8 char — CJK, emoji, accented letter —
/// split across two reads is never corrupted to U+FFFD, since the `\n`
/// delimiter is ASCII and can never fall inside a multi-byte sequence.
///
/// Genuine invalid bytes fail closed (`Err(InvalidSseUtf8)`); we do not
/// substitute U+FFFD.
fn take_sse_line(buffer: &mut Vec<u8>) -> Result<Option<String>, InvalidSseUtf8> {
    let Some(line_end) = buffer.iter().position(|&b| b == b'\n') else {
        return Ok(None);
    };
    // Strip a preceding `\r` so CRLF-delimited SSE frames do not leave CR.
    let mut end = line_end;
    if end > 0 && buffer[end - 1] == b'\r' {
        end -= 1;
    }
    let decoded = decode_sse_line_bytes(&buffer[..end]).map(|text| text.trim().to_string());
    buffer.drain(..=line_end);
    decoded.map(Some)
}

/// Decode the unterminated tail left in `buffer` at stream end.
///
/// Same fail-closed UTF-8 contract as [`take_sse_line`]. Empty / whitespace-only
/// tails yield `Ok(None)`.
fn flush_sse_line(buffer: &mut Vec<u8>) -> Result<Option<String>, InvalidSseUtf8> {
    if buffer.is_empty() {
        return Ok(None);
    }
    let mut end = buffer.len();
    if buffer[end - 1] == b'\r' {
        end -= 1;
    }
    let decoded = decode_sse_line_bytes(&buffer[..end]).map(|text| text.trim().to_string());
    buffer.clear();
    decoded.map(|line| (!line.is_empty()).then_some(line))
}

/// Next decoded SSE line. When `at_end` is false, wait for `\n`. When `at_end`
/// is true, also flush an unterminated tail (stream closed).
fn next_sse_line(buffer: &mut Vec<u8>, at_end: bool) -> Result<Option<String>, InvalidSseUtf8> {
    match take_sse_line(buffer)? {
        Some(line) => Ok(Some(line)),
        None if at_end => flush_sse_line(buffer),
        None => Ok(None),
    }
}

/// Incremental raw-byte SSE line assembler for tests and the Chat Completions
/// decoder. HTTP/2 DATA may split a multi-byte UTF-8 character across chunks;
/// we never decode until a complete line or [`SseLineDecoder::finish`].
#[cfg(test)]
struct SseLineDecoder {
    buffer: Vec<u8>,
}

#[cfg(test)]
impl SseLineDecoder {
    fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, InvalidSseUtf8> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(line) = take_sse_line(&mut self.buffer)? {
            lines.push(line);
        }
        Ok(lines)
    }

    fn finish(mut self) -> Result<Option<String>, InvalidSseUtf8> {
        flush_sse_line(&mut self.buffer)
    }
}

pub(crate) use chat::{CacheWarmupKey, PromptInspection};
pub(crate) use prepared::{
    CallerStreamMode, EndpointIdentity, PreparedOutboundRequest, RouteShape, WireBodyView,
    WireDialect, canonical_json,
};
pub(crate) use provider_native_search::{ProviderNativeSearchClient, ProviderNativeSearchRequest};

/// Whether a route speaks ordinary `/chat/completions` (not Messages or Responses).
#[must_use]
pub(crate) fn provider_speaks_chat_completions(api_provider: ApiProvider) -> bool {
    provider_default_wire_format(api_provider) == WireFormat::ChatCompletions
}

pub(crate) fn inspect_prompt_for_request(request: &MessageRequest) -> PromptInspection {
    chat::inspect_prompt_for_request(request)
}

pub(crate) fn build_cache_warmup_request(request: &MessageRequest) -> MessageRequest {
    chat::build_cache_warmup_request(request)
}

pub(crate) use chat::CACHE_WARMUP_MAX_TOKENS;
pub(crate) use chat::is_reasoning_replay_placeholder;

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
    use crate::config::{
        DEFAULT_CONCENTRATE_BASE_URL, DEFAULT_CONCENTRATE_MODEL, DEFAULT_EDENAI_MODEL,
        DEFAULT_TELECOMJS_MODEL, OPENROUTER_QWEN_3_6_FLASH_MODEL, ProviderConfig, ProvidersConfig,
    };
    use crate::models::{
        ContentBlock, ContentBlockStart, Delta, Message, MessageRequest, MessageResponse,
        StreamEvent, Tool,
    };
    use crate::tools::apply_patch::ApplyPatchTool;
    use crate::tools::spec::ToolSpec;
    use crate::tools::{ToolContext, ToolRegistryBuilder};
    use codewhale_protocol::runtime::DynamicToolSpec;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// OpenRouter app attribution: the two headers that put an app on
    /// openrouter.ai's rankings are sent for OpenRouter routes only, and a
    /// user-configured header of the same name wins.
    #[test]
    fn openrouter_routes_carry_app_attribution_headers() {
        let headers = build_default_headers(
            "sk-or-key",
            &HashMap::new(),
            ApiProvider::Openrouter,
            "https://openrouter.ai/api/v1",
            WireFormat::ChatCompletions,
            false,
        )
        .expect("headers");
        assert_eq!(
            headers.get("http-referer").and_then(|v| v.to_str().ok()),
            Some("https://codewhale.net")
        );
        assert_eq!(
            headers.get("x-title").and_then(|v| v.to_str().ok()),
            Some("Codewhale")
        );

        // Attribution identifies this app to OpenRouter's rankings and has
        // no business on any other route, whichever provider that is.
        for provider in [
            ApiProvider::Deepseek,
            ApiProvider::Moonshot,
            ApiProvider::Zai,
            ApiProvider::Openai,
            ApiProvider::Custom,
        ] {
            let other = build_default_headers(
                "sk-key",
                &HashMap::new(),
                provider,
                "https://example.invalid/v1",
                WireFormat::ChatCompletions,
                false,
            )
            .expect("headers");
            assert!(
                other.get("http-referer").is_none() && other.get("x-title").is_none(),
                "{provider:?} must not receive OpenRouter attribution headers"
            );
        }

        let overridden = build_default_headers(
            "sk-or-key",
            &HashMap::from([("X-Title".to_string(), "My Fork".to_string())]),
            ApiProvider::Openrouter,
            "https://openrouter.ai/api/v1",
            WireFormat::ChatCompletions,
            false,
        )
        .expect("headers");
        assert_eq!(
            overridden.get("x-title").and_then(|v| v.to_str().ok()),
            Some("My Fork")
        );
    }

    #[test]
    fn openrouter_pricing_maps_cache_write_per_token_to_per_million() {
        let payload = r#"{"data":[{
            "id":"anthropic/claude-sonnet-4-6",
            "pricing":{
                "prompt":"0.000003",
                "completion":"0.000015",
                "input_cache_read":"0.0000003",
                "input_cache_write":"0.00000375"
            }
        },{
            "id":"some/no-write-row",
            "pricing":{"prompt":"0.000001","completion":"0.000002"}
        }]}"#;

        let items = parse_openrouter_models_response(payload).expect("parses");
        let priced = openrouter_to_catalog_offering(&items[0], "openrouter", "fp", 42);
        let cost = priced.cost.as_ref().expect("pricing row");
        assert_eq!(cost.input, Some(3.0));
        assert_eq!(cost.output, Some(15.0));
        assert_eq!(cost.cache_read, Some(0.3));
        assert_eq!(cost.cache_write, Some(3.75));

        // A cache-write premium must actually reach the estimator: the same
        // tokens cost more when they are cache-creation rather than cache-read.
        let pricing = codewhale_config::pricing::OfferingPricing::from_catalog_offering(&priced)
            .expect("priced offering");
        let write = codewhale_config::pricing::TokenUsage {
            cache_write: 1_000_000,
            ..Default::default()
        };
        assert_eq!(pricing.estimate_cost(&write), Some(3.75));
        assert!(pricing.unpriced_used_classes(&write).is_empty());

        // A row without a published write rate stays unknown, not zero, and
        // fails closed for cache-creation turns.
        let unwritten = openrouter_to_catalog_offering(&items[1], "openrouter", "fp", 42);
        assert_eq!(
            unwritten.cost.as_ref().and_then(|cost| cost.cache_write),
            None
        );
        let unwritten =
            codewhale_config::pricing::OfferingPricing::from_catalog_offering(&unwritten)
                .expect("priced offering");
        assert_eq!(unwritten.estimate_cost(&write), None);
        assert_eq!(
            unwritten.unpriced_used_classes(&write),
            vec![codewhale_config::pricing::TokenClass::CacheWrite]
        );
    }

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

    fn apply_patch_request_tool() -> Tool {
        let spec = ApplyPatchTool;
        Tool {
            tool_type: None,
            name: spec.name().to_string(),
            description: spec.description().to_string(),
            input_schema: spec.input_schema(),
            allowed_callers: None,
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        }
    }

    fn deferred_dynamic_request_tool() -> Tool {
        let registry = ToolRegistryBuilder::new()
            .with_dynamic_tools(&[DynamicToolSpec {
                namespace: Some("capture".to_string()),
                name: "deferred_lookup".to_string(),
                description: "Look up a record after deferred loading".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "mode": {"type": "string", "const": "fast"},
                        "query": {
                            "anyOf": [
                                {"type": "string"},
                                {"type": "null"}
                            ]
                        }
                    },
                    "required": ["mode"]
                }),
                defer_loading: true,
            }])
            .build(ToolContext::new(
                std::env::temp_dir().join("codewhale-k3-deferred-capture"),
            ));
        registry
            .to_api_tools()
            .into_iter()
            .find(|tool| tool.name == "deferred_lookup")
            .expect("dynamic tool remains model-visible")
    }

    fn value_contains_key(value: &Value, needle: &str) -> bool {
        match value {
            Value::Object(object) => {
                object.contains_key(needle)
                    || object
                        .values()
                        .any(|child| value_contains_key(child, needle))
            }
            Value::Array(values) => values.iter().any(|child| value_contains_key(child, needle)),
            _ => false,
        }
    }

    fn captured_function<'a>(body: &'a Value, name: &str) -> &'a Value {
        body["tools"]
            .as_array()
            .and_then(|tools| tools.iter().find(|tool| tool["function"]["name"] == name))
            .map(|tool| &tool["function"])
            .unwrap_or_else(|| panic!("captured tool catalog is missing {name}: {body}"))
    }

    fn moonshot_request_boundary_client(
        route_base_url: &str,
        model: &str,
        transport_base_url: String,
    ) -> DeepSeekClient {
        let mut client = DeepSeekClient::new(&Config {
            provider: Some("moonshot".to_string()),
            providers: Some(ProvidersConfig {
                moonshot: ProviderConfig {
                    api_key: Some("moonshot-request-boundary-key".to_string()),
                    base_url: Some(route_base_url.to_string()),
                    model: Some(model.to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("Moonshot request-boundary client");
        assert_eq!(client.base_url, route_base_url);
        client.test_chat_transport_base_url = Some(transport_base_url);
        client
    }

    fn zai_request_boundary_client(
        route_base_url: &str,
        model: &str,
        transport_base_url: String,
    ) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut client = DeepSeekClient::new(&Config {
            provider: Some("zai".to_string()),
            providers: Some(ProvidersConfig {
                zai: ProviderConfig {
                    api_key: Some("zai-request-boundary-key".to_string()),
                    base_url: Some(route_base_url.to_string()),
                    model: Some(model.to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("Z.ai request-boundary client");
        assert_eq!(client.base_url, route_base_url);
        client.test_chat_transport_base_url = Some(transport_base_url);
        client
    }

    fn minimax_request_boundary_client(
        route_base_url: &str,
        model: &str,
        transport_base_url: String,
    ) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut client = DeepSeekClient::new(&Config {
            provider: Some("minimax".to_string()),
            providers: Some(ProvidersConfig {
                minimax: ProviderConfig {
                    api_key: Some("minimax-request-boundary-key".to_string()),
                    base_url: Some(route_base_url.to_string()),
                    model: Some(model.to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("MiniMax request-boundary client");
        assert_eq!(client.base_url, route_base_url);
        client.test_chat_transport_base_url = Some(transport_base_url);
        client
    }

    fn deepseek_request_boundary_client(
        route_base_url: &str,
        transport_base_url: String,
    ) -> DeepSeekClient {
        let mut client = DeepSeekClient::new(&Config {
            provider: Some("deepseek".to_string()),
            api_key: Some("deepseek-request-boundary-key".to_string()),
            base_url: Some(route_base_url.to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..Config::default()
        })
        .expect("DeepSeek request-boundary client");
        client.test_chat_transport_base_url = Some(transport_base_url);
        client
    }

    fn ollama_cloud_request_boundary_client(transport_base_url: String) -> DeepSeekClient {
        let mut client = DeepSeekClient::new(&Config {
            provider: Some("ollama-cloud".to_string()),
            providers: Some(ProvidersConfig {
                ollama_cloud: ProviderConfig {
                    api_key: Some("ollama-cloud-request-boundary-key".to_string()),
                    base_url: Some(crate::config::DEFAULT_OLLAMA_CLOUD_BASE_URL.to_string()),
                    model: Some("gpt-oss:120b".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("Ollama Cloud request-boundary client");
        assert_eq!(
            client.base_url,
            crate::config::DEFAULT_OLLAMA_CLOUD_BASE_URL
        );
        client.test_chat_transport_base_url = Some(transport_base_url);
        client
    }

    /// The per-chunk line cap is backpressure relief, not a data budget. When
    /// one transport chunk carries more SSE lines than the cap, the drain loop
    /// stops mid-buffer and the outer loop waits for the *next* chunk before
    /// draining any more — so whatever is still buffered when the stream ends
    /// never reaches the decoder. `flush_sse_line` cannot rescue it: it treats
    /// the whole remainder as one unterminated line. A long stream of small
    /// deltas (or provider heartbeats) therefore loses its tail — the last
    /// tokens, `finish_reason`, and usage — silently.
    #[tokio::test]
    async fn chat_stream_drains_chunks_carrying_more_lines_than_the_per_chunk_cap() {
        // Comment lines are counted by the drain loop and are cheap enough
        // that one transport read holds far more than SSE_MAX_LINES_PER_CHUNK.
        let heartbeats = ": ping\n".repeat(20 * SSE_MAX_LINES_PER_CHUNK * 4);
        let body = format!(
            "{heartbeats}data: {}\n\ndata: [DONE]\n\n",
            json!({
                "choices": [{
                    "index": 0,
                    "delta": {"content": "pong"},
                    "finish_reason": "stop"
                }]
            })
        );

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = deepseek_request_boundary_client("https://api.deepseek.com/v1", server.uri());
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "sse drain regression".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 64,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("off".to_string()),
            stream: Some(true),
            temperature: None,
            top_p: None,
        };

        let mut stream = client
            .create_message_stream(request)
            .await
            .expect("streaming request succeeds");
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            if let StreamEvent::ContentBlockDelta {
                delta: Delta::TextDelta { text: chunk },
                ..
            } = event.expect("heartbeat-padded SSE stays valid")
            {
                text.push_str(&chunk);
            }
        }

        assert_eq!(
            text, "pong",
            "the data frame after the heartbeat flood must still be decoded"
        );
    }

    #[tokio::test]
    async fn chat_stream_eof_without_done_or_finish_reason_is_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(": provider heartbeat\n\n"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = deepseek_request_boundary_client("https://api.deepseek.com/v1", server.uri());
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "premature EOF regression".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 64,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("off".to_string()),
            stream: Some(true),
            temperature: None,
            top_p: None,
        };

        let mut stream = client
            .create_message_stream(request)
            .await
            .expect("HTTP request succeeds before the stream closes");
        let mut saw_stop = false;
        let mut failure = None;
        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::MessageStop) => saw_stop = true,
                Ok(_) => {}
                Err(error) => failure = Some(error.to_string()),
            }
        }

        assert!(
            !saw_stop,
            "premature EOF must not be reported as MessageStop"
        );
        assert!(
            failure
                .as_deref()
                .is_some_and(|message| message.contains("before [DONE] or finish_reason")),
            "premature EOF must remain a typed stream failure: {failure:?}"
        );
    }

    #[tokio::test]
    async fn chat_stream_finish_reason_without_done_is_terminal() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(concat!(
                        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
                        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    )),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = deepseek_request_boundary_client("https://api.deepseek.com/v1", server.uri());
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "finish reason regression".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 64,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("off".to_string()),
            stream: Some(true),
            temperature: None,
            top_p: None,
        };

        let mut stream = client
            .create_message_stream(request)
            .await
            .expect("streaming request succeeds");
        let mut saw_stop = false;
        while let Some(event) = stream.next().await {
            if matches!(
                event.expect("terminal stream stays valid"),
                StreamEvent::MessageStop
            ) {
                saw_stop = true;
            }
        }
        assert!(
            saw_stop,
            "finish_reason is valid terminal proof without [DONE]"
        );
    }

    async fn capture_deepseek_chat_request(
        route_base_url: &str,
        strict: bool,
        streaming: bool,
    ) -> (String, Value) {
        let server = MockServer::start().await;
        let response = if streaming {
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n")
        } else {
            ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-deepseek-request-boundary",
                "object": "chat.completion",
                "model": "deepseek-v4-pro",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2
                }
            }))
        };
        Mock::given(method("POST"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;

        let mut tool = test_tool("lookup");
        if !strict {
            tool.strict = None;
        }
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "provider-free DeepSeek route fixture".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 64,
            system: None,
            tools: Some(vec![tool]),
            tool_choice: Some(json!(if strict { "required" } else { "auto" })),
            metadata: None,
            thinking: None,
            reasoning_effort: Some("off".to_string()),
            stream: Some(streaming),
            temperature: None,
            top_p: None,
        };
        let client = deepseek_request_boundary_client(route_base_url, server.uri());

        if streaming {
            let mut stream = client
                .create_message_stream(request)
                .await
                .expect("streaming request succeeds");
            while let Some(event) = stream.next().await {
                event.expect("captured SSE response remains valid");
            }
        } else {
            client
                .create_message(request)
                .await
                .expect("non-streaming request succeeds");
        }

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        let path = requests[0].url.path().to_string();
        let body = serde_json::from_slice(&requests[0].body).expect("captured request JSON");
        (path, body)
    }

    #[tokio::test]
    async fn core_primary_request_preparation_matches_captured_transport_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let request = codewhale_core::request::prepare_primary_turn_request(
            codewhale_core::request::PrimaryTurnRequest {
                model: "deepseek-v4-pro".to_string(),
                messages: vec![Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "core request boundary".to_string(),
                        cache_control: None,
                    }],
                }],
                max_tokens: 64,
                system: None,
                tools: Some(vec![Tool {
                    input_schema: json!({
                        "zeta": {"type": "string"},
                        "alpha": {"type": "number"},
                        "type": "object",
                    }),
                    ..test_tool("lookup")
                }]),
                tool_choice: Some(json!({"type": "auto"})),
                reasoning_effort: Some("off".to_string()),
            },
        );
        let client = deepseek_request_boundary_client("https://api.deepseek.com/v1", server.uri());
        let prepared = client
            .prepare_outbound_request(request.clone(), true)
            .expect("core request prepares through the production seam");
        let prepared_bytes = serde_json::to_vec(&prepared.body).expect("prepared body serializes");

        let mut stream = client
            .create_message_stream(request)
            .await
            .expect("production transport accepts the core request");
        while let Some(event) = stream.next().await {
            event.expect("captured SSE response remains valid");
        }

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].body, prepared_bytes);
        let captured = std::str::from_utf8(&requests[0].body).expect("request body is UTF-8 JSON");
        assert!(
            captured.contains(
                r#""parameters":{"zeta":{"type":"string"},"alpha":{"type":"number"},"type":"object"}"#
            ),
            "nested core-owned JSON order drifted: {captured}"
        );
    }

    #[tokio::test]
    async fn ollama_cloud_uses_authenticated_openai_compatible_v1_wire() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header(
                "authorization",
                "Bearer ollama-cloud-request-boundary-key",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-ollama-cloud-request-boundary",
                "object": "chat.completion",
                "model": "gpt-oss:120b",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2
                }
            })))
            .expect(5)
            .mount(&server)
            .await;

        let client = ollama_cloud_request_boundary_client(server.uri());
        for requested in ["off", "low", "medium", "high", "max"] {
            client
                .create_message(MessageRequest {
                    model: "gpt-oss:120b".to_string(),
                    messages: vec![Message {
                        role: Role::User,
                        content: vec![ContentBlock::Text {
                            text: "Ollama Cloud request boundary".to_string(),
                            cache_control: None,
                        }],
                    }],
                    max_tokens: 64,
                    system: None,
                    tools: None,
                    tool_choice: None,
                    metadata: None,
                    thinking: None,
                    reasoning_effort: Some(requested.to_string()),
                    stream: Some(false),
                    temperature: None,
                    top_p: None,
                })
                .await
                .expect("Ollama Cloud request succeeds");
        }

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 5);
        for (request, expected) in requests
            .iter()
            .zip(["none", "low", "medium", "high", "max"])
        {
            let body: Value = serde_json::from_slice(&request.body).expect("captured request JSON");
            assert_eq!(body["model"], "gpt-oss:120b");
            assert_eq!(body["reasoning_effort"], expected);
            assert!(
                body.get("think").is_none(),
                "native Ollama field leaked: {body}"
            );
            assert!(
                body.get("thinking").is_none(),
                "foreign field leaked: {body}"
            );
        }
    }

    // This synchronous guard deliberately spans every await: the assertions
    // require exclusive access to process-global retry state for the full call.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn cache_free_message_call_neither_reads_nor_writes_global_cache() {
        let _retry_guard = crate::retry_status::test_guard();
        crate::retry_status::clear();
        crate::retry_status::clear_rate_limit();
        crate::retry_status::start(7, Duration::from_secs(60), "foreground sentinel");
        crate::retry_status::note_rate_limit(Duration::from_secs(60));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-cache-free-provider",
                "object": "chat.completion",
                "model": "deepseek-v4-pro",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "provider result"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = deepseek_request_boundary_client("https://api.deepseek.com/v1", server.uri());
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "preview-router-cache-isolation-regression".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 128,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("off".to_string()),
            stream: Some(false),
            temperature: Some(0.0),
            top_p: None,
        };
        let prepared = client
            .prepare_outbound_request(request.clone(), false)
            .expect("request prepares");
        let wire_body = serde_json::to_vec(&prepared.body).expect("wire body serializes");
        let cache_key = crate::llm_response_cache::ResponseCache::make_key(
            client.api_provider.as_str(),
            &client.base_url,
            client.path_suffix.as_deref(),
            &client.api_key,
            &wire_body,
        );
        crate::llm_response_cache::response_cache().put(
            cache_key,
            MessageResponse {
                id: "cached-sentinel-must-survive".to_string(),
                r#type: "message".to_string(),
                role: "assistant".to_string(),
                content: Vec::new(),
                model: "deepseek-v4-pro".to_string(),
                stop_reason: Some("end_turn".to_string()),
                stop_sequence: None,
                container: None,
                usage: Default::default(),
            },
        );

        let response = client
            .create_message_without_response_cache(request)
            .await
            .expect("cache-free provider call succeeds");
        assert_eq!(response.id, "chatcmpl-cache-free-provider");
        assert_eq!(
            crate::llm_response_cache::response_cache()
                .get(&cache_key)
                .expect("sentinel remains")
                .id,
            "cached-sentinel-must-survive"
        );
        match crate::retry_status::snapshot() {
            crate::retry_status::RetryState::Active(banner) => {
                assert_eq!(banner.attempt, 7);
                assert_eq!(banner.reason, "foreground sentinel");
            }
            state => panic!("isolated success mutated retry state: {state:?}"),
        }
        assert!(
            crate::retry_status::rate_limit_remaining().is_some(),
            "isolated success must not clear the foreground provider pause"
        );
        crate::retry_status::clear();
        crate::retry_status::clear_rate_limit();
    }

    // This synchronous guard deliberately spans every await: the assertions
    // require exclusive access to process-global retry state for the full call.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn cache_free_classifier_429_does_not_publish_global_retry_or_rate_limit_state() {
        let _retry_guard = crate::retry_status::test_guard();
        crate::retry_status::clear();
        crate::retry_status::clear_rate_limit();
        crate::retry_status::start(9, Duration::from_secs(60), "foreground sentinel 429");
        crate::retry_status::note_rate_limit(Duration::from_secs(60));

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "120")
                    .set_body_string("rate limited"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut client =
            deepseek_request_boundary_client("https://api.deepseek.com/v1", server.uri());
        client.retry.enabled = false;
        client.retry.max_retries = 0;
        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "preview-router-429-isolation-regression".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 64,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: Some("off".to_string()),
            stream: Some(false),
            temperature: Some(0.0),
            top_p: None,
        };
        let error = client
            .create_message_without_response_cache(request)
            .await
            .expect_err("429 must fail when isolated retries are disabled");
        assert!(
            matches!(
                error.downcast_ref::<LlmError>(),
                Some(LlmError::RateLimited { .. })
            ),
            "{error:#}"
        );
        match crate::retry_status::snapshot() {
            crate::retry_status::RetryState::Active(banner) => {
                assert_eq!(banner.attempt, 9);
                assert_eq!(banner.reason, "foreground sentinel 429");
            }
            state => panic!("isolated 429 mutated retry state: {state:?}"),
        }
        let remaining =
            crate::retry_status::rate_limit_remaining().expect("foreground provider pause remains");
        assert!(
            remaining < Duration::from_secs(70),
            "classifier Retry-After must not extend the global pause: {remaining:?}"
        );
        crate::retry_status::clear();
        crate::retry_status::clear_rate_limit();
    }

    async fn assert_deepseek_strict_request_route_boundary(streaming: bool) {
        for (route_base_url, strict, expected_path, expected_wire_strict) in [
            (
                "https://api.deepseek.com/beta",
                false,
                "/v1/chat/completions",
                None,
            ),
            (
                "https://api.deepseek.com/beta",
                true,
                "/beta/chat/completions",
                Some(true),
            ),
            (
                "https://api.deepseek.com/v1",
                true,
                "/v1/chat/completions",
                None,
            ),
        ] {
            let (captured_path, body) =
                capture_deepseek_chat_request(route_base_url, strict, streaming).await;
            assert_eq!(captured_path, expected_path, "{route_base_url} {body}");
            assert_eq!(
                body.pointer("/tools/0/function/strict")
                    .and_then(Value::as_bool),
                expected_wire_strict,
                "{route_base_url} {body}"
            );
        }
    }

    fn k3_request_fixture(model: &str, effort: Option<&str>, stream: bool) -> MessageRequest {
        MessageRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "request-boundary fixture".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 64,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: effort.map(str::to_string),
            stream: Some(stream),
            temperature: Some(0.25),
            top_p: Some(0.75),
        }
    }

    async fn capture_moonshot_chat_request(
        route_base_url: &str,
        model: &str,
        effort: Option<&str>,
        streaming: bool,
    ) -> Value {
        let request = k3_request_fixture(model, effort, streaming);
        capture_moonshot_chat_request_body(route_base_url, model, request).await
    }

    async fn capture_moonshot_chat_request_body(
        route_base_url: &str,
        model: &str,
        request: MessageRequest,
    ) -> Value {
        let streaming = request.stream == Some(true);
        let server = MockServer::start().await;
        let response = if streaming {
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n")
        } else {
            ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-k3-request-boundary",
                "object": "chat.completion",
                "model": model,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2
                }
            }))
        };
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;

        let client = moonshot_request_boundary_client(route_base_url, model, server.uri());

        if streaming {
            let mut stream = client
                .create_message_stream(request)
                .await
                .expect("streaming request succeeds");
            while let Some(event) = stream.next().await {
                event.expect("captured SSE response remains valid");
            }
        } else {
            client
                .create_message(request)
                .await
                .expect("non-streaming request succeeds");
        }

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        serde_json::from_slice(&requests[0].body).expect("captured request JSON")
    }

    async fn capture_route_chat_request_body(
        model: &str,
        request: MessageRequest,
        client_for_transport: impl FnOnce(String) -> DeepSeekClient,
    ) -> (String, Value) {
        let streaming = request.stream == Some(true);
        let server = MockServer::start().await;
        let response = if streaming {
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n")
        } else {
            ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-provider-request-boundary",
                "object": "chat.completion",
                "model": model,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2
                }
            }))
        };
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for_transport(server.uri());
        if streaming {
            let mut stream = client
                .create_message_stream(request)
                .await
                .expect("streaming request succeeds");
            while let Some(event) = stream.next().await {
                event.expect("captured SSE response remains valid");
            }
        } else {
            client
                .create_message(request)
                .await
                .expect("non-streaming request succeeds");
        }

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        (
            requests[0].url.path().to_string(),
            serde_json::from_slice(&requests[0].body).expect("captured request JSON"),
        )
    }

    async fn capture_zai_chat_request(
        route_base_url: &str,
        model: &str,
        effort: Option<&str>,
        streaming: bool,
    ) -> (String, Value) {
        capture_route_chat_request_body(
            model,
            k3_request_fixture(model, effort, streaming),
            |uri| zai_request_boundary_client(route_base_url, model, uri),
        )
        .await
    }

    async fn capture_minimax_chat_request(
        route_base_url: &str,
        model: &str,
        effort: Option<&str>,
        streaming: bool,
    ) -> (String, Value) {
        capture_route_chat_request_body(
            model,
            k3_request_fixture(model, effort, streaming),
            |uri| minimax_request_boundary_client(route_base_url, model, uri),
        )
        .await
    }

    fn modelstudio_request_boundary_client(
        route_base_url: &str,
        model: &str,
        transport_base_url: String,
    ) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut client = DeepSeekClient::new(&Config {
            provider: Some("modelstudio-token-plan".to_string()),
            providers: Some(ProvidersConfig {
                modelstudio_token_plan: ProviderConfig {
                    api_key: Some("modelstudio-request-boundary-key".to_string()),
                    base_url: Some(route_base_url.to_string()),
                    model: Some(model.to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("Model Studio request-boundary client");
        assert_eq!(client.base_url, route_base_url);
        client.test_chat_transport_base_url = Some(transport_base_url);
        client
    }

    async fn capture_modelstudio_chat_request(
        route_base_url: &str,
        model: &str,
        effort: Option<&str>,
        streaming: bool,
    ) -> (String, Value) {
        capture_route_chat_request_body(
            model,
            k3_request_fixture(model, effort, streaming),
            |uri| modelstudio_request_boundary_client(route_base_url, model, uri),
        )
        .await
    }

    async fn assert_modelstudio_request_truth(streaming: bool) {
        // Token Plan and Coding Plan share DashScope's reasoning controls on
        // their OpenAI-compatible Chat Completions endpoints — but the fields
        // are model-specific, not provider-wide.
        for base_url in [
            crate::config::DEFAULT_MODELSTUDIO_TOKEN_PLAN_BASE_URL,
            crate::config::DEFAULT_MODELSTUDIO_CODING_PLAN_BASE_URL,
        ] {
            // The default model, qwen3.8-max, is thinking-only: the bundled
            // catalog records it as `thinking: always_on`, and
            // qwen3.8-max-preview has effort/budget options with no toggle.
            // Neither accepts an enable/disable switch, so CodeWhale must not
            // send one — not even `false` for an explicit `off`. This assertion
            // used to pin the opposite; PR #5233 caught it.
            for effort in [None, Some("off"), Some("high"), Some("max")] {
                let (path, body) = capture_modelstudio_chat_request(
                    base_url,
                    crate::config::DEFAULT_MODELSTUDIO_TOKEN_PLAN_MODEL,
                    effort,
                    streaming,
                )
                .await;
                assert_eq!(path, "/v1/chat/completions");
                assert!(
                    body.get("enable_thinking").is_none(),
                    "{base_url} {effort:?}: {body}"
                );
                assert!(
                    body.get("thinking").is_none(),
                    "{base_url} {effort:?}: {body}"
                );
                assert!(
                    body.get("reasoning_effort").is_none(),
                    "{base_url} {effort:?}: {body}"
                );
            }

            // A hybrid model does get the documented switch, plus
            // `preserve_thinking` so the next turn keeps its trace.
            for (effort, enabled) in [(None, true), (Some("high"), true), (Some("off"), false)] {
                let (_, body) =
                    capture_modelstudio_chat_request(base_url, "qwen3.7-plus", effort, streaming)
                        .await;
                assert_eq!(
                    body["enable_thinking"],
                    json!(enabled),
                    "{base_url} {effort:?}: {body}"
                );
                assert_eq!(
                    body["preserve_thinking"],
                    json!(enabled),
                    "{base_url} {effort:?}: {body}"
                );
                // The hybrid Qwen families have no effort ladder on the wire.
                assert!(
                    body.get("reasoning_effort").is_none(),
                    "{base_url} {effort:?}: {body}"
                );
            }

            // DeepSeek-V4 is one of the two families with a documented effort
            // ladder (`high` / `max`).
            let (_, deepseek) = capture_modelstudio_chat_request(
                base_url,
                "deepseek-v4-pro",
                Some("xhigh"),
                streaming,
            )
            .await;
            assert_eq!(
                deepseek["enable_thinking"],
                json!(true),
                "{base_url}: {deepseek}"
            );
            assert_eq!(
                deepseek["reasoning_effort"],
                json!("max"),
                "{base_url}: {deepseek}"
            );
        }

        // Fail closed: the same provider identity pointed at a custom gateway
        // must not be handed Alibaba's dialect.
        let (_, proxied) = capture_modelstudio_chat_request(
            "https://proxy.example/v1",
            "qwen3.7-plus",
            Some("high"),
            streaming,
        )
        .await;
        assert!(proxied.get("enable_thinking").is_none(), "{proxied}");
        assert!(proxied.get("preserve_thinking").is_none(), "{proxied}");
        assert!(proxied.get("reasoning_effort").is_none(), "{proxied}");
    }

    async fn assert_zai_request_truth(streaming: bool) {
        for base_url in [
            crate::config::DEFAULT_ZAI_BASE_URL,
            "https://api.z.ai/api/paas/v4",
        ] {
            let (high_path, high) = capture_zai_chat_request(
                base_url,
                crate::config::ZAI_GLM_5_2_MODEL,
                Some("high"),
                streaming,
            )
            .await;
            let (max_path, max) = capture_zai_chat_request(
                base_url,
                crate::config::ZAI_GLM_5_2_MODEL,
                Some("max"),
                streaming,
            )
            .await;
            assert_eq!(high_path, "/v1/chat/completions");
            assert_eq!(max_path, "/v1/chat/completions");
            assert_eq!(high["reasoning_effort"], "high", "{base_url}: {high}");
            assert_eq!(max["reasoning_effort"], "max", "{base_url}: {max}");
            for body in [&high, &max] {
                assert_eq!(
                    body["thinking"],
                    json!({"type": "enabled", "clear_thinking": false}),
                    "{base_url}: {body}"
                );
                assert_eq!(body["model"], crate::config::ZAI_GLM_5_2_MODEL);
            }
            let mut high_without_effort = high.clone();
            let mut max_without_effort = max.clone();
            high_without_effort
                .as_object_mut()
                .expect("object")
                .remove("reasoning_effort");
            max_without_effort
                .as_object_mut()
                .expect("object")
                .remove("reasoning_effort");
            assert_eq!(high_without_effort, max_without_effort);

            for model in [
                crate::config::ZAI_GLM_5_1_MODEL,
                crate::config::ZAI_GLM_5_TURBO_MODEL,
            ] {
                for requested in ["high", "max"] {
                    let (_, toggle_only) =
                        capture_zai_chat_request(base_url, model, Some(requested), streaming).await;
                    assert!(
                        toggle_only.get("reasoning_effort").is_none(),
                        "{model}: {toggle_only}"
                    );
                    assert_eq!(
                        toggle_only["thinking"],
                        json!({"type": "enabled", "clear_thinking": false}),
                        "{model}: {toggle_only}"
                    );
                }
            }

            let (_, unknown) =
                capture_zai_chat_request(base_url, "glm-future-unknown", Some("max"), streaming)
                    .await;
            assert!(unknown.get("reasoning_effort").is_none(), "{unknown}");
            assert!(unknown.get("thinking").is_none(), "{unknown}");
        }

        let (_, gateway) = capture_zai_chat_request(
            "https://gateway.example/v1",
            crate::config::ZAI_GLM_5_2_MODEL,
            Some("max"),
            streaming,
        )
        .await;
        assert!(gateway.get("reasoning_effort").is_none(), "{gateway}");
        assert!(gateway.get("thinking").is_none(), "{gateway}");

        let (_, gateway_turbo) = capture_zai_chat_request(
            "https://gateway.example/v1",
            crate::config::ZAI_GLM_5_TURBO_MODEL,
            Some("max"),
            streaming,
        )
        .await;
        assert!(
            gateway_turbo.get("reasoning_effort").is_none(),
            "{gateway_turbo}"
        );
        assert!(gateway_turbo.get("thinking").is_none(), "{gateway_turbo}");
    }

    async fn assert_minimax_request_truth(streaming: bool) {
        for base_url in [
            crate::config::DEFAULT_MINIMAX_BASE_URL,
            "https://api.minimaxi.com/v1",
        ] {
            for (effort, expected_thinking) in [
                ("off", json!({"type": "disabled"})),
                ("high", json!({"type": "adaptive"})),
                ("max", json!({"type": "adaptive"})),
            ] {
                let (_, body) = capture_minimax_chat_request(
                    base_url,
                    crate::config::DEFAULT_MINIMAX_MODEL,
                    Some(effort),
                    streaming,
                )
                .await;
                assert_eq!(
                    body["max_completion_tokens"], 64,
                    "{base_url} {effort}: {body}"
                );
                assert!(
                    body.get("max_tokens").is_none(),
                    "{base_url} {effort}: {body}"
                );
                assert_eq!(body["reasoning_split"], true, "{base_url}: {body}");
                assert_eq!(
                    body["thinking"], expected_thinking,
                    "{base_url} {effort}: {body}"
                );
            }
        }

        for (base_url, model) in [
            (crate::config::DEFAULT_MINIMAX_BASE_URL, "MiniMax-M2"),
            (
                "https://gateway.example/v1",
                crate::config::DEFAULT_MINIMAX_MODEL,
            ),
        ] {
            for effort in ["off", "high", "max"] {
                let (_, body) =
                    capture_minimax_chat_request(base_url, model, Some(effort), streaming).await;
                assert_eq!(
                    body["max_tokens"], 64,
                    "{base_url} {model} {effort}: {body}"
                );
                assert!(
                    body.get("max_completion_tokens").is_none(),
                    "{base_url} {model} {effort}: {body}"
                );
                assert!(
                    body.get("reasoning_split").is_none(),
                    "{base_url} {model} {effort}: {body}"
                );
                assert!(
                    body.get("thinking").is_none(),
                    "{base_url} {model} {effort}: {body}"
                );
            }
        }
    }

    async fn assert_k3_request_json_route_boundaries(streaming: bool) {
        for (requested, expected) in [("off", "low"), ("high", "high"), ("max", "max")] {
            let body = capture_moonshot_chat_request(
                crate::config::DEFAULT_MOONSHOT_BASE_URL,
                crate::config::MOONSHOT_KIMI_K3_MODEL,
                Some(requested),
                streaming,
            )
            .await;
            assert_eq!(body["reasoning_effort"], json!(expected), "{body}");
            assert!(body.get("thinking").is_none(), "{body}");
            assert_eq!(body["max_completion_tokens"], json!(64), "{body}");
            assert!(body.get("max_tokens").is_none(), "{body}");
            assert!(body.get("temperature").is_none(), "{body}");
            assert!(body.get("top_p").is_none(), "{body}");
            assert_eq!(
                body.get("stream").and_then(Value::as_bool),
                streaming.then_some(true)
            );
        }

        for (requested, expected) in [
            ("off", Some(json!({"type": "enabled", "effort": "low"}))),
            ("max", Some(json!({"type": "enabled", "effort": "max"}))),
        ] {
            let membership = capture_moonshot_chat_request(
                crate::config::DEFAULT_KIMI_CODE_BASE_URL,
                crate::config::KIMI_CODE_K3_MODEL,
                Some(requested),
                streaming,
            )
            .await;
            match expected {
                Some(thinking) => assert_eq!(membership["thinking"], thinking, "{membership}"),
                None => assert!(membership.get("thinking").is_none(), "{membership}"),
            }
            assert!(membership.get("reasoning_effort").is_none(), "{membership}");
            assert_eq!(membership["max_tokens"], json!(64), "{membership}");
            assert!(
                membership.get("max_completion_tokens").is_none(),
                "{membership}"
            );
            // Kimi Code's documented membership models own their sampling
            // behavior: the exact first-party membership route strips generic
            // controls (apply_kimi_code_fixed_sampling).
            assert!(membership.get("temperature").is_none(), "{membership}");
            assert!(membership.get("top_p").is_none(), "{membership}");
        }

        let provider_default = capture_moonshot_chat_request(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            None,
            streaming,
        )
        .await;
        assert!(
            provider_default.get("thinking").is_none(),
            "only a genuinely omitted effort leaves the provider default in control: {provider_default}"
        );
        assert!(provider_default.get("reasoning_effort").is_none());

        let neighbor = capture_moonshot_chat_request(
            "https://proxy.example/v1",
            crate::config::MOONSHOT_KIMI_K3_MODEL,
            Some("max"),
            streaming,
        )
        .await;
        assert_eq!(
            neighbor["thinking"],
            json!({"type": "enabled"}),
            "{neighbor}"
        );
        assert!(neighbor.get("reasoning_effort").is_none(), "{neighbor}");
        assert!(neighbor.pointer("/thinking/effort").is_none(), "{neighbor}");
        assert_eq!(neighbor["max_tokens"], json!(64), "{neighbor}");
        assert!(
            neighbor.get("max_completion_tokens").is_none(),
            "{neighbor}"
        );
        assert_eq!(neighbor["temperature"], json!(0.25), "{neighbor}");
        assert_eq!(neighbor["top_p"], json!(0.75), "{neighbor}");
    }

    async fn assert_kimi_code_raw_off_replays_tool_history(streaming: bool) {
        let mut request =
            k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("off"), streaming);
        request.messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "Inspect the saved tool state".to_string(),
                        signature: None,
                        state: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call-k3-replay".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "src/lib.rs"}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-k3-replay".to_string(),
                    content: "file contents".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];

        let body = capture_moonshot_chat_request_body(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            request,
        )
        .await;
        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "effort": "low"}),
            "raw Off must still normalize to K3's always-thinking low tier: {body}"
        );
        let assistant = body["messages"]
            .as_array()
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message["role"] == "assistant")
            })
            .expect("captured assistant tool-call history");
        assert_eq!(
            assistant["reasoning_content"],
            json!("Inspect the saved tool state"),
            "exact membership K3 must replay reasoning even for a stale raw Off caller: {body}"
        );
        assert!(assistant["tool_calls"].is_array(), "{assistant}");
    }

    async fn assert_kimi_code_apply_patch_schema_is_mfjs_compatible(streaming: bool) {
        let mut request =
            k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("low"), streaming);
        request.tools = Some(vec![apply_patch_request_tool()]);

        let body = capture_moonshot_chat_request_body(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            request,
        )
        .await;
        let function = &body["tools"][0]["function"];
        let parameters = &function["parameters"];
        assert_eq!(parameters["type"], "object", "{parameters}");
        assert!(parameters.get("oneOf").is_none(), "{parameters}");
        assert!(parameters.get("anyOf").is_none(), "{parameters}");
        assert!(parameters.get("allOf").is_none(), "{parameters}");
        assert_eq!(parameters["properties"]["patch"]["type"], "string");
        assert_eq!(parameters["properties"]["replace"]["type"], "array");
        assert_eq!(parameters["properties"]["changes"]["type"], "array");
        assert!(
            function["description"]
                .as_str()
                .is_some_and(|description| description
                    .contains("Exactly one of these parameter groups must be provided")),
            "the relaxed wire schema must preserve the runtime constraint in its description: {function}"
        );
    }

    // Per-tool degradation regression: the old behavior failed the whole
    // request before transport when any tool's parameters failed MFJS
    // validation; now only the incompatible tool is dropped from the wire
    // body and the request still sends, so one bad MCP server cannot sink
    // every Moonshot-routed turn.
    async fn assert_kimi_code_invalid_root_ref_drops_only_that_tool(streaming: bool) {
        let mut request =
            k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("low"), streaming);
        let mut tool = test_tool("private_schema_tool");
        tool.input_schema = json!({
            "$ref": "#/$defs/private-root-name-3158",
            "$defs": {}
        });
        request.tools = Some(vec![tool]);
        request.tool_choice = Some(json!("auto"));

        let body = capture_moonshot_chat_request_body(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            request,
        )
        .await;
        assert!(
            body.get("tools").is_none(),
            "the only tool was dropped, so the wire body must omit tools entirely: {body}"
        );
        assert!(
            body.get("tool_choice").is_none(),
            "tool_choice must not be sent when every tool was dropped: {body}"
        );
        assert!(
            !body.to_string().contains("private-root-name-3158"),
            "the wire body must not leak the private $ref value: {body}"
        );
    }

    async fn assert_kimi_code_untyped_default_drops_only_that_tool(streaming: bool) {
        let mut request =
            k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("low"), streaming);
        let mut tool = test_tool("private_default_tool");
        tool.input_schema = json!({
            "type": "object",
            "properties": {
                "private-field-4401": {
                    "default": "private-default-value-4402"
                }
            }
        });
        request.tools = Some(vec![tool]);

        let body = capture_moonshot_chat_request_body(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            request,
        )
        .await;
        assert!(
            body.get("tools").is_none(),
            "the only tool was dropped, so the wire body must omit tools entirely: {body}"
        );
        assert!(
            body.get("tool_choice").is_none(),
            "tool_choice must not be sent when every tool was dropped: {body}"
        );
        assert!(!body.to_string().contains("private-field-4401"));
        assert!(!body.to_string().contains("private-default-value-4402"));
    }

    async fn assert_kimi_code_streams_mfjs_safe_deferred_dynamic_tool() {
        let tool = deferred_dynamic_request_tool();
        assert_eq!(tool.defer_loading, Some(true));
        assert_eq!(
            tool.input_schema["properties"]["query"]["nullable"], true,
            "ToolRegistry must exercise the provider-neutral nullable collapse"
        );
        assert!(
            tool.input_schema["properties"]["query"]
                .get("anyOf")
                .is_none()
        );
        assert_eq!(tool.input_schema["properties"]["mode"]["const"], "fast");

        let mut request = k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("low"), true);
        request.tools = Some(vec![tool]);
        let body = capture_moonshot_chat_request_body(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            request,
        )
        .await;

        assert_eq!(
            body["stream"], true,
            "this must exercise the SSE path: {body}"
        );
        let parameters = &captured_function(&body, "deferred_lookup")["parameters"];
        assert_eq!(parameters["properties"]["mode"]["enum"], json!(["fast"]));
        assert!(
            parameters["properties"]["mode"].get("const").is_none(),
            "{parameters}"
        );
        assert_eq!(
            parameters["properties"]["query"]["anyOf"],
            json!([{"type": "string"}, {"type": "null"}])
        );
        assert!(
            parameters["properties"]["query"].get("nullable").is_none(),
            "{parameters}"
        );
        crate::tools::schema_sanitize::validate_mfjs_parameters(parameters).unwrap();
    }

    async fn assert_kimi_code_captures_exact_general_child_catalog() {
        let tools = crate::tools::subagent::kimi_general_child_request_tools_fixture();
        let source_len = tools.len();
        // Specialized tools remain discoverable beyond the fixed eager head.
        assert_eq!(
            source_len,
            crate::core::engine::default_active_native_tool_names().len() + 1,
            "expected the seven-tool General child catalog: {tools:?}"
        );
        let source_names = tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let expected_names = crate::core::engine::default_active_native_tool_names()
            .iter()
            .copied()
            .chain([crate::core::engine::tool_catalog::TOOL_SEARCH_NAME])
            .map(str::to_string)
            .collect();
        assert_eq!(source_names, expected_names);

        // Name the offending first-party tool in test-only diagnostics while
        // production errors remain fixed and non-secret.
        for tool in &tools {
            let mut parameters = tool.input_schema.clone();
            crate::tools::schema_sanitize::sanitize_for_kimi_parameters(&mut parameters)
                .unwrap_or_else(|error| panic!("General child tool {}: {error}", tool.name));
        }

        let mut request = k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("low"), false);
        request.tools = Some(tools);
        let body = capture_moonshot_chat_request_body(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            request,
        )
        .await;

        let captured = body["tools"].as_array().expect("captured tool catalog");
        assert_eq!(captured.len(), source_len);
        for required in &source_names {
            assert!(
                captured_function(&body, required).is_object(),
                "{required} must reach the Kimi Code wire"
            );
        }
        assert!(
            captured
                .iter()
                .all(|tool| tool["function"]["name"] != "create_goal")
        );
        assert!(
            captured
                .iter()
                .all(|tool| tool["function"]["name"] != "update_goal")
        );

        for tool in captured {
            let parameters = &tool["function"]["parameters"];
            for unsupported in ["const", "nullable", "oneOf", "allOf"] {
                assert!(
                    !value_contains_key(parameters, unsupported),
                    "captured {} still contains {unsupported}: {parameters}",
                    tool["function"]["name"]
                );
            }
            crate::tools::schema_sanitize::validate_mfjs_parameters(parameters).unwrap();
        }
    }

    #[tokio::test]
    async fn create_message_scenario() {
        // Scenario consolidation of: create_message_request_json_honors_exact_k3_route_boundaries, create_message_stream_request_json_honors_exact_k3_route_boundaries, create_message_request_json_keeps_zai_effort_route_exact, create_message_stream_request_json_keeps_zai_effort_route_exact, create_message_request_json_keeps_minimax_token_dialect_exact, create_message_stream_request_json_keeps_minimax_token_dialect_exact, create_message_request_json_keeps_modelstudio_enable_thinking_exact, create_message_stream_request_json_keeps_modelstudio_enable_thinking_exact
        // from create_message_request_json_honors_exact_k3_route_boundaries
        {
            assert_k3_request_json_route_boundaries(false).await;
        }
        // from create_message_stream_request_json_honors_exact_k3_route_boundaries
        {
            assert_k3_request_json_route_boundaries(true).await;
        }
        // from create_message_request_json_keeps_zai_effort_route_exact
        {
            assert_zai_request_truth(false).await;
        }
        // from create_message_stream_request_json_keeps_zai_effort_route_exact
        {
            assert_zai_request_truth(true).await;
        }
        // from create_message_request_json_keeps_minimax_token_dialect_exact
        {
            assert_minimax_request_truth(false).await;
        }
        // from create_message_stream_request_json_keeps_minimax_token_dialect_exact
        {
            assert_minimax_request_truth(true).await;
        }
        // from create_message_request_json_keeps_modelstudio_enable_thinking_exact
        {
            assert_modelstudio_request_truth(false).await;
        }
        // from create_message_stream_request_json_keeps_modelstudio_enable_thinking_exact
        {
            assert_modelstudio_request_truth(true).await;
        }
    }

    #[tokio::test]
    async fn kimi_code_compaction_shape_omits_sampling_parameters_on_wire() {
        for model in crate::config::KIMI_CODE_MEMBERSHIP_MODELS {
            let mut request = k3_request_fixture(model, None, /*stream*/ false);
            request.temperature = Some(0.3);
            request.top_p = Some(0.8);
            let body = capture_moonshot_chat_request_body(
                crate::config::DEFAULT_KIMI_CODE_BASE_URL,
                model,
                request,
            )
            .await;

            assert_eq!(body["model"], model);
            assert!(body.get("temperature").is_none(), "{model}: {body}");
            assert!(body.get("top_p").is_none(), "{model}: {body}");
        }
    }

    /// v0.9.1 kimi-k3 dogfood report: the id the user selects has to be the id on the wire. A
    /// dogfood user selecting `kimi-k3` was served `kimi-k2.7-code`, so this
    /// asserts the wire `model` field for each K3 product on its own endpoint,
    /// and that neither one's request carries the other's id.
    #[tokio::test]
    async fn selected_moonshot_k3_model_is_the_model_on_the_wire() {
        let platform = capture_moonshot_chat_request(
            crate::config::DEFAULT_MOONSHOT_BASE_URL,
            crate::config::MOONSHOT_KIMI_K3_MODEL,
            Some("high"),
            false,
        )
        .await;
        assert_eq!(
            platform["model"],
            json!(crate::config::MOONSHOT_KIMI_K3_MODEL),
            "the direct platform route must send the id the user named: {platform}"
        );
        assert_ne!(
            platform["model"],
            json!(crate::config::DEFAULT_MOONSHOT_MODEL),
            "an explicit selection is never replaced by the provider default: {platform}"
        );
        assert_ne!(
            platform["model"],
            json!(crate::config::KIMI_CODE_K3_MODEL),
            "the coding-plan id must not leak onto the platform route: {platform}"
        );

        let membership = capture_moonshot_chat_request(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            Some("high"),
            false,
        )
        .await;
        assert_eq!(
            membership["model"],
            json!(crate::config::KIMI_CODE_K3_MODEL),
            "the Kimi Code membership route must send bare `k3`: {membership}"
        );
        assert_ne!(
            membership["model"],
            json!(crate::config::MOONSHOT_KIMI_K3_MODEL),
            "the platform id must not leak onto the coding-plan route: {membership}"
        );
    }

    #[tokio::test]
    async fn create_message_scenario_2() {
        // Scenario consolidation of: create_message_routes_only_strict_deepseek_tools_to_beta, create_message_stream_routes_only_strict_deepseek_tools_to_beta, create_message_request_replays_kimi_code_history_for_raw_off, create_message_stream_replays_kimi_code_history_for_raw_off, create_message_request_sends_mfjs_compatible_apply_patch_schema, create_message_stream_sends_mfjs_compatible_apply_patch_schema, create_message_request_drops_invalid_kimi_root_ref_tool, create_message_stream_drops_invalid_kimi_root_ref_tool
        // from create_message_routes_only_strict_deepseek_tools_to_beta
        {
            assert_deepseek_strict_request_route_boundary(false).await;
        }
        // from create_message_stream_routes_only_strict_deepseek_tools_to_beta
        {
            assert_deepseek_strict_request_route_boundary(true).await;
        }
        // from create_message_request_replays_kimi_code_history_for_raw_off
        {
            assert_kimi_code_raw_off_replays_tool_history(false).await;
        }
        // from create_message_stream_replays_kimi_code_history_for_raw_off
        {
            assert_kimi_code_raw_off_replays_tool_history(true).await;
        }
        // from create_message_request_sends_mfjs_compatible_apply_patch_schema
        {
            assert_kimi_code_apply_patch_schema_is_mfjs_compatible(false).await;
        }
        // from create_message_stream_sends_mfjs_compatible_apply_patch_schema
        {
            assert_kimi_code_apply_patch_schema_is_mfjs_compatible(true).await;
        }
        // from create_message_request_drops_invalid_kimi_root_ref_tool
        {
            assert_kimi_code_invalid_root_ref_drops_only_that_tool(false).await;
        }
        // from create_message_stream_drops_invalid_kimi_root_ref_tool
        {
            assert_kimi_code_invalid_root_ref_drops_only_that_tool(true).await;
        }
    }

    #[tokio::test]
    async fn create_message_scenario_3() {
        // Scenario consolidation of: create_message_request_drops_untyped_kimi_default_tool, create_message_stream_drops_untyped_kimi_default_tool, create_message_stream_sends_mfjs_safe_deferred_dynamic_tool, create_message_captures_exact_mfjs_safe_general_child_catalog
        // from create_message_request_drops_untyped_kimi_default_tool
        {
            assert_kimi_code_untyped_default_drops_only_that_tool(false).await;
        }
        // from create_message_stream_drops_untyped_kimi_default_tool
        {
            assert_kimi_code_untyped_default_drops_only_that_tool(true).await;
        }
        // from create_message_stream_sends_mfjs_safe_deferred_dynamic_tool
        {
            assert_kimi_code_streams_mfjs_safe_deferred_dynamic_tool().await;
        }
        // from create_message_captures_exact_mfjs_safe_general_child_catalog
        {
            assert_kimi_code_captures_exact_general_child_catalog().await;
        }
    }

    #[tokio::test]
    async fn moonshot_drops_only_incompatible_tool() {
        for streaming in [false, true] {
            let mut request =
                k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("low"), streaming);
            let good = test_tool("compatible_lookup");
            let mut bad = test_tool("mcp_pattern_tool");
            bad.input_schema = json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "pattern": "^private-regex-7701$"}
                }
            });
            request.tools = Some(vec![good, bad]);
            request.tool_choice = Some(json!("auto"));

            let body = capture_moonshot_chat_request_body(
                crate::config::DEFAULT_KIMI_CODE_BASE_URL,
                crate::config::KIMI_CODE_K3_MODEL,
                request,
            )
            .await;
            let tools = body["tools"]
                .as_array()
                .expect("compatible tool must stay on the wire");
            assert_eq!(
                tools.len(),
                1,
                "only the incompatible tool may be dropped (streaming={streaming}): {body}"
            );
            assert_eq!(tools[0]["function"]["name"], "compatible_lookup");
            assert_eq!(
                body["tool_choice"],
                json!("auto"),
                "tool_choice survives while any tool remains: {body}"
            );
            assert!(
                !body.to_string().contains("private-regex-7701"),
                "the dropped tool's private schema values must not reach the wire: {body}"
            );
        }
    }

    #[tokio::test]
    async fn moonshot_rejects_named_choice_for_omitted_tool() {
        for streaming in [false, true] {
            let server = MockServer::start().await;
            let client = moonshot_request_boundary_client(
                crate::config::DEFAULT_KIMI_CODE_BASE_URL,
                crate::config::KIMI_CODE_K3_MODEL,
                server.uri(),
            );
            let mut request =
                k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("low"), streaming);
            let good = test_tool("compatible_lookup");
            let mut bad = test_tool("mcp_pattern_tool");
            bad.input_schema = json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "pattern": "^private-regex-8801$"}
                }
            });
            request.tools = Some(vec![good, bad]);
            request.tool_choice = Some(json!({
                "type": "tool",
                "name": "mcp_pattern_tool"
            }));

            let error = if streaming {
                match client.create_message_stream(request).await {
                    Ok(_) => {
                        panic!("a named choice for an omitted tool must fail before transport")
                    }
                    Err(error) => error,
                }
            } else {
                match client.create_message(request).await {
                    Ok(_) => {
                        panic!("a named choice for an omitted tool must fail before transport")
                    }
                    Err(error) => error,
                }
            };
            let diagnostic = error.to_string();
            assert!(
                diagnostic.contains("cannot force tool 'mcp_pattern_tool'"),
                "streaming={streaming}: {diagnostic}"
            );
            assert!(
                !diagnostic.contains("private-regex-8801"),
                "schema values must not leak into the user-visible diagnostic: {diagnostic}"
            );
            assert!(
                server
                    .received_requests()
                    .await
                    .expect("request log")
                    .is_empty(),
                "dangling named tool_choice must fail locally (streaming={streaming})"
            );
        }
    }

    #[tokio::test]
    async fn moonshot_stream_emits_one_projection_warning() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = moonshot_request_boundary_client(
            crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            crate::config::KIMI_CODE_K3_MODEL,
            server.uri(),
        );
        let mut request = k3_request_fixture(crate::config::KIMI_CODE_K3_MODEL, Some("low"), true);
        let good = test_tool("compatible_lookup");
        let mut bad = test_tool("mcp_pattern_tool");
        bad.input_schema = json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "pattern": "^private-regex-9901$"}
            }
        });
        request.tools = Some(vec![good, bad]);
        request.tool_choice = Some(json!("auto"));

        let mut stream = client
            .create_message_stream(request)
            .await
            .expect("compatible tools keep the request sendable");
        let first = stream
            .next()
            .await
            .expect("projection warning precedes provider SSE")
            .expect("projection warning is not a stream error");
        let (provider, omitted_tool_names, omitted_tool_count) = match first {
            crate::models::StreamEvent::ToolProjectionWarning {
                provider,
                omitted_tool_names,
                omitted_tool_count,
            } => (provider, omitted_tool_names, omitted_tool_count),
            other => panic!("first event must be the projection warning, got {other:?}"),
        };
        assert!(provider.contains("Moonshot"), "{provider}");
        assert_eq!(omitted_tool_names, vec!["mcp_pattern_tool"]);
        assert_eq!(omitted_tool_count, 1);

        let mut additional_warnings = 0;
        while let Some(event) = stream.next().await {
            if matches!(
                event.expect("captured SSE response remains valid"),
                crate::models::StreamEvent::ToolProjectionWarning { .. }
            ) {
                additional_warnings += 1;
            }
        }
        assert_eq!(
            additional_warnings, 0,
            "warning must be emitted once per request"
        );

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).expect("request JSON");
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        assert!(
            !body.to_string().contains("private-regex-9901"),
            "omitted schema values must not reach the wire: {body}"
        );
    }

    fn concentrate_client(server: &MockServer, model: &str) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = Config {
            provider: Some("concentrate".to_string()),
            providers: Some(ProvidersConfig {
                concentrate: ProviderConfig {
                    api_key: Some("concentrate-test-key".to_string()),
                    base_url: Some(format!("{}/v1", server.uri())),
                    model: Some(model.to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        DeepSeekClient::new(&config).expect("Concentrate client should resolve its route")
    }

    /// The documented Concentrate stream: `event:`-typed `response.*` frames
    /// with sequence numbers and NO `data: [DONE]` sentinel.
    /// https://concentrate.ai/docs/api-reference/endpoint/streaming
    fn concentrate_sse_fixture(model: &str) -> String {
        let completed = json!({
            "id": "resp_1",
            "object": "response",
            "status": "completed",
            "model": model,
            "output": [{
                "type": "message", "id": "msg_1", "status": "completed", "role": "assistant",
                "content": [{"type": "output_text", "text": "ok from stub", "annotations": []}]
            }],
            "usage": {"input_tokens": 12, "output_tokens": 5, "total_tokens": 17,
                      "input_tokens_details": {"cached_tokens": 0}}
        });
        let frame = |event: &str, payload: Value| format!("event: {event}\ndata: {}\n\n", payload);
        [
            frame("response.created", json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "status": "in_progress"}})),
            frame("response.output_item.added", json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0, "item": {"type": "message", "id": "msg_1", "status": "in_progress", "role": "assistant", "content": []}})),
            frame("response.content_part.added", json!({"type": "response.content_part.added", "sequence_number": 2, "item_id": "msg_1", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": ""}})),
            frame("response.output_text.delta", json!({"type": "response.output_text.delta", "sequence_number": 3, "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": "ok "})),
            frame("response.output_text.delta", json!({"type": "response.output_text.delta", "sequence_number": 4, "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": "from stub"})),
            frame("response.output_text.done", json!({"type": "response.output_text.done", "sequence_number": 5, "item_id": "msg_1", "output_index": 0, "content_index": 0, "text": "ok from stub"})),
            frame("response.output_item.done", json!({"type": "response.output_item.done", "sequence_number": 6, "output_index": 0, "item": completed["output"][0].clone()})),
            frame("response.completed", json!({"type": "response.completed", "sequence_number": 7, "response": completed})),
        ]
        .concat()
    }

    /// Request URL, bearer header, verbatim model, documented-fields body, and
    /// the typed SSE stream (no `[DONE]`) — the whole Concentrate contract on
    /// a loopback wiremock, never the live gateway.
    #[tokio::test]
    async fn concentrate_responses_request_matches_the_documented_contract() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer concentrate-test-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string(concentrate_sse_fixture("openai/gpt-5.6-sol")),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = concentrate_client(&server, "openai/gpt-5.6-sol");
        assert_eq!(client.wire_format, WireFormat::Responses);
        assert_eq!(client.api_provider, ApiProvider::Concentrate);
        assert_eq!(
            responses_api_url(DEFAULT_CONCENTRATE_BASE_URL, ApiProvider::Concentrate),
            "https://api.concentrate.ai/v1/responses",
            "the official base URL maps to the documented Responses endpoint"
        );

        let mut stream = client
            .create_message_stream(minimal_zen_request("openai/gpt-5.6-sol"))
            .await
            .expect("Concentrate Responses request should start");
        let mut text = String::new();
        let mut usage = None;
        let mut stopped = false;
        while let Some(event) = stream.next().await {
            match event.expect("Concentrate stream event") {
                StreamEvent::ContentBlockDelta {
                    delta: Delta::TextDelta { text: piece },
                    ..
                } => text.push_str(&piece),
                StreamEvent::MessageDelta { usage: Some(u), .. } => usage = Some(u),
                StreamEvent::MessageStop => stopped = true,
                _ => {}
            }
        }
        assert_eq!(text, "ok from stub", "typed deltas assemble the reply");
        let usage = usage.expect("response.completed carries usage");
        assert_eq!((usage.input_tokens, usage.output_tokens), (12, 5));
        assert!(
            stopped,
            "the stream ends on response.completed without a [DONE] sentinel"
        );

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.url.path(), "/v1/responses");
        for forbidden in [
            "openai-beta",
            "originator",
            "chatgpt-account-id",
            "x-api-key",
        ] {
            assert!(
                request.headers.get(forbidden).is_none(),
                "Concentrate request must not include {forbidden}"
            );
        }
        let body: Value = serde_json::from_slice(&request.body).expect("Responses JSON body");
        assert_eq!(
            body["model"], "openai/gpt-5.6-sol",
            "model id verbatim: {body}"
        );
        assert_eq!(body["stream"], true);
        assert!(
            body.get("messages").is_none(),
            "Responses body, not Chat: {body}"
        );
        for undocumented in [
            "store",
            "include",
            "instructions",
            "metadata",
            "previous_response_id",
        ] {
            assert!(
                body.get(undocumented).is_none(),
                "undocumented field {undocumented} on the wire: {body}"
            );
        }
        assert_eq!(
            body["input"][0]["role"], "system",
            "system prompt rides as a system input item: {body}"
        );
    }

    /// The documented error body surfaces verbatim and classifies by message:
    /// 401 → authentication, 402 → quota (insufficient credits).
    /// https://concentrate.ai/docs/api-reference/endpoint/errors
    #[tokio::test]
    async fn concentrate_error_bodies_surface_verbatim_and_classify() {
        for (status, body, expected, needle) in [
            (
                401,
                r#"{"error":"Unauthorized","message":"Invalid API key"}"#,
                crate::error_taxonomy::ErrorCategory::Authentication,
                "Invalid API key",
            ),
            (
                402,
                r#"{"error":"Insufficient funds","message":"Your account has insufficient credits. Please add credits to continue."}"#,
                crate::error_taxonomy::ErrorCategory::RateLimit,
                "insufficient credits",
            ),
            (
                400,
                r#"{"error":"Bad Request","message":"Invalid model name: 'invalid-model-xyz'"}"#,
                crate::error_taxonomy::ErrorCategory::InvalidInput,
                "Invalid model name",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/responses"))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .mount(&server)
                .await;
            let client = concentrate_client(&server, "gpt-5.6-sol");
            let error = match client
                .create_message_stream(minimal_zen_request("gpt-5.6-sol"))
                .await
            {
                Ok(mut stream) => {
                    let mut failure = None;
                    while let Some(event) = stream.next().await {
                        if let Err(err) = event {
                            failure = Some(err);
                            break;
                        }
                    }
                    failure.expect("HTTP {status} must fail the stream")
                }
                Err(err) => err,
            };
            let message = format!("{error:#}");
            assert!(
                message.contains(needle),
                "HTTP {status}: message must carry the documented text, got: {message}"
            );
            assert_eq!(
                crate::error_taxonomy::classify_error_message(&message),
                expected,
                "HTTP {status}: {message}"
            );
        }
    }

    /// Concentrate's `GET /v1/models` is unauthenticated, so a 2xx must not
    /// count as key verification. Guided setup treats the probe as unobserved;
    /// health_check must not issue the request either.
    #[tokio::test]
    async fn concentrate_health_check_does_not_treat_unauthenticated_models_as_key_proof() {
        let server = MockServer::start().await;
        let client = concentrate_client(&server, DEFAULT_CONCENTRATE_MODEL);

        assert!(client.health_check().await.expect("health check"));
        assert!(!provider_api_key_verification_is_observed(
            ApiProvider::Concentrate
        ));
        let requests = server.received_requests().await.expect("recorded requests");
        assert!(
            requests.is_empty(),
            "Concentrate must not treat unauthenticated GET /v1/models as key verification"
        );
    }

    /// `GET /v1/models` needs no key and answers the OpenAI list shape; rows
    /// are provider-scoped, the default is marked, and unknowns stay unclaimed.
    /// https://concentrate.ai/docs/api-reference/endpoint/list-models
    #[tokio::test]
    async fn concentrate_live_catalog_is_provider_scoped_and_marks_the_default() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    {"id": "claude-fable-5", "object": "model", "owned_by": "anthropic"},
                    {"id": DEFAULT_CONCENTRATE_MODEL, "object": "model", "owned_by": "deepseek"},
                    {"id": "gpt-5.6-sol", "object": "model", "owned_by": "openai"}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let delta = concentrate_client(&server, DEFAULT_CONCENTRATE_MODEL)
            .fetch_catalog_delta()
            .await
            .expect("Concentrate catalog delta");
        assert_eq!(delta.provider, "concentrate");
        assert_eq!(delta.offerings.len(), 3);
        let default = delta
            .offerings
            .iter()
            .find(|offering| offering.wire_model_id == DEFAULT_CONCENTRATE_MODEL)
            .expect("default row");
        assert!(default.default_for_provider);
        let unknown = delta
            .offerings
            .iter()
            .find(|offering| offering.wire_model_id == "claude-fable-5")
            .expect("unclaimed row");
        assert!(!unknown.default_for_provider);
        assert_eq!(unknown.canonical_model, None);
        assert_eq!(
            unknown.cost, None,
            "no pricing claim from a gateway catalog"
        );
        assert!(matches!(unknown.source, CatalogSource::Live { .. }));
    }

    fn opencode_zen_client(server: &MockServer, model: &str) -> DeepSeekClient {
        let config = Config {
            provider: Some("opencode-zen".to_string()),
            providers: Some(ProvidersConfig {
                opencode_zen: ProviderConfig {
                    api_key: Some("zen-test-key".to_string()),
                    base_url: Some(server.uri()),
                    model: Some(model.to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        DeepSeekClient::new(&config).expect("OpenCode Zen client should resolve its model route")
    }

    fn minimal_zen_request(model: &str) -> MessageRequest {
        translation_message_request("hello", model.to_string(), "English", 4096)
    }

    fn assert_zen_bearer_without_codex_headers(request: &wiremock::Request) {
        assert_eq!(
            request
                .headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer zen-test-key")
        );
        for forbidden in [
            "openai-beta",
            "originator",
            "chatgpt-account-id",
            "x-api-key",
        ] {
            assert!(
                request.headers.get(forbidden).is_none(),
                "Zen request must not include {forbidden}"
            );
        }
    }

    fn assert_zen_messages_api_key_without_bearer(request: &wiremock::Request) {
        assert_eq!(
            request
                .headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("zen-test-key")
        );
        assert!(
            request.headers.get(AUTHORIZATION).is_none(),
            "Zen Messages request must not include Authorization"
        );
        for forbidden in ["openai-beta", "originator", "chatgpt-account-id"] {
            assert!(
                request.headers.get(forbidden).is_none(),
                "Zen request must not include {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn opencode_zen_responses_request_uses_responses_route_without_oauth_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = opencode_zen_client(&server, "gpt-5.5");
        assert_eq!(client.wire_format, WireFormat::Responses);
        let mut stream = client
            .create_message_stream(minimal_zen_request("gpt-5.5"))
            .await
            .expect("Zen Responses request should start");
        while let Some(event) = stream.next().await {
            event.expect("Zen Responses stream event");
        }

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        assert_zen_bearer_without_codex_headers(&requests[0]);
        let body: Value = serde_json::from_slice(&requests[0].body).expect("Responses JSON body");
        assert_eq!(body.get("model").and_then(Value::as_str), Some("gpt-5.5"));
        assert!(body.get("input").is_some(), "Responses body: {body}");
        assert!(body.get("messages").is_none(), "Responses body: {body}");
    }

    #[tokio::test]
    async fn opencode_zen_messages_request_shape_uses_api_key_anthropic_route() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_zen",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ok"}],
                "model": "claude-sonnet-4-6",
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = opencode_zen_client(&server, "claude-sonnet-4-6");
        assert_eq!(client.wire_format, WireFormat::AnthropicMessages);
        client
            .create_message(minimal_zen_request("claude-sonnet-4-6"))
            .await
            .expect("Zen Messages request should succeed");

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        assert_zen_messages_api_key_without_bearer(&requests[0]);
        assert_eq!(
            requests[0]
                .headers
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some("2023-06-01")
        );
    }

    #[tokio::test]
    async fn opencode_zen_chat_request_uses_chat_completions_route() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl_zen",
                "object": "chat.completion",
                "model": "deepseek-v4-pro",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = opencode_zen_client(&server, "deepseek-v4-pro");
        assert_eq!(client.wire_format, WireFormat::ChatCompletions);
        client
            .create_message(minimal_zen_request("deepseek-v4-pro"))
            .await
            .expect("Zen Chat Completions request should succeed");

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        assert_zen_bearer_without_codex_headers(&requests[0]);
        assert!(requests[0].headers.get("anthropic-version").is_none());
    }

    #[tokio::test]
    async fn opencode_zen_client_fails_closed_when_request_model_changes_protocol() {
        let server = MockServer::start().await;
        let client = opencode_zen_client(&server, "gpt-5.5");

        let error = client
            .create_message(minimal_zen_request("claude-sonnet-4-6"))
            .await
            .expect_err("a Responses-bound client must not send a Messages model");
        assert!(format!("{error:#}").contains("resolve a new model route"));
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded requests")
                .is_empty()
        );
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

    #[test]
    fn codex_client_uses_one_coherent_external_credential_snapshot() {
        let _env = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("credential fixture");
        let path = temp
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("auth.json");
        let token_a = crate::test_support::future_test_jwt("a");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "tokens": {"access_token": token_a.clone(), "account_id": "account-a"}
            }))
            .expect("serialize fixture"),
        )
        .expect("write fixture");
        let _auth_path = crate::test_support::EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &path);
        let _access = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _legacy_access = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let config = Config {
            provider: Some(ApiProvider::OpenaiCodex.as_str().to_string()),
            providers: Some(ProvidersConfig {
                openai_codex: ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(
                        codewhale_config::ExternalCredentialConsentToml::read_only(
                            codewhale_config::ProviderKind::OpenaiCodex,
                            codewhale_config::ExternalCredentialSource::CodexCli,
                            path.clone(),
                        ),
                    ),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };

        crate::external_credentials::reset_side_effect_trap();
        let client = DeepSeekClient::new(&config).expect("Codex client");
        assert_eq!(client.api_key, token_a);
        assert_eq!(client.codex_account_id.as_deref(), Some("account-a"));
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (1, 1),
            "bearer and account id must come from one secure open/read"
        );

        // An owner rotation after construction cannot splice account B into
        // the already-resolved bearer snapshot.
        std::fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({"tokens": {"access_token": crate::test_support::future_test_jwt("b"), "account_id": "account-b"}})).expect("serialize rotated fixture"),
        )
        .expect("rotate fixture");
        assert_eq!(client.api_key, token_a);
        assert_eq!(client.codex_account_id.as_deref(), Some("account-a"));
    }

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
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "call-secret-test".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "config.toml"}),
                        caller: None,
                        thought_signature: None,
                    }],
                },
                Message {
                    role: Role::User,
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
    fn model_bound_request_repairs_dangling_tool_call_before_adapter_projection() {
        let client = client_with_config_secret_sentinels();
        let mut request = request_with_tool_result("unused");
        request.messages.pop();

        let prepared = client.prepare_model_bound_request(request);

        assert!(prepared.messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error: Some(true),
                        ..
                    } if tool_use_id == "call-secret-test"
                        && content.contains("crashed_and_repaired")
                )
            })
        }));
        assert_eq!(
            prepared.messages.last().expect("repaired result").role,
            "user"
        );
        assert!(!prepared.messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { text, .. }
                        if text.contains("[tool_history_repair]")
                )
            })
        }));
    }

    #[test]
    fn model_bound_tool_results_keep_ordinary_code_byte_exact() {
        // #5546: key-only hits in source files must reach the model unchanged
        // so exact-match edits and read-back verification keep working.
        let client = client_with_config_secret_sentinels();
        let source = "\
    \"jsonwebtoken\": \"^9.0.2\",
      password: credentials?.password,
    token = generate_verification_token()
  secret: process.env.NEXTAUTH_SECRET!,
{\"id\":1, \"password\": \"x\", \"language\": \"en\"}
";
        let prepared =
            client.prepare_model_bound_request(request_with_tool_result(source.to_string()));
        assert_eq!(tool_result_content(&prepared), source);

        // A configured credential and a credential-shaped value are still hidden.
        let leaking = format!(
            "api_key = \"{}\"\nsession = \"{}\"\n",
            CONFIG_SECRET_SENTINELS[0],
            ["sk-", "abcdef1234567890abcdef"].concat()
        );
        let prepared = client.prepare_model_bound_request(request_with_tool_result(leaking));
        let content = tool_result_content(&prepared);
        assert!(!content.contains(CONFIG_SECRET_SENTINELS[0]));
        assert!(!content.contains("abcdef1234567890abcdef"));
        assert_eq!(
            content
                .matches(codewhale_config::persistence::REDACTED)
                .count(),
            2
        );
    }

    #[test]
    fn model_bound_scenario() {
        // Scenario consolidation of: model_bound_request_redacts_configured_secrets_and_bare_active_key, model_bound_request_leaves_ordinary_tool_output_unchanged
        // from model_bound_request_redacts_configured_secrets_and_bare_active_key
        {
            let client = client_with_config_secret_sentinels();
            let config_dump = format!(
                "api_key = \"{}\"\n[providers.arcee]\napi_key = \"{}\"\n\
                 ordinary_setting = \"keep-me\"\nall bare values: {}",
                CONFIG_SECRET_SENTINELS[0],
                CONFIG_SECRET_SENTINELS[1],
                CONFIG_SECRET_SENTINELS.join(" ")
            );

            let prepared =
                client.prepare_model_bound_request(request_with_tool_result(config_dump));
            let content = tool_result_content(&prepared);

            for secret in CONFIG_SECRET_SENTINELS {
                assert!(!content.contains(secret), "secret survived redaction");
            }
            assert!(content.contains(codewhale_config::persistence::REDACTED));
            assert!(content.contains("ordinary_setting"));
            assert!(content.contains("keep-me"));
        }
        // from model_bound_request_leaves_ordinary_tool_output_unchanged
        {
            let client = client_with_config_secret_sentinels();
            let ordinary = "tests passed: 42\nREADME.md updated\n";
            let prepared =
                client.prepare_model_bound_request(request_with_tool_result(ordinary.to_string()));
            assert_eq!(tool_result_content(&prepared), ordinary);
        }
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
    fn whitespace_codewhale_home_does_not_load_ambient_redaction_secrets() {
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ambient_home = tmp.path().join("ambient-home");
        std::fs::create_dir_all(&ambient_home).expect("create ambient home");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &ambient_home);
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", &ambient_home);
        let _codewhale_home_unset = crate::test_support::EnvVarGuard::remove("CODEWHALE_HOME");
        let _secret_backend =
            crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        codewhale_secrets::Secrets::file_backed()
            .set("arcee", "ambient-redaction-secret-sentinel")
            .expect("seed ambient file secret store");
        let _whitespace_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", " \t ");
        let mut values = Vec::new();

        push_file_backed_model_bound_secrets(&mut values);

        assert!(
            !values
                .iter()
                .any(|value| value == "ambient-redaction-secret-sentinel"),
            "whitespace must not opt tests into reading the ambient secret store"
        );
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
        crate::tools::truncate::publish_legacy_spillover_ownership(
            &path,
            "workspace",
            raw.as_bytes(),
        )
        .expect("publish compatibility ownership proof");
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
    fn wire_adapter_does_not_persist_sessionless_sha_spillover() {
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
        assert!(raw.len() > 1024, "fixture must enter wire dedup size class");
        let raw_sha = crate::hashing::sha256_hex(raw.as_bytes());
        let prepared = client.prepare_model_bound_request(request_with_tool_result(raw));
        let sanitized = tool_result_content(&prepared).to_string();
        let sanitized_sha = crate::hashing::sha256_hex(sanitized.as_bytes());

        let wire = build_chat_messages_for_request(&prepared);
        let serialized = serde_json::to_string(&wire).expect("serialize chat wire messages");
        assert!(!serialized.contains(CONFIG_SECRET_SENTINELS[6]));

        let sanitized_path = crate::tools::truncate::sha_spillover_path(&sanitized_sha)
            .expect("sanitized spillover path");
        assert!(
            !sanitized_path.exists(),
            "sessionless wire fallback must not create an ownerless SHA artifact"
        );

        let raw_path =
            crate::tools::truncate::sha_spillover_path(&raw_sha).expect("raw spillover path");
        assert!(
            !raw_path.exists(),
            "unsanitized tool output must never be persisted by the wire adapter"
        );
        assert!(!serialized.contains("retrieve_tool_result ref=sha:"));
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

    fn runtime_chat_gate_client(isolated: bool, unrelated: bool) -> DeepSeekClient {
        DeepSeekClient::new(&Config {
            provider: Some("ollama".to_string()),
            default_text_model: Some(crate::config::DEFAULT_OLLAMA_MODEL.to_string()),
            runtime_chat_isolated: isolated,
            runtime_thread_inference_unrelated: unrelated,
            ..Config::default()
        })
        .expect("runtime chat gate test client")
    }

    #[tokio::test]
    async fn runtime_chat_provider_gate_blocks_only_attached_run_participants() {
        let participant = runtime_chat_gate_client(false, false);
        let participant_clone = participant.clone();
        assert!(participant.remote_control_inference_participant);
        assert!(participant_clone.remote_control_inference_participant);
        assert!(
            !runtime_chat_gate_client(true, false).remote_control_inference_participant,
            "the isolated relay client must not deadlock on its own exclusive lease"
        );
        assert!(
            !runtime_chat_gate_client(false, true).remote_control_inference_participant,
            "an unrelated RuntimeThreadManager must remain concurrent"
        );

        let ownership = acquire_runtime_chat_inference_ownership().await;
        let waiting = tokio::spawn(async move {
            participant_clone
                .acquire_remote_control_inference_permit()
                .await
        });
        let mut waiting = waiting;
        assert!(
            tokio::time::timeout(Duration::from_millis(40), &mut waiting)
                .await
                .is_err(),
            "an attached-run provider call must wait behind Runtime Chat ownership"
        );
        assert!(
            runtime_chat_gate_client(true, false)
                .acquire_remote_control_inference_permit()
                .await
                .is_none()
        );
        assert!(
            runtime_chat_gate_client(false, true)
                .acquire_remote_control_inference_permit()
                .await
                .is_none()
        );
        drop(ownership);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), waiting)
                .await
                .expect("participant should resume")
                .expect("permit task")
                .is_some()
        );
    }

    #[tokio::test]
    async fn provider_request_scenario() {
        // Scenario consolidation of: provider_request_concurrency_limiter_is_shared_across_client_clones, provider_request_permit_lives_until_stream_is_consumed
        // from provider_request_concurrency_limiter_is_shared_across_client_clones
        {
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
        // from provider_request_permit_lives_until_stream_is_consumed
        {
            let client = zai_client_for_test();
            let permit = client
                .acquire_provider_request_permit()
                .await
                .expect("zai default should install provider request limiter");
            let stream: crate::llm_client::StreamEventBox = Box::pin(futures_util::stream::iter(
                vec![Ok(StreamEvent::MessageStop)],
            ));
            let mut wrapped =
                DeepSeekClient::hold_provider_request_permit_for_stream(stream, Some(permit));

            assert_eq!(client.active_provider_requests(), 1);
            assert!(wrapped.next().await.is_some());
            assert!(wrapped.next().await.is_none());
            assert_eq!(client.active_provider_requests(), 0);
        }
    }

    #[tokio::test]
    async fn runtime_chat_read_permit_lives_until_stream_is_dropped() {
        let client = runtime_chat_gate_client(false, false);
        let permit = client
            .acquire_remote_control_inference_permit()
            .await
            .expect("interactive participant read permit");
        let stream: crate::llm_client::StreamEventBox = Box::pin(futures_util::stream::pending());
        let wrapped =
            DeepSeekClient::hold_remote_control_inference_permit_for_stream(stream, Some(permit));

        let mut writer = tokio::spawn(acquire_runtime_chat_inference_ownership());
        assert!(
            tokio::time::timeout(Duration::from_millis(40), &mut writer)
                .await
                .is_err(),
            "a live participant stream must retain attached-run ownership through EOF/drop"
        );
        drop(wrapped);
        let ownership = tokio::time::timeout(Duration::from_secs(1), writer)
            .await
            .expect("writer resumes after stream drop")
            .expect("writer task");
        drop(ownership);
    }

    #[test]
    fn parse_speech_scenario() {
        // Scenario consolidation of: parse_speech_audio_response_accepts_message_audio, parse_speech_audio_response_accepts_data_uri
        // from parse_speech_audio_response_accepts_message_audio
        {
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
        // from parse_speech_audio_response_accepts_data_uri
        {
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
    }

    #[test]
    fn speech_synthesis_scenario() {
        // Scenario consolidation of: speech_synthesis_body_omits_user_message_without_instruction, speech_synthesis_body_ignores_blank_instruction, speech_synthesis_body_includes_non_empty_instruction_first
        // from speech_synthesis_body_omits_user_message_without_instruction
        {
            let body = build_speech_synthesis_body(
                "mimo-v2.5-tts",
                "hello",
                None,
                json!({"format": "wav"}),
            );
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
        // from speech_synthesis_body_ignores_blank_instruction
        {
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
        // from speech_synthesis_body_includes_non_empty_instruction_first
        {
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
    }

    #[test]
    fn tool_name_scenario() {
        // Scenario consolidation of: tool_name_roundtrip_dot, tool_name_decode_mangled_dot_prefix, tool_name_decode_bare_hex_no_trailing_dash, tool_name_bare_hex_preserves_alnum, tool_name_bare_hex_preserves_underscore, tool_name_roundtrip_colon
        // from tool_name_roundtrip_dot
        {
            let original = "multi_tool_use.parallel";
            let encoded = to_api_tool_name(original);
            assert_eq!(encoded, "multi_tool_use-x00002E-parallel");
            let decoded = from_api_tool_name(&encoded);
            assert_eq!(decoded, original);
        }
        // from tool_name_decode_mangled_dot_prefix
        {
            let mangled = "multi_tool_use.x00002E-parallel";
            let decoded = from_api_tool_name(mangled);
            assert_eq!(decoded, "multi_tool_use..parallel");
        }
        // from tool_name_decode_bare_hex_no_trailing_dash
        {
            let mangled = "foo_x00002Ebar";
            let decoded = from_api_tool_name(mangled);
            assert_eq!(decoded, "foo_.bar");
        }
        // from tool_name_bare_hex_preserves_alnum
        {
            let input = "foox000041bar";
            let decoded = from_api_tool_name(input);
            assert_eq!(decoded, input);
        }
        // from tool_name_bare_hex_preserves_underscore
        {
            let input = "foox00005Fbar";
            let decoded = from_api_tool_name(input);
            assert_eq!(decoded, input);
        }
        // from tool_name_roundtrip_colon
        {
            let original = "mcp__server:tool_name";
            let encoded = to_api_tool_name(original);
            let decoded = from_api_tool_name(&encoded);
            assert_eq!(decoded, original);
        }
    }

    #[test]
    fn api_url_scenario() {
        // Scenario consolidation of: api_url_handles_default_v1_and_beta_base_urls, api_url_routes_beta_paths_from_any_deepseek_base, api_url_with_suffix_strips_version_before_chat_suffix, api_url_with_suffix_handles_leading_slash, api_url_with_suffix_ignores_suffix_for_models, api_url_with_suffix_ignores_suffix_for_beta_paths, api_url_with_suffix_default_behavior_without_suffix
        // from api_url_handles_default_v1_and_beta_base_urls
        {
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
        // from api_url_routes_beta_paths_from_any_deepseek_base
        {
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
        // from api_url_with_suffix_strips_version_before_chat_suffix
        {
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
        // from api_url_with_suffix_handles_leading_slash
        {
            assert_eq!(
                api_url_with_suffix(
                    "https://api.example.com/v1",
                    "chat/completions",
                    Some("chat/completions")
                ),
                "https://api.example.com/chat/completions"
            );
        }
        // from api_url_with_suffix_ignores_suffix_for_models
        {
            assert_eq!(
                api_url_with_suffix(
                    "https://api.example.com/v1",
                    "models",
                    Some("/chat/completions")
                ),
                "https://api.example.com/v1/models"
            );
        }
        // from api_url_with_suffix_ignores_suffix_for_beta_paths
        {
            assert_eq!(
                api_url_with_suffix(
                    "https://api.example.com/v1",
                    "beta/completions",
                    Some("/chat/completions")
                ),
                "https://api.example.com/beta/completions"
            );
        }
        // from api_url_with_suffix_default_behavior_without_suffix
        {
            assert_eq!(
                api_url_with_suffix("https://api.deepseek.com", "chat/completions", None),
                "https://api.deepseek.com/v1/chat/completions"
            );
        }
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
    fn default_headers_scenario() {
        // Scenario consolidation of: default_headers_include_custom_headers_when_configured, default_headers_ignore_blank_custom_headers
        // from default_headers_include_custom_headers_when_configured
        {
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
        // from default_headers_ignore_blank_custom_headers
        {
            let mut extra = HashMap::new();
            extra.insert("X-Blank".to_string(), "   ".to_string());
            let headers = DeepSeekClient::default_headers("sk-test", &extra).expect("headers");
            assert!(headers.get("x-blank").is_none());
        }
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
                max_model_steps: None,
                turn_wall_clock_secs: None,
                stream_max_content_mb: None,
                stream_max_duration_secs: None,
                ..crate::config::TuiConfig::default()
            }),
            ..Config::default()
        })
        .expect("client");

        assert_eq!(client.stream_idle_timeout, Duration::from_secs(777));
    }

    #[test]
    fn xiaomi_mimo_scenario() {
        // Scenario consolidation of: xiaomi_mimo_token_plan_endpoint_uses_api_key_header, xiaomi_mimo_tp_key_uses_api_key_header_with_custom_base_url, xiaomi_mimo_pay_as_you_go_endpoint_keeps_bearer_header
        // from xiaomi_mimo_token_plan_endpoint_uses_api_key_header
        {
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
        // from xiaomi_mimo_tp_key_uses_api_key_header_with_custom_base_url
        {
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
        // from xiaomi_mimo_pay_as_you_go_endpoint_keeps_bearer_header
        {
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
            body.get("temperature").is_none() && body.get("top_p").is_none(),
            "translation must not inject sampling controls: {body}"
        );
        assert_eq!(
            body.get("max_tokens").and_then(Value::as_u64),
            Some(u64::from(
                crate::route_budget::effective_max_output_tokens_for_route(
                    ApiProvider::DeepseekAnthropic,
                    "deepseek-chat",
                    None,
                )
            )),
            "translation must inherit its resolved route allowance: {body}"
        );
        assert!(
            body.get("system")
                .and_then(Value::as_str)
                .is_some_and(|system| system.contains("Spanish")),
            "target language should be in system prompt: {body}"
        );
    }

    #[tokio::test]
    async fn deepseek_anthropic_scenario() {
        // Scenario consolidation of: deepseek_anthropic_health_check_skips_models_probe, deepseek_anthropic_fim_fails_without_http_request
        // from deepseek_anthropic_health_check_skips_models_probe
        {
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
        // from deepseek_anthropic_fim_fails_without_http_request
        {
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
            assert!(message.contains("no proven FIM wire contract"), "{message}");
            let requests = server.received_requests().await.expect("recorded requests");
            assert!(
                requests.is_empty(),
                "unsupported FIM should fail locally before any HTTP call"
            );
        }
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

        let mut client = minimax_anthropic_client_with_base_url(
            crate::config::DEFAULT_MINIMAX_ANTHROPIC_BASE_URL.to_string(),
        );
        client.test_messages_transport_base_url = Some(format!("{}/anthropic", server.uri()));
        let response = client
            .create_message(MessageRequest {
                model: "MiniMax-M3".to_string(),
                messages: vec![Message {
                    role: Role::User,
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
    fn chat_messages_keep_current_turn_reasoning_content() {
        let message = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    signature: None,
                    state: None,
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
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        state: None,
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
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Need the date".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        state: None,
                        thinking: "Need to call a tool".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "get_date".to_string(),
                        input: json!({}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
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
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Need the date".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        state: None,
                        thinking: "Need to call a tool".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "get_date".to_string(),
                        input: json!({}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "2026-04-23".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "It is 2026-04-23.".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::User,
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
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Explain it".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        state: None,
                        thinking: "Internal explanation plan".to_string(),
                    },
                    ContentBlock::Text {
                        text: "Final answer".to_string(),
                        cache_control: None,
                    },
                ],
            },
            Message {
                role: Role::User,
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
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    signature: None,
                    state: None,
                    thinking: "I should explain step by step.".to_string(),
                },
                ContentBlock::Text {
                    text: "Here is the explanation.".to_string(),
                    cache_control: None,
                },
            ],
        };
        let user_initial = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Explain it".to_string(),
                cache_control: None,
            }],
        };
        let user_follow_up = Message {
            role: Role::User,
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
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "call-no-thinking".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "Cargo.toml"}),
                        caller: None,
                        thought_signature: None,
                    }],
                },
                Message {
                    role: Role::User,
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
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "Previous answer".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: Role::User,
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
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "Prior answer".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: Role::User,
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
                        role: Role::Assistant,
                        content: vec![ContentBlock::Text {
                            text: "Prior answer".to_string(),
                            cache_control: None,
                        }],
                    },
                    Message {
                        role: Role::User,
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
            role: Role::Assistant,
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
                role: Role::User,
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
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "Stable prior answer".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: Role::User,
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
        assert_eq!(warmup.temperature, None);
        assert_eq!(warmup.top_p, None);
        assert_eq!(warmup.reasoning_effort.as_deref(), Some("off"));
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
    fn reasoning_effort_scenario() {
        // Scenario consolidation of: reasoning_effort_uses_deepseek_top_level_thinking_parameter, reasoning_effort_off_disables_top_level_thinking, reasoning_effort_off_is_omitted_for_strict_openai_like_providers, reasoning_effort_atlascloud_speaks_deepseek_dialect, reasoning_effort_modelstudio_writes_nothing_without_a_verified_route, reasoning_effort_moonshot_toggles_thinking, reasoning_effort_edenai_does_not_guess_a_model_dialect, reasoning_effort_ollama_toggles_think_flag
        // from reasoning_effort_uses_deepseek_top_level_thinking_parameter
        {
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
        // from reasoning_effort_off_disables_top_level_thinking
        {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Deepseek);

            assert_eq!(
                body.pointer("/thinking/type").and_then(Value::as_str),
                Some("disabled")
            );
            assert!(body.get("reasoning_effort").is_none());
            assert!(body.get("extra_body").is_none());
        }
        // from reasoning_effort_off_is_omitted_for_strict_openai_like_providers
        {
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
        // from reasoning_effort_atlascloud_speaks_deepseek_dialect
        {
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
        // from reasoning_effort_modelstudio_writes_nothing_without_a_verified_route
        {
            // The provider enum cannot decide DashScope's controls: `enable_thinking`
            // is wrong for the thinking-only models, `reasoning_effort` is only
            // valid for DeepSeek-V4/GLM, and a custom `base_url` on any of these
            // identities is an arbitrary gateway. All four variants must therefore
            // leave the body untouched here — the route shaper in client::chat is
            // the sole writer.
            for provider in [
                ApiProvider::ModelstudioTokenPlan,
                ApiProvider::ModelstudioTokenPlanAnthropic,
                ApiProvider::ModelstudioCodingPlan,
                ApiProvider::ModelstudioCodingPlanAnthropic,
            ] {
                for effort in [None, Some("off"), Some("low"), Some("high"), Some("max")] {
                    let mut body = json!({});
                    apply_reasoning_effort(&mut body, effort, provider);
                    assert_eq!(body, json!({}), "{provider:?} {effort:?}");
                }
            }
        }
        // from reasoning_effort_moonshot_toggles_thinking
        {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some("high"), ApiProvider::Moonshot);
            assert_eq!(body, json!({ "thinking": { "type": "enabled" } }));

            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Moonshot);
            assert_eq!(body, json!({ "thinking": { "type": "disabled" } }));
        }
        // from reasoning_effort_edenai_does_not_guess_a_model_dialect
        {
            for effort in ["off", "low", "medium", "high", "max", "xhigh"] {
                let mut body = json!({});
                apply_reasoning_effort(&mut body, Some(effort), ApiProvider::Edenai);
                assert_eq!(body, json!({}), "unexpected Eden AI fields for {effort}");
            }
        }
        // from reasoning_effort_ollama_toggles_think_flag
        {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some("high"), ApiProvider::Ollama);
            assert_eq!(body, json!({ "think": true }));

            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some("off"), ApiProvider::Ollama);
            assert_eq!(body, json!({ "think": false }));
        }
    }

    /// First-party DeepSeek routes document `reasoning_effort` low/high/max on
    /// the wire (no medium): low is a real cheaper tier, medium rounds up to
    /// high (#52). Hosted DeepSeek-compatible routes keep the historic
    /// low/medium → high collapse because their own wire contracts are not
    /// verified here.
    #[test]
    fn reasoning_effort_deepseek_maps_the_documented_wire_ladder() {
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("low"), ApiProvider::Deepseek);
        assert_eq!(
            body,
            json!({ "reasoning_effort": "low", "thinking": { "type": "enabled" } })
        );

        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("medium"), ApiProvider::Deepseek);
        assert_eq!(
            body,
            json!({ "reasoning_effort": "high", "thinking": { "type": "enabled" } })
        );

        for provider in [ApiProvider::Deepseek, ApiProvider::DeepseekCN] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some("high"), provider);
            assert_eq!(
                body,
                json!({ "reasoning_effort": "high", "thinking": { "type": "enabled" } }),
                "provider {provider:?}"
            );
        }

        for provider in [ApiProvider::Siliconflow, ApiProvider::Deepinfra] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some("low"), provider);
            assert_eq!(
                body,
                json!({ "reasoning_effort": "high", "thinking": { "type": "enabled" } }),
                "hosted route {provider:?} keeps the collapse"
            );
        }
    }

    /// #5055: the Chat and Responses DeepSeek mappings are two spellings of
    /// one table. If they ever disagree, one of them was edited alone.
    #[test]
    fn deepseek_chat_and_responses_wires_agree_with_the_shared_effort_table() {
        use super::deepseek_effort::{
            DEEPSEEK_DEFAULT_EFFORT_TIER, DEEPSEEK_EFFORT_ALIASES, deepseek_effort_tier,
        };

        for &(alias, tier) in DEEPSEEK_EFFORT_ALIASES {
            for provider in [ApiProvider::Deepseek, ApiProvider::DeepseekCN] {
                let mut body = json!({});
                apply_reasoning_effort(&mut body, Some(alias), provider);
                assert_eq!(
                    body.get("reasoning_effort").and_then(Value::as_str),
                    tier.chat_reasoning_effort(),
                    "chat wire disagrees with the table for {alias:?} on {provider:?}"
                );
                assert_eq!(
                    body.pointer("/thinking/type").and_then(Value::as_str),
                    Some(if tier.chat_thinking_enabled() {
                        "enabled"
                    } else {
                        "disabled"
                    }),
                    "chat thinking toggle disagrees with the table for {alias:?}"
                );
            }

            assert_eq!(
                super::responses::responses_reasoning_effort(alias, true),
                Some(tier.responses_effort()),
                "responses wire disagrees with the table for {alias:?}"
            );
        }

        // A spelling the table does not name: the Chat wire writes nothing,
        // the Responses wire must still send a documented label.
        assert_eq!(deepseek_effort_tier("auto"), None);
        let mut body = json!({});
        apply_reasoning_effort(&mut body, Some("auto"), ApiProvider::Deepseek);
        assert_eq!(body, json!({}));
        assert_eq!(
            super::responses::responses_reasoning_effort("auto", true),
            Some(DEEPSEEK_DEFAULT_EFFORT_TIER.responses_effort())
        );
    }

    async fn capture_deepseek_chat_body_for_effort(effort: Option<&str>) -> Value {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-deepseek-effort-ladder",
                "object": "chat.completion",
                "model": "deepseek-v4-pro",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let request = MessageRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "effort ladder capture".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 64,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: effort.map(str::to_string),
            stream: Some(false),
            temperature: None,
            top_p: None,
        };
        let client = deepseek_request_boundary_client(
            crate::config::DEFAULT_DEEPSEEK_BASE_URL,
            server.uri(),
        );
        client
            .create_message(request)
            .await
            .expect("non-streaming request succeeds");

        let requests = server.received_requests().await.expect("recorded request");
        assert_eq!(requests.len(), 1);
        serde_json::from_slice(&requests[0].body).expect("captured request JSON")
    }

    /// Request-body capture per effort level on the first-party DeepSeek chat
    /// route: the wire must carry the documented low/high/max ladder and the
    /// thinking toggle, never an invented value (#52).
    #[tokio::test]
    async fn deepseek_chat_wire_body_tracks_the_documented_effort_ladder() {
        for (effort, expected_effort, expected_thinking) in [
            (Some("low"), Some("low"), Some("enabled")),
            (Some("medium"), Some("high"), Some("enabled")),
            (Some("high"), Some("high"), Some("enabled")),
            (Some("max"), Some("max"), Some("enabled")),
            (Some("off"), None, Some("disabled")),
            (None, None, None),
        ] {
            let body = capture_deepseek_chat_body_for_effort(effort).await;
            assert_eq!(
                body.get("reasoning_effort").and_then(Value::as_str),
                expected_effort,
                "reasoning_effort on the wire for {effort:?}: {body}"
            );
            assert_eq!(
                body.pointer("/thinking/type").and_then(Value::as_str),
                expected_thinking,
                "thinking on the wire for {effort:?}: {body}"
            );
        }
    }

    /// TelecomJS TokenHub: the gateway's OpenAI Chat Completions API does NOT
    /// support `reasoning_effort` or `thinking` fields (#4188 review). Verify
    /// that no reasoning fields are injected for any effort level, since not
    /// every gateway model (qwen-max, deepseek-chat, gpt-4o, claude, etc.)
    /// accepts the same reasoning dialect.
    #[test]
    fn reasoning_effort_telecomjs_does_not_inject_reasoning_fields() {
        for effort in &["off", "low", "medium", "high", "max", "xhigh"] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, Some(effort), ApiProvider::Telecomjs);
            assert!(
                body.get("reasoning_effort").is_none(),
                "TelecomJS must not inject reasoning_effort for effort={effort}: {body}"
            );
            assert!(
                body.get("thinking").is_none(),
                "TelecomJS must not inject thinking for effort={effort}: {body}"
            );
            assert!(
                body.get("think").is_none(),
                "TelecomJS must not inject think for effort={effort}: {body}"
            );
        }
    }

    #[test]
    fn moonshot_uses_codewhale_user_agent_not_kimi_cli_identity() {
        let user_agent = client_user_agent(ApiProvider::Moonshot);

        assert!(user_agent.contains("codewhale/"));
        assert!(!user_agent.to_ascii_lowercase().contains("kimi_cli"));
        assert!(!user_agent.to_ascii_lowercase().contains("kimi-code-cli"));
    }

    #[test]
    fn reasoning_effort_scenario_2() {
        // Scenario consolidation of: reasoning_effort_ollama_cloud_uses_openai_compatible_field, reasoning_effort_uses_nvidia_nim_chat_template_kwargs, reasoning_effort_off_disables_nvidia_nim_thinking, reasoning_effort_uses_openai_compatible_shape_for_fireworks, reasoning_effort_uses_arcee_reasoning_effort_without_thinking_object, reasoning_effort_maps_openrouter_scale_without_deepseek_max_label, reasoning_effort_uses_xiaomi_mimo_thinking_parameter_only, reasoning_effort_zai_uses_documented_thinking_shape
        // from reasoning_effort_ollama_cloud_uses_openai_compatible_field
        {
            for (effort, expected) in [
                ("off", "none"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
                ("max", "max"),
            ] {
                let mut body = json!({});
                apply_reasoning_effort(&mut body, Some(effort), ApiProvider::OllamaCloud);
                assert_eq!(body, json!({ "reasoning_effort": expected }));
            }

            let mut local = json!({});
            apply_reasoning_effort(&mut local, Some("high"), ApiProvider::Ollama);
            assert_eq!(local, json!({ "think": true }));
        }
        // from reasoning_effort_uses_nvidia_nim_chat_template_kwargs
        {
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
        // from reasoning_effort_off_disables_nvidia_nim_thinking
        {
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
        // from reasoning_effort_uses_openai_compatible_shape_for_fireworks
        {
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
        // from reasoning_effort_uses_arcee_reasoning_effort_without_thinking_object
        {
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
        // from reasoning_effort_maps_openrouter_scale_without_deepseek_max_label
        {
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
        // from reasoning_effort_uses_xiaomi_mimo_thinking_parameter_only
        {
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
        // from reasoning_effort_zai_uses_documented_thinking_shape
        {
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
    }

    #[test]
    fn reasoning_effort_minimax_requires_exact_route_to_split_reasoning() {
        let mut body = json!({});
        chat::apply_route_reasoning_controls(
            &mut body,
            ApiProvider::Minimax,
            crate::config::DEFAULT_MINIMAX_BASE_URL,
            crate::config::DEFAULT_MINIMAX_MODEL,
            Some("high"),
        );
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
        chat::apply_route_reasoning_controls(
            &mut body,
            ApiProvider::Minimax,
            crate::config::DEFAULT_MINIMAX_BASE_URL,
            crate::config::DEFAULT_MINIMAX_MODEL,
            Some("max"),
        );
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("adaptive")
        );
        assert!(body.get("reasoning_effort").is_none());

        let mut body = json!({});
        chat::apply_route_reasoning_controls(
            &mut body,
            ApiProvider::Minimax,
            crate::config::DEFAULT_MINIMAX_BASE_URL,
            crate::config::DEFAULT_MINIMAX_MODEL,
            Some("off"),
        );
        assert_eq!(
            body.get("reasoning_split").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            body.pointer("/thinking/type").and_then(Value::as_str),
            Some("disabled")
        );

        let mut body = json!({});
        chat::apply_route_reasoning_controls(
            &mut body,
            ApiProvider::Minimax,
            crate::config::DEFAULT_MINIMAX_BASE_URL,
            crate::config::DEFAULT_MINIMAX_MODEL,
            None,
        );
        assert_eq!(body, json!({ "reasoning_split": true }));

        for (base_url, model) in [
            (
                "https://gateway.example/v1",
                crate::config::DEFAULT_MINIMAX_MODEL,
            ),
            (crate::config::DEFAULT_MINIMAX_BASE_URL, "MiniMax-M2"),
        ] {
            for effort in ["off", "high", "max"] {
                let mut body = json!({});
                chat::apply_route_reasoning_controls(
                    &mut body,
                    ApiProvider::Minimax,
                    base_url,
                    model,
                    Some(effort),
                );
                assert_eq!(body, json!({}), "{base_url} {model} {effort}");
            }
        }
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
    fn chat_tool_scenario() {
        // Scenario consolidation of: chat_tool_strict_flag_is_nested_under_function, chat_tool_wire_shape_omits_anthropic_only_metadata
        // from chat_tool_strict_flag_is_nested_under_function
        {
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
        // from chat_tool_wire_shape_omits_anthropic_only_metadata
        {
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
    fn chat_messages_scenario() {
        // Scenario consolidation of: chat_messages_drop_thinking_only_assistant_for_non_reasoning_model, chat_messages_drop_orphan_tool_results
        // from chat_messages_drop_thinking_only_assistant_for_non_reasoning_model
        {
            let message = Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Thinking {
                    signature: None,
                    state: None,
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
        // from chat_messages_drop_orphan_tool_results
        {
            let messages = vec![Message {
                role: Role::User,
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
    fn chat_messages_include_tool_results_when_call_present() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        state: None,
                        thinking: "Need to inspect the directory".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "list_dir".to_string(),
                        input: json!({}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
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
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        state: None,
                        thinking: "Need to search".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "web.run".to_string(),
                        input: json!({}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
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
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tool-orphan".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "src/main.rs"}),
                    caller: None,
                    thought_signature: None,
                }],
            },
            // No tool result follows — it was removed by compaction.
            Message {
                role: Role::User,
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
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        state: None,
                        thinking: "Need to list files".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-ok".to_string(),
                        name: "list_dir".to_string(),
                        input: json!({}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
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
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "a.rs"}),
                        caller: None,
                        thought_signature: None,
                    },
                    ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "b.rs"}),
                        caller: None,
                        thought_signature: None,
                    },
                    ContentBlock::ToolUse {
                        id: "t3".to_string(),
                        name: "shell".to_string(),
                        input: json!({"cmd": "ls"}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "content a".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t2".to_string(),
                    content: "content b".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            // No result for t3
            Message {
                role: Role::User,
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

    fn telecomjs_client_for(server: &MockServer) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        DeepSeekClient::new(&Config {
            provider: Some("telecomjs".to_string()),
            providers: Some(ProvidersConfig {
                telecomjs: ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    base_url: Some(server.uri()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("TelecomJS client")
    }

    fn edenai_client_for(server: &MockServer) -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        DeepSeekClient::new(&Config {
            provider: Some("edenai".to_string()),
            providers: Some(ProvidersConfig {
                edenai: ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    base_url: Some(format!("{}/v3", server.uri())),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        })
        .expect("Eden AI client")
    }

    async fn mount_models_json(server: &MockServer, status: u16, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn verify_provider_scenario() {
        // Scenario consolidation of: verify_provider_api_key_accepts_mocked_models_success, verify_provider_api_key_returns_status_and_unicode_body_without_panic
        // from verify_provider_api_key_accepts_mocked_models_success
        {
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
        // from verify_provider_api_key_returns_status_and_unicode_body_without_panic
        {
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
    async fn telecomjs_live_catalog_keeps_cross_provider_metadata_unknown() {
        let server = MockServer::start().await;
        let ambiguous_id = codewhale_config::catalog::bundled_catalog_offerings()
            .into_iter()
            .find(|offering| {
                !offering.provider.eq_ignore_ascii_case("telecomjs")
                    && !offering
                        .wire_model_id
                        .eq_ignore_ascii_case(DEFAULT_TELECOMJS_MODEL)
                    && (offering.canonical_model.is_some()
                        || offering.family.is_some()
                        || offering.limit.is_some()
                        || offering.cost.is_some()
                        || offering.reasoning.is_some()
                        || offering.tool_call.is_some())
            })
            .expect("bundled catalog should contain a metadata-bearing non-TelecomJS row")
            .wire_model_id;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {"id": ambiguous_id.clone()},
                    {"id": DEFAULT_TELECOMJS_MODEL}
                ]
            })))
            .mount(&server)
            .await;

        let delta = telecomjs_client_for(&server)
            .fetch_catalog_delta()
            .await
            .expect("TelecomJS catalog delta");
        assert_eq!(delta.provider, "telecomjs");
        assert_eq!(delta.offerings.len(), 2);

        let ambiguous = delta
            .offerings
            .iter()
            .find(|offering| offering.wire_model_id == ambiguous_id)
            .expect("ambiguous cross-provider id");
        assert!(!ambiguous.default_for_provider);
        assert_eq!(ambiguous.endpoint_key, "chat");
        assert_eq!(ambiguous.canonical_model, None);
        assert_eq!(ambiguous.family, None);
        assert_eq!(ambiguous.limit, None);
        assert_eq!(ambiguous.cost, None);
        assert_eq!(ambiguous.modalities, None);
        assert_eq!(ambiguous.attachment, None);
        assert_eq!(ambiguous.reasoning, None);
        assert_eq!(ambiguous.tool_call, None);
        assert_eq!(ambiguous.structured_output, None);
        assert!(ambiguous.reasoning_options.is_empty());
        assert!(matches!(ambiguous.source, CatalogSource::Live { .. }));

        let default = delta
            .offerings
            .iter()
            .find(|offering| offering.wire_model_id == DEFAULT_TELECOMJS_MODEL)
            .expect("TelecomJS default row");
        assert!(default.default_for_provider);
    }

    #[tokio::test]
    async fn edenai_live_catalog_marks_the_default_and_keeps_unknowns_unclaimed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v3/models"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {"id": "synthetic/vendor-model"},
                    {"id": DEFAULT_EDENAI_MODEL}
                ]
            })))
            .mount(&server)
            .await;

        let delta = edenai_client_for(&server)
            .fetch_catalog_delta()
            .await
            .expect("Eden AI catalog delta");
        assert_eq!(delta.provider, "edenai");
        assert_eq!(delta.offerings.len(), 2);

        let unknown = delta
            .offerings
            .iter()
            .find(|offering| offering.wire_model_id == "synthetic/vendor-model")
            .expect("synthetic Eden AI row");
        assert_eq!(unknown.canonical_model, None);
        assert_eq!(unknown.reasoning, None);
        assert_eq!(unknown.tool_call, None);
        assert!(!unknown.default_for_provider);

        let default = delta
            .offerings
            .iter()
            .find(|offering| offering.wire_model_id == DEFAULT_EDENAI_MODEL)
            .expect("Eden AI default row");
        assert!(default.default_for_provider);
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
    async fn fetch_catalog_scenario() {
        // Scenario consolidation of: fetch_catalog_delta_maps_http_statuses_to_typed_errors, fetch_catalog_delta_maps_invalid_json_and_empty_list
        // from fetch_catalog_delta_maps_http_statuses_to_typed_errors
        {
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
        // from fetch_catalog_delta_maps_invalid_json_and_empty_list
        {
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
    fn parse_usage_scenario() {
        // Scenario consolidation of: parse_usage_reads_deepseek_cache_and_reasoning_tokens, parse_usage_saturates_every_u64_token_field, parse_usage_counts_reasoning_tokens_when_completion_tokens_are_zero, parse_usage_derives_completion_tokens_from_total_tokens_when_needed, parse_usage_reads_v4_prompt_tokens_details_cached_tokens, parse_usage_infers_cache_miss_from_selected_hit_source
        // from parse_usage_reads_deepseek_cache_and_reasoning_tokens
        {
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
        // from parse_usage_saturates_every_u64_token_field
        {
            let usage = parse_usage(Some(&json!({
                "input_tokens": u64::MAX,
                "output_tokens": u64::MAX,
                "prompt_cache_hit_tokens": u64::MAX,
                "prompt_cache_miss_tokens": u64::MAX,
                "completion_tokens_details": { "reasoning_tokens": u64::MAX },
                "server_tool_use": {
                    "code_execution_requests": u64::MAX,
                    "tool_search_requests": u64::MAX
                }
            })));
            assert_eq!(usage.input_tokens, u32::MAX);
            assert_eq!(usage.output_tokens, u32::MAX);
            assert_eq!(usage.prompt_cache_hit_tokens, Some(u32::MAX));
            assert_eq!(usage.prompt_cache_miss_tokens, Some(u32::MAX));
            assert_eq!(usage.reasoning_tokens, Some(u32::MAX));
            let server = usage.server_tool_use.expect("server usage");
            assert_eq!(server.code_execution_requests, Some(u32::MAX));
            assert_eq!(server.tool_search_requests, Some(u32::MAX));
        }
        // from parse_usage_counts_reasoning_tokens_when_completion_tokens_are_zero
        {
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
        // from parse_usage_derives_completion_tokens_from_total_tokens_when_needed
        {
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
        // from parse_usage_reads_v4_prompt_tokens_details_cached_tokens
        {
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
        // from parse_usage_infers_cache_miss_from_selected_hit_source
        {
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
    }

    #[test]
    fn client_route_envelope_freezes_saved_minimax_billing_mode_and_wire_model() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = Config {
            provider: Some("minimax".to_string()),
            providers: Some(ProvidersConfig {
                minimax: ProviderConfig {
                    api_key: Some("test-key".to_string()),
                    mode: Some("pay-as-you-go".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        let client = DeepSeekClient::new(&config).expect("MiniMax client");
        let dispatched_at =
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_234, 0).expect("timestamp");
        let route = client.effective_route_envelope("MiniMax-M3", dispatched_at);

        assert_eq!(route.provider, ApiProvider::Minimax);
        assert_eq!(route.provider_identity, "minimax");
        assert_eq!(route.model, "MiniMax-M3");
        assert_eq!(
            route.billing_surface.as_deref(),
            Some(crate::pricing::MINIMAX_PAYG_BILLING_SURFACE)
        );
        assert_eq!(
            route.billing_mode,
            crate::cost_status::RouteBillingMode::Metered
        );
        assert_eq!(route.dispatched_at.timestamp(), 1_234);
    }

    /// Real-shaped Chat-Completions usage payloads from the three providers most
    /// likely to report reasoning tokens, carried end-to-end into pricing.
    ///
    /// Two invariants hold for every fixture: `reasoning_tokens <= output_tokens`,
    /// and pricing never adds reasoning on top of output — dropping the reasoning
    /// field entirely must not change the cost by a single cent.
    #[test]
    fn reasoning_parser_fixtures_never_exceed_or_add_to_billable_output() {
        use crate::config::ApiProvider;
        use crate::pricing::{calculate_turn_cost_estimate_for_provider, token_usage_for_pricing};

        // (label, provider, model, payload)
        let fixtures: [(&str, ApiProvider, &str, serde_json::Value); 3] = [
            (
                "moonshot",
                ApiProvider::Moonshot,
                "kimi-k2.7-code",
                json!({
                    "prompt_tokens": 30_000,
                    "completion_tokens": 2_400,
                    "total_tokens": 32_400,
                    "prompt_tokens_details": { "cached_tokens": 24_000 },
                    "completion_tokens_details": { "reasoning_tokens": 1_900 }
                }),
            ),
            (
                "minimax",
                ApiProvider::Minimax,
                "minimax-m3",
                json!({
                    "prompt_tokens": 12_000,
                    "completion_tokens": 3_000,
                    "total_tokens": 15_000,
                    "prompt_tokens_details": { "cached_tokens": 4_000 },
                    "completion_tokens_details": { "reasoning_tokens": 2_950 }
                }),
            ),
            (
                "openrouter",
                ApiProvider::Openrouter,
                "qwen/qwen3.7-plus",
                json!({
                    "prompt_tokens": 8_000,
                    "completion_tokens": 1_500,
                    "total_tokens": 9_500,
                    "prompt_tokens_details": { "cached_tokens": 2_000 },
                    "completion_tokens_details": { "reasoning_tokens": 1_500 }
                }),
            ),
        ];

        for (label, provider, model, payload) in fixtures {
            let usage = parse_usage(Some(&payload));
            let reasoning = usage.reasoning_tokens.expect("fixture reports reasoning");

            // Invariant 1: reasoning is a subset of the billed completion count.
            assert!(
                reasoning <= usage.output_tokens,
                "{label}: reasoning {reasoning} exceeds output {}",
                usage.output_tokens
            );
            // Billable output is exactly the reported completion count.
            let classes = token_usage_for_pricing(&usage);
            assert_eq!(
                classes.output,
                u64::from(usage.output_tokens),
                "{label}: reasoning leaked into billable output"
            );

            // Invariant 2: pricing does not add reasoning a second time. The same
            // usage with the reasoning field removed must cost the same.
            let without = crate::models::Usage {
                reasoning_tokens: None,
                ..usage.clone()
            };
            assert_eq!(
                calculate_turn_cost_estimate_for_provider(provider, model, &usage),
                calculate_turn_cost_estimate_for_provider(provider, model, &without),
                "{label}: reasoning changed the price"
            );
        }
    }

    /// A payload claiming more reasoning than output contradicts the subset
    /// invariant. That is broken telemetry, so the field is discarded — and it
    /// must never become extra billable output.
    #[test]
    fn pathological_reasoning_above_output_is_rejected_not_billed() {
        let usage = parse_usage(Some(&json!({
            "prompt_tokens": 1_000,
            "completion_tokens": 100,
            "completion_tokens_details": { "reasoning_tokens": 5_000 }
        })));

        assert_eq!(usage.output_tokens, 100, "output stays as reported");
        assert_eq!(
            usage.reasoning_tokens, None,
            "impossible reasoning telemetry is dropped rather than trusted"
        );
        let classes = crate::pricing::token_usage_for_pricing(&usage);
        assert_eq!(classes.output, 100);

        // `completion_tokens: 0` with reasoning present is the *legitimate*
        // shape this filter must not break: providers that report only reasoning
        // set output from it, keeping reasoning == output.
        let zero_output = parse_usage(Some(&json!({
            "prompt_tokens": 1_000,
            "completion_tokens": 0,
            "completion_tokens_details": { "reasoning_tokens": 12 }
        })));
        assert_eq!(zero_output.output_tokens, 12);
        assert_eq!(zero_output.reasoning_tokens, Some(12));
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

    /// Every queued waiter must be given a *distinct* wake time. `client.rs`
    /// releases the bucket lock before sleeping (`wait_for_rate_limit`), and a
    /// clone of the client shares one `Arc<AsyncMutex<TokenBucket>>` across
    /// sub-agents, so if the bucket hands two waiters the same delay they both
    /// wake at the same instant and fire together — a burst the configured
    /// limit was supposed to prevent.
    #[test]
    fn token_bucket_queues_concurrent_waiters_instead_of_stacking_them() {
        let now = Instant::now();
        let mut bucket = TokenBucket {
            enabled: true,
            capacity: 1.0,
            tokens: 1.0,
            refill_per_sec: 1.0,
            last_refill: now,
        };

        assert!(bucket.delay_until_available(1.0).is_none());
        let first = bucket
            .delay_until_available(1.0)
            .expect("second caller waits for a refill");
        let second = bucket
            .delay_until_available(1.0)
            .expect("third caller waits for a refill");

        assert!(
            first >= Duration::from_millis(900) && first <= Duration::from_millis(1100),
            "unexpected first wait: {first:?}"
        );
        assert!(
            second >= Duration::from_millis(1900) && second <= Duration::from_millis(2100),
            "third caller must queue behind the second, not wake with it: {second:?}"
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
    fn base_url_scenario() {
        // Scenario consolidation of: base_url_security_rejects_insecure_non_local_http, base_url_security_errors_redact_sensitive_url_parts, base_url_security_allows_localhost_http, base_url_security_allows_non_local_http_with_explicit_opt_in
        // from base_url_security_rejects_insecure_non_local_http
        {
            let _lock = ALLOW_INSECURE_HTTP_ENV_LOCK.lock().unwrap();
            let _guard = AllowInsecureHttpEnvGuard::capture();
            unsafe { std::env::remove_var(ALLOW_INSECURE_HTTP_ENV) };

            let err = validate_base_url_security("http://api.deepseek.com")
                .expect_err("non-local insecure HTTP should be rejected");
            assert!(err.to_string().contains("Refusing insecure base URL"));
        }
        // from base_url_security_errors_redact_sensitive_url_parts
        {
            let _lock = ALLOW_INSECURE_HTTP_ENV_LOCK.lock().unwrap();
            let _guard = AllowInsecureHttpEnvGuard::capture();
            unsafe { std::env::remove_var(ALLOW_INSECURE_HTTP_ENV) };

            let err = validate_base_url_security(
                "http://user:secret@example.com/v1?api_key=sk-test&ok=1",
            )
            .expect_err("non-local insecure HTTP should be rejected");
            let message = err.to_string();

            assert!(message.contains("http://***:***@example.com/v1?api_key=***&ok=1"));
            assert!(!message.contains("user:secret"));
            assert!(!message.contains("sk-test"));
        }
        // from base_url_security_allows_localhost_http
        {
            let _lock = ALLOW_INSECURE_HTTP_ENV_LOCK.lock().unwrap();
            let _guard = AllowInsecureHttpEnvGuard::capture();
            unsafe { std::env::remove_var(ALLOW_INSECURE_HTTP_ENV) };

            assert!(validate_base_url_security("http://localhost:8080").is_ok());
            assert!(validate_base_url_security("http://127.0.0.1:8080").is_ok());
        }
        // from base_url_security_allows_non_local_http_with_explicit_opt_in
        {
            let _lock = ALLOW_INSECURE_HTTP_ENV_LOCK.lock().unwrap();
            let _guard = AllowInsecureHttpEnvGuard::capture();
            unsafe { std::env::set_var(ALLOW_INSECURE_HTTP_ENV, "1") };

            assert!(validate_base_url_security("http://192.168.0.110:8000/v1").is_ok());
        }
    }

    /// Serialize tests that mutate `DEEPSEEK_ALLOW_INSECURE_HTTP`; env vars are
    /// process-global and would otherwise leak across security checks.
    static ALLOW_INSECURE_HTTP_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct AllowInsecureHttpEnvGuard {
        prior: Option<std::ffi::OsString>,
        prior_legacy: Option<std::ffi::OsString>,
    }
    impl AllowInsecureHttpEnvGuard {
        fn capture() -> Self {
            let guard = Self {
                prior: std::env::var_os(ALLOW_INSECURE_HTTP_ENV),
                prior_legacy: std::env::var_os(LEGACY_ALLOW_INSECURE_HTTP_ENV),
            };
            // Clear the legacy alias so ambient shell state cannot satisfy
            // the CODEWHALE-first fallback chain behind a test's back.
            unsafe { std::env::remove_var(LEGACY_ALLOW_INSECURE_HTTP_ENV) };
            guard
        }
    }
    impl Drop for AllowInsecureHttpEnvGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(v) => unsafe { std::env::set_var(ALLOW_INSECURE_HTTP_ENV, v) },
                None => unsafe { std::env::remove_var(ALLOW_INSECURE_HTTP_ENV) },
            }
            match &self.prior_legacy {
                Some(v) => unsafe { std::env::set_var(LEGACY_ALLOW_INSECURE_HTTP_ENV, v) },
                None => unsafe { std::env::remove_var(LEGACY_ALLOW_INSECURE_HTTP_ENV) },
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
    fn force_http1_scenario() {
        // Scenario consolidation of: force_http1_unset_is_false, force_http1_truthy_values, force_http1_falsy_values
        // from force_http1_unset_is_false
        {
            let _lock = FORCE_HTTP1_ENV_LOCK.lock().unwrap();
            let _guard = ForceHttp1EnvGuard::capture();
            unsafe { std::env::remove_var("DEEPSEEK_FORCE_HTTP1") };
            assert!(!force_http1_from_env());
        }
        // from force_http1_truthy_values
        {
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
        // from force_http1_falsy_values
        {
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

    fn mid_char_split(text: &str, ch: char) -> usize {
        let needle = ch.to_string();
        let start = text
            .as_bytes()
            .windows(needle.len())
            .position(|window| window == needle.as_bytes())
            .unwrap_or_else(|| panic!("{ch:?} present in {text:?}"));
        start + 1
    }

    #[test]
    fn take_sse_scenario() {
        // Scenario consolidation of: take_sse_line_preserves_multibyte_split_across_reads, take_sse_line_returns_none_without_newline, take_sse_line_reassembles_cjk_and_rejects_invalid_bytes, take_sse_line_rejects_invalid_bytes_without_replacement
        // from take_sse_line_preserves_multibyte_split_across_reads
        {
            // "你好" streamed so the 3-byte '好' straddles a read boundary.
            let full = "data: 你好\n";
            let bytes = full.as_bytes();
            let split = mid_char_split(full, '好');
            let mut buffer: Vec<u8> = Vec::new();
            // First read: no complete line yet.
            buffer.extend_from_slice(&bytes[..split]);
            assert_eq!(take_sse_line(&mut buffer).expect("valid prefix"), None);
            // Second read completes the line; '好' must be intact, not U+FFFD.
            buffer.extend_from_slice(&bytes[split..]);
            let line = take_sse_line(&mut buffer)
                .expect("valid utf-8")
                .expect("a complete line");
            assert_eq!(line, "data: 你好");
            assert!(!line.contains('\u{FFFD}'), "multibyte char was corrupted");
            assert_eq!(extract_sse_data_value(&line), Some("你好"));
            // Buffer fully drained.
            assert!(buffer.is_empty());
        }
        // from take_sse_line_returns_none_without_newline
        {
            let mut buffer = b"data: partial".to_vec();
            assert_eq!(take_sse_line(&mut buffer).expect("valid utf-8"), None);
            assert_eq!(buffer, b"data: partial");
        }
        // from take_sse_line_reassembles_cjk_and_rejects_invalid_bytes
        {
            let full = "data: 测试中文\n";
            let split = mid_char_split(full, '试');
            let mut buffer = full.as_bytes()[..split].to_vec();
            assert_eq!(take_sse_line(&mut buffer).expect("valid prefix"), None);
            buffer.extend_from_slice(&full.as_bytes()[split..]);
            let line = take_sse_line(&mut buffer)
                .expect("valid utf-8")
                .expect("complete line");
            assert_eq!(line, "data: 测试中文");
            assert!(!line.contains('\u{FFFD}'));

            let mut invalid = b"data: ok".to_vec();
            invalid.push(0xFF);
            invalid.push(b'\n');
            let err = take_sse_line(&mut invalid).expect_err("invalid bytes must fail closed");
            assert!(!err.to_string().contains('\u{FFFD}'));
            assert_eq!(err.valid_up_to, 8);
            assert!(
                invalid.is_empty(),
                "invalid line is consumed so retries cannot loop"
            );
        }
        // from take_sse_line_rejects_invalid_bytes_without_replacement
        {
            let mut buffer = b"data: ok".to_vec();
            buffer.push(0xFF);
            buffer.extend_from_slice(b"\n");
            let err = take_sse_line(&mut buffer).expect_err("0xFF is not UTF-8");
            assert_eq!(err.valid_up_to, 8);
            assert!(!err.to_string().contains('\u{FFFD}'));
            assert!(buffer.is_empty(), "invalid line must be drained");
        }
    }

    #[test]
    fn flush_sse_scenario() {
        // Scenario consolidation of: flush_sse_line_reassembles_cjk_and_rejects_invalid_bytes, flush_sse_line_preserves_unterminated_cjk, flush_sse_line_rejects_truncated_multibyte_sequence
        // from flush_sse_line_reassembles_cjk_and_rejects_invalid_bytes
        {
            let text = "data: 你好世界";
            let split = mid_char_split(text, '好');
            let mut buffer = text.as_bytes()[..split].to_vec();
            assert_eq!(take_sse_line(&mut buffer).expect("no newline yet"), None);
            buffer.extend_from_slice(&text.as_bytes()[split..]);
            let line = flush_sse_line(&mut buffer)
                .expect("valid utf-8")
                .expect("unterminated tail");
            assert_eq!(line, "data: 你好世界");
            assert!(!line.contains('\u{FFFD}'));
            assert!(buffer.is_empty());
            assert_eq!(flush_sse_line(&mut buffer).expect("empty"), None);

            let mut invalid = vec![0x80, 0xBF];
            let err = flush_sse_line(&mut invalid).expect_err("invalid flush must fail closed");
            assert!(!err.to_string().contains('\u{FFFD}'));
            assert_eq!(err.valid_up_to, 0);
            assert!(invalid.is_empty());
        }
        // from flush_sse_line_preserves_unterminated_cjk
        {
            let mut buffer = "data: 你好".as_bytes().to_vec();
            let line = flush_sse_line(&mut buffer)
                .expect("valid utf-8")
                .expect("residual line");
            assert_eq!(line, "data: 你好");
            assert!(!line.contains('\u{FFFD}'));
            assert!(buffer.is_empty());
        }
        // from flush_sse_line_rejects_truncated_multibyte_sequence
        {
            let mut buffer = "data: ".as_bytes().to_vec();
            buffer.extend_from_slice(&"好".as_bytes()[..2]);
            let err = flush_sse_line(&mut buffer).expect_err("truncated UTF-8");
            assert_eq!(err.valid_up_to, 6);
            assert!(!err.to_string().contains('\u{FFFD}'));
            assert!(buffer.is_empty());
        }
    }

    #[test]
    fn decode_sse_line_bytes_rejects_invalid_without_replacement() {
        let ok = decode_sse_line_bytes("data: 你好".as_bytes()).expect("valid");
        assert_eq!(ok, "data: 你好");
        assert!(!ok.contains('\u{FFFD}'));

        let err = decode_sse_line_bytes(&[0xFF]).expect_err("bare 0xFF is invalid");
        assert!(!err.to_string().contains('\u{FFFD}'));
        assert_eq!(err.valid_up_to, 0);
    }

    #[test]
    fn extract_sse_scenario() {
        // Scenario consolidation of: extract_sse_data_value_accepts_optional_space, extract_sse_data_value_handles_done_marker, extract_sse_data_value_rejects_non_data_lines
        // from extract_sse_data_value_accepts_optional_space
        {
            assert_eq!(
                extract_sse_data_value("data: {\"ok\":true}"),
                Some("{\"ok\":true}")
            );
            assert_eq!(
                extract_sse_data_value("data:{\"ok\":true}"),
                Some("{\"ok\":true}")
            );
        }
        // from extract_sse_data_value_handles_done_marker
        {
            assert_eq!(extract_sse_data_value("data: [DONE]"), Some("[DONE]"));
            assert_eq!(extract_sse_data_value("data:[DONE]"), Some("[DONE]"));
        }
        // from extract_sse_data_value_rejects_non_data_lines
        {
            assert_eq!(extract_sse_data_value("event: message"), None);
            assert_eq!(extract_sse_data_value(": heartbeat"), None);
        }
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
    fn from_candidate_scenario() {
        // Scenario consolidation of: from_candidate_uses_candidate_base_url_and_wire_model, from_candidate_matches_new_when_config_agrees
        // from from_candidate_uses_candidate_base_url_and_wire_model
        {
            let (_config, route) =
                deepseek_route_for_test("https://route.example.com/v1", "deepseek-v4-pro");

            let client = DeepSeekClient::from_candidate(&route.config, &route.candidate)
                .expect("client should construct from candidate");

            // The transport is bound to the candidate, not re-derived from Config.
            assert_eq!(client.base_url, route.candidate.endpoint().base_url);
            assert_eq!(
                client.default_model,
                route.candidate.wire_model_id().as_str()
            );
        }
        // from from_candidate_matches_new_when_config_agrees
        {
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
    }

    fn route_cap_test_client(wire_format: WireFormat, limits: RouteLimits) -> DeepSeekClient {
        let config = Config {
            provider: Some("custom".to_string()),
            api_key: Some("route-cap-test".to_string()),
            base_url: Some("https://route-cap.example/v1".to_string()),
            default_text_model: Some("DeepSeek-V4-Flash".to_string()),
            ..Config::default()
        };
        DeepSeekClient::from_parts(
            "https://route-cap.example/v1".to_string(),
            "DeepSeek-V4-Flash".to_string(),
            wire_format,
            Some(limits),
            &config,
        )
        .expect("route cap test client")
    }

    #[test]
    fn outbound_seam_clamps_every_dialect_to_the_exact_route_envelope() {
        let _lock = crate::test_support::lock_test_env();
        let _canonical =
            crate::test_support::EnvVarGuard::set("CODEWHALE_MAX_OUTPUT_TOKENS", "384000");
        let _legacy = crate::test_support::EnvVarGuard::remove("DEEPSEEK_MAX_OUTPUT_TOKENS");

        for (limits, expected) in [
            (
                RouteLimits {
                    context_tokens: Some(327_680),
                    ..RouteLimits::default()
                },
                325_632_u64,
            ),
            (
                RouteLimits {
                    context_tokens: Some(327_680),
                    output_tokens: Some(100_000),
                    ..RouteLimits::default()
                },
                100_000,
            ),
            (
                RouteLimits {
                    context_tokens: Some(327_680),
                    output_tokens: Some(128),
                    ..RouteLimits::default()
                },
                128,
            ),
        ] {
            for (wire_format, body_field) in [
                (WireFormat::ChatCompletions, "max_tokens"),
                (WireFormat::Responses, "max_output_tokens"),
                (WireFormat::AnthropicMessages, "max_tokens"),
            ] {
                let client = route_cap_test_client(wire_format, limits);
                let prepared = client
                    .prepare_outbound_request(
                        MessageRequest {
                            model: "DeepSeek-V4-Flash".to_string(),
                            messages: vec![Message {
                                role: Role::User,
                                content: vec![ContentBlock::Text {
                                    text: "route cap".to_string(),
                                    cache_control: None,
                                }],
                            }],
                            max_tokens: 384_000,
                            system: None,
                            tools: None,
                            tool_choice: None,
                            metadata: None,
                            thinking: None,
                            reasoning_effort: Some("max".to_string()),
                            stream: Some(false),
                            temperature: None,
                            top_p: None,
                        },
                        false,
                    )
                    .expect("request prepares through preview/wire seam");
                assert_eq!(
                    prepared.body[body_field].as_u64(),
                    Some(expected),
                    "wire={wire_format:?} limits={limits:?}"
                );
            }
        }
    }

    #[test]
    fn same_protocol_model_switch_rebinds_exact_candidate_identity_and_limits() {
        let _lock = crate::test_support::lock_test_env();
        let _canonical =
            crate::test_support::EnvVarGuard::set("CODEWHALE_MAX_OUTPUT_TOKENS", "384000");
        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(ProvidersConfig {
                openrouter: ProviderConfig {
                    api_key: Some("openrouter-route-cap-test".to_string()),
                    base_url: Some("https://openrouter.ai/api/v1".to_string()),
                    model: Some("deepseek/deepseek-v4-pro".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        let client = DeepSeekClient::new(&config).expect("OpenRouter client resolves");
        assert_eq!(client.wire_format, WireFormat::ChatCompletions);
        assert!(client.route_limits.is_some());

        let rebound = client
            .rebound_for_model_protocol(Some(&config), OPENROUTER_QWEN_3_6_FLASH_MODEL)
            .expect("same-protocol alternate route resolves")
            .expect("model/limit identity change requires a rebound");
        assert_eq!(rebound.wire_format, WireFormat::ChatCompletions);
        assert_eq!(rebound.default_model, OPENROUTER_QWEN_3_6_FLASH_MODEL);
        assert_ne!(rebound.route_limits, client.route_limits);

        let pro_cap = client.effective_max_output_tokens("deepseek/deepseek-v4-pro");
        let alternate_cap = client.effective_max_output_tokens(OPENROUTER_QWEN_3_6_FLASH_MODEL);
        assert!(
            alternate_cap < pro_cap,
            "fixture must prove a smaller same-protocol alternate route: pro={pro_cap}, alternate={alternate_cap}"
        );
        assert_eq!(
            alternate_cap,
            rebound.effective_max_output_tokens(OPENROUTER_QWEN_3_6_FLASH_MODEL),
            "the original bound client and rebound client must resolve the same alternate envelope"
        );
        let prepared = client
            .prepare_outbound_request(
                MessageRequest {
                    model: OPENROUTER_QWEN_3_6_FLASH_MODEL.to_string(),
                    messages: vec![Message {
                        role: Role::User,
                        content: vec![ContentBlock::Text {
                            text: "alternate route cap".to_string(),
                            cache_control: None,
                        }],
                    }],
                    max_tokens: 384_000,
                    system: None,
                    tools: None,
                    tool_choice: None,
                    metadata: None,
                    thinking: None,
                    reasoning_effort: Some("max".to_string()),
                    stream: Some(false),
                    temperature: None,
                    top_p: None,
                },
                false,
            )
            .expect("same-protocol alternate prepares");
        assert_eq!(prepared.body["max_tokens"], json!(alternate_cap));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn fim_non_message_request_is_clamped_to_bound_route() {
        let _lock = crate::test_support::lock_test_env();
        let _canonical =
            crate::test_support::EnvVarGuard::set("CODEWHALE_MAX_OUTPUT_TOKENS", "384000");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/beta/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"text": "middle"}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let base_url = format!("{}/v1", server.uri());
        let config = Config {
            provider: Some("custom".to_string()),
            api_key: Some("fim-cap-test".to_string()),
            base_url: Some(base_url.clone()),
            default_text_model: Some("local-fim".to_string()),
            ..Config::default()
        };
        let client = DeepSeekClient::from_parts(
            base_url,
            "local-fim".to_string(),
            WireFormat::ChatCompletions,
            Some(RouteLimits {
                context_tokens: Some(327_680),
                output_tokens: Some(128),
                ..RouteLimits::default()
            }),
            &config,
        )
        .expect("FIM route client");

        assert_eq!(
            client
                .fim_completion("local-fim", "prefix", "suffix", 4_096)
                .await
                .expect("FIM response"),
            "middle"
        );
        let requests = server
            .received_requests()
            .await
            .expect("recorded FIM request");
        let body: Value = serde_json::from_slice(&requests[0].body).expect("FIM JSON");
        assert_eq!(body["max_tokens"], json!(128));
    }

    #[test]
    fn official_deepseek_flash_binds_responses_request_and_endpoint() {
        let (_config, route) =
            deepseek_route_for_test("https://api.deepseek.com/beta", "deepseek-v4-flash");
        assert_eq!(route.candidate.protocol(), WireFormat::Responses);

        let client = DeepSeekClient::new(&route.config).expect("Flash client resolves");
        assert_eq!(client.wire_format, WireFormat::Responses);

        let prepared = client
            .prepare_outbound_request(
                MessageRequest {
                    model: "deepseek-v4-flash".to_string(),
                    messages: vec![Message {
                        role: Role::User,
                        content: vec![ContentBlock::Text {
                            text: "hello".to_string(),
                            cache_control: None,
                        }],
                    }],
                    max_tokens: 64,
                    system: None,
                    tools: None,
                    tool_choice: None,
                    metadata: None,
                    thinking: None,
                    reasoning_effort: Some("max".to_string()),
                    stream: Some(true),
                    temperature: None,
                    top_p: None,
                },
                true,
            )
            .expect("Flash Responses request prepares");

        assert_eq!(prepared.dialect, WireDialect::OpenAiResponses);
        assert_eq!(prepared.endpoint.url, "https://api.deepseek.com/responses");
        assert_eq!(prepared.body["model"], "deepseek-v4-flash");
        assert_eq!(prepared.body["reasoning"]["effort"], "max");
    }

    #[test]
    fn rebinding_a_chat_bound_client_for_flash_switches_to_responses() {
        // #5042: fleet dispatch binds the child client before the profile
        // model is resolved; a chat-bound DeepSeek client asked to run flash
        // must be rebuilt on the Responses protocol by the central resolver
        // instead of failing deterministically at first send.
        let (_config, route) =
            deepseek_route_for_test("https://api.deepseek.com/beta", "deepseek-v4-pro");
        let client = DeepSeekClient::new(&route.config).expect("pro client resolves");
        assert_eq!(client.wire_format, WireFormat::ChatCompletions);

        let rebound = client
            .rebound_for_model_protocol(Some(&route.config), "deepseek-v4-flash")
            .expect("flash rebind resolves")
            .expect("flash requires a different protocol");
        assert_eq!(rebound.wire_format, WireFormat::Responses);
        assert_eq!(rebound.default_model, "deepseek-v4-flash");

        assert!(
            client
                .rebound_for_model_protocol(Some(&route.config), "deepseek-v4-pro")
                .expect("pro rebind resolves")
                .is_none(),
            "a matching protocol must not rebuild the client"
        );
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
            route.candidate.endpoint().base_url,
            "https://api.example.com/v1"
        );
        assert_eq!(route.candidate.wire_model_id().as_str(), "custom-model-v1");
    }
}
