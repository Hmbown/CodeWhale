//! Shared error taxonomy across client, tools, runtime, and UI.
use std::fmt;

use crate::llm_client::LlmError;
use crate::tools::spec::ToolError;

/// Broad category for typed error handling and policy decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Network,
    Authentication,
    Authorization,
    RateLimit,
    Timeout,
    InvalidInput,
    Parse,
    Tool,
    State,
    Internal,
}

/// Severity hint for UI and logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Unified envelope used when crossing subsystem boundaries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ErrorEnvelope {
    pub category: ErrorCategory,
    pub severity: ErrorSeverity,
    pub recoverable: bool,
    pub code: String,
    pub message: String,
    /// Optional next-step hint for the user ("Run /compact …"). Populated
    /// automatically per category/code by [`suggested_action_for`]; `None`
    /// when there is no obviously right next step. Skipped on the wire when
    /// absent so existing serialized envelopes stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<String>,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Network => "network",
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::RateLimit => "rate_limit",
            Self::Timeout => "timeout",
            Self::InvalidInput => "invalid_input",
            Self::Parse => "parse",
            Self::Tool => "tool",
            Self::State => "state",
            Self::Internal => "internal",
        };
        f.write_str(label)
    }
}

impl fmt::Display for ErrorSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        };
        f.write_str(label)
    }
}

impl fmt::Display for ErrorEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.severity, self.code, self.message)?;
        if let Some(action) = &self.suggested_action {
            write!(f, " — {action}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ErrorEnvelope {}

impl ErrorEnvelope {
    #[must_use]
    pub fn new(
        category: ErrorCategory,
        severity: ErrorSeverity,
        recoverable: bool,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let code = code.into();
        let suggested_action = suggested_action_for(category, &code);
        Self {
            category,
            severity,
            recoverable,
            code,
            message: message.into(),
            suggested_action,
        }
    }

    /// Recoverable internal error — stream stalls, transient retries, generic
    /// engine errors that the user can resolve by retrying. Severity is
    /// `Warning` so the UI surfaces it in amber rather than red.
    #[must_use]
    pub fn transient(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::Internal,
            ErrorSeverity::Warning,
            true,
            "transient",
            message,
        )
    }

    /// Non-recoverable internal error — missing client, spawn failure, etc.
    /// Flips the session into offline mode.
    #[must_use]
    pub fn fatal(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::Internal,
            ErrorSeverity::Error,
            false,
            "fatal",
            message,
        )
    }

    /// Authentication failure — fatal and blocks the session.
    #[must_use]
    pub fn fatal_auth(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::Authentication,
            ErrorSeverity::Critical,
            false,
            "auth_fatal",
            message,
        )
    }

    /// Context length / overflow — invalid input, recoverable via /compact.
    #[must_use]
    pub fn context_overflow(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::InvalidInput,
            ErrorSeverity::Error,
            true,
            "context_overflow",
            message,
        )
    }

    /// Recoverable network / transport hiccup.
    #[must_use]
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::Network,
            ErrorSeverity::Warning,
            true,
            "network_transient",
            message,
        )
    }

    /// Tool execution failure.
    #[must_use]
    pub fn tool(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::Tool,
            ErrorSeverity::Error,
            true,
            "tool_failed",
            message,
        )
    }

    /// Build an envelope by classifying a raw error message string. Used at
    /// boundaries where the underlying error type was already stringified.
    #[must_use]
    pub fn classify(message: impl Into<String>, recoverable: bool) -> Self {
        let message = message.into();
        let category = classify_error_message(&message);
        let severity = match category {
            ErrorCategory::Authentication => ErrorSeverity::Critical,
            ErrorCategory::RateLimit | ErrorCategory::Timeout | ErrorCategory::Network => {
                ErrorSeverity::Warning
            }
            ErrorCategory::InvalidInput | ErrorCategory::Authorization | ErrorCategory::Parse => {
                ErrorSeverity::Error
            }
            ErrorCategory::Tool | ErrorCategory::State | ErrorCategory::Internal => {
                if recoverable {
                    ErrorSeverity::Warning
                } else {
                    ErrorSeverity::Error
                }
            }
        };
        Self::new(
            category,
            severity,
            recoverable,
            category.to_string(),
            message,
        )
    }
}

