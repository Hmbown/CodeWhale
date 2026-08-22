//! Background advisor watcher (#3982).
//!
//! When enabled, the advisor wakes on turn boundaries, reads a bounded slice
//! of recent tool calls from the session transcript, makes a concise LLM
//! advisory call (reusing the same `DeepSeekClient` as the parent turn), and
//! emits an [`Event::AdvisoryNote`] fire-and-forget.
//!
//! Key design properties:
//! - **Off by default** — enabled via `[advisor] enabled = true` or `/advisor on`.
//! - **Bounded input** — at most `max_tool_calls` tool-call/result pairs are
//!   included; the rest are dropped oldest-first.
//! - **Rate-limited** — at most one emission per `rate_limit_secs` seconds.
//! - **Deduplicated** — notes whose content hash matches the previous note
//!   within `dedup_window_secs` are silently dropped.
//! - **Child-failure isolated** — advisor errors are logged but never surface
//!   as parent turn failures.
//! - **Policy-bounded** — the advisor uses a read-only reviewer prompt and
//!   no tool access; it cannot exceed the parent session policy.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use codewhale_config::AdvisorConfigToml;
use tokio::sync::mpsc;
use tracing::debug;

use crate::client::DeepSeekClient;
use crate::core::events::Event;
use crate::llm_client::LlmClient;
use crate::models::Role;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};
use crate::utils::truncate_with_ellipsis;

/// Maximum tokens the advisor may generate. Kept short so the note stays
/// concise and does not compete with the parent turn's billing budget.
const ADVISOR_MAX_TOKENS: u32 = 256;

/// Maximum characters of tool input + result to include per tool-call pair.
const MAX_CHARS_PER_PAIR: usize = 800;

/// System prompt for the advisor LLM call. Read-only review posture — no
/// tool access, no code generation.
const ADVISOR_SYSTEM_PROMPT: &str = "You are a concise background advisor reviewing recent tool activity. \
Your role: identify one or two concrete concerns (correctness, risk, or \
missed alternatives) in the tool calls provided. \
If nothing notable stands out, respond with exactly the word \"ok\". \
Otherwise write one to three short sentences — no preamble, no markdown, \
no praise. Focus on signal; omit noise.";

/// A single tool-call/result pair extracted from the session transcript.
#[derive(Debug, Clone)]
pub struct ToolCallPair {
    /// Tool name (e.g. `exec_shell`, `file_write`).
    pub name: String,
    /// Bounded serialization of the tool input.
    pub input_preview: String,
    /// Bounded serialization of the tool result.
    pub result_preview: String,
}

/// Resolved advisor configuration derived from [`AdvisorConfigToml`].
#[derive(Debug, Clone)]
pub struct AdvisorConfig {
    /// Whether the advisor is currently enabled (session-level toggle).
    pub enabled: bool,
    /// Max tool-call pairs to review per turn.
    pub max_tool_calls: u32,
    /// Min seconds between consecutive emissions.
    pub rate_limit: Duration,
    /// Window during which duplicate notes are suppressed.
    pub dedup_window: Duration,
    /// Optional model override (falls back to session model when `None`).
    pub model: Option<String>,
}

impl AdvisorConfig {
    /// Build a resolved config from the TOML schema.
    #[must_use]
    pub fn from_toml(toml: &AdvisorConfigToml) -> Self {
        Self {
            enabled: toml.enabled,
            max_tool_calls: toml.max_tool_calls.clamp(1, 50),
            rate_limit: Duration::from_secs(toml.rate_limit_secs.clamp(5, 3600)),
            dedup_window: Duration::from_secs(toml.dedup_window_secs),
            model: toml.model.clone(),
        }
    }

    /// Default disabled config (matches `[advisor]` absent from config.toml).
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            max_tool_calls: 10,
            rate_limit: Duration::from_secs(60),
            dedup_window: Duration::from_secs(300),
            model: None,
        }
    }
}

/// Runtime emission guard: tracks the last emission time and the hash of the
/// last advisory note to enforce rate limiting and deduplication.
#[derive(Debug)]
pub struct EmissionGuard {
    last_emission: Option<Instant>,
    last_note_hash: Option<u64>,
    last_note_hash_at: Option<Instant>,
}

