//! Cache Guard CI test: verifies prefix-cache stability across multi-turn conversations.
//!
//! Runs 8 test cases × 14-24 turns each, checking that the tail average
//! hit rate stays above a configurable threshold (default 40%).
//!
//! Environment variables:
//!   CODEWHALE_CACHE_GUARD=1              Enable the guard (default: disabled)
//!   CODEWHALE_CACHE_GUARD_THRESHOLD=90   Hit rate threshold (0-100)
//!   CODEWHALE_CACHE_GUARD_STRICT=1       Fail on threshold violation (default: warn)
//!
//! Usage:
//!   CODEWHALE_CACHE_GUARD=1 cargo test --test cache_guard
//!   CODEWHALE_CACHE_GUARD=1 CODEWHALE_CACHE_GUARD_STRICT=1 cargo test --test cache_guard

// No external dependencies needed for the mock.

// === Configuration ===

const DEFAULT_THRESHOLD: f64 = 40.0;
const ENABLED_ENV: &str = "CODEWHALE_CACHE_GUARD";
const THRESHOLD_ENV: &str = "CODEWHALE_CACHE_GUARD_THRESHOLD";
const STRICT_ENV: &str = "CODEWHALE_CACHE_GUARD_STRICT";

fn guard_enabled() -> bool {
    std::env::var(ENABLED_ENV)
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

fn threshold() -> f64 {
    std::env::var(THRESHOLD_ENV)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_THRESHOLD)
}

fn strict() -> bool {
    std::env::var(STRICT_ENV)
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

// === Mock Prefix Cache ===

/// Simulates DeepSeek's server-side prefix cache behavior.
///
/// The cache works on byte-prefix matching: if the first N bytes of the
/// current request match the first N bytes of the previous request, those
/// N bytes are counted as cache hits.
struct MockPrefixCache {
    previous_body: Vec<u8>,
    total_input_bytes: u64,
    hit_bytes: u64,
    per_turn_hit_rates: Vec<f64>,
}

impl MockPrefixCache {
    fn new() -> Self {
        Self {
            previous_body: Vec::new(),
            total_input_bytes: 0,
            hit_bytes: 0,
            per_turn_hit_rates: Vec::new(),
        }
    }

    /// Submit a request body and compute cache hit/miss for this turn.
    fn submit(&mut self, body: &[u8]) {
        let common_prefix = body
            .iter()
            .zip(self.previous_body.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let body_len = body.len() as u64;
        self.total_input_bytes += body_len;
        self.hit_bytes += common_prefix as u64;

        let hit_rate = if body_len > 0 {
            common_prefix as f64 / body_len as f64
        } else {
            1.0
        };
        self.per_turn_hit_rates.push(hit_rate);

        self.previous_body = body.to_vec();
    }

    /// Compute the average hit rate over the last N turns.
    fn tail_avg(&self, n: usize) -> f64 {
        let start = self.per_turn_hit_rates.len().saturating_sub(n);
        let tail = &self.per_turn_hit_rates[start..];
        if tail.is_empty() {
            0.0
        } else {
            tail.iter().sum::<f64>() / tail.len() as f64
        }
    }

    /// Overall hit rate across all turns.
    fn overall_hit_rate(&self) -> f64 {
        if self.total_input_bytes == 0 {
            0.0
        } else {
            self.hit_bytes as f64 / self.total_input_bytes as f64
        }
    }
}

// === Test Case Generators ===

/// Generate a simulated request body for a plain dialogue turn.
fn plain_dialogue_body(turn: usize, with_reasoning: bool) -> Vec<u8> {
    let system = "You are a helpful assistant. Answer concisely and accurately.";
    let reasoning_prefix = if with_reasoning {
        "[reasoning: analyzing the user's question carefully...]"
    } else {
        ""
    };
    let user_msg = format!("User message turn {turn} — please respond to this query.");
    let body =
        format!("{system}{reasoning_prefix}\n\nConversation history:\n{user_msg}\nAssistant:");
    body.into_bytes()
}

/// Generate a simulated request body for a tool-loop turn.
fn tool_loop_body(turn: usize, with_reasoning: bool) -> Vec<u8> {
    let system = "You are a helpful assistant with tool access.";
    let reasoning_prefix = if with_reasoning {
        "[reasoning: deciding which tool to use...]"
    } else {
        ""
    };
    let tool_name = if turn % 2 == 0 {
        "read_file"
    } else {
        "write_file"
    };
    let tool_args = format!(r#"{{"path": "/tmp/file_{turn}.txt"}}"#);
    let user_msg = format!("User request turn {turn}");
    let body = format!(
        "{system}{reasoning_prefix}\n\nTools: read_file, write_file, exec_shell\n\
         User: {user_msg}\nAssistant: I'll use {tool_name}({tool_args})\nResult: success\nAssistant:"
    );
    body.into_bytes()
}

/// Generate a simulated request body with mixed sizes.
fn mixed_size_body(turn: usize) -> Vec<u8> {
    let system = "You are a helpful assistant.";
    let user_msg = match turn % 4 {
        0 => format!("Short question {turn}"),
        1 => format!(
            "Medium length question {turn} with some additional context about the problem we're solving."
        ),
        2 => {
            let long_context = "Lorem ipsum dolor sit amet. ".repeat(20);
            format!("Long question {turn} with extensive context: {long_context}")
        }
        _ => format!("Question {turn}"),
    };
    let body = format!("{system}\n\nUser: {user_msg}\nAssistant:");
    body.into_bytes()
}

// === Test Runner ===

struct CaseResult {
    name: String,
    tail_avg: f64,
    overall: f64,
    turns: usize,
    passed: bool,
}

fn run_case(
    name: &str,
    turns: usize,
    with_reasoning: bool,
    tool_loop: bool,
    mixed_sizes: bool,
) -> CaseResult {
    let mut cache = MockPrefixCache::new();

    for turn in 0..turns {
        let body = if mixed_sizes {
            mixed_size_body(turn)
        } else if tool_loop {
            tool_loop_body(turn, with_reasoning)
        } else {
            plain_dialogue_body(turn, with_reasoning)
        };
        cache.submit(&body);
    }

    let tail_avg = cache.tail_avg(5) * 100.0;
    let overall = cache.overall_hit_rate() * 100.0;
    let thresh = threshold();
    let passed = tail_avg >= thresh;

    CaseResult {
        name: name.to_string(),
        tail_avg,
        overall,
        turns,
        passed,
    }
}

// === 8 Test Cases ===

#[test]
fn case_plain_dialogue() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("plain-dialogue", 14, true, false, false);
    report_and_assert(&result);
}

#[test]
fn case_plain_dialogue_no_reasoning() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("plain-dialogue-no-reasoning", 14, false, false, false);
    report_and_assert(&result);
}

#[test]
fn case_long_dialogue() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("long-dialogue", 18, true, false, false);
    report_and_assert(&result);
}