impl From<LlmError> for ErrorEnvelope {
    fn from(value: LlmError) -> Self {
        match value {
            LlmError::RateLimited { message, .. } => Self::new(
                ErrorCategory::RateLimit,
                ErrorSeverity::Warning,
                true,
                "llm_rate_limited",
                message,
            ),
            LlmError::ServerError { status, message } => Self::new(
                ErrorCategory::Internal,
                ErrorSeverity::Error,
                true,
                format!("llm_server_{status}"),
                message,
            ),
            LlmError::NetworkError(message) => Self::new(
                ErrorCategory::Network,
                ErrorSeverity::Error,
                true,
                "llm_network_error",
                message,
            ),
            LlmError::Timeout(duration) => Self::new(
                ErrorCategory::Timeout,
                ErrorSeverity::Warning,
                true,
                "llm_timeout",
                format!("Request timed out after {duration:?}"),
            ),
            LlmError::AuthenticationError(auth) => Self::new(
                ErrorCategory::Authentication,
                ErrorSeverity::Critical,
                false,
                "llm_auth_error",
                auth.to_user_message(),
            ),
            LlmError::AuthorizationError(message) => Self::new(
                ErrorCategory::Authorization,
                ErrorSeverity::Error,
                false,
                "llm_authorization_error",
                message,
            ),
            LlmError::InvalidRequest { message, .. } => Self::new(
                ErrorCategory::InvalidInput,
                ErrorSeverity::Error,
                false,
                "llm_invalid_request",
                message,
            ),
            LlmError::ModelError(message) => Self::new(
                ErrorCategory::InvalidInput,
                ErrorSeverity::Error,
                false,
                "llm_model_error",
                message,
            ),
            LlmError::ContentPolicyError(message) => Self::new(
                ErrorCategory::Authorization,
                ErrorSeverity::Error,
                false,
                "llm_content_policy",
                message,
            ),
            LlmError::ParseError(message) => Self::new(
                ErrorCategory::Parse,
                ErrorSeverity::Error,
                false,
                "llm_parse_error",
                message,
            ),
            LlmError::ContextLengthError(message) => Self::new(
                ErrorCategory::InvalidInput,
                ErrorSeverity::Error,
                false,
                "llm_context_length",
                message,
            ),
            LlmError::Other(message) => Self::new(
                ErrorCategory::Internal,
                ErrorSeverity::Error,
                true,
                "llm_other",
                message,
            ),
        }
    }
}

/// Map an error category/code onto a next-step hint for the user.
///
/// Context overflow is keyed on code rather than category because it shares
/// `ErrorCategory::InvalidInput` with unrelated request failures where the
/// compaction hint would be misleading. Returns `None` when there is no
/// clearly right action.
#[must_use]
pub fn suggested_action_for(category: ErrorCategory, code: &str) -> Option<String> {
    if matches!(code, "context_overflow" | "llm_context_length") {
        return Some("Run /compact to free context, or /restore to rewind.".to_string());
    }
    if category == ErrorCategory::Authentication {
        return Some("Check /provider — your API key may be missing or expired.".to_string());
    }
    None
}