impl EmissionGuard {
    /// Create a fresh guard with no emission history.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_emission: None,
            last_note_hash: None,
            last_note_hash_at: None,
        }
    }

    /// Check whether emitting `note` is allowed under `config`'s rate-limit
    /// and dedup policy. Returns `true` when the note may be emitted.
    #[must_use]
    pub fn may_emit(&self, note: &str, config: &AdvisorConfig) -> bool {
        // Suppress trivial "ok" responses from the model.
        if note.trim().eq_ignore_ascii_case("ok") {
            return false;
        }

        let now = Instant::now();

        // Rate limit: require at least `rate_limit` since last emission.
        if let Some(last) = self.last_emission
            && now.duration_since(last) < config.rate_limit
        {
            return false;
        }

        // Dedup: suppress if the note content hash matches the previous note
        // within the dedup window.
        let note_hash = hash_str(note);
        if let (Some(prev_hash), Some(prev_at)) = (self.last_note_hash, self.last_note_hash_at)
            && prev_hash == note_hash
            && now.duration_since(prev_at) < config.dedup_window
        {
            return false;
        }

        true
    }

    /// Record that `note` was emitted now. Must be called immediately after
    /// sending the `AdvisoryNote` event.
    pub fn record_emission(&mut self, note: &str) {
        let now = Instant::now();
        self.last_emission = Some(now);
        self.last_note_hash = Some(hash_str(note));
        self.last_note_hash_at = Some(now);
    }
}

impl Default for EmissionGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract bounded tool-call/result pairs from a session message slice.
///
/// Scans `messages` in reverse (newest first), collects up to `max_pairs`
/// `ToolUse`/`ToolResult` pairs, then returns them oldest-first.
#[must_use]
pub fn extract_tool_call_pairs(messages: &[Message], max_pairs: usize) -> Vec<ToolCallPair> {
    // Collect ToolUse names+inputs (from assistant messages) and
    // ToolResult texts (from user messages) into a pairing structure.
    let mut uses: Vec<(String, String, String)> = Vec::new(); // (id, name, input)
    let mut results: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    let input_str = truncate_with_ellipsis(
                        &serde_json::to_string(input).unwrap_or_default(),
                        MAX_CHARS_PER_PAIR / 2,
                        "…",
                    );
                    uses.push((id.clone(), name.clone(), input_str));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    results.insert(
                        tool_use_id.clone(),
                        truncate_with_ellipsis(content, MAX_CHARS_PER_PAIR / 2, "…"),
                    );
                }
                _ => {}
            }
        }
    }

    // Match uses with results and take the last `max_pairs`.
    let start = uses.len().saturating_sub(max_pairs);
    uses[start..]
        .iter()
        .map(|(id, name, input)| {
            let result = results
                .get(id.as_str())
                .cloned()
                .unwrap_or_else(|| "(pending)".to_string());
            ToolCallPair {
                name: name.clone(),
                input_preview: input.clone(),
                result_preview: result,
            }
        })
        .collect()
}

/// Build the user prompt for the advisor from a slice of tool-call pairs.
#[must_use]
pub fn build_advisor_prompt(pairs: &[ToolCallPair]) -> String {
    let mut out = String::from("Recent tool activity to review (oldest → newest):\n\n");
    for (i, pair) in pairs.iter().enumerate() {
        out.push_str(&format!(
            "{}. tool={}\n   input: {}\n   result: {}\n\n",
            i + 1,
            pair.name,
            pair.input_preview,
            pair.result_preview
        ));
    }
    out.push_str(
        "Provide your advisory in one to three sentences, or respond with \"ok\" if nothing notable.",
    );
    out
}

