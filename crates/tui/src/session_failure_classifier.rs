//! Redacted session/tool failure classification.
//!
//! This module is deliberately pure: callers provide already-parsed,
//! caller-constructed records and receive aggregate counts plus redacted
//! source handles. It does not read session files or copy raw tool output.

use std::collections::BTreeMap;

use serde::Serialize;

/// Environment/tool failure shapes that should be separated from model-quality
/// failures during triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    CommandExit,
    Network,
    SandboxApproval,
    MissingDependencyPath,
    Timeout,
    UnclosedTurn,
    Unknown,
}

impl FailureCategory {
    #[must_use]
    pub fn is_environment_suspect(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// One caller-supplied synthetic session record.
#[derive(Debug, Clone)]
pub struct SessionFailureRecord<'a> {
    /// Untrusted source locator. The classifier hashes it before output.
    pub source_hint: &'a str,
    /// Optional timestamp to preserve enough local evidence metadata for
    /// maintainers who have access to the private source.
    pub timestamp: Option<&'a str>,
    pub event: SessionFailureEvent<'a>,
}

/// Synthetic event shape used by the classifier.
#[derive(Debug, Clone)]
pub enum SessionFailureEvent<'a> {
    TurnStarted { turn_id: &'a str },
    TurnCompleted { turn_id: &'a str },
    Tool(ToolFailureRecord<'a>),
}

/// Caller-supplied tool record. Text fields are classification inputs only and
/// are never copied into [`FailureEvidence`].
#[derive(Debug, Clone, Default)]
pub struct ToolFailureRecord<'a> {
    pub tool_name: &'a str,
    pub success: Option<bool>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub sandbox_denied: bool,
    pub approval_denied: bool,
    pub diagnostic: Option<&'a str>,
    pub output_excerpt: Option<&'a str>,
}

/// Redacted per-failure locator emitted by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureEvidence {
    pub category: FailureCategory,
    pub source_handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_handle: Option<String>,
}

/// Aggregate classifier output safe for status, handoff, or bug-report
/// preflight surfaces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FailureSummary {
    pub counts: BTreeMap<FailureCategory, usize>,
    pub evidence: Vec<FailureEvidence>,
}

impl FailureSummary {
    #[must_use]
    pub fn count_for(&self, category: FailureCategory) -> usize {
        self.counts.get(&category).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn environment_suspect_count(&self) -> usize {
        self.evidence
            .iter()
            .filter(|item| item.category.is_environment_suspect())
            .count()
    }

    fn push(&mut self, evidence: FailureEvidence) {
        *self.counts.entry(evidence.category).or_insert(0) += 1;
        self.evidence.push(evidence);
    }
}

#[derive(Debug, Clone)]
struct OpenTurn {
    source_handle: String,
    timestamp: Option<String>,
    turn_handle: String,
}

/// Classify a caller-supplied slice of synthetic records.
#[must_use]
pub fn summarize_records(records: &[SessionFailureRecord<'_>]) -> FailureSummary {
    let mut summary = FailureSummary::default();
    let mut open_turns: BTreeMap<String, OpenTurn> = BTreeMap::new();

    for record in records {
        let source_handle = redacted_handle("src", record.source_hint);
        let timestamp = record.timestamp.map(ToOwned::to_owned);

        match &record.event {
            SessionFailureEvent::TurnStarted { turn_id } => {
                open_turns.insert(
                    (*turn_id).to_owned(),
                    OpenTurn {
                        source_handle,
                        timestamp,
                        turn_handle: redacted_handle("turn", turn_id),
                    },
                );
            }
            SessionFailureEvent::TurnCompleted { turn_id } => {
                open_turns.remove(*turn_id);
            }
            SessionFailureEvent::Tool(tool) => {
                if let Some(category) = classify_tool_record(tool) {
                    summary.push(FailureEvidence {
                        category,
                        source_handle,
                        timestamp,
                        tool_name: Some(sanitize_tool_name(tool.tool_name)),
                        exit_code: tool.exit_code.filter(|code| *code != 0),
                        turn_handle: None,
                    });
                }
            }
        }
    }

    for turn in open_turns.into_values() {
        summary.push(FailureEvidence {
            category: FailureCategory::UnclosedTurn,
            source_handle: turn.source_handle,
            timestamp: turn.timestamp,
            tool_name: None,
            exit_code: None,
            turn_handle: Some(turn.turn_handle),
        });
    }

    summary
}

/// Classify one tool record. Returns `None` for successful/no-signal records.
#[must_use]
pub fn classify_tool_record(record: &ToolFailureRecord<'_>) -> Option<FailureCategory> {
    let failed = record.success == Some(false)
        || record.exit_code.is_some_and(|code| code != 0)
        || record.timed_out
        || record.sandbox_denied
        || record.approval_denied
        || record.diagnostic.is_some()
        || record.output_excerpt.is_some();

    if !failed {
        return None;
    }

    if record.timed_out || record.matches_text(timeout_signal) {
        return Some(FailureCategory::Timeout);
    }
    if record.sandbox_denied
        || record.approval_denied
        || record.matches_text(sandbox_or_approval_signal)
    {
        return Some(FailureCategory::SandboxApproval);
    }
    if record.matches_text(network_signal) {
        return Some(FailureCategory::Network);
    }
    if record.matches_text(missing_dependency_or_path_signal) {
        return Some(FailureCategory::MissingDependencyPath);
    }
    if record.exit_code.is_some_and(|code| code != 0) {
        return Some(FailureCategory::CommandExit);
    }

    Some(FailureCategory::Unknown)
}

impl ToolFailureRecord<'_> {
    fn matches_text(&self, predicate: fn(&str) -> bool) -> bool {
        self.diagnostic.is_some_and(predicate) || self.output_excerpt.is_some_and(predicate)
    }
}

fn timeout_signal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("deadline exceeded")
        || lower.contains("operation took too long")
}