/// Classify an error message string into an ErrorCategory.
///
/// Uses heuristic keyword matching on the lowercased message.
/// This is a replacement for ad-hoc string matching in callers.
#[must_use]
pub fn classify_error_message(message: &str) -> ErrorCategory {
    let lower = message.to_lowercase();

    if lower.contains("maximum context length")
        || lower.contains("context length")
        || lower.contains("context_length")
        || lower.contains("prompt is too long")
        || (lower.contains("requested") && lower.contains("tokens") && lower.contains("maximum"))
        || lower.contains("context window")
    {
        return ErrorCategory::InvalidInput;
    }
    if lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("429")
        || lower.contains("quota")
    {
        return ErrorCategory::RateLimit;
    }
    if lower.contains("timeout") || lower.contains("timed out") {
        return ErrorCategory::Timeout;
    }
    if lower.contains("authentication")
        || lower.contains("auth failed")
        || lower.contains("auth error")
        || lower.contains("unauthorized")
        || lower.contains("api key")
        || lower.contains("invalid key")
        || lower.contains("invalid token")
        || lower.contains("bearer token")
    {
        return ErrorCategory::Authentication;
    }
    if lower.contains("authorization")
        || lower.contains("permission")
        || lower.contains("forbidden")
        || lower.contains("denied")
    {
        return ErrorCategory::Authorization;
    }
    if lower.contains("network")
        || lower.contains("connection")
        || lower.contains("dns")
        || lower.contains("stream read error")
        || lower.contains("error decoding response body")
        || lower.contains("chunk decode error")
        || lower.contains("body decode")
        || lower.contains("temporarily unavailable")
        || lower.contains(" 502 ")
        || lower.contains(" 503 ")
        || lower.contains(" 504 ")
        || lower.starts_with("502 ")
        || lower.starts_with("503 ")
        || lower.starts_with("504 ")
        || lower.ends_with(" 502")
        || lower.ends_with(" 503")
        || lower.ends_with(" 504")
        || lower == "502"
        || lower == "503"
        || lower == "504"
    {
        return ErrorCategory::Network;
    }
    if lower.contains("parse") || lower.contains("syntax") || lower.contains("malformed") {
        return ErrorCategory::Parse;
    }
    if lower.contains("not found")
        || lower.contains("unavailable")
        || lower.contains("not available")
    {
        return ErrorCategory::State;
    }
    if lower.contains("tool") {
        return ErrorCategory::Tool;
    }

    ErrorCategory::Internal
}

impl From<ToolError> for ErrorEnvelope {
    fn from(value: ToolError) -> Self {
        match value {
            ToolError::InvalidInput { message } => Self::new(
                ErrorCategory::InvalidInput,
                ErrorSeverity::Error,
                false,
                "tool_invalid_input",
                message,
            ),
            ToolError::MissingField { field } => Self::new(
                ErrorCategory::InvalidInput,
                ErrorSeverity::Error,
                false,
                "tool_missing_field",
                format!("Missing required field: {field}"),
            ),
            ToolError::PathEscape { path } => Self::new(
                ErrorCategory::Authorization,
                ErrorSeverity::Error,
                false,
                "tool_path_escape",
                format!("Path escapes workspace: {}", path.display()),
            ),
            ToolError::ExecutionFailed { message } => Self::new(
                ErrorCategory::Tool,
                ErrorSeverity::Error,
                true,
                "tool_execution_failed",
                message,
            ),
            ToolError::Timeout { seconds } => Self::new(
                ErrorCategory::Timeout,
                ErrorSeverity::Warning,
                true,
                "tool_timeout",
                format!("Tool timed out after {seconds}s"),
            ),
            ToolError::NotAvailable { message } => Self::new(
                ErrorCategory::State,
                ErrorSeverity::Error,
                false,
                "tool_not_available",
                message,
            ),
            ToolError::PermissionDenied { message } => Self::new(
                ErrorCategory::Authorization,
                ErrorSeverity::Error,
                false,
                "tool_permission_denied",
                message,
            ),
        }
    }
}

/// Stream‑level error discriminated by origin.
///
/// Each variant maps to an `ErrorCategory` so the UI can render
/// stream‑specific icons or formatting. Wired into engine.rs at the three
/// stream guard sites (chunk timeout, max-bytes overflow, max-duration).
#[derive(Debug, Clone)]
pub enum StreamError {
    /// Stream stalled — no chunk received within the idle timeout.
    Stall { timeout_secs: u64 },
    /// Stream exceeded content size limit.
    Overflow { limit_bytes: usize },
    /// Stream exceeded wall‑clock duration limit.
    DurationLimit { limit_secs: u64 },
}