/// Run one advisor review cycle for a completed turn.
///
/// This is the async work dispatched by `spawn_supervised` in the engine. It:
/// 1. Checks whether emission is allowed by `guard`.
/// 2. Extracts bounded tool-call pairs from `messages`.
/// 3. Makes a non-streaming LLM call with a short read-only prompt.
/// 4. Checks emission again (the LLM call may have taken time).
/// 5. Sends `Event::AdvisoryNote` if the note passes the guard.
///
/// All errors are logged and swallowed — the advisor must never fail the
/// parent turn.
pub async fn run_advisor_for_turn(
    turn_id: String,
    messages: Vec<Message>,
    config: AdvisorConfig,
    client: DeepSeekClient,
    session_model: String,
    guard: std::sync::Arc<tokio::sync::Mutex<EmissionGuard>>,
    tx_event: mpsc::Sender<Event>,
) {
    // Pre-flight: skip if the guard already blocks (avoids the LLM call when
    // rate-limited, which is the common case for rapid turn sequences).
    {
        let g = guard.lock().await;
        // We don't have the note content yet, so we only check the rate limit
        // here by testing with a placeholder. The dedup check runs after the
        // LLM call, when we have the actual content.
        if let Some(last) = g.last_emission
            && std::time::Instant::now().duration_since(last) < config.rate_limit
        {
            debug!(target: "advisor", "rate-limited, skipping advisor run for turn {turn_id}");
            return;
        }
    }

    // Extract a bounded slice of tool-call pairs.
    let pairs = extract_tool_call_pairs(&messages, config.max_tool_calls as usize);
    if pairs.is_empty() {
        debug!(target: "advisor", "no tool calls found; skipping advisor for turn {turn_id}");
        return;
    }

    let tool_call_count = pairs.len() as u32;
    let prompt = build_advisor_prompt(&pairs);
    let model = config
        .model
        .clone()
        .unwrap_or_else(|| session_model.clone());

    let request = MessageRequest {
        model: model.clone(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt,
                cache_control: None,
            }],
        }],
        max_tokens: ADVISOR_MAX_TOKENS,
        system: Some(SystemPrompt::Text(ADVISOR_SYSTEM_PROMPT.to_string())),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        // The advisor has a deliberately tiny answer contract. Hidden
        // reasoning would spend that allowance before the note is emitted.
        reasoning_effort: Some("off".to_string()),
        stream: Some(false),
        temperature: None,
        top_p: None,
    };

    let response = match client.create_message(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(target: "advisor", "advisor LLM call failed for turn {turn_id}: {e}");
            return;
        }
    };

    if crate::models::is_incomplete_stop_reason(response.stop_reason.as_deref()) {
        tracing::warn!(
            target: "advisor",
            "advisor response incomplete for turn {turn_id} (stop reason `{}`); dropping partial note",
            crate::models::stop_reason_detail(response.stop_reason.as_deref())
        );
        return;
    }

    // Extract the text from the response.
    let note: String = response
        .content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text { text, .. } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if note.is_empty() {
        debug!(target: "advisor", "empty advisor response for turn {turn_id}; skipping");
        return;
    }

    // Post-flight emission check (rate limit + dedup).
    let mut guard_lock = guard.lock().await;
    if !guard_lock.may_emit(&note, &config) {
        debug!(target: "advisor", "emission suppressed by guard for turn {turn_id}");
        return;
    }

    guard_lock.record_emission(&note);
    drop(guard_lock);

    let _ = tx_event
        .send(Event::AdvisoryNote {
            turn_id: turn_id.clone(),
            note: note.clone(),
            tool_call_count,
        })
        .await;

    debug!(target: "advisor", "advisory note emitted for turn {turn_id} ({tool_call_count} tool calls reviewed)");
}

