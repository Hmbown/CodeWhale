//! `/restore` slash command — roll back the workspace to a prior snapshot.
//!
//! `/restore` (no arg) lists the 20 most recent snapshots so the user can
//! see what's available. `/restore list [N]` lists more snapshots, capped
//! at 100. `/restore <N>` restores the *N*th-most-recent snapshot, where
//! `N=1` is the newest. In non-YOLO mode we refuse to mutate files unless
//! the user has explicitly trusted the workspace (`/trust on` or YOLO) —
//! the user can always view the list, just not one-shot revert without a
//! safety net.

use super::CommandResult;
use crate::snapshot::{Snapshot, SnapshotRepo};
use crate::tui::app::App;
use chrono::TimeZone;

const DEFAULT_LIST_LIMIT: usize = 20;
const MAX_LIST_LIMIT: usize = 100;
const MAX_RESTORE_INDEX: usize = 1000;

/// Entry point for `/restore [N|list [N]]`.
pub fn restore(app: &mut App, arg: Option<&str>) -> CommandResult {
    let workspace = app.workspace.clone();
    let repo = match SnapshotRepo::open_or_init(&workspace) {
        Ok(r) => r,
        Err(e) => {
            return CommandResult::error(format!(
                "Snapshot repo unavailable for {}: {e}",
                workspace.display(),
            ));
        }
    };

    let Some(arg) = arg.map(str::trim).filter(|s| !s.is_empty()) else {
        let snapshots = match repo.list(DEFAULT_LIST_LIMIT) {
            Ok(s) => s,
            Err(e) => return CommandResult::error(format!("Failed to list snapshots: {e}")),
        };
        if snapshots.is_empty() {
            return no_snapshots_message();
        }
        return CommandResult::message(format_listing(&snapshots));
    };

    if let Some(limit) = match parse_list_arg(arg) {
        Ok(limit) => limit,
        Err(message) => return CommandResult::error(message),
    } {
        let snapshots = match repo.list(limit) {
            Ok(s) => s,
            Err(e) => return CommandResult::error(format!("Failed to list snapshots: {e}")),
        };
        if snapshots.is_empty() {
            return no_snapshots_message();
        }
        return CommandResult::message(format_listing(&snapshots));
    }

    let n: usize = match arg.parse() {
        Ok(n) if (1..=MAX_RESTORE_INDEX).contains(&n) => n,
        Ok(n) if n > MAX_RESTORE_INDEX => {
            return CommandResult::error(format!(
                "Restore index must be <= {MAX_RESTORE_INDEX}; got {n}. Use /restore list [N] to inspect snapshots first.",
            ));
        }
        _ => {
            return CommandResult::error(format!(
                "Usage: /restore <N> or /restore list [N]  (N is 1-based; got '{arg}')",
            ));
        }
    };
    let snapshots = match repo.list(n.max(DEFAULT_LIST_LIMIT)) {
        Ok(s) => s,
        Err(e) => return CommandResult::error(format!("Failed to list snapshots: {e}")),
    };
    if snapshots.is_empty() {
        return no_snapshots_message();
    }

    if n > snapshots.len() {
        return CommandResult::error(format!(
            "Only {} snapshot(s) available; asked for #{n}.",
            snapshots.len(),
        ));
    }

    // Non-YOLO sessions get a confirmation gate. We don't have a true
    // modal-confirmation path inside slash commands today, so the gate
    // is "require trust mode" — `/trust on` or YOLO. Users in plain
    // Agent mode get a clear message explaining how to proceed.
    if !(app.yolo || app.trust_mode) {
        return CommandResult::message(format!(
            "Refusing to restore snapshot #{n} ('{}') outside trusted mode.\n\
             Run `/trust on` or `/mode yolo` first, then re-run `/restore {n}`.",
            snapshots[n - 1].label,
        ));
    }

    let target = &snapshots[n - 1];
    if let Err(e) = repo.restore(&target.id) {
        return CommandResult::error(format!("Restore failed: {e}"));
    }

    CommandResult::message(format!(
        "Restored snapshot #{n} ('{}', {}). Workspace files have been reverted; conversation history is unchanged.",
        target.label,
        short_sha(target.id.as_str()),
    ))
}