impl StreamError {
    /// Convert directly into an `ErrorEnvelope` for emission on the engine
    /// event channel. Stalls are warning-severity and recoverable; size and
    /// duration limits are errors (the user must restart the turn).
    #[must_use]
    pub fn into_envelope(self) -> ErrorEnvelope {
        match self {
            Self::Stall { timeout_secs } => ErrorEnvelope::new(
                ErrorCategory::Timeout,
                ErrorSeverity::Warning,
                true,
                "stream_stall",
                format!("Stream stalled: no data received for {timeout_secs}s, closing stream"),
            ),
            Self::Overflow { limit_bytes } => ErrorEnvelope::new(
                ErrorCategory::Internal,
                ErrorSeverity::Error,
                true,
                "stream_overflow",
                format!("Stream exceeded maximum content size of {limit_bytes} bytes, closing"),
            ),
            Self::DurationLimit { limit_secs } => ErrorEnvelope::new(
                ErrorCategory::Timeout,
                ErrorSeverity::Error,
                true,
                "stream_duration_limit",
                format!("Stream exceeded maximum duration of {limit_secs}s, closing"),
            ),
        }
    }
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stall { timeout_secs } => {
                write!(f, "Stream stalled after {timeout_secs}s idle")
            }
            Self::Overflow { limit_bytes } => {
                write!(f, "Stream exceeded {limit_bytes} bytes limit")
            }
            Self::DurationLimit { limit_secs } => {
                write!(f, "Stream exceeded {limit_secs}s duration limit")
            }
        }
    }
}

