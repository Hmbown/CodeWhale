//! One worker system.
//!
//! A child exists to do a slice of work. The spawn API is `spawn(prompt)`:
//! the child inherits the parent's tools, sandbox, network, git, and write
//! authority. There is no permission-preset catalog, no operate-only flag
//! set, and no role-keyed weaker tool list.
//!
//! The only subtractions from the parent:
//! - no payments / top-ups
//! - no deleting customer data
//! - occupied checkout → isolated worktree (never stash/reset the main tree)
//!
//! Parent ceiling still applies: a child cannot gain what the parent lacks.
//! The Auto-Review safety floor (publish-like, destructive detached work,
//! secrets) stays authoritative.

use serde_json::Value;

/// Hard carve-outs that stay denied even when the child inherits the parent.
/// These are product law, not a permission package.
pub fn is_hard_carve_out(name: &str, input: &Value) -> bool {
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let blob = format!(
        "{name} {command} {}",
        input
            .get("args")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase()
    );
    is_payment_or_topup(&blob) || is_customer_data_delete(&blob)
}

fn is_payment_or_topup(blob: &str) -> bool {
    blob.contains("sk_live_")
        || blob.contains("stripe charges")
        || blob.contains("payment_intent")
        || blob.contains("top-up")
        || blob.contains("topup")
        || blob.contains("stripe payment")
}

fn is_customer_data_delete(blob: &str) -> bool {
    blob.contains("drop table")
        || blob.contains("delete from users")
        || blob.contains("delete from cwc_")
        || blob.contains("delete customer")
        || blob.contains("truncate cwc_")
}

/// Isolate writes when the parent checkout is occupied or another writer is
/// already live. Never stash/reset the occupied tree.
#[must_use]
pub fn should_isolate_worktree(checkout_occupied: bool, parallel_writers: bool) -> bool {
    checkout_occupied || parallel_writers
}

/// Root children inherit the parent's session. Launching the child is the
/// approval for ordinary work in that slice. Safety-floor holds remain.
/// Plan is a user mode: when the parent is Plan, inherit that read-only
/// contract instead of granting write+shell.
#[must_use]
pub fn root_child_inherits_parent(spawn_depth: u32) -> bool {
    spawn_depth == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_permission_preset_enum_exists() {
        // Compile-time + string guard: this crate must not grow a preset
        // matrix. Roles are prompt labels; authority is inherit-parent.
        let src = include_str!("simple_worker.rs");
        assert!(
            !src.contains(concat!("ChildPermission", "Preset"))
                && !src.contains(concat!("Operate", "Worker"))
                && !src.contains(concat!("Write", "Worktree")),
            "do not add a permission-preset catalog"
        );
    }

    #[test]
    fn spawn_prompt_needs_no_extra_flags() {
        assert!(root_child_inherits_parent(0));
        assert!(!root_child_inherits_parent(1));
    }

    #[test]
    fn ordinary_workspace_work_is_not_a_carve_out() {
        assert!(!is_hard_carve_out(
            "bash",
            &json!({"command": "git status"})
        ));
        assert!(!is_hard_carve_out(
            "bash",
            &json!({"command": "cargo test"})
        ));
        assert!(!is_hard_carve_out(
            "bash",
            &json!({"command": "gh pr create --title fix"})
        ));
        assert!(!is_hard_carve_out(
            "write",
            &json!({"path": "src/lib.rs", "content": "ok"})
        ));
    }

    #[test]
    fn payments_and_customer_deletes_stay_blocked() {
        assert!(is_hard_carve_out(
            "bash",
            &json!({"command": "stripe charges create --amount 500"})
        ));
        assert!(is_hard_carve_out(
            "bash",
            &json!({"command": "stripe top-ups create"})
        ));
        assert!(is_hard_carve_out(
            "bash",
            &json!({"command": "psql -c 'delete from users'"})
        ));
        assert!(is_hard_carve_out(
            "bash",
            &json!({"command": "psql -c 'drop table customers'"})
        ));
    }

    #[test]
    fn occupied_checkout_uses_a_worktree() {
        assert!(should_isolate_worktree(true, false));
        assert!(should_isolate_worktree(false, true));
        assert!(!should_isolate_worktree(false, false));
    }
}
