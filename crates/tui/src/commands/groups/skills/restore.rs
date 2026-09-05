//! `/restore` slash command — roll back the workspace to a prior snapshot.
//!
//! `/restore` (no arg) lists the 20 most recent snapshots so the user can
//! see what's available. `/restore list [N]` lists more snapshots, capped
//! at 100. `/restore <N>` restores the *N*th-most-recent snapshot, where
//! `N=1` is the newest. Without trusted/full access we refuse to mutate files unless
//! the user has explicitly trusted the workspace (`/trust on` or Full Access) —
//! the user can always view the list, just not one-shot revert without a
//! safety net.
//!
//! FEAT-022 Phase 4: portable contextual dispatch. `SnapshotRepo` and the
//! approval state stay host-side (`CommandSkillGroupContext` delegates); the
//! portable handler owns all parsing, formatting, and the trust gate.

use chrono::TimeZone;

use codewhale_command_contract::facets::{CommandSkillGroupContext, SnapshotEntry};
use codewhale_command_contract::handler::{CommandContexts, CommandHandler};
use codewhale_command_contract::metadata::{CommandInfo, RegisterCommand};

use crate::commands::CommandResult;

const DEFAULT_LIST_LIMIT: usize = 20;
const MAX_LIST_LIMIT: usize = 100;
const MAX_RESTORE_INDEX: usize = 1000;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "restore",
    aliases: &[],
    usage: "/restore [N|list [N]]",
    description_key: "cmd_restore_description",
};

pub(in crate::commands) struct RestoreCmd;

impl RegisterCommand<CommandResult> for RestoreCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn handler() -> CommandHandler<CommandResult> {
        CommandHandler::Contextual {
            capabilities: codewhale_command_contract::handler::CommandCapabilities::SKILL_GROUP,
            handler: restore_contextual,
        }
    }
}

/// Contextual `/restore` dispatch (FEAT-022 D4): exactly the skill-group facet
/// (snapshot list/restore + approval state — no `MODE_POLICY` declaration).
fn restore_contextual(contexts: CommandContexts<'_>, arg: Option<&str>) -> CommandResult {
    let mut parts = contexts.into_parts();
    let Some(skill_group) = parts.skill_group.as_deref_mut() else {
        return CommandResult::error("Command capability unavailable: skill_group");
    };
    restore(skill_group, arg)
}

