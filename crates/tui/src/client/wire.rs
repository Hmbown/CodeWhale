//! SSE wire-parsing helpers shared by the Chat Completions and Responses
//! transports: raw-byte SSE line assembly (fail-closed UTF-8, never U+FFFD
//! substitution), `data:` payload extraction, and usage-object parsing into
//! [`Usage`].
//!
//! Extracted mechanically from `client.rs` (#5586); no behavior change.

use serde_json::Value;

use crate::models::{ServerToolUsage, Usage};

pub(crate) const SSE_BACKPRESSURE_HIGH_WATERMARK: usize = 1024 * 1024; // 1 MB
pub(crate) const SSE_BACKPRESSURE_SLEEP_MS: u64 = 10;
pub(crate) const SSE_MAX_LINES_PER_CHUNK: usize = 256;

pub(crate) fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(crate) fn parse_usage(usage: Option<&Value>) -> Usage {
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

pub(crate) fn extract_sse_data_value(line: &str) -> Option<&str> {
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
pub(crate) struct InvalidSseUtf8 {
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
pub(crate) fn decode_sse_line_bytes(bytes: &[u8]) -> Result<&str, InvalidSseUtf8> {
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
pub(crate) fn take_sse_line(buffer: &mut Vec<u8>) -> Result<Option<String>, InvalidSseUtf8> {
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
pub(crate) fn flush_sse_line(buffer: &mut Vec<u8>) -> Result<Option<String>, InvalidSseUtf8> {
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
pub(crate) fn next_sse_line(
    buffer: &mut Vec<u8>,
    at_end: bool,
) -> Result<Option<String>, InvalidSseUtf8> {
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
pub(crate) struct SseLineDecoder {
    buffer: Vec<u8>,
}

#[cfg(test)]
impl SseLineDecoder {
    pub(crate) fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, InvalidSseUtf8> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(line) = take_sse_line(&mut self.buffer)? {
            lines.push(line);
        }
        Ok(lines)
    }

    pub(crate) fn finish(mut self) -> Result<Option<String>, InvalidSseUtf8> {
        flush_sse_line(&mut self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn parse_usage_saturates_every_u64_token_field() {
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
    fn take_sse_line_preserves_multibyte_split_across_reads() {
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

    #[test]
    fn take_sse_line_returns_none_without_newline() {
        let mut buffer = b"data: partial".to_vec();
        assert_eq!(take_sse_line(&mut buffer).expect("valid utf-8"), None);
        assert_eq!(buffer, b"data: partial");
    }

    #[test]
    fn take_sse_line_reassembles_cjk_and_rejects_invalid_bytes() {
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

    #[test]
    fn take_sse_line_rejects_invalid_bytes_without_replacement() {
        let mut buffer = b"data: ok".to_vec();
        buffer.push(0xFF);
        buffer.extend_from_slice(b"\n");
        let err = take_sse_line(&mut buffer).expect_err("0xFF is not UTF-8");
        assert_eq!(err.valid_up_to, 8);
        assert!(!err.to_string().contains('\u{FFFD}'));
        assert!(buffer.is_empty(), "invalid line must be drained");
    }

    #[test]
    fn flush_sse_line_reassembles_cjk_and_rejects_invalid_bytes() {
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

    #[test]
    fn flush_sse_line_preserves_unterminated_cjk() {
        let mut buffer = "data: 你好".as_bytes().to_vec();
        let line = flush_sse_line(&mut buffer)
            .expect("valid utf-8")
            .expect("residual line");
        assert_eq!(line, "data: 你好");
        assert!(!line.contains('\u{FFFD}'));
        assert!(buffer.is_empty());
    }

    #[test]
    fn flush_sse_line_rejects_truncated_multibyte_sequence() {
        let mut buffer = "data: ".as_bytes().to_vec();
        buffer.extend_from_slice(&"好".as_bytes()[..2]);
        let err = flush_sse_line(&mut buffer).expect_err("truncated UTF-8");
        assert_eq!(err.valid_up_to, 6);
        assert!(!err.to_string().contains('\u{FFFD}'));
        assert!(buffer.is_empty());
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
}
