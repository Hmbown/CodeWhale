//! Durable automation formatting and operator actions.

use crate::automation_manager::{
    AutomationRecord, AutomationRunRecord, AutomationRunStatus, AutomationStatus,
    SharedAutomationManager, run_now_shared,
};
use crate::localization::{Locale, MessageId, tr};
use crate::task_manager::SharedTaskManager;
use crate::tui::app::{App, AutomationAction};
use crate::tui::history::HistoryCell;

pub(super) async fn handle_action(
    app: &mut App,
    action: AutomationAction,
    task_manager: &SharedTaskManager,
) {
    let locale = app.ui_locale;
    let Some(automations) = app.runtime_services.automations.clone() else {
        add_message(
            app,
            tr(locale, MessageId::AutomationManagerUnavailable).into_owned(),
        );
        return;
    };

    let content = match action {
        AutomationAction::List => list(locale, &automations).await,
        AutomationAction::Show(id) => show(locale, &automations, &id).await,
        AutomationAction::Pause(id) => mutate(locale, &automations, &id, Mutation::Pause).await,
        AutomationAction::Resume(id) => mutate(locale, &automations, &id, Mutation::Resume).await,
        AutomationAction::Delete { id, confirmation } => {
            delete(locale, &automations, &id, confirmation.as_deref()).await
        }
        AutomationAction::Run(id) => match run_now_shared(&automations, &id, task_manager).await {
            Ok(run) => format_run_enqueued(locale, &id, &run),
            Err(error) => action_failed(locale, MessageId::AutomationActionRun, &id, &error),
        },
    };
    add_message(app, content);
}

async fn list(locale: Locale, automations: &SharedAutomationManager) -> String {
    match automations.lock().await.list_automations() {
        Ok(records) => format_list(locale, &records),
        Err(error) => {
            tr(locale, MessageId::AutomationListFailed).replace("{error}", &error.to_string())
        }
    }
}

async fn show(locale: Locale, automations: &SharedAutomationManager, id: &str) -> String {
    let manager = automations.lock().await;
    match manager.get_automation(id) {
        Ok(record) => {
            let runs = manager.list_runs(id, Some(5)).ok();
            format_detail(locale, &record, runs.as_deref())
        }
        Err(error) => action_failed(locale, MessageId::AutomationActionInspect, id, &error),
    }
}

#[derive(Clone, Copy)]
enum Mutation {
    Pause,
    Resume,
}

impl Mutation {
    const fn action_id(self) -> MessageId {
        match self {
            Self::Pause => MessageId::AutomationActionPause,
            Self::Resume => MessageId::AutomationActionResume,
        }
    }

    const fn receipt_id(self) -> MessageId {
        match self {
            Self::Pause => MessageId::AutomationActionPaused,
            Self::Resume => MessageId::AutomationActionResumed,
        }
    }
}

async fn mutate(
    locale: Locale,
    automations: &SharedAutomationManager,
    id: &str,
    mutation: Mutation,
) -> String {
    let manager = automations.lock().await;
    let result = match mutation {
        Mutation::Pause => manager.pause_automation(id),
        Mutation::Resume => manager.resume_automation(id),
    };

    match result {
        Ok(record) => tr(locale, MessageId::AutomationMutationReceipt)
            .replace("{name}", &display_text(&record.name))
            .replace("{action}", &tr(locale, mutation.receipt_id()))
            .replace(
                "{status_label}",
                &tr(locale, MessageId::AutomationStatusLabel),
            )
            .replace("{status}", &status_label(locale, record.status)),
        Err(error) => action_failed(locale, mutation.action_id(), id, &error),
    }
}

async fn delete(
    locale: Locale,
    automations: &SharedAutomationManager,
    id: &str,
    confirmation: Option<&str>,
) -> String {
    let manager = automations.lock().await;
    let record = match manager.get_automation(id) {
        Ok(record) => record,
        Err(error) => {
            return action_failed(locale, MessageId::AutomationActionDelete, id, &error);
        }
    };
    let runs = match manager.list_runs(id, None) {
        Ok(runs) => runs,
        Err(error) => {
            return action_failed(locale, MessageId::AutomationActionDelete, id, &error);
        }
    };
    let token = match deletion_token(&record, &runs) {
        Ok(token) => token,
        Err(error) => {
            return action_failed(locale, MessageId::AutomationActionDelete, id, &error);
        }
    };

    let Some(confirmation) = confirmation else {
        let command = format!("/automation delete {id} --confirm {token}");
        let recent_len = runs.len().min(5);
        let detail = format_detail(locale, &record, Some(&runs[..recent_len]));
        let preview = tr(locale, MessageId::AutomationDeletePreview)
            .replace("{id}", id)
            .replace("{name}", &display_text(&record.name))
            .replace("{run_count}", &runs.len().to_string())
            .replace("{command}", &command);
        return format!("{detail}\n\n{preview}");
    };

    if confirmation != token {
        let command = format!("/automation delete {id}");
        return tr(locale, MessageId::AutomationDeleteConfirmationStale)
            .replace("{id}", id)
            .replace("{command}", &command);
    }

    match manager.delete_automation(id) {
        Ok(record) => tr(locale, MessageId::AutomationDeleted)
            .replace("{id}", id)
            .replace("{name}", &display_text(&record.name))
            .replace("{run_count}", &runs.len().to_string()),
        Err(error) => action_failed(locale, MessageId::AutomationActionDelete, id, &error),
    }
}