/// Portable `/restore` dispatch — byte-identical to the baseline handler.
///
/// The host owns `SnapshotRepo` open/list/restore and the yolo/trust posture;
/// the handler composes every message, error, listing, and the trust gate.
fn restore(group: &mut dyn CommandSkillGroupContext, arg: Option<&str>) -> CommandResult {
    let Some(arg) = arg.map(str::trim).filter(|s| !s.is_empty()) else {
        let snapshots = match group.snapshot_list(DEFAULT_LIST_LIMIT) {
            Ok(s) => s,
            Err(err) => return CommandResult::error(err),
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
        let snapshots = match group.snapshot_list(limit) {
            Ok(s) => s,
            Err(err) => return CommandResult::error(err),
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
    let snapshots = match group.snapshot_list(n.max(DEFAULT_LIST_LIMIT)) {
        Ok(s) => s,
        Err(err) => return CommandResult::error(err),
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

    // Sessions without trusted/full access get a confirmation gate. We don't have a true
    // modal-confirmation path inside slash commands today, so the gate
    // is "require trust mode" — `/trust on` or Full Access. Users in plain
    // Agent mode get a clear message explaining how to proceed.
    let approval = group.approval_state();
    if !(approval.yolo || approval.trust_mode) {
        return CommandResult::message(format!(
            "Refusing to restore snapshot #{n} ('{}') outside trusted mode.\n\
             Run `/trust on` or select Full Access with Shift+Tab, then re-run `/restore {n}`.",
            snapshots[n - 1].label,
        ));
    }

    let target = &snapshots[n - 1];
    if let Err(err) = group.restore_snapshot(&target.id) {
        return CommandResult::error(err);
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
    match value.parse::<usize>() {
        Ok(limit @ 1..=MAX_LIST_LIMIT) => Ok(Some(limit)),
        Ok(limit) if limit > MAX_LIST_LIMIT => Err(format!(
            "Restore list limit must be <= {MAX_LIST_LIMIT}; got {limit}.",
        )),
        _ => Err(format!(
            "Usage: /restore list [N]  (N must be >= 1; got '{value}')",
        )),
    }
}

fn no_snapshots_message() -> CommandResult {
    CommandResult::message(
        "No snapshots yet. Send a message to create the first pre-turn snapshot.",
    )
}

fn format_listing(snapshots: &[SnapshotEntry]) -> String {
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
    use codewhale_command_contract::facets::{
        CommandApprovalState, RemoteRegistryOutcome, ReviewOutcome, SkillActivationError,
        SkillMutationReceipt, SkillRecommendation, SkillSyncOutcome, SkillTargetScope,
    };

    struct FakeSkillGroup {
        snapshots: Result<Vec<SnapshotEntry>, String>,
        restore: Result<(), String>,
        approval: CommandApprovalState,
    }
    impl FakeSkillGroup {
        fn new(snapshots: Vec<SnapshotEntry>) -> Self {
            Self {
                snapshots: Ok(snapshots),
                restore: Ok(()),
                approval: CommandApprovalState {
                    yolo: true,
                    trust_mode: false,
                },
            }
        }
    }
    impl CommandSkillGroupContext for FakeSkillGroup {
        fn skill_registry_projection(
            &self,
        ) -> codewhale_command_contract::facets::SkillRegistryProjection {
            unimplemented!("not used by restore tests")
        }
        fn activate_skill(
            &mut self,
            _name: &str,
        ) -> Result<codewhale_command_contract::facets::SkillActivationOutcome, SkillActivationError>
        {
            unimplemented!("not used by restore tests")
        }
        fn install_skill(
            &mut self,
            _scope: Option<SkillTargetScope>,
            _spec: &str,
        ) -> Result<SkillMutationReceipt, String> {
            unimplemented!("not used by restore tests")
        }
        fn update_skill(
            &mut self,
            _scope: Option<SkillTargetScope>,
            _name: &str,
        ) -> Result<SkillMutationReceipt, String> {
            unimplemented!("not used by restore tests")
        }
        fn uninstall_skill(
            &mut self,
            _scope: Option<SkillTargetScope>,
            _name: &str,
        ) -> Result<SkillMutationReceipt, String> {
            unimplemented!("not used by restore tests")
        }
        fn trust_skill(
            &mut self,
            _scope: Option<SkillTargetScope>,
            _name: &str,
        ) -> Result<SkillMutationReceipt, String> {
            unimplemented!("not used by restore tests")
        }
        fn fetch_remote_registry(&mut self) -> Result<RemoteRegistryOutcome, String> {
            unimplemented!("not used by restore tests")
        }
        fn recommend_skills(&mut self, _task: &str) -> Result<Vec<SkillRecommendation>, String> {
            unimplemented!("not used by restore tests")
        }
        fn sync_registry(&mut self) -> Result<SkillSyncOutcome, String> {
            unimplemented!("not used by restore tests")
        }
        fn run_review(&mut self) -> Result<ReviewOutcome, String> {
            unimplemented!("not used by restore tests")
        }
        fn snapshot_list(&mut self, limit: usize) -> Result<Vec<SnapshotEntry>, String> {
            match &self.snapshots {
                Ok(snapshots) => Ok(snapshots.iter().take(limit).cloned().collect()),
                Err(err) => Err(err.clone()),
            }
        }
        fn restore_snapshot(&mut self, _id: &str) -> Result<(), String> {
            self.restore.clone()
        }
        fn approval_state(&self) -> CommandApprovalState {
            self.approval
        }
    }

    fn snap(label: &str, id: &str, timestamp: i64) -> SnapshotEntry {
        SnapshotEntry {
            id: id.to_string(),
            label: label.to_string(),
            timestamp,
        }
    }

    #[test]
    fn restore_with_no_snapshots_shows_empty_message() {
        let mut group = FakeSkillGroup::new(vec![]);
        let result = restore(&mut group, None);
        let msg = result.message.expect("expected message");
        assert!(msg.contains("No snapshots"));
    }

    #[test]
    fn restore_lists_when_no_arg_provided() {
        let mut group = FakeSkillGroup::new(vec![
            snap("post-turn:1", "11111111", 1_700_000_000),
            snap("pre-turn:1", "22222222", 1_699_000_000),
        ]);
        let result = restore(&mut group, None);
        let msg = result.message.expect("expected message");
        assert!(msg.contains("post-turn:1"));
        assert!(msg.contains("pre-turn:1"));
        assert!(msg.contains("#1"));
        assert!(msg.contains("#2"));
        assert!(msg.contains("2023-11-14 22:13 UTC"), "{msg}");
    }

    #[test]
    fn restore_list_subcommand_accepts_explicit_limit() {
        let mut group = FakeSkillGroup::new(vec![
            snap("turn:1", "11111111", 1_700_000_000),
            snap("turn:2", "22222222", 1_699_000_000),
            snap("turn:3", "33333333", 1_698_000_000),
        ]);
        let result = restore(&mut group, Some("list 2"));
        let msg = result.message.expect("expected message");
        assert!(msg.contains("#2"), "{msg}");
        assert!(!msg.contains("#3"), "{msg}");
    }

    #[test]
    fn restore_list_subcommand_rejects_invalid_limit() {
        let mut group = FakeSkillGroup::new(vec![]);
        let result = restore(&mut group, Some("list nope"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage: /restore list [N]"));
    }

    #[test]
    fn restore_list_subcommand_rejects_limit_above_cap() {
        let mut group = FakeSkillGroup::new(vec![]);
        let result = restore(&mut group, Some("list 101"));
        assert!(result.is_error);
        assert!(
            result
                .message
                .unwrap()
                .contains("Restore list limit must be <= 100")
        );
    }

    #[test]
    fn restore_numeric_index_rejects_unbounded_query() {
        let mut group = FakeSkillGroup::new(vec![]);
        let result = restore(&mut group, Some("1001"));
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
        let mut group = FakeSkillGroup::new(vec![
            snap("post-turn:1", "22222222", 1_700_000_000),
            snap("pre-turn:1", "11111111", 1_699_000_000),
        ]);
        let result = restore(&mut group, Some("2"));
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("Restored snapshot #2"));
    }

    #[test]
    fn restore_outside_trust_mode_refuses() {
        let mut group = FakeSkillGroup::new(vec![snap("pre-turn:1", "11111111", 1_700_000_000)]);
        group.approval = CommandApprovalState {
            yolo: false,
            trust_mode: false,
        };
        let result = restore(&mut group, Some("1"));
        let msg = result.message.expect("expected message");
        assert!(msg.contains("Refusing"));
        assert!(msg.contains("/trust on"));
    }

    #[test]
    fn restore_invalid_index_returns_error() {
        let mut group = FakeSkillGroup::new(vec![snap("pre-turn:1", "11111111", 1_700_000_000)]);
        let result = restore(&mut group, Some("99"));
        let msg = result.message.expect("expected message");
        assert!(msg.contains("Only 1 snapshot"));
    }

    #[test]
    fn restore_zero_index_returns_error() {
        let mut group = FakeSkillGroup::new(vec![snap("pre-turn:1", "11111111", 1_700_000_000)]);
        let result = restore(&mut group, Some("0"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage:"));
    }

    #[test]
    fn restore_host_error_reaches_boundary() {
        let mut group = FakeSkillGroup::new(vec![]);
        group.snapshots = Err("Snapshot repo unavailable for /ws: boom".to_string());
        let result = restore(&mut group, None);
        assert!(result.is_error);
        assert_eq!(
            result.message.unwrap(),
            "Error: Snapshot repo unavailable for /ws: boom"
        );
    }

    #[test]
    fn restore_missing_facet_errors_are_safe() {
        let result = restore_contextual(CommandContexts::empty(), Some("1"));
        assert!(result.is_error);
        assert_eq!(
            result.message.unwrap(),
            "Error: Command capability unavailable: skill_group"
        );
    }
}
