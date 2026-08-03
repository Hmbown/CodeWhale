//! Conservative structural detection for model turns that make no progress.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const DEFAULT_REPEAT_WARN_THRESHOLD: usize = 3;
const DEFAULT_ALTERNATION_WARN_THRESHOLD: usize = 1;
const DEFAULT_NO_PROGRESS_WARN_THRESHOLD: usize = 4;
const DEFAULT_REPEATS_AFTER_WARN_TO_STOP: usize = 2;
const DEFAULT_ALTERNATION_HISTORY: usize = 4;

/// A compact, semantic description of one completed model step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StepFingerprint {
    Tool {
        name: String,
        arguments_hash: u64,
        error_signature: Option<u64>,
    },
    AssistantNoTool {
        text_hash: u64,
    },
    WaitingForSubagents {
        running: usize,
    },
}

impl StepFingerprint {
    pub(super) fn tool(
        name: impl Into<String>,
        arguments: &serde_json::Value,
        error: Option<&str>,
    ) -> Self {
        Self::Tool {
            name: name.into(),
            arguments_hash: stable_hash(&canonical_json(arguments).to_string()),
            error_signature: error.map(normalized_text_hash),
        }
    }

    pub(super) fn assistant_no_tool(text: &str) -> Self {
        Self::AssistantNoTool {
            text_hash: normalized_text_hash(text),
        }
    }

    pub(super) fn waiting_for_subagents(running: usize) -> Self {
        Self::WaitingForSubagents { running }
    }

    fn short_label(&self) -> String {
        match self {
            Self::Tool { name, .. } => format!("tool `{name}`"),
            Self::AssistantNoTool { .. } => "model wait response".to_string(),
            Self::WaitingForSubagents { running } => {
                format!("waiting for {running} running sub-agent(s)")
            }
        }
    }
}