impl std::error::Error for StreamError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(msg: &str) -> ErrorCategory {
        classify_error_message(msg)
    }

    // === suggested_action population (quick-fix batch, v0.8.67) ===

    #[test]
    fn context_overflow_envelope_suggests_compact_or_restore() {
        let envelope = ErrorEnvelope::context_overflow("prompt too long");
        assert_eq!(
            envelope.suggested_action.as_deref(),
            Some("Run /compact to free context, or /restore to rewind.")
        );
    }

    #[test]
    fn llm_context_length_error_suggests_compact_or_restore() {
        let envelope: ErrorEnvelope =
            LlmError::ContextLengthError("maximum context length exceeded".to_string()).into();
        assert_eq!(
            envelope.suggested_action.as_deref(),
            Some("Run /compact to free context, or /restore to rewind.")
        );
    }

    #[test]
    fn auth_failure_envelopes_suggest_provider_check() {
        let expected = "Check /provider — your API key may be missing or expired.";
        let fatal = ErrorEnvelope::fatal_auth("Authentication failed: invalid API key");
        assert_eq!(fatal.suggested_action.as_deref(), Some(expected));

        let classified = ErrorEnvelope::classify("unauthorized: api key rejected", false);
        assert_eq!(classified.category, ErrorCategory::Authentication);
        assert_eq!(classified.suggested_action.as_deref(), Some(expected));
    }

    #[test]
    fn unrelated_envelopes_have_no_suggested_action() {
        assert_eq!(ErrorEnvelope::transient("hiccup").suggested_action, None);
        assert_eq!(ErrorEnvelope::network("dns broke").suggested_action, None);
        assert_eq!(ErrorEnvelope::tool("tool exploded").suggested_action, None);
        // Non-overflow invalid input must NOT get the compaction hint.
        let invalid: ErrorEnvelope = LlmError::InvalidRequest {
            status: 400,
            message: "bad payload".to_string(),
        }
        .into();
        assert_eq!(invalid.suggested_action, None);
    }

    #[test]
    fn display_appends_suggested_action_when_present() {
        let envelope = ErrorEnvelope::context_overflow("prompt too long");
        let rendered = envelope.to_string();
        assert!(rendered.contains("Run /compact to free context, or /restore to rewind."));

        let plain = ErrorEnvelope::transient("hiccup");
        assert!(!plain.to_string().contains(" — "));
    }

    #[test]
    fn suggested_action_is_skipped_on_the_wire_when_absent() {
        let plain = ErrorEnvelope::transient("hiccup");
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("suggested_action"));

        // Old envelopes without the field still deserialize.
        let legacy = r#"{"category":"internal","severity":"warning","recoverable":true,"code":"transient","message":"m"}"#;
        let parsed: ErrorEnvelope = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.suggested_action, None);
    }

    #[test]
    fn invalid_input_catches_context_overflow_phrasings() {
        // Provider phrasing varies: DeepSeek/OpenAI/Anthropic/etc each
        // surface context-overflow as a slightly different string.
        // The classifier needs all of them on the same branch.
        for msg in [
            "This model's maximum context length is 1000000 tokens",
            "Error: context_length_exceeded",
            "Your prompt is too long for the current model",
            "You requested 100000 tokens but the maximum is 65536",
            "request exceeds context window",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::InvalidInput,
                "expected InvalidInput for `{msg}`",
            );
        }
    }

    #[test]
    fn rate_limit_catches_429_and_quota_phrasings() {
        for msg in [
            "Rate limit reached for gpt-4",
            "Too Many Requests",
            "HTTP 429 from upstream",
            "Your quota has been exceeded",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::RateLimit,
                "expected RateLimit for `{msg}`",
            );
        }
    }

    #[test]
    fn timeout_catches_both_spellings() {
        assert_eq!(classify("connection timeout"), ErrorCategory::Timeout);
        assert_eq!(
            classify("request timed out after 30s"),
            ErrorCategory::Timeout
        );
    }

    #[test]
    fn network_catches_stream_body_decode_failures() {
        for msg in [
            "Warn Stream read error: error decoding response body",
            "Stream read error: error decoding response body",
            "chunk decode error",
            "provider body decode failed mid-stream",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::Network,
                "expected Network for `{msg}`",
            );
        }
    }

    #[test]
    fn authentication_beats_authorization_when_api_key_phrasing_is_used() {
        // "api key" landing on Authentication (not Authorization) keeps
        // the operator-facing message correct: the user needs to fix
        // their key, not their permissions.
        for msg in [
            "Invalid API key provided",
            "Authentication failed",
            "401 Unauthorized",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::Authentication,
                "expected Authentication for `{msg}`",
            );
        }
    }

    #[test]
    fn llm_auth_error_envelope_renders_context_without_secret() {
        let api_key = "tp-secret-token-plan-value";
        let env = ErrorEnvelope::from(LlmError::from_http_response_with_request_context(
            401,
            &format!("Invalid API Key: {api_key}"),
            Some("Xiaomi MiMo"),
            Some("https://token-plan-sgp.xiaomimimo.com/v1"),
            Some("mimo-v2.5"),
            Some("env"),
            Some(api_key),
        ));

        assert_eq!(env.category, ErrorCategory::Authentication);
        assert_eq!(env.severity, ErrorSeverity::Critical);
        assert!(!env.recoverable);
        assert!(env.message.contains("provider: Xiaomi MiMo"));
        assert!(
            env.message
                .contains("base URL authority: token-plan-sgp.xiaomimimo.com")
        );
        assert!(env.message.contains("model: mimo-v2.5"));
        assert!(env.message.contains("key source: env"));
        assert!(env.message.contains("key fingerprint: tp-... (len=26)"));
        assert!(env.message.contains("key type: Xiaomi MiMo Token Plan key"));
        assert!(!env.message.contains(api_key));
        assert!(!env.message.contains("secret-token-plan-value"));
    }

    #[test]
    fn authorization_catches_forbidden_and_denied() {
        for msg in [
            "403 Forbidden",
            "Authorization failed: Arcee AI API returned Cloudflare Access Denied",
            "Permission denied for resource",
            "Tool 'edit_file' denied by user",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::Authorization,
                "expected Authorization for `{msg}`",
            );
        }
    }

    #[test]
    fn network_catches_dns_connection_5xx() {
        for msg in [
            "Network is unreachable",
            "Connection reset by peer",
            "DNS resolution failed for api.deepseek.com",
            "503 Service Unavailable",
            "Upstream returned 502 Bad Gateway",
            "Service temporarily unavailable",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::Network,
                "expected Network for `{msg}`",
            );
        }
        // Edge-case precedence: "504 Gateway Timeout" mentions both
        // a 504 status code AND the word "timeout". The classifier
        // picks Timeout, which is correct — the operator-actionable
        // category for a 504 is "wait and retry" (Timeout semantics)
        // rather than "DNS / connection broken" (Network semantics).
        assert_eq!(
            classify("504 Gateway Timeout"),
            ErrorCategory::Timeout,
            "504 with the literal word `timeout` resolves as Timeout, not Network"
        );
    }

    #[test]
    fn parse_catches_syntax_and_malformed_json() {
        for msg in [
            "Failed to parse response JSON",
            "Syntax error in tool arguments",
            "Malformed event from stream",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::Parse,
                "expected Parse for `{msg}`",
            );
        }
    }

    #[test]
    fn state_catches_not_found_and_unavailable() {
        for msg in [
            "Session not found",
            "Model is unavailable for this provider",
            "Endpoint not available in this region",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::State,
                "expected State for `{msg}`",
            );
        }
    }

    #[test]
    fn tool_is_a_low_priority_catchall_for_tool_keyword() {
        // The Tool branch is the last keyword check before falling
        // through to Internal. Anything mentioning "tool" that didn't
        // match an earlier category should land here.
        assert_eq!(
            classify("Tool returned non-zero exit status"),
            ErrorCategory::Tool,
        );
    }

    #[test]
    fn unknown_messages_fall_through_to_internal() {
        for msg in [
            "Something exploded",
            "panic at the disco",
            "u-200 something happened",
            "",
        ] {
            assert_eq!(
                classify(msg),
                ErrorCategory::Internal,
                "expected Internal for `{msg}`",
            );
        }
    }

    #[test]
    fn classifier_is_case_insensitive() {
        // The function lowercases internally — every category must
        // match regardless of input casing.
        assert_eq!(classify("RATE LIMIT EXCEEDED"), ErrorCategory::RateLimit);
        assert_eq!(classify("TimeOut"), ErrorCategory::Timeout);
        assert_eq!(classify("UNAUTHORIZED"), ErrorCategory::Authentication);
    }

    #[test]
    fn precedence_invalid_input_beats_tool() {
        // A "context length" tool error should classify as
        // InvalidInput, not Tool — InvalidInput is the more actionable
        // category (the user needs to shorten their prompt; "tool
        // failure" wouldn't tell them that).
        assert_eq!(
            classify("tool returned: maximum context length is 1000000"),
            ErrorCategory::InvalidInput,
        );
    }

    #[test]
    fn precedence_timeout_beats_network() {
        // A timeout that mentions a network call should still classify
        // as Timeout — the retry policy for timeouts is gentler than
        // for outright network failures.
        assert_eq!(
            classify("network call timed out after 30s"),
            ErrorCategory::Timeout,
        );
    }

    #[test]
    fn precedence_rate_limit_beats_authentication() {
        // 429 messages sometimes mention "api" or "auth" tokens, but
        // RateLimit's retry semantics (back off + retry) are what the
        // operator actually wants.
        assert_eq!(
            classify("Rate limit on your API quota exceeded"),
            ErrorCategory::RateLimit,
        );
    }

    #[test]
    fn classifier_handles_unicode_safely() {
        // Unicode shouldn't trip the lowercase step or the keyword
        // scan — Chinese/Japanese error messages from
        // OpenAI-compatible providers go through the same path.
        assert_eq!(
            classify("\u{8d85}\u{51fa}\u{6700}\u{5927}\u{4e0a}\u{4e0b}\u{6587} context length"),
            ErrorCategory::InvalidInput,
        );
        // Pure-Chinese messages with no keyword match land on Internal.
        assert_eq!(
            classify("\u{4e0d}\u{77e5}\u{9053}\u{600e}\u{4e48}\u{56de}\u{4e8b}"),
            ErrorCategory::Internal,
        );
    }

    #[test]
    fn error_envelope_display_includes_severity_code_message() {
        let env = ErrorEnvelope::new(
            ErrorCategory::Network,
            ErrorSeverity::Warning,
            true,
            "net_transient",
            "DNS resolution failed",
        );
        assert_eq!(
            format!("{env}"),
            "[warning] net_transient: DNS resolution failed"
        );
    }

    #[test]
    fn error_category_display_round_trips_via_snake_case() {
        // The snake_case labels are what crosses the wire / hits logs;
        // pin them so a future rename doesn't silently shift consumer
        // contracts.
        assert_eq!(format!("{}", ErrorCategory::Network), "network");
        assert_eq!(format!("{}", ErrorCategory::RateLimit), "rate_limit");
        assert_eq!(format!("{}", ErrorCategory::InvalidInput), "invalid_input");
        assert_eq!(format!("{}", ErrorSeverity::Critical), "critical");
    }
}