fn deletion_token(
    record: &AutomationRecord,
    runs: &[AutomationRunRecord],
) -> Result<String, serde_json::Error> {
    let mut canonical_runs = runs.iter().collect::<Vec<_>>();
    canonical_runs.sort_by(|left, right| left.id.cmp(&right.id));
    serde_json::to_vec(&(record, canonical_runs)).map(crate::hashing::sha256_hex)
}

fn action_failed(
    locale: Locale,
    action_id: MessageId,
    id: &str,
    error: &impl std::fmt::Display,
) -> String {
    tr(locale, MessageId::AutomationActionFailed)
        .replace("{action}", &tr(locale, action_id))
        .replace("{id}", id)
        .replace("{error}", &error.to_string())
}

fn format_list(locale: Locale, records: &[AutomationRecord]) -> String {
    if records.is_empty() {
        return tr(locale, MessageId::AutomationEmpty).into_owned();
    }

    let lines = records
        .iter()
        .map(|record| {
            format!(
                "{}  [{}]  {}  ({}; {}: {})",
                record.id,
                status_label(locale, record.status),
                display_text(&record.name),
                delivery_mode_label(record),
                tr(locale, MessageId::AutomationNextLabel),
                timestamp(record.next_run_at)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}:\n{lines}", tr(locale, MessageId::AutomationListHeading))
}

fn format_detail(
    locale: Locale,
    record: &AutomationRecord,
    runs: Option<&[AutomationRunRecord]>,
) -> String {
    let runs = match runs {
        Some([]) => format!("  {}", tr(locale, MessageId::AutomationNoRuns)),
        Some(runs) => runs
            .iter()
            .map(|run| {
                format!(
                    "  {}  {}  ({} {})",
                    run_status_label(locale, run.status),
                    run.scheduled_for.to_rfc3339(),
                    tr(locale, MessageId::AutomationTaskLabel),
                    run.task_id.as_deref().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => format!("  {}", tr(locale, MessageId::AutomationRunsUnavailable)),
    };
    let mut lines = vec![
        format!(
            "{} {} [{}]",
            tr(locale, MessageId::AutomationNoun),
            record.id,
            status_label(locale, record.status)
        ),
        field(
            locale,
            MessageId::AutomationNameLabel,
            &display_text(&record.name),
        ),
        format!("  {}:", tr(locale, MessageId::AutomationPromptLabel)),
    ];
    lines.extend(
        display_text(&record.prompt)
            .lines()
            .map(|line| format!("    {line}")),
    );
    lines.extend(record.cwds.iter().map(|cwd| {
        field(
            locale,
            MessageId::AutomationCwdLabel,
            &display_text(&crate::utils::display_path(cwd)),
        )
    }));
    if let Some(mode) = record.mode.as_deref() {
        lines.push(field(
            locale,
            MessageId::AutomationModeLabel,
            &display_text(mode),
        ));
    }
    if let Some(allow_shell) = record.allow_shell {
        lines.push(field(
            locale,
            MessageId::AutomationAllowShellLabel,
            &allow_shell.to_string(),
        ));
    }
    if let Some(trust_mode) = record.trust_mode {
        lines.push(field(
            locale,
            MessageId::AutomationTrustModeLabel,
            &trust_mode.to_string(),
        ));
    }
    if let Some(auto_approve) = record.auto_approve {
        lines.push(field(
            locale,
            MessageId::AutomationAutoApproveLabel,
            &auto_approve.to_string(),
        ));
    }
    lines.extend([
        field(locale, MessageId::AutomationRruleLabel, &record.rrule),
        field(
            locale,
            MessageId::AutomationDeliveryLabel,
            &delivery_mode_label(record),
        ),
        field(
            locale,
            MessageId::AutomationNextLabel,
            &timestamp(record.next_run_at),
        ),
        field(
            locale,
            MessageId::AutomationLastLabel,
            &timestamp(record.last_run_at),
        ),
        format!(
            "{}:\n{runs}",
            tr(locale, MessageId::AutomationRecentRunsLabel)
        ),
    ]);
    lines.join("\n")
}

fn field(locale: Locale, label: MessageId, value: &str) -> String {
    format!("  {}: {value}", tr(locale, label))
}

fn display_text(value: &str) -> String {
    let mut visible = String::with_capacity(value.len());
    crate::tui::osc8::strip_ansi_into(value, &mut visible);
    codewhale_config::persistence::redact_secrets(&visible)
}

fn format_run_enqueued(locale: Locale, id: &str, run: &AutomationRunRecord) -> String {
    tr(locale, MessageId::AutomationRunEnqueued)
        .replace("{id}", id)
        .replace("{status}", &run_status_label(locale, run.status))
        .replace("{task}", run.task_id.as_deref().unwrap_or("-"))
}

fn status_label(locale: Locale, status: AutomationStatus) -> String {
    let id = match status {
        AutomationStatus::Active => MessageId::AutomationStatusActive,
        AutomationStatus::Paused => MessageId::AutomationStatusPaused,
    };
    tr(locale, id).into_owned()
}

fn run_status_label(locale: Locale, status: AutomationRunStatus) -> String {
    let id = match status {
        AutomationRunStatus::Queued => MessageId::AutomationRunStatusQueued,
        AutomationRunStatus::Running => MessageId::AutomationRunStatusRunning,
        AutomationRunStatus::Completed => MessageId::AutomationRunStatusCompleted,
        AutomationRunStatus::Failed => MessageId::AutomationRunStatusFailed,
        AutomationRunStatus::Canceled => MessageId::AutomationRunStatusCanceled,
    };
    tr(locale, id).into_owned()
}

fn timestamp(value: Option<chrono::DateTime<chrono::Utc>>) -> String {
    value
        .map(|timestamp| timestamp.to_rfc3339())
        .unwrap_or_else(|| "-".to_string())
}

/// Delivery mode is a stored enum value, not prose — render it raw like
/// `mode`/`rrule` rather than translating it. Unset means the default `task`.
fn delivery_mode_label(record: &AutomationRecord) -> String {
    format!("{:?}", record.delivery_mode.unwrap_or_default()).to_ascii_lowercase()
}

fn add_message(app: &mut App, content: String) {
    app.add_message(HistoryCell::System { content });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use super::*;
    use crate::automation_manager::{
        AutomationDeliveryMode, AutomationManager, CreateAutomationRequest,
    };
    use chrono::Utc;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    fn record(status: AutomationStatus) -> AutomationRecord {
        let now = Utc::now();
        AutomationRecord {
            schema_version: 1,
            id: "auto_1".to_string(),
            name: "Nightly checks".to_string(),
            prompt: "Run checks".to_string(),
            rrule: "FREQ=DAILY".to_string(),
            cwds: Vec::new(),
            model: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            delivery_mode: None,
            status,
            created_at: now,
            updated_at: now,
            next_run_at: None,
            last_run_at: None,
        }
    }

    #[test]
    fn list_explains_empty_state_and_operator_controls() {
        assert!(format_list(Locale::En, &[]).contains("`automation` tool to create one"));
        let text = format_list(Locale::En, &[record(AutomationStatus::Paused)]);
        assert!(text.contains("auto_1  [paused]  Nightly checks"));
        assert!(text.contains("next: -"));
    }

    #[test]
    fn detail_keeps_schedule_and_recent_run_shape() {
        let text = format_detail(Locale::En, &record(AutomationStatus::Active), Some(&[]));
        assert!(text.contains("Automation auto_1 [active]"));
        assert!(text.contains("rrule: FREQ=DAILY"));
        assert!(text.contains("recent runs:"));
    }

    #[test]
    fn list_and_detail_surface_delivery_mode() {
        let mut automation = record(AutomationStatus::Active);
        automation.delivery_mode = Some(AutomationDeliveryMode::Watcher);

        let list = format_list(Locale::En, std::slice::from_ref(&automation));
        assert!(list.contains("(watcher; next:"));

        let detail = format_detail(Locale::En, &automation, Some(&[]));
        assert!(detail.contains("  delivery: watcher"));

        let default_detail =
            format_detail(Locale::En, &record(AutomationStatus::Active), Some(&[]));
        assert!(default_detail.contains("  delivery: task"));
    }

    #[test]
    fn detail_exposes_configured_execution_contract_and_redacts_prompt() {
        let mut automation = record(AutomationStatus::Active);
        automation.prompt = "Run release checks\napi_key = \"sk-audit-secret-value\"".to_string();
        automation.cwds = vec!["release-workspace".into()];
        automation.mode = Some("agent".to_string());
        automation.allow_shell = Some(true);
        automation.trust_mode = Some(false);
        automation.auto_approve = Some(true);

        let text = format_detail(Locale::En, &automation, Some(&[]));

        assert!(text.contains("  prompt:\n    Run release checks"));
        assert!(text.contains("[redacted]"));
        assert!(!text.contains("sk-audit-secret-value"));
        assert!(text.contains("  cwd: release-workspace"));
        assert!(text.contains("  mode: agent"));
        assert!(text.contains("  allow_shell: true"));
        assert!(text.contains("  trust_mode: false"));
        assert!(text.contains("  auto_approve: true"));
    }

    #[test]
    fn list_stays_compact_and_detail_omits_unset_execution_overrides() {
        let automation = record(AutomationStatus::Paused);

        let list = format_list(Locale::En, std::slice::from_ref(&automation));
        assert!(!list.contains(&automation.prompt));
        assert!(!list.contains("prompt:"));
        assert!(!list.contains("cwd:"));
        assert!(!list.contains("mode:"));
        assert!(!list.contains("allow_shell:"));
        assert!(!list.contains("trust_mode:"));
        assert!(!list.contains("auto_approve:"));

        let detail = format_detail(Locale::En, &automation, Some(&[]));
        assert!(!detail.contains("cwd:"));
        assert!(!detail.contains("mode:"));
        assert!(!detail.contains("allow_shell:"));
        assert!(!detail.contains("trust_mode:"));
        assert!(!detail.contains("auto_approve:"));
    }

    #[test]
    fn automation_output_routes_through_the_selected_locale() {
        let french = format_list(Locale::Fr, &[record(AutomationStatus::Paused)]);
        assert!(french.starts_with(tr(Locale::Fr, MessageId::AutomationListHeading).as_ref()));
        assert!(french.contains(tr(Locale::Fr, MessageId::AutomationStatusPaused).as_ref()));
        assert!(!french.starts_with(tr(Locale::En, MessageId::AutomationListHeading).as_ref()));

        for locale in Locale::shipped_complete() {
            for id in [
                MessageId::AutomationManagerUnavailable,
                MessageId::AutomationDeletePreview,
                MessageId::AutomationDeleted,
                MessageId::AutomationRunEnqueued,
            ] {
                assert_ne!(tr(*locale, id).as_ref(), format!("{id:?}"), "{locale:?}");
            }
        }
    }

    #[tokio::test]
    async fn delete_is_a_noop_until_snapshot_confirmation_then_removes_definition_and_runs() {
        let temp = TempDir::new().expect("temp dir");
        let manager = AutomationManager::open(temp.path().to_path_buf()).expect("manager");
        let automation = manager
            .create_automation(CreateAutomationRequest {
                name: "Nightly checks".to_string(),
                prompt: "Run checks".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                delivery_mode: None,
                status: Some(AutomationStatus::Paused),
            })
            .expect("automation");
        let now = Utc::now();
        let run = AutomationRunRecord {
            schema_version: 1,
            id: "run_1".to_string(),
            automation_id: automation.id.clone(),
            scheduled_for: now,
            status: AutomationRunStatus::Completed,
            created_at: now,
            started_at: Some(now),
            ended_at: Some(now),
            task_id: Some("task_1".to_string()),
            thread_id: None,
            turn_id: None,
            error: None,
        };
        let runs_dir = temp.path().join("runs").join(&automation.id);
        fs::create_dir_all(&runs_dir).expect("runs dir");
        fs::write(
            runs_dir.join("run_1.json"),
            serde_json::to_vec_pretty(&run).expect("serialize run"),
        )
        .expect("write run");
        let manager = Arc::new(Mutex::new(manager));

        let preview = delete(Locale::En, &manager, &automation.id, None).await;
        assert!(preview.contains("Nothing was deleted"), "{preview}");
        assert!(preview.contains("Recorded runs: 1"), "{preview}");
        assert!(
            manager.lock().await.get_automation(&automation.id).is_ok(),
            "preview must preserve the definition"
        );
        assert_eq!(
            manager
                .lock()
                .await
                .list_runs(&automation.id, None)
                .expect("runs after preview")
                .len(),
            1,
            "preview must preserve run history"
        );

        let stale = delete(Locale::En, &manager, &automation.id, Some("wrong-receipt")).await;
        assert!(stale.contains("no longer matches"), "{stale}");
        assert!(
            manager.lock().await.get_automation(&automation.id).is_ok(),
            "a mismatched receipt must not delete"
        );

        let token = preview
            .lines()
            .find(|line| line.starts_with("/automation delete "))
            .and_then(|line| line.split_whitespace().last())
            .expect("preview confirmation receipt");
        let deleted = delete(Locale::En, &manager, &automation.id, Some(token)).await;
        assert!(deleted.contains("Recorded runs deleted: 1"), "{deleted}");
        assert!(
            manager.lock().await.get_automation(&automation.id).is_err(),
            "confirmed deletion removes definition"
        );
        assert!(!runs_dir.exists(), "confirmed deletion removes run history");
    }
}
