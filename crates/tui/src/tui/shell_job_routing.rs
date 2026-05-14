//! Background shell job-center helpers for slash commands and pagers.

use crate::tools::shell::{ShellJobDetail, ShellJobSnapshot, ShellResult, ShellStatus};
use crate::tui::app::App;
use crate::tui::history::HistoryCell;
use crate::tui::pager::PagerView;

fn status_label(status: &ShellStatus, stale: bool) -> &'static str {
    if stale {
        return "stale";
    }
    match status {
        ShellStatus::Running => "running",
        ShellStatus::Completed => "complete",
        ShellStatus::Failed => "failed",
        ShellStatus::Killed => "killed",
        ShellStatus::TimedOut => "timeout",
    }
}

fn format_elapsed(ms: u64) -> String {
    if ms == 0 {
        return "-".to_string();
    }
    if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.1}m", ms as f64 / 60_000.0)
    }
}

pub(super) fn format_shell_job_list(jobs: &[ShellJobSnapshot]) -> String {
    if jobs.is_empty() {
        return "No live background shell jobs. Jobs are process-local; after a restart, inspect durable task artifacts for prior command output.".to_string();
    }

    let mut lines = vec![
        format!("Background shell jobs ({})", jobs.len()),
        "----------------------------------------".to_string(),
    ];
    for job in jobs {
        let task = job
            .linked_task_id
            .as_ref()
            .map(|id| format!(" task={id}"))
            .unwrap_or_default();
        lines.push(format!(
            "{}  {:8}  {}  exit={:?}{}",
            job.id,
            status_label(&job.status, job.stale),
            format_elapsed(job.elapsed_ms),
            job.exit_code,
            task
        ));
        lines.push(format!("  cwd: {}", crate::utils::display_path(&job.cwd)));
        lines.push(format!("  cmd: {}", job.command));
        let tail = if !job.stderr_tail.trim().is_empty() {
            job.stderr_tail.trim()
        } else {
            job.stdout_tail.trim()
        };
        if !tail.is_empty() {
            lines.push(format!("  tail: {}", tail.replace('\n', "\\n")));
        }
    }
    lines.push(
        "Controls: /jobs show <id>, /jobs poll <id>, /jobs wait <id>, /jobs stdin <id> <input>, /jobs cancel <id>, /jobs cancel-all."
            .to_string(),
    );
    lines.join("\n")
}

pub(super) fn format_shell_poll(result: &ShellResult) -> String {
    let mut lines = vec![
        format!(
            "Shell job {}: {} exit={:?} elapsed={}",
            result.task_id.as_deref().unwrap_or("(unknown)"),
            status_label(&result.status, false),
            result.exit_code,
            format_elapsed(result.duration_ms)
        ),
        String::new(),
    ];
    if result.stdout.is_empty() && result.stderr.is_empty() {
        lines.push("(no new output)".to_string());
    } else {
        if !result.stdout.is_empty() {
            lines.push("STDOUT:".to_string());
            lines.push(result.stdout.clone());
        }
        if !result.stderr.is_empty() {
            lines.push("STDERR:".to_string());
            lines.push(result.stderr.clone());
        }
    }
    lines.join("\n")
}

pub(super) fn open_shell_job_pager(app: &mut App, detail: &ShellJobDetail) {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(100)
        .saturating_sub(4);
    app.view_stack.push(PagerView::from_text(
        format!("Shell Job {}", detail.snapshot.id),
        &format_shell_job_detail(detail),
        width.max(60),
    ));
}

fn format_shell_job_detail(detail: &ShellJobDetail) -> String {
    let job = &detail.snapshot;
    let mut lines = vec![
        format!("Job: {}", job.id),
        format!("Status: {}", status_label(&job.status, job.stale)),
        format!("Command: {}", job.command),
        format!("Cwd: {}", crate::utils::display_path(&job.cwd)),
        format!("Elapsed: {}", format_elapsed(job.elapsed_ms)),
        format!("Exit Code: {:?}", job.exit_code),
        format!("Stdin Available: {}", job.stdin_available),
    ];
    if let Some(task_id) = job.linked_task_id.as_ref() {
        lines.push(format!("Linked Task: {task_id}"));
    }
    if job.stale {
        lines.push("Completion State: stale after restart; process is not attached.".to_string());
    } else {
        lines.push("Completion State: live in this TUI process.".to_string());
    }
    lines.push(String::new());
    lines.push(format!("STDOUT ({} bytes):", job.stdout_len));
    lines.push(if detail.stdout.is_empty() {
        "(empty)".to_string()
    } else {
        detail.stdout.clone()
    });
    lines.push(String::new());
    lines.push(format!("STDERR ({} bytes):", job.stderr_len));
    lines.push(if detail.stderr.is_empty() {
        "(empty)".to_string()
    } else {
        detail.stderr.clone()
    });
    lines.join("\n")
}

pub(super) fn add_shell_job_message(app: &mut App, content: String) {
    app.add_message(HistoryCell::System { content });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn list_shows_controls_and_stale_state() {
        let jobs = vec![ShellJobSnapshot {
            id: "shell_dead".to_string(),
            job_id: "shell_dead".to_string(),
            command: "cargo test".to_string(),
            cwd: PathBuf::from("/tmp/repo"),
            status: ShellStatus::Killed,
            exit_code: None,
            elapsed_ms: 0,
            stdout_tail: String::new(),
            stderr_tail: "detached".to_string(),
            stdout_len: 0,
            stderr_len: 8,
            stdin_available: false,
            stale: true,
            linked_task_id: Some("task_1".to_string()),
        }];
        let formatted = format_shell_job_list(&jobs);
        assert!(formatted.contains("shell_dead"));
        assert!(formatted.contains("stale"));
        assert!(formatted.contains("/jobs poll <id>"));
        assert!(formatted.contains("task=task_1"));
    }
}