fn parse_list_arg(arg: &str) -> Result<Option<usize>, String> {
    let mut parts = arg.split_whitespace();
    let action = match parts.next() {
        Some(action) => action,
        None => return Ok(None),
    };
    if action != "list" {
        return Ok(None);
    }
    let Some(value) = parts.next() else {
        return Ok(Some(DEFAULT_LIST_LIMIT));
    };
    if parts.next().is_some() {
        return Err(format!(
            "Usage: /restore list [N]  (got extra arguments in '{arg}')",
        ));
    }
    let limit = match value.parse::<usize>() {
        Ok(limit) if limit >= 1 => limit.min(MAX_LIST_LIMIT),
        _ => {
            return Err(format!(
                "Usage: /restore list [N]  (N must be >= 1; got '{value}')",
            ));
        }
    };
    Ok(Some(limit))
}

fn no_snapshots_message() -> CommandResult {
    CommandResult::message(
        "No snapshots yet. Send a message to create the first pre-turn snapshot.",
    )
}

fn format_listing(snapshots: &[Snapshot]) -> String {
    let mut out = String::from(
        "Recent snapshots (newest first; pass /restore <N> to revert; /restore list 50 shows more):\n",
    );
    for (i, s) in snapshots.iter().enumerate() {
        out.push_str(&format!(
            "  #{:<2}  {}  {}  {}\n",
            i + 1,
            format_snapshot_time(s.timestamp),
            short_sha(s.id.as_str()),
            s.label,
        ));
    }
    out
}

