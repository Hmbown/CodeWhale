//! Pure-data guardrails for repeated tool-call loops.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

use serde_json::Value;

const IDENTICAL_CALL_BLOCK_THRESHOLD: u32 = 3;
const IDENTICAL_READ_ONLY_CALL_BLOCK_THRESHOLD: u32 = 2;
const BROAD_READ_ONLY_TOOL_LOOP_BLOCK_THRESHOLD: u32 = 6;
const FAILURE_WARN_THRESHOLD: u32 = 3;
const FAILURE_HALT_THRESHOLD: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AttemptDecision {
    Proceed,
    Block {
        kind: AttemptBlockKind,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttemptBlockKind {
    IdenticalToolCall,
    NoProgressToolLoop,
}

impl AttemptBlockKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::IdenticalToolCall => "identical_tool_call",
            Self::NoProgressToolLoop => "no_progress_tool_loop",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OutcomeDecision {
    Continue,
    Warn(String),
    Halt(String),
}

#[derive(Debug, Default)]
pub(super) struct LoopGuard {
    call_counts: HashMap<(String, u64), u32>,
    broad_tool_counts: HashMap<String, u32>,
    failure_counts: HashMap<String, u32>,
}

impl LoopGuard {
    pub(super) fn record_attempt(
        &mut self,
        tool: &str,
        args: &Value,
        read_only: bool,
    ) -> AttemptDecision {
        let key = (tool.to_string(), hash_args(args));
        let count = self.call_counts.entry(key).or_insert(0);
        *count = count.saturating_add(1);
        let identical_threshold = if read_only || is_delegated_tool(tool) {
            IDENTICAL_READ_ONLY_CALL_BLOCK_THRESHOLD
        } else {
            IDENTICAL_CALL_BLOCK_THRESHOLD
        };
        if *count >= identical_threshold {
            return AttemptDecision::Block {
                kind: AttemptBlockKind::IdenticalToolCall,
                message: format!(
                    "This `{tool}` call already ran this turn with the same arguments. Use the prior result and synthesize from the evidence you have; do not repeat the same broad work unless the user asks for a focused follow-up."
                ),
            };
        }

        if let Some(threshold) = no_progress_attempt_threshold(tool, read_only) {
            let total = self.broad_tool_counts.entry(tool.to_string()).or_insert(0);
            *total = total.saturating_add(1);
            if *total >= threshold {
                return AttemptDecision::Block {
                    kind: AttemptBlockKind::NoProgressToolLoop,
                    message: format!(
                        "Stop calling `{tool}` for this turn: it has been used {total} times without new user input. Answer now from the current evidence, with any limits or missing facts stated plainly."
                    ),
                };
            }
        }
        AttemptDecision::Proceed
    }

    pub(super) fn record_outcome(&mut self, tool: &str, ok: bool) -> OutcomeDecision {
        let failures = self.failure_counts.entry(tool.to_string()).or_insert(0);
        if ok {
            *failures = 0;
            return OutcomeDecision::Continue;
        }

        *failures = failures.saturating_add(1);
        if *failures >= FAILURE_HALT_THRESHOLD {
            return OutcomeDecision::Halt(format!(
                "Stop retrying `{tool}` - it has failed {failures} consecutive times. Choose a different approach."
            ));
        }
        if *failures == FAILURE_WARN_THRESHOLD {
            return OutcomeDecision::Warn(format!(
                "Tool `{tool}` has failed {failures} consecutive times this turn."
            ));
        }
        OutcomeDecision::Continue
    }
}

fn is_delegated_tool(tool: &str) -> bool {
    matches!(tool, "agent" | "delegate")
}

fn no_progress_attempt_threshold(tool: &str, _read_only: bool) -> Option<u32> {
    let tool_name = tool.to_ascii_lowercase();
    let search_like = matches!(
        tool,
        "grep_files"
            | "file_search"
            | "list_dir"
            | "web_search"
            | "fetch_url"
            | "tool_search_tool_regex"
            | "tool_search_tool_bm25"
    ) || tool_name.contains("search");

    if search_like {
        return Some(BROAD_READ_ONLY_TOOL_LOOP_BLOCK_THRESHOLD);
    }

    None
}

fn hash_args(args: &Value) -> u64 {
    let mut canonical = String::new();
    write_canonical_json(args, &mut canonical);
    let mut hasher = DefaultHasher::new();
    canonical.hash(&mut hasher);
    hasher.finish()
}

fn write_canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => {
            let _ = write!(out, "{value}");
        }
        Value::String(value) => {
            out.push_str(&serde_json::to_string(value).expect("serializing string cannot fail"));
        }
        Value::Array(values) => {
            out.push('[');
            for (idx, item) in values.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                write_canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(values) => {
            out.push('{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (idx, (key, item)) in entries.into_iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("serializing key cannot fail"));
                out.push(':');
                write_canonical_json(item, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn third_identical_tool_call_is_blocked() {
        let mut guard = LoopGuard::default();
        let args = json!({"path": "src/main.rs"});

        assert_eq!(
            guard.record_attempt("read_file", &args, false),
            AttemptDecision::Proceed
        );
        assert_eq!(
            guard.record_attempt("read_file", &args, false),
            AttemptDecision::Proceed
        );

        let AttemptDecision::Block { kind, message } =
            guard.record_attempt("read_file", &args, false)
        else {
            panic!("third identical call should be blocked");
        };
        assert_eq!(kind, AttemptBlockKind::IdenticalToolCall);
        assert!(message.contains("read_file"));
        assert!(message.contains("already ran this turn"));
    }

    #[test]
    fn second_identical_read_only_tool_call_is_blocked() {
        let mut guard = LoopGuard::default();
        let args = json!({"pattern": "LoopGuard"});

        assert_eq!(
            guard.record_attempt("grep_files", &args, true),
            AttemptDecision::Proceed
        );
        let AttemptDecision::Block { kind, message } =
            guard.record_attempt("grep_files", &args, true)
        else {
            panic!("second identical read-only call should be blocked");
        };
        assert_eq!(kind, AttemptBlockKind::IdenticalToolCall);
        assert!(message.contains("prior result"));
    }

    #[test]
    fn paginated_reads_are_not_false_positives() {
        let mut guard = LoopGuard::default();

        for offset in [0, 100, 200] {
            assert_eq!(
                guard.record_attempt(
                    "read_file",
                    &json!({"path": "src/main.rs", "offset": offset}),
                    true
                ),
                AttemptDecision::Proceed
            );
        }
    }

    #[test]
    fn broad_read_only_search_loop_forces_synthesis() {
        let mut guard = LoopGuard::default();

        for idx in 0..(BROAD_READ_ONLY_TOOL_LOOP_BLOCK_THRESHOLD - 1) {
            assert_eq!(
                guard.record_attempt("grep_files", &json!({"pattern": format!("p{idx}")}), true),
                AttemptDecision::Proceed
            );
        }

        let AttemptDecision::Block { kind, message } = guard.record_attempt(
            "grep_files",
            &json!({"pattern": "last distinct query"}),
            true,
        ) else {
            panic!("repeated broad searches should force synthesis");
        };
        assert_eq!(kind, AttemptBlockKind::NoProgressToolLoop);
        assert!(message.contains("Answer now"));
    }

    #[test]
    fn search_named_dynamic_tool_is_capped_even_without_read_only_metadata() {
        let mut guard = LoopGuard::default();

        for idx in 0..(BROAD_READ_ONLY_TOOL_LOOP_BLOCK_THRESHOLD - 1) {
            assert_eq!(
                guard.record_attempt("KB_search", &json!({"query": format!("q{idx}")}), false),
                AttemptDecision::Proceed
            );
        }

        let AttemptDecision::Block { kind, message } =
            guard.record_attempt("KB_search", &json!({"query": "final"}), false)
        else {
            panic!("search-named dynamic tools should force synthesis");
        };
        assert_eq!(kind, AttemptBlockKind::NoProgressToolLoop);
        assert!(message.contains("KB_search"));
    }

    #[test]
    fn distinct_agent_delegation_is_not_turn_capped() {
        let mut guard = LoopGuard::default();

        for idx in 0..12 {
            assert_eq!(
                guard.record_attempt("agent", &json!({"prompt": format!("task {idx}")}), false),
                AttemptDecision::Proceed
            );
        }
    }

    #[test]
    fn identical_agent_delegation_is_still_blocked() {
        let mut guard = LoopGuard::default();
        let args = json!({"prompt": "repeat the same work"});

        assert_eq!(
            guard.record_attempt("agent", &args, false),
            AttemptDecision::Proceed
        );

        let AttemptDecision::Block { kind, message } = guard.record_attempt("agent", &args, false)
        else {
            panic!("identical delegation should still be blocked");
        };
        assert_eq!(kind, AttemptBlockKind::IdenticalToolCall);
        assert!(message.contains("already ran this turn"));
    }

    #[test]
    fn tool_failure_counter_warns_at_three_and_halts_at_eight() {
        let mut guard = LoopGuard::default();

        assert_eq!(
            guard.record_outcome("grep_files", false),
            OutcomeDecision::Continue
        );
        assert_eq!(
            guard.record_outcome("grep_files", false),
            OutcomeDecision::Continue
        );
        assert!(matches!(
            guard.record_outcome("grep_files", false),
            OutcomeDecision::Warn(message) if message.contains("failed 3 consecutive times")
        ));

        for _ in 4..8 {
            assert_eq!(
                guard.record_outcome("grep_files", false),
                OutcomeDecision::Continue
            );
        }
        assert!(matches!(
            guard.record_outcome("grep_files", false),
            OutcomeDecision::Halt(message) if message.contains("failed 8 consecutive times")
        ));
    }

    #[test]
    fn successful_tool_call_resets_failure_counter() {
        let mut guard = LoopGuard::default();

        assert_eq!(
            guard.record_outcome("grep_files", false),
            OutcomeDecision::Continue
        );
        assert_eq!(
            guard.record_outcome("grep_files", false),
            OutcomeDecision::Continue
        );
        assert_eq!(
            guard.record_outcome("grep_files", true),
            OutcomeDecision::Continue
        );
        assert_eq!(
            guard.record_outcome("grep_files", false),
            OutcomeDecision::Continue
        );
    }

    #[test]
    fn argument_hash_is_independent_of_object_key_order() {
        let mut guard = LoopGuard::default();

        assert_eq!(
            guard.record_attempt("read_file", &json!({"path": "a", "offset": 0}), false),
            AttemptDecision::Proceed
        );
        assert_eq!(
            guard.record_attempt("read_file", &json!({"offset": 0, "path": "a"}), false),
            AttemptDecision::Proceed
        );
        assert!(matches!(
            guard.record_attempt("read_file", &json!({"path": "a", "offset": 0}), false),
            AttemptDecision::Block {
                kind: AttemptBlockKind::IdenticalToolCall,
                ..
            }
        ));
    }
}
