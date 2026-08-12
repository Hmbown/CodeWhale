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
    ("bash", "run", "exec_shell"),
    ("bash", "wait", "exec_shell_wait"),
    ("bash", "interact", "exec_shell_interact"),
    ("bash", "cancel", "exec_shell_cancel"),
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

/// The conservative action label policy uses when the model omits `action`.
///
/// This is a *policy* fallback only. Execution rejects an actionless call in
/// every family (see [`required_action`]); approval and parallel-safety
/// predicates cannot return an error, so they still need a label, and it must
/// be the family's least dangerous action.
///
/// `None` means the family never had even a policy default — `rlm`'s contract:
/// [`crate::tools::rlm::RlmTool::resolve_action`] errors rather than guessing.
/// Policy still resolves an *explicit* action for such a family — see
/// [`canonical_action_alias`].
#[must_use]
pub(crate) fn action_family_default(tool_name: &str) -> Option<Option<&'static str>> {
    match tool_name {
        "bash" | "Bash" => Some(Some("run")),
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

/// Require the `action` discriminator on a canonical action-family call.
///
/// Every family schema marks `action` required, but the wrappers used to
/// default a missing one (`File` → read, `Git` → status, `Web` → search,
/// `Run` → tests). A call that merely omitted or misspelled the discriminator
/// therefore ran a *different* operation and returned that operation's success
/// receipt: `File{path, content}` answered an intended write with the file's
/// current contents, so the write silently never happened. Same shape as
/// #5209 — refuse, and name the values that actually dispatch.
///
/// `actions` must be the set this tool instance can really run, so a mode that
/// hides `write` never suggests it.
pub(crate) fn required_action(
    input: &Value,
    tool: &str,
    actions: &[&str],
) -> Result<String, crate::tools::spec::ToolError> {
    use crate::tools::spec::ToolError;
    match input.get("action") {
        Some(Value::String(action)) => Ok(action.clone()),
        Some(other) => Err(ToolError::invalid_input(format!(
            "{tool} requires `action` to be a string, got {other}; nothing was run. Pass one of: {}.",
            actions.join(", ")
        ))),
        None => Err(ToolError::invalid_input(format!(
            "{tool} requires an `action` parameter; nothing was run. Pass one of: {}.",
            actions.join(", ")
        ))),
    }
}

/// Resolve a canonical action tool to the legacy name for that exact action.
///
/// A missing action falls back to the family's conservative default so the
/// *policy* label is never absent; execution itself refuses the call (see
/// `required_action`). Unknown actions stay canonical so policy remains
/// conservative and the eventual tool error is attributed to the call the
/// model actually made.
///
/// A family with no default (`rlm`) still resolves an **explicit** action. The
/// earlier shape returned the family name for any such call, which meant
/// `rlm{action:"eval"}` never resolved to `rlm_eval` and therefore never met a
/// deny list entry naming it.
#[must_use]
pub(crate) fn canonical_action_alias<'a>(tool_name: &'a str, input: &Value) -> &'a str {
    // The new model-facing file primitives deliberately reuse the old
    // semantic policy names. This keeps permissions.toml, repo law, resource
    // envelopes, approval caches, audit aggregation, and saved policy state
    // compatible across the presentation change.
    match tool_name {
        "read" => return "read_file",
        "write" => return "write_file",
        "edit" => return "edit_file",
        _ => {}
    }
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

    /// Names the v0.9.3 consolidation retired. None of them can dispatch —
    /// `ToolRegistry::resolve` has no fuzzy step — so any one of them inside a
    /// model-visible description or schema teaches a call that cannot work.
    const RETIRED_TOOL_NAMES: &[&str] = &[
        "read_file",
        "write_file",
        "edit_file",
        "list_dir",
        "file_search",
        "grep_files",
        "git_status",
        "git_diff",
        "git_log",
        "git_show",
        "git_blame",
        "run_tests",
        "run_verifiers",
        "web_search",
        "fetch_url",
        "wait_for_dev_server",
        "exec_shell",
        "exec_shell_wait",
        "exec_shell_interact",
        "exec_shell_cancel",
    ];

    /// The catalog is re-sent on every request, so a retired name in it is a
    /// per-turn lie to every model. `verifier.rs` already guarded one such
    /// description by hand; this covers the whole advertised surface at once.
    #[test]
    fn no_advertised_tool_teaches_a_retired_name() {
        use crate::tools::registry::ToolRegistryBuilder;
        use crate::tools::spec::ToolContext;

        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = ToolRegistryBuilder::new()
            .with_file_tools()
            .with_search_tools()
            .with_git_tools()
            .with_git_history_tools()
            .with_test_runner_tool()
            .with_web_tools()
            .with_patch_tools()
            .build(ToolContext::new(tmp.path().to_path_buf()));

        for tool in registry.to_api_tools() {
            let advertised = format!("{} {}", tool.description, tool.input_schema);
            for retired in RETIRED_TOOL_NAMES {
                assert!(
                    !advertised.contains(retired),
                    "tool `{}` advertises the retired name `{retired}`; \
                     name the canonical action form instead",
                    tool.name
                );
            }
        }
    }

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
    fn lowercase_primitives_preserve_legacy_policy_names() {
        assert_eq!(canonical_action_alias("read", &json!({})), "read_file");
        assert_eq!(canonical_action_alias("write", &json!({})), "write_file");
        assert_eq!(canonical_action_alias("edit", &json!({})), "edit_file");
        assert_eq!(
            canonical_action_alias("bash", &json!({"command": "pwd"})),
            "exec_shell"
        );
        assert_eq!(
            canonical_action_alias("bash", &json!({"action": "cancel"})),
            "exec_shell_cancel"
        );
    }

    /// Execution refuses an actionless call; policy still needs a label for
    /// it, and that label must stay the family's most conservative action.
    #[test]
    fn actionless_calls_keep_a_conservative_policy_label() {
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