fn sandbox_or_approval_signal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("sandbox")
        || lower.contains("seatbelt")
        || lower.contains("landlock")
        || lower.contains("seccomp")
        || lower.contains("approval")
        || lower.contains("denied by user")
        || lower.contains("user denied")
        || lower.contains("permission denied")
        || lower.contains("operation not permitted")
        || lower.contains("blocked by policy")
}

fn network_signal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("network")
        || lower.contains("dns")
        || lower.contains("could not resolve")
        || lower.contains("name or service not known")
        || lower.contains("temporary failure in name resolution")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("connection closed")
        || lower.contains("failed to connect")
        || lower.contains("tls")
        || lower.contains("ssl")
        || lower.contains("http 502")
        || lower.contains("http 503")
        || lower.contains("http 504")
        || lower.contains(" 502 ")
        || lower.contains(" 503 ")
        || lower.contains(" 504 ")
        || lower.starts_with("502 ")
        || lower.starts_with("503 ")
        || lower.starts_with("504 ")
        || lower.ends_with(" 502")
        || lower.ends_with(" 503")
        || lower.ends_with(" 504")
        || matches!(lower.as_str(), "502" | "503" | "504")
        || lower.contains("curl: (6)")
        || lower.contains("curl: (7)")
        || lower.contains("curl: (35)")
        || lower.contains("curl: (56)")
}

fn missing_dependency_or_path_signal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("command not found")
        || lower.contains("no such file or directory")
        || lower.contains("enoent")
        || lower.contains("not recognized as an internal or external command")
        || lower.contains("cannot find the path")
        || lower.contains("failed to locate tool")
        || lower.contains("module not found")
        || lower.contains("modulenotfounderror")
        || lower.contains("no module named")
        || lower.contains("missing binary")
        || lower.contains("missing dependency")
}

fn sanitize_tool_name(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        .take(64)
        .collect();
    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized
    }
}

fn redacted_handle(prefix: &str, raw: &str) -> String {
    if raw.trim().is_empty() {
        return format!("{prefix}_unspecified");
    }
    format!("{prefix}_{:016x}", stable_hash(raw))
}

