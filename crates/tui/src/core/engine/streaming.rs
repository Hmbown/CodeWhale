//! Streaming response state and guardrails.
//!
//! This module owns the local state used while decoding one model stream:
//! content block kind tracking, streamed tool-use buffers, transparent retry
//! policy, and scrubbers for text that looks like a forged tool-call wrapper.

use crate::models::ToolCaller;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContentBlockKind {
    Text,
    Thinking,
    ToolUse,
}

#[derive(Debug, Clone)]
pub(super) struct ToolUseState {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: serde_json::Value,
    pub(super) caller: Option<ToolCaller>,
    pub(super) input_buffer: String,
}

/// Default maximum time to wait for a single stream chunk before assuming a stall.
/// **This is the idle timeout** — it resets on every SSE chunk, so long
/// thinking turns that ARE producing reasoning_content stay alive. Only a
/// genuine `chunk_timeout` window of silence kills the stream.
const DEFAULT_STREAM_CHUNK_TIMEOUT_SECS: u64 = 300;
const MIN_STREAM_CHUNK_TIMEOUT_SECS: u64 = 1;
const MAX_STREAM_CHUNK_TIMEOUT_SECS: u64 = 3600;
const STREAM_IDLE_TIMEOUT_ENV: &str = "DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS";

/// Reads the shared stream idle-timeout override used by the SSE client.
pub(super) fn stream_chunk_timeout_secs() -> u64 {
    stream_chunk_timeout_secs_from_env(std::env::var(STREAM_IDLE_TIMEOUT_ENV).ok().as_deref())
}

fn stream_chunk_timeout_secs_from_env(value: Option<&str>) -> u64 {
    value
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STREAM_CHUNK_TIMEOUT_SECS)
        .clamp(MIN_STREAM_CHUNK_TIMEOUT_SECS, MAX_STREAM_CHUNK_TIMEOUT_SECS)
}
/// Maximum total bytes of text/thinking content before aborting the stream.
pub(super) const STREAM_MAX_CONTENT_BYTES: usize = 10 * 1024 * 1024; // 10 MB
/// Sanity backstop for total stream wall-clock duration. **Not** a routine
/// kill switch — the stream chunk idle timeout is the primary stall
/// detector. The wall-clock cap is here only to bound pathological cases
/// (e.g. a server that keeps sending heartbeats forever without progress).
///
/// History: this used to be 300s (5 min) which was too aggressive — V4
/// thinking turns on hard prompts legitimately exceed 5 minutes wall-clock
/// while still emitting reasoning_content chunks the whole way. Bumped to
/// 30 min in v0.6.6 to address `TODO_FIXES.md` #1. Codex defaults to a
/// per-chunk idle of 300s with no wall-clock cap; we keep both layers but
/// give the wall-clock a generous window so it never fires in practice.
pub(super) const STREAM_MAX_DURATION_SECS: u64 = 1800; // 30 minutes (was 300s; #103/#1)
/// Hard cap on consecutive recoverable stream errors before we surface a turn
/// failure. Bumped 3 → 5 in v0.6.7 along with the HTTP/2 keepalive defaults
/// (#103) — keepalive should make spurious decode errors rarer, so we can
/// tolerate a longer streak before giving up on the turn.
pub(super) const MAX_STREAM_ERRORS_BEFORE_FAIL: u32 = 5;
/// Cap on transparent stream-level retries — these only happen when the wire
/// dies before any content was streamed, so DeepSeek hasn't billed us and
/// the user hasn't seen anything. Two attempts is enough to ride out a
/// flaky edge node without amplifying real outages (#103).
pub(super) const MAX_TRANSPARENT_STREAM_RETRIES: u32 = 2;

/// Decide whether a stream error is eligible for a transparent retry.
///
/// True only when ALL three conditions hold:
/// 1. No content has been received on the current attempt — otherwise DeepSeek
///    has already billed us for output tokens and the user has seen partial
///    deltas; resending would double-bill and desync the UI.
/// 2. We still have transparent-retry budget remaining.
/// 3. The turn has not been cancelled.
///
/// Extracted as a pure function so the four #103 retry cases can be exercised
/// in unit tests without booting the full engine state machine.
pub(super) fn should_transparently_retry_stream(
    any_content_received: bool,
    transparent_attempts: u32,
    cancelled: bool,
) -> bool {
    !any_content_received && transparent_attempts < MAX_TRANSPARENT_STREAM_RETRIES && !cancelled
}

pub(crate) const TOOL_CALL_START_MARKERS: [&str; 5] = [
    "[TOOL_CALL]",
    "<deepseek:tool_call",
    "<tool_call",
    "<invoke ",
    "<function_calls>",
];

pub(crate) const TOOL_CALL_END_MARKERS: [&str; 5] = [
    "[/TOOL_CALL]",
    "</deepseek:tool_call>",
    "</tool_call>",
    "</invoke>",
    "</function_calls>",
];

/// Compact one-shot notice emitted when a model attempts to forge a tool-call
/// wrapper in plain text instead of using the API tool channel. The visible
/// content is still scrubbed; this exists so the user can see why their text
/// shrank.
pub(crate) const FAKE_WRAPPER_NOTICE: &str =
    "Stripped non-API tool-call wrapper from model output (use the API tool channel)";

/// True if `text` contains any of the known fake-wrapper start markers. Used by
/// the streaming loop to decide whether to emit `FAKE_WRAPPER_NOTICE`.
pub(crate) fn contains_fake_tool_wrapper(text: &str) -> bool {
    TOOL_CALL_START_MARKERS.iter().any(|m| text.contains(m))
}

fn find_first_marker(text: &str, markers: &[&str]) -> Option<(usize, usize)> {
    markers
        .iter()
        .filter_map(|marker| text.find(marker).map(|idx| (idx, marker.len())))
        .min_by_key(|(idx, _)| *idx)
}

pub(crate) fn filter_tool_call_delta(delta: &str, in_tool_call: &mut bool) -> String {
    if delta.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    let mut rest = delta;

    loop {
        if *in_tool_call {
            let Some((idx, len)) = find_first_marker(rest, &TOOL_CALL_END_MARKERS) else {
                break;
            };
            rest = &rest[idx + len..];
            *in_tool_call = false;
        } else {
            let Some((idx, len)) = find_first_marker(rest, &TOOL_CALL_START_MARKERS) else {
                output.push_str(rest);
                break;
            };
            output.push_str(&rest[..idx]);
            rest = &rest[idx + len..];
            *in_tool_call = true;
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_chunk_timeout_defaults_and_clamps_env_values() {
        assert_eq!(stream_chunk_timeout_secs_from_env(None), 300);
        assert_eq!(
            stream_chunk_timeout_secs_from_env(Some("not-a-number")),
            300
        );
        assert_eq!(stream_chunk_timeout_secs_from_env(Some("0")), 1);
        assert_eq!(stream_chunk_timeout_secs_from_env(Some("90")), 90);
        assert_eq!(stream_chunk_timeout_secs_from_env(Some("99999")), 3600);
    }
}