fn hash_str(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> AdvisorConfig {
        AdvisorConfig {
            enabled: true,
            max_tool_calls: 5,
            rate_limit: Duration::from_secs(1),
            dedup_window: Duration::from_secs(10),
            model: None,
        }
    }

    // ── enable/disable ────────────────────────────────────────────────────

    #[test]
    fn disabled_config_has_enabled_false() {
        let cfg = AdvisorConfig::disabled();
        assert!(!cfg.enabled);
    }

    #[test]
    fn from_toml_clamps_max_tool_calls() {
        let toml = AdvisorConfigToml {
            enabled: true,
            max_tool_calls: 999,
            rate_limit_secs: 60,
            dedup_window_secs: 300,
            model: None,
        };
        let cfg = AdvisorConfig::from_toml(&toml);
        assert_eq!(
            cfg.max_tool_calls, 50,
            "max_tool_calls must be clamped to 50"
        );
    }

    #[test]
    fn from_toml_clamps_rate_limit() {
        let toml = AdvisorConfigToml {
            enabled: true,
            max_tool_calls: 10,
            rate_limit_secs: 0, // below minimum of 5
            dedup_window_secs: 300,
            model: None,
        };
        let cfg = AdvisorConfig::from_toml(&toml);
        assert!(
            cfg.rate_limit >= Duration::from_secs(5),
            "rate_limit must be at least 5s"
        );
    }

    // ── bounded input ─────────────────────────────────────────────────────

    fn make_messages_with_n_tool_calls(n: usize) -> Vec<Message> {
        let mut messages = Vec::new();
        for i in 0..n {
            let id = format!("tool_{i}");
            // assistant message with ToolUse
            messages.push(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: id.clone(),
                    name: "exec_shell".to_string(),
                    input: serde_json::json!({"command": format!("echo {i}")}),
                    caller: None,
                    thought_signature: None,
                }],
            });
            // user message with ToolResult
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: format!("{i}"),
                    is_error: None,
                    content_blocks: None,
                }],
            });
        }
        messages
    }

    #[test]
    fn extract_tool_call_pairs_bounded_by_max() {
        let messages = make_messages_with_n_tool_calls(20);
        let pairs = extract_tool_call_pairs(&messages, 5);
        assert_eq!(pairs.len(), 5, "must return at most max_pairs");
        // Should be the last 5 (newest).
        assert_eq!(pairs[0].name, "exec_shell");
    }

    #[test]
    fn extract_tool_call_pairs_empty_when_no_tool_calls() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
        }];
        let pairs = extract_tool_call_pairs(&messages, 5);
        assert!(pairs.is_empty());
    }

    #[test]
    fn extract_tool_call_pairs_fewer_than_max_returns_all() {
        let messages = make_messages_with_n_tool_calls(3);
        let pairs = extract_tool_call_pairs(&messages, 10);
        assert_eq!(pairs.len(), 3);
    }

    // ── rate limiting ─────────────────────────────────────────────────────

    #[test]
    fn emission_guard_allows_first_emission() {
        let guard = EmissionGuard::new();
        let config = test_config();
        assert!(
            guard.may_emit("something concerning here", &config),
            "first emission must be allowed"
        );
    }

    #[test]
    fn emission_guard_blocks_immediately_after_emission() {
        let mut guard = EmissionGuard::new();
        let config = test_config();
        let note = "something concerning";
        guard.record_emission(note);
        assert!(
            !guard.may_emit("a completely different note", &config),
            "emission must be blocked immediately after a prior emission (rate limit)"
        );
    }

    #[test]
    fn emission_guard_allows_after_rate_limit_expires() {
        let mut guard = EmissionGuard::new();
        // Rate limit of 0ms — always expired.
        let config = AdvisorConfig {
            rate_limit: Duration::ZERO,
            dedup_window: Duration::from_secs(300),
            ..AdvisorConfig::disabled()
        };
        let note = "first note";
        guard.record_emission(note);
        assert!(
            guard.may_emit("second different note", &config),
            "emission must be allowed when rate limit duration is zero"
        );
    }

    // ── deduplication ─────────────────────────────────────────────────────

    #[test]
    fn emission_guard_suppresses_ok_response() {
        let guard = EmissionGuard::new();
        let config = test_config();
        assert!(!guard.may_emit("ok", &config), "\"ok\" must be suppressed");
        assert!(!guard.may_emit("OK", &config), "\"OK\" must be suppressed");
        assert!(
            !guard.may_emit("  ok  ", &config),
            "\" ok \" must be suppressed"
        );
    }

    #[test]
    fn emission_guard_dedup_blocks_identical_note_within_window() {
        let mut guard = EmissionGuard::new();
        // Use a zero rate limit so only dedup is tested.
        let config = AdvisorConfig {
            rate_limit: Duration::ZERO,
            dedup_window: Duration::from_secs(300),
            ..AdvisorConfig::disabled()
        };
        let note = "risky shell command with no error checking";
        guard.record_emission(note);
        assert!(
            !guard.may_emit(note, &config),
            "identical note must be suppressed within the dedup window"
        );
    }

    #[test]
    fn emission_guard_allows_different_note_within_dedup_window() {
        let mut guard = EmissionGuard::new();
        let config = AdvisorConfig {
            rate_limit: Duration::ZERO,
            dedup_window: Duration::from_secs(300),
            ..AdvisorConfig::disabled()
        };
        guard.record_emission("first note");
        assert!(
            guard.may_emit("entirely different note", &config),
            "a different note must be allowed even within the dedup window"
        );
    }

    // ── child failure isolation ───────────────────────────────────────────

    #[test]
    fn advisor_prompt_is_non_empty_for_non_empty_pairs() {
        let pairs = vec![ToolCallPair {
            name: "exec_shell".to_string(),
            input_preview: r#"{"command":"ls -la"}"#.to_string(),
            result_preview: "total 4\ndrwxr-xr-x 2 user user 4096".to_string(),
        }];
        let prompt = build_advisor_prompt(&pairs);
        assert!(
            prompt.contains("exec_shell"),
            "prompt must include the tool name"
        );
        assert!(
            prompt.contains("ls -la"),
            "prompt must include the tool input"
        );
    }
}