fn stable_hash(raw: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in raw.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool<'a>(
        source_hint: &'a str,
        tool_name: &'a str,
        exit_code: Option<i32>,
        diagnostic: &'a str,
    ) -> SessionFailureRecord<'a> {
        SessionFailureRecord {
            source_hint,
            timestamp: Some("2026-05-24T21:00:00Z"),
            event: SessionFailureEvent::Tool(ToolFailureRecord {
                tool_name,
                success: Some(false),
                exit_code,
                diagnostic: Some(diagnostic),
                ..ToolFailureRecord::default()
            }),
        }
    }

    #[test]
    fn classifies_synthetic_environment_and_tool_failure_shapes() {
        let records = vec![
            tool(
                "/Users/hunter/private/session-a.jsonl",
                "exec_shell",
                Some(101),
                "cargo test failed",
            ),
            tool(
                "/Users/hunter/private/session-b.jsonl",
                "web_run",
                Some(6),
                "curl: (6) Could not resolve host: example.invalid",
            ),
            SessionFailureRecord {
                source_hint: "/Users/hunter/private/session-c.jsonl",
                timestamp: Some("2026-05-24T21:01:00Z"),
                event: SessionFailureEvent::Tool(ToolFailureRecord {
                    tool_name: "exec_shell",
                    success: Some(false),
                    exit_code: Some(1),
                    sandbox_denied: true,
                    diagnostic: Some("sandbox-exec blocked file-write"),
                    ..ToolFailureRecord::default()
                }),
            },
            tool(
                "/Users/hunter/private/session-d.jsonl",
                "exec_shell",
                Some(127),
                "zsh: command not found: cargo-nextest",
            ),
            SessionFailureRecord {
                source_hint: "/Users/hunter/private/session-e.jsonl",
                timestamp: Some("2026-05-24T21:02:00Z"),
                event: SessionFailureEvent::Tool(ToolFailureRecord {
                    tool_name: "fetch_url",
                    success: Some(false),
                    timed_out: true,
                    diagnostic: Some("operation timed out after 60s"),
                    ..ToolFailureRecord::default()
                }),
            },
            SessionFailureRecord {
                source_hint: "/Users/hunter/private/session-f.jsonl",
                timestamp: Some("2026-05-24T21:03:00Z"),
                event: SessionFailureEvent::TurnStarted {
                    turn_id: "turn-private-123",
                },
            },
        ];

        let summary = summarize_records(&records);

        assert_eq!(summary.count_for(FailureCategory::CommandExit), 1);
        assert_eq!(summary.count_for(FailureCategory::Network), 1);
        assert_eq!(summary.count_for(FailureCategory::SandboxApproval), 1);
        assert_eq!(summary.count_for(FailureCategory::MissingDependencyPath), 1);
        assert_eq!(summary.count_for(FailureCategory::Timeout), 1);
        assert_eq!(summary.count_for(FailureCategory::UnclosedTurn), 1);
        assert_eq!(summary.environment_suspect_count(), 6);
    }

    #[test]
    fn specific_environment_signals_beat_generic_nonzero_exit() {
        let network = ToolFailureRecord {
            tool_name: "exec_shell",
            success: Some(false),
            exit_code: Some(1),
            diagnostic: Some("DNS lookup failed"),
            ..ToolFailureRecord::default()
        };
        let missing = ToolFailureRecord {
            tool_name: "exec_shell",
            success: Some(false),
            exit_code: Some(127),
            diagnostic: Some("No such file or directory"),
            ..ToolFailureRecord::default()
        };
        let approval = ToolFailureRecord {
            tool_name: "edit_file",
            success: Some(false),
            exit_code: Some(1),
            approval_denied: true,
            diagnostic: Some("denied by user"),
            ..ToolFailureRecord::default()
        };
        let timeout = ToolFailureRecord {
            tool_name: "web_run",
            success: Some(false),
            exit_code: Some(124),
            diagnostic: Some("deadline exceeded"),
            ..ToolFailureRecord::default()
        };

        assert_eq!(
            classify_tool_record(&network),
            Some(FailureCategory::Network)
        );
        assert_eq!(
            classify_tool_record(&missing),
            Some(FailureCategory::MissingDependencyPath)
        );
        assert_eq!(
            classify_tool_record(&approval),
            Some(FailureCategory::SandboxApproval)
        );
        assert_eq!(
            classify_tool_record(&timeout),
            Some(FailureCategory::Timeout)
        );
    }

    #[test]
    fn successful_records_and_closed_turns_do_not_emit_failures() {
        let records = vec![
            SessionFailureRecord {
                source_hint: "session-ok",
                timestamp: None,
                event: SessionFailureEvent::TurnStarted { turn_id: "turn-1" },
            },
            SessionFailureRecord {
                source_hint: "session-ok",
                timestamp: None,
                event: SessionFailureEvent::Tool(ToolFailureRecord {
                    tool_name: "exec_shell",
                    success: Some(true),
                    exit_code: Some(0),
                    diagnostic: None,
                    ..ToolFailureRecord::default()
                }),
            },
            SessionFailureRecord {
                source_hint: "session-ok",
                timestamp: None,
                event: SessionFailureEvent::TurnCompleted { turn_id: "turn-1" },
            },
        ];

        let summary = summarize_records(&records);

        assert!(summary.counts.is_empty());
        assert!(summary.evidence.is_empty());
    }

    #[test]
    fn summary_uses_redacted_handles_and_does_not_copy_raw_content() {
        let records = vec![
            SessionFailureRecord {
                source_hint: "/Users/hunter/private/session-secret.jsonl",
                timestamp: Some("2026-05-24T21:04:00Z"),
                event: SessionFailureEvent::Tool(ToolFailureRecord {
                    tool_name: "exec shell with spaces",
                    success: Some(false),
                    exit_code: Some(1),
                    diagnostic: Some("fatal output contained sk-test-secret and /private/path"),
                    output_excerpt: Some("raw transcript text that must stay private"),
                    ..ToolFailureRecord::default()
                }),
            },
            SessionFailureRecord {
                source_hint: "/Users/hunter/private/session-secret.jsonl",
                timestamp: Some("2026-05-24T21:05:00Z"),
                event: SessionFailureEvent::TurnStarted {
                    turn_id: "private-turn-id",
                },
            },
        ];

        let encoded = serde_json::to_string(&summarize_records(&records)).unwrap();

        assert!(!encoded.contains("/Users/hunter"));
        assert!(!encoded.contains("session-secret"));
        assert!(!encoded.contains("sk-test-secret"));
        assert!(!encoded.contains("raw transcript text"));
        assert!(!encoded.contains("private-turn-id"));
        assert!(encoded.contains("src_"));
        assert!(encoded.contains("turn_"));
        assert!(encoded.contains("execshellwithspaces"));
    }
}