fn format_snapshot_time(timestamp: i64) -> String {
    match chrono::Utc.timestamp_opt(timestamp, 0).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => "unknown time".to_string(),
    }
}

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_support::lock_test_env;
    use crate::tui::app::TuiOptions;
    use std::sync::MutexGuard;
    use tempfile::TempDir;

    fn make_app(tmp: &TempDir, yolo: bool) -> App {
        let workspace = tmp.path().to_path_buf();
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace,
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmp.path().join("skills"),
            memory_path: tmp.path().join("memory.md"),
            notes_path: tmp.path().join("notes.txt"),
            mcp_config_path: tmp.path().join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    /// Pins HOME to a tempdir for the duration of the test under the
    /// crate-wide env mutex.
    struct ScopedHome {
        prev: Option<std::ffi::OsString>,
        _home: TempDir,
        _guard: MutexGuard<'static, ()>,
    }
    impl Drop for ScopedHome {
        fn drop(&mut self) {
            // SAFETY: process-wide lock still held.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }
    fn scoped_home(_workspace: &TempDir) -> ScopedHome {
        let guard = lock_test_env();
        let prev = std::env::var_os("HOME");
        let home = TempDir::new().expect("home tempdir");
        // SAFETY: serialised by the global env lock.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        ScopedHome {
            prev,
            _home: home,
            _guard: guard,
        }
    }

    #[test]
    fn restore_with_no_snapshots_shows_empty_message() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);
        let result = restore(&mut app, None);
        let msg = result.message.expect("expected message");
        assert!(msg.contains("No snapshots"));
    }

    #[test]
    fn restore_lists_when_no_arg_provided() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);
        let repo = SnapshotRepo::open_or_init(&app.workspace).unwrap();
        std::fs::write(app.workspace.join("a.txt"), b"v1").unwrap();
        repo.snapshot("pre-turn:1").unwrap();
        std::fs::write(app.workspace.join("a.txt"), b"v2").unwrap();
        repo.snapshot("post-turn:1").unwrap();

        let result = restore(&mut app, None);
        let msg = result.message.expect("expected message");
        assert!(msg.contains("post-turn:1"));
        assert!(msg.contains("pre-turn:1"));
        assert!(msg.contains("#1"));
        assert!(msg.contains("#2"));
    }

    #[test]
    fn restore_lists_more_than_ten_snapshots_by_default() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);
        let repo = SnapshotRepo::open_or_init(&app.workspace).unwrap();
        for i in 0..12 {
            std::fs::write(app.workspace.join("a.txt"), format!("v{i}")).unwrap();
            repo.snapshot(&format!("turn:{i}")).unwrap();
        }

        let result = restore(&mut app, None);
        let msg = result.message.expect("expected message");
        assert!(msg.contains("#12"), "{msg}");
        assert!(msg.contains("turn:0"), "{msg}");
    }

    #[test]
    fn restore_listing_includes_snapshot_utc_time() {
        let snapshots = [Snapshot {
            id: crate::snapshot::SnapshotId("abcdef123456".to_string()),
            label: "turn:demo".to_string(),
            timestamp: 1_700_000_000,
        }];

        let msg = format_listing(&snapshots);

        assert!(msg.contains("2023-11-14 22:13 UTC"), "{msg}");
        assert!(msg.contains("abcdef12"), "{msg}");
        assert!(msg.contains("turn:demo"), "{msg}");
    }

    #[test]
    fn restore_list_subcommand_accepts_explicit_limit() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);
        let repo = SnapshotRepo::open_or_init(&app.workspace).unwrap();
        for i in 0..15 {
            std::fs::write(app.workspace.join("a.txt"), format!("v{i}")).unwrap();
            repo.snapshot(&format!("turn:{i}")).unwrap();
        }

        let result = restore(&mut app, Some("list 12"));
        let msg = result.message.expect("expected message");
        assert!(msg.contains("#12"), "{msg}");
        assert!(!msg.contains("#13"), "{msg}");
    }

    #[test]
    fn restore_list_subcommand_rejects_invalid_limit() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);

        let result = restore(&mut app, Some("list nope"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage: /restore list [N]"));
    }

    #[test]
    fn restore_numeric_index_can_target_beyond_default_listing() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);
        let repo = SnapshotRepo::open_or_init(&app.workspace).unwrap();
        let f = app.workspace.join("a.txt");
        for i in 0..12 {
            std::fs::write(&f, format!("v{i}")).unwrap();
            repo.snapshot(&format!("turn:{i}")).unwrap();
        }
        std::fs::write(&f, "changed").unwrap();

        let result = restore(&mut app, Some("12"));
        assert!(result.message.unwrap().contains("Restored"));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v0");
    }

    #[test]
    fn restore_numeric_index_rejects_unbounded_query() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);

        let result = restore(&mut app, Some("1001"));

        assert!(result.is_error);
        assert!(
            result
                .message
                .unwrap()
                .contains("Restore index must be <= 1000")
        );
    }

    #[test]
    fn restore_in_yolo_reverts_workspace() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);
        let repo = SnapshotRepo::open_or_init(&app.workspace).unwrap();
        let f = app.workspace.join("a.txt");

        std::fs::write(&f, b"original").unwrap();
        repo.snapshot("pre-turn:1").unwrap();
        std::fs::write(&f, b"clobbered").unwrap();
        repo.snapshot("post-turn:1").unwrap();

        let result = restore(&mut app, Some("2"));
        assert!(result.message.unwrap().contains("Restored"));
        let after = std::fs::read_to_string(&f).unwrap();
        assert_eq!(after, "original");
    }

    #[test]
    fn restore_outside_trust_mode_refuses() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, false);
        let repo = SnapshotRepo::open_or_init(&app.workspace).unwrap();
        std::fs::write(app.workspace.join("a.txt"), b"v1").unwrap();
        repo.snapshot("pre-turn:1").unwrap();

        let result = restore(&mut app, Some("1"));
        let msg = result.message.expect("expected message");
        assert!(msg.contains("Refusing"));
        assert!(msg.contains("/trust on"));
    }

    #[test]
    fn restore_invalid_index_returns_error() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);
        let repo = SnapshotRepo::open_or_init(&app.workspace).unwrap();
        std::fs::write(app.workspace.join("a.txt"), b"v1").unwrap();
        repo.snapshot("pre-turn:1").unwrap();

        let result = restore(&mut app, Some("99"));
        let msg = result.message.expect("expected message");
        assert!(msg.contains("Only 1 snapshot"));
    }

    #[test]
    fn restore_zero_index_returns_error() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let mut app = make_app(&tmp, true);
        // Need at least one snapshot so we exercise the parse-index
        // branch instead of the "no snapshots" early return.
        let repo = SnapshotRepo::open_or_init(&app.workspace).unwrap();
        std::fs::write(app.workspace.join("a.txt"), b"v1").unwrap();
        repo.snapshot("pre-turn:1").unwrap();

        let result = restore(&mut app, Some("0"));
        let msg = result.message.expect("expected message");
        assert!(msg.contains("Usage:"));
    }
}
