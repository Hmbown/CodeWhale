//! Streaming response state and guardrails.
//!
//! This module owns the local state used while decoding one model stream:
//! content block kind tracking, streamed tool-use buffers, transparent retry
//! policy, and scrubbers for text that looks like a forged tool-call wrapper.

use crate::models::ToolCaller;
use std::time::Duration;

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
/// 30 min in v0.6.6 after long-reasoning turns hit the old cap. Codex defaults to a
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

/// Budget for re-issuing the whole request after a dead stream. Shared by the
/// nothing-streamed outer retry (#103 Phase 3) and the sleep-resume retry
/// (#2990).
pub(super) const MAX_STREAM_RETRIES: u32 = 3;

/// Wall-clock vs monotonic divergence above which we conclude the host slept
/// mid-stream (#2990). `Instant` pauses during system sleep (CLOCK_UPTIME_RAW
/// on macOS, CLOCK_MONOTONIC on Linux) while `SystemTime` keeps advancing, so
/// a large positive gap can only come from a suspend/resume cycle — ordinary
/// network flakes never produce one. Windows `Instant` may keep ticking
/// through sleep, in which case this simply never fires (no behavior change).
pub(super) const SLEEP_GAP_THRESHOLD: Duration = Duration::from_secs(10);

/// True when the gap between wall-clock and monotonic elapsed time since the
/// last stream progress says the host was suspended.
pub(super) fn sleep_gap_detected(monotonic_elapsed: Duration, wallclock_elapsed: Duration) -> bool {
    wallclock_elapsed.saturating_sub(monotonic_elapsed) > SLEEP_GAP_THRESHOLD
}

/// Decide whether a failed stream should be silently re-issued because the
/// host slept mid-turn (#2990).
///
/// Unlike the transparent retry (#103), this fires even after content has
/// streamed: the partial output predates the sleep, the user was not
/// watching, and re-running the identical request is the correct
/// user-visible behavior. The double-billing concern that blocks ordinary
/// post-content retries is accepted here because the alternative is a dead
/// turn the user must re-prompt (and pay for) anyway.
pub(super) fn should_resume_after_sleep(
    sleep_detected: bool,
    retry_attempts: u32,
    cancelled: bool,
) -> bool {
    sleep_detected && retry_attempts < MAX_STREAM_RETRIES && !cancelled
}

pub(crate) const TOOL_CALL_START_MARKERS: [&str; 5] = [
    "[TOOL_CALL]",
    "<codewhale:tool_call",
    "<tool_call",
    "<invoke ",
    "<function_calls>",
];

pub(crate) const TOOL_CALL_END_MARKERS: [&str; 5] = [
    "[/TOOL_CALL]",
    "</codewhale:tool_call>",
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