/// Signal emitted by [`StuckGuard::observe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StuckSignal {
    Warn { reason: String },
    Stop { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StuckGuardConfig {
    pub repeat_warn_threshold: usize,
    pub alternation_warn_threshold: usize,
    pub no_progress_warn_threshold: usize,
    pub repeats_after_warn_to_stop: usize,
    pub alternation_history: usize,
}

impl Default for StuckGuardConfig {
    fn default() -> Self {
        Self {
            repeat_warn_threshold: DEFAULT_REPEAT_WARN_THRESHOLD,
            alternation_warn_threshold: DEFAULT_ALTERNATION_WARN_THRESHOLD,
            no_progress_warn_threshold: DEFAULT_NO_PROGRESS_WARN_THRESHOLD,
            repeats_after_warn_to_stop: DEFAULT_REPEATS_AFTER_WARN_TO_STOP,
            alternation_history: DEFAULT_ALTERNATION_HISTORY,
        }
    }
}

/// Per-turn detector. A change in the fingerprint resets the active episode,
/// so legitimate repeated tool names with different arguments are progress.
#[derive(Debug)]
pub(super) struct StuckGuard {
    config: StuckGuardConfig,
    last_step: Option<StepFingerprint>,
    last_tool_action: Option<(String, u64)>,
    repeated_actions: usize,
    repeated_pairs: usize,
    no_progress_messages: usize,
    step_history: VecDeque<StepFingerprint>,
    alternation_repeats: usize,
    warned: bool,
    repeats_after_warning: usize,
    last_reason: Option<String>,
}

impl Default for StuckGuard {
    fn default() -> Self {
        Self::new(StuckGuardConfig::default())
    }
}

impl StuckGuard {
    pub(super) fn new(config: StuckGuardConfig) -> Self {
        Self {
            config,
            last_step: None,
            last_tool_action: None,
            repeated_actions: 0,
            repeated_pairs: 0,
            no_progress_messages: 0,
            step_history: VecDeque::with_capacity(config.alternation_history),
            alternation_repeats: 0,
            warned: false,
            repeats_after_warning: 0,
            last_reason: None,
        }
    }

    pub(super) fn observe(&mut self, step: StepFingerprint) -> Option<StuckSignal> {
        let signal = match &step {
            StepFingerprint::AssistantNoTool { .. } => self.observe_assistant(step.clone()),
            StepFingerprint::WaitingForSubagents { .. } => self.observe_waiting(step.clone()),
            StepFingerprint::Tool { .. } => self.observe_tool(step.clone()),
        };
        self.step_history.push_back(step);
        while self.step_history.len() > self.config.alternation_history {
            self.step_history.pop_front();
        }
        signal.or_else(|| self.observe_alternation_cycle())
    }

    fn observe_assistant(&mut self, step: StepFingerprint) -> Option<StuckSignal> {
        if self.last_step.as_ref() == Some(&step) {
            self.no_progress_messages = self.no_progress_messages.saturating_add(1);
        } else {
            self.reset_episode();
            self.last_step = Some(step);
            self.no_progress_messages = 1;
        }
        if self.no_progress_messages >= self.config.no_progress_warn_threshold {
            return self.signal_for_repeat(
                "model repeated an equivalent wait response without tool/model progress"
                    .to_string(),
            );
        }
        None
    }

    fn observe_waiting(&mut self, step: StepFingerprint) -> Option<StuckSignal> {
        if self.last_step.as_ref() == Some(&step) {
            self.no_progress_messages = self.no_progress_messages.saturating_add(1);
        } else {
            self.reset_episode();
            self.last_step = Some(step.clone());
            self.no_progress_messages = 1;
        }
        if self.no_progress_messages < self.config.no_progress_warn_threshold {
            return None;
        }
        let reason = match step {
            StepFingerprint::WaitingForSubagents { running } => format!(
                "waiting for {running} sub-agent(s) is repeating without terminal child updates"
            ),
            _ => "waiting for sub-agents is repeating without terminal child updates".to_string(),
        };
        self.signal_for_repeat(reason)
    }

    fn observe_tool(&mut self, step: StepFingerprint) -> Option<StuckSignal> {
        self.no_progress_messages = 0;
        let action = match &step {
            StepFingerprint::Tool {
                name,
                arguments_hash,
                ..
            } => (name.clone(), *arguments_hash),
            StepFingerprint::AssistantNoTool { .. }
            | StepFingerprint::WaitingForSubagents { .. } => {
                unreachable!()
            }
        };
        let same_action = self.last_tool_action.as_ref() == Some(&action);
        let same_pair = self.last_step.as_ref() == Some(&step);
        let tool_name_for_reason = action.0.clone();
        if same_action {
            self.repeated_actions = self.repeated_actions.saturating_add(1);
        } else {
            self.last_tool_action = Some(action);
            self.repeated_actions = 1;
        }
        self.repeated_pairs = if same_pair {
            self.repeated_pairs.saturating_add(1)
        } else {
            1
        };
        self.last_step = Some(step.clone());
        if self.repeated_actions >= self.config.repeat_warn_threshold {
            return self.signal_for_repeat(format!(
                "repeating equivalent tool retry cycle for `{tool_name_for_reason}` without progress"
            ));
        }
        if self.repeated_pairs >= self.config.repeat_warn_threshold {
            return self.signal_for_repeat(format!(
                "repeating equivalent `{}` tool result without progress",
                step.short_label()
            ));
        }
        None
    }

    fn observe_alternation_cycle(&mut self) -> Option<StuckSignal> {
        let needed = self.config.alternation_history;
        if needed < 4 || self.step_history.len() < needed {
            return None;
        }
        let history: Vec<_> = self.step_history.iter().rev().take(4).collect();
        if history[0] != history[2] || history[1] != history[3] || history[0] == history[1] {
            return None;
        }
        self.alternation_repeats = self.alternation_repeats.saturating_add(1);
        if self.alternation_repeats < self.config.alternation_warn_threshold {
            return None;
        }
        self.signal_for_repeat(format!(
            "equivalent retry cycle detected: {} ↔ {}",
            history[0].short_label(),
            history[1].short_label()
        ))
    }

    fn signal_for_repeat(&mut self, reason: String) -> Option<StuckSignal> {
        let reason_changed = self.last_reason.as_ref() != Some(&reason);
        if !self.warned || reason_changed {
            self.warned = true;
            self.repeats_after_warning = 0;
            self.last_reason = Some(reason.clone());
            Some(StuckSignal::Warn { reason })
        } else {
            self.repeats_after_warning = self.repeats_after_warning.saturating_add(1);
            (self.repeats_after_warning >= self.config.repeats_after_warn_to_stop)
                .then_some(StuckSignal::Stop { reason })
        }
    }

    fn reset_episode(&mut self) {
        self.last_tool_action = None;
        self.repeated_actions = 0;
        self.repeated_pairs = 0;
        self.no_progress_messages = 0;
        self.step_history.clear();
        self.alternation_repeats = 0;
        self.warned = false;
        self.repeats_after_warning = 0;
        self.last_reason = None;
    }
}

pub(super) const RUNTIME_NOTICE: &str = "<codewhale:runtime_event kind=\"stuck_guard\" visibility=\"internal\">\n\
This is an internal runtime event. The previous steps appear to be repeating without progress.\n\
Change strategy: vary the tool arguments or method, inspect the latest result, or ask for the\n\
missing information. Do not repeat the same action unchanged.\n\
</codewhale:runtime_event>";

fn normalized_text_hash(text: &str) -> u64 {
    stable_hash(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn stable_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => {
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            let mut canonical = serde_json::Map::new();
            for (key, value) in entries {
                canonical.insert(key.clone(), canonical_json(value));
            }
            serde_json::Value::Object(canonical)
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, args: serde_json::Value) -> StepFingerprint {
        StepFingerprint::tool(name, &args, None)
    }

    fn failed_tool(name: &str, args: serde_json::Value, error: &str) -> StepFingerprint {
        StepFingerprint::tool(name, &args, Some(error))
    }

    #[test]
    fn identical_actions_warn_then_stop() {
        let step = tool("read_file", json!({"path": "a.txt"}));
        let mut guard = StuckGuard::default();
        assert_eq!(guard.observe(step.clone()), None);
        assert_eq!(guard.observe(step.clone()), None);
        assert!(matches!(
            guard.observe(step.clone()),
            Some(StuckSignal::Warn { .. })
        ));
        assert_eq!(guard.observe(step.clone()), None);
        assert!(matches!(
            guard.observe(step.clone()),
            Some(StuckSignal::Stop { .. })
        ));
        assert!(matches!(
            guard.observe(step),
            Some(StuckSignal::Stop { .. })
        ));
    }

    #[test]
    fn identical_action_error_pairs_are_detected() {
        let step = failed_tool("exec_shell", json!({"command": "missing"}), "not found");
        let mut guard = StuckGuard::default();
        assert_eq!(guard.observe(step.clone()), None);
        assert_eq!(guard.observe(step.clone()), None);
        assert!(matches!(
            guard.observe(step),
            Some(StuckSignal::Warn { .. })
        ));
    }

    #[test]
    fn identical_actions_with_different_errors_are_detected_too() {
        let args = json!({"command": "missing"});
        let mut guard = StuckGuard::default();
        assert_eq!(
            guard.observe(failed_tool("exec_shell", args.clone(), "not found")),
            None
        );
        assert_eq!(
            guard.observe(failed_tool("exec_shell", args.clone(), "still missing")),
            None
        );
        assert_eq!(
            guard.observe(failed_tool("exec_shell", args, "no such file")),
            Some(StuckSignal::Warn {
                reason: "repeating equivalent tool retry cycle for `exec_shell` without progress"
                    .to_string()
            })
        );
    }

    #[test]
    fn alternating_actions_warn_and_stop_after_two_more_repeats() {
        let a = tool("read_file", json!({"path": "a"}));
        let b = tool("read_file", json!({"path": "b"}));
        let mut guard = StuckGuard::default();
        assert_eq!(guard.observe(a.clone()), None);
        assert_eq!(guard.observe(b.clone()), None);
        assert_eq!(guard.observe(a.clone()), None);
        assert!(matches!(
            guard.observe(b.clone()),
            Some(StuckSignal::Warn { .. })
        ));
        assert_eq!(guard.observe(a.clone()), None);
        assert!(matches!(
            guard.observe(b.clone()),
            Some(StuckSignal::Stop { .. })
        ));
    }

    #[test]
    fn repeated_no_tool_messages_are_detected() {
        let step = StepFingerprint::assistant_no_tool("I need to try again.");
        let mut guard = StuckGuard::default();
        assert_eq!(guard.observe(step.clone()), None);
        assert_eq!(guard.observe(step.clone()), None);
        assert_eq!(guard.observe(step.clone()), None);
        assert!(matches!(
            guard.observe(step.clone()),
            Some(StuckSignal::Warn { .. })
        ));
    }

    #[test]
    fn repeated_waiting_for_same_child_state_is_detected() {
        let step = StepFingerprint::waiting_for_subagents(2);
        let mut guard = StuckGuard::default();
        assert_eq!(guard.observe(step.clone()), None);
        assert_eq!(guard.observe(step.clone()), None);
        assert_eq!(guard.observe(step.clone()), None);
        assert!(matches!(
            guard.observe(step.clone()),
            Some(StuckSignal::Warn { reason })
            if reason.contains("waiting for 2 sub-agent(s)")
        ));
    }

    #[test]
    fn alternating_model_wait_and_tool_retry_cycle_is_detected() {
        let wait = StepFingerprint::assistant_no_tool("waiting for tool output");
        let retry = failed_tool("web_search", json!({"query": "same"}), "timeout");
        let mut guard = StuckGuard::new(StuckGuardConfig {
            repeat_warn_threshold: 99,
            alternation_warn_threshold: 1,
            no_progress_warn_threshold: 99,
            repeats_after_warn_to_stop: 2,
            alternation_history: 4,
        });
        assert_eq!(guard.observe(wait.clone()), None);
        assert_eq!(guard.observe(retry.clone()), None);
        assert_eq!(guard.observe(wait.clone()), None);
        assert!(matches!(
            guard.observe(retry),
            Some(StuckSignal::Warn { reason })
            if reason.contains("equivalent retry cycle detected")
        ));
    }

    #[test]
    fn changed_arguments_reset_the_episode() {
        let mut guard = StuckGuard::default();
        let same = tool("read_file", json!({"path": "a"}));
        let progress = tool("read_file", json!({"path": "b"}));
        assert_eq!(guard.observe(same.clone()), None);
        assert_eq!(guard.observe(same), None);
        assert_eq!(guard.observe(progress.clone()), None);
        assert_eq!(guard.observe(progress), None);
    }

    #[test]
    fn argument_object_key_order_does_not_change_fingerprint() {
        assert_eq!(
            tool("x", json!({"a": 1, "b": 2})),
            tool("x", json!({"b": 2, "a": 1}))
        );
    }
}