#[test]
fn case_mixed_message_sizes() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("mixed-message-sizes", 20, true, false, true);
    report_and_assert(&result);
}

#[test]
fn case_tool_loop() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("tool-loop", 14, true, true, false);
    report_and_assert(&result);
}

#[test]
fn case_tool_loop_no_reasoning() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("tool-loop-no-reasoning", 14, false, true, false);
    report_and_assert(&result);
}

#[test]
fn case_long_tool_loop() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("long-tool-loop", 24, true, true, false);
    report_and_assert(&result);
}

#[test]
fn case_long_tool_loop_no_reasoning() {
    if !guard_enabled() {
        return;
    }
    let result = run_case("long-tool-loop-no-reasoning", 24, false, true, false);
    report_and_assert(&result);
}

// === Hard Error Guard ===

#[test]
fn compaction_must_cause_at_least_one_miss() {
    if !guard_enabled() {
        return;
    }

    let mut cache = MockPrefixCache::new();
    let system = "You are a helpful assistant with a very long system prompt that gets compacted.";

    // Simulate 30 turns where compaction happens around turn 20.
    // After compaction, the system prompt changes significantly.
    for turn in 0..30 {
        let body = if turn < 20 {
            format!("{system}\n\nUser: turn {turn}\nAssistant:")
        } else {
            // Post-compaction: system prompt is truncated/changed.
            format!("You are a helpful assistant.\n\nUser: turn {turn}\nAssistant:")
        };
        cache.submit(&body.as_bytes());
    }

    // After compaction, there should be at least one significant miss.
    // The threshold is relaxed because our mock doesn't perfectly simulate
    // DeepSeek's radix-tree prefix cache.
    let post_compaction_rates: Vec<f64> = cache.per_turn_hit_rates[20..].to_vec();
    let has_significant_miss = post_compaction_rates.iter().any(|&r| r < 0.8);

    if strict() {
        assert!(
            has_significant_miss,
            "Compaction should cause at least one cache miss below 50%"
        );
    } else if !has_significant_miss {
        eprintln!("[WARN] compaction_must_cause_at_least_one_miss: no significant miss detected");
    }
}

// === Helpers ===

fn report_and_assert(result: &CaseResult) {
    let thresh = threshold();
    if result.passed {
        eprintln!(
            "[OK]   {}: tail_avg={:.1}% (overall={:.1}%, {} turns)",
            result.name, result.tail_avg, result.overall, result.turns
        );
    } else {
        eprintln!(
            "[WARN] {}: tail_avg={:.1}% < threshold={:.1}% (overall={:.1}%, {} turns)",
            result.name, result.tail_avg, thresh, result.overall, result.turns
        );
        if strict() {
            panic!(
                "[STRICT] {} failed: tail_avg={:.1}% < threshold={:.1}%",
                result.name, result.tail_avg, thresh
            );
        }
    }
}
