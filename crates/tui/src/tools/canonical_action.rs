//! Semantic aliases for the model-facing action tools.
//!
//! `Bash`, `File`, `Git`, `Run`, `Web`, and `rlm` deliberately keep their
//! canonical names at the execution and audit boundaries.  Presentation and
//! policy consumers, however, still understand the older per-action names.
//! Resolve that semantic name in one place so live calls and saved legacy
//! transcripts receive identical downstream behavior without rewriting the
//! original call.
//!
//! This table is not documentation — it is the **action-policy seam**. A
//! permission check that denies `fetch_url` only reaches `Web{action:"fetch"}`
//! because the pair is listed here. A family that is missing from the table is
//! a family whose actions no deny list can see, which is why `rlm` was added:
//! `rlm{action:"open", url:...}` calls `FetchUrlTool` *inside the process*,
//! under its own name, and a name-keyed deny list never sees that call.

use serde_json::Value;

pub(crate) const CANONICAL_ACTION_ALIASES: &[(&str, &str, &str)] = &[
    ("Bash", "run", "exec_shell"),
    ("Bash", "wait", "exec_shell_wait"),
    ("Bash", "interact", "exec_shell_interact"),
    ("Bash", "cancel", "exec_shell_cancel"),
    ("File", "read", "read_file"),
    ("File", "list", "list_dir"),
    ("File", "search_name", "file_search"),
    ("File", "search_content", "grep_files"),
    ("File", "write", "write_file"),
    ("File", "edit", "edit_file"),
    ("File", "patch", "apply_patch"),
    ("Git", "status", "git_status"),
    ("Git", "diff", "git_diff"),
    ("Git", "log", "git_log"),
    ("Git", "show", "git_show"),
    ("Git", "blame", "git_blame"),
    ("Run", "tests", "run_tests"),
    ("Run", "verifiers", "run_verifiers"),
    ("Web", "search", "web_search"),
    ("Web", "fetch", "fetch_url"),
    ("Web", "wait", "wait_for_dev_server"),
    // The RLM session family. `open` reaches the network (it fetches a `url`
    // through `FetchUrlTool` in-process) and `eval` runs operator-supplied
    // Python against a live kernel — sockets and filesystem both. The other
    // three actions are bounded local metadata. Listing every pair is what lets
    // a deny list keep the local half and remove the reaching half, instead of
    // having to choose between the whole family and nothing.
    ("rlm", "session_objects", "rlm_session_objects"),
    ("rlm", "open", "rlm_open"),
    ("rlm", "eval", "rlm_eval"),
    ("rlm", "configure", "rlm_configure"),
    ("rlm", "close", "rlm_close"),
    // The durable-work families. These were absent for the same reason `rlm`
    // was: they are model-visible under one canonical name (`tasks`,
    // `automation`, `github`) and their per-action legacy names are registered
    // as *hidden* aliases. A deny list naming `task_gate_run` therefore never
    // saw `tasks{action:"gate_run"}`, and the action-enum pruner never saw the
    // family at all — so an operator ceiling could not express "durable task
    // bookkeeping, yes; running a gate command, no".
    //
    // `gate_run` runs an operator-supplied command, `automation.run` executes a
    // stored automation, and the mutating `automation.*` actions schedule agent
    // runs with their own cwd. All three are execution primitives spelled as
    // bookkeeping, which is exactly the shape
    // [`crate::tools::execution_envelope`] classifies from capabilities.
    ("tasks", "create", "task_create"),
    ("tasks", "list", "task_list"),
    ("tasks", "read", "task_read"),
    ("tasks", "cancel", "task_cancel"),
    ("tasks", "gate_run", "task_gate_run"),
    ("tasks", "pr_attempt_record", "pr_attempt_record"),
    ("tasks", "pr_attempt_list", "pr_attempt_list"),
    ("tasks", "pr_attempt_read", "pr_attempt_read"),
    ("tasks", "pr_attempt_preflight", "pr_attempt_preflight"),
    ("automation", "create", "automation_create"),
    ("automation", "list", "automation_list"),
    ("automation", "read", "automation_read"),
    ("automation", "update", "automation_update"),
    ("automation", "pause", "automation_pause"),
    ("automation", "resume", "automation_resume"),
    ("automation", "delete", "automation_delete"),
    ("automation", "run", "automation_run"),
    ("github", "issue_context", "github_issue_context"),
    ("github", "pr_context", "github_pr_context"),
    ("github", "comment", "github_comment"),
    ("github", "close_issue", "github_close_issue"),
    ("github", "close_pr", "github_close_pr"),
];

/// Resolve a legacy per-action tool id (e.g. `exec_shell`) to its canonical
/// family tool name (`Bash`). `None` when the id is not a known legacy alias.
/// Lets older API clients that still name pre-0.9 action ids in allow/deny
/// lists keep addressing the tools that replaced them.
#[must_use]
pub(crate) fn canonical_tool_for_action_id(action_id: &str) -> Option<&'static str> {
    CANONICAL_ACTION_ALIASES
        .iter()
        .find(|(_, _, alias)| alias.eq_ignore_ascii_case(action_id))
        .map(|(tool, _, _)| *tool)
}

/// The action a family runs when the model omits `action`, mirroring each
/// wrapper's own execution default.
///
/// `None` means the family has no default and rejects an actionless call, which
/// is `rlm`'s contract: [`crate::tools::rlm::RlmTool::resolve_action`] errors
/// rather than guessing. Policy still resolves an *explicit* action for such a
/// family — see [`canonical_action_alias`].
#[must_use]
pub(crate) fn action_family_default(tool_name: &str) -> Option<Option<&'static str>> {
    match tool_name {
        "Bash" => Some(Some("run")),
        "File" => Some(Some("read")),
        "Git" => Some(Some("status")),
        "Run" => Some(Some("tests")),
        "Web" => Some(Some("search")),
        // Families whose wrappers reject an actionless call rather than
        // guessing. Policy still resolves an *explicit* action for them.
        "rlm" | "tasks" | "automation" | "github" => Some(None),
        _ => None,
    }
}

/// Whether `tool_name` is a model-facing action family whose `action` enum
/// policy may prune.
///
/// Derived from [`action_family_default`] rather than spelled out at each call
/// site: a hard-coded family list that falls behind the alias table is a family
/// whose actions stay visible after policy removed them.
#[must_use]
pub(crate) fn is_action_family(tool_name: &str) -> bool {
    action_family_default(tool_name).is_some()
}

/// Resolve a canonical action tool to the legacy name for that exact action.
///
/// Missing actions follow each wrapper's execution default. Unknown actions
/// stay canonical so policy remains conservative and the eventual tool error
/// is attributed to the call the model actually made.
///
/// A family with no default (`rlm`) still resolves an **explicit** action. The
/// earlier shape returned the family name for any such call, which meant
/// `rlm{action:"eval"}` never resolved to `rlm_eval` and therefore never met a
/// deny list entry naming it.
#[must_use]
pub(crate) fn canonical_action_alias<'a>(tool_name: &'a str, input: &Value) -> &'a str {
    let Some(default_action) = action_family_default(tool_name) else {
        return tool_name;
    };
    let Some(action) = input
        .get("action")
        .and_then(Value::as_str)
        .or(default_action)
    else {
        return tool_name;
    };

    CANONICAL_ACTION_ALIASES
        .iter()
        .find_map(|(family, candidate_action, alias)| {
            (*family == tool_name && *candidate_action == action).then_some(*alias)
        })
        .unwrap_or(tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_canonical_action_resolves_to_its_legacy_semantic_alias() {
        for (family, action, alias) in CANONICAL_ACTION_ALIASES {
            assert_eq!(
                canonical_action_alias(family, &json!({"action": action})),
                *alias,
                "{family}.{action}"
            );
        }
    }

    #[test]
    fn canonical_defaults_match_wrapper_execution_defaults() {
        for (family, alias) in [
            ("Bash", "exec_shell"),
            ("File", "read_file"),
            ("Git", "git_status"),
            ("Run", "run_tests"),
            ("Web", "web_search"),
        ] {
            assert_eq!(
                canonical_action_alias(family, &json!({})),
                alias,
                "{family}"
            );
        }
    }

    #[test]
    fn legacy_unknown_and_invalid_calls_keep_their_original_names() {
        for name in ["exec_shell", "read_file", "future_tool"] {
            assert_eq!(canonical_action_alias(name, &json!({})), name);
        }
        assert_eq!(
            canonical_action_alias("File", &json!({"action": "delete"})),
            "File"
        );
        assert_eq!(
            canonical_action_alias("Bash", &json!({"action": 42})),
            "exec_shell"
        );
    }

    /// A family with no execution default still resolves an explicit action.
    /// Without this, `rlm{action:"eval"}` resolves to `rlm` and slips past every
    /// deny list entry that names `rlm_eval`.
    #[test]
    fn a_family_without_a_default_still_resolves_an_explicit_action() {
        assert_eq!(
            canonical_action_alias("rlm", &json!({"action": "eval"})),
            "rlm_eval"
        );
        assert_eq!(
            canonical_action_alias("rlm", &json!({"action": "open", "url": "https://x.test/a"})),
            "rlm_open"
        );
        // No action, no default: nothing to resolve, and `RlmTool` will reject
        // the call on its own terms.
        assert_eq!(canonical_action_alias("rlm", &json!({})), "rlm");
        assert_eq!(
            canonical_action_alias("rlm", &json!({"action": "nope"})),
            "rlm"
        );
    }

    /// Every family named in the alias table must be recognised as a family, or
    /// its actions are unprunable by the visibility filter.
    #[test]
    fn every_aliased_family_is_a_known_action_family() {
        for (family, _, _) in CANONICAL_ACTION_ALIASES {
            assert!(
                is_action_family(family),
                "{family} is aliased but not registered as an action family"
            );
        }
        assert!(!is_action_family("read_file"));
        assert!(!is_action_family("future_tool"));
    }
}
