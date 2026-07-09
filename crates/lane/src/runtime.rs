//! Runtime backends: tmux durability, inline, vm/ci stubs (#4176).
//!
//! Runtime owns process/session lifecycle and stream-json log capture.
//! Fleet modules must not import this module.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::registry::{LaneRecord, LaneRegistry, LaneStatus};
use crate::worktree::{WorktreeProvision, provision_worktree, remove_worktree_if_expired};

/// Execution backend for a lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackendKind {
    Tmux,
    Inline,
    Vm,
    Ci,
}

impl RuntimeBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Inline => "inline",
            Self::Vm => "vm",
            Self::Ci => "ci",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tmux" => Ok(Self::Tmux),
            "inline" => Ok(Self::Inline),
            "vm" => Ok(Self::Vm),
            "ci" => Ok(Self::Ci),
            other => bail!("unknown runtime backend `{other}` (use tmux|inline|vm|ci)"),
        }
    }
}

/// Inputs for starting a lane under a runtime backend.
#[derive(Debug, Clone)]
pub struct LaneStartSpec {
    /// Command argv to run inside the backend (e.g. `codewhale exec …`).
    pub command: Vec<String>,
    /// Working directory for the command (defaults to worktree or cwd).
    pub cwd: Option<PathBuf>,
    /// When set, provision an isolated git worktree + branch under this repo.
    pub worktree: Option<WorktreeProvision>,
}

/// Runtime adapter contract.
pub trait RuntimeBackend {
    fn kind(&self) -> RuntimeBackendKind;

    /// Start the lane process/session; mutates record with attach/log metadata.
    fn start(
        &self,
        registry: &LaneRegistry,
        record: &mut LaneRecord,
        spec: &LaneStartSpec,
    ) -> Result<()>;

    /// Human attach command, if any (tmux).
    fn attach_command(&self, record: &LaneRecord) -> Option<String>;

    /// Stop the running session/process.
    fn stop(&self, registry: &LaneRegistry, record: &mut LaneRecord) -> Result<()>;

    /// Optional worktree TTL cleanup after stop.
    fn cleanup_worktree(&self, record: &LaneRecord) -> Result<()> {
        if let Some(path) = record.worktree_path.as_ref() {
            remove_worktree_if_expired(
                path,
                record.worktree_ttl_secs,
                record.stopped_at.as_deref(),
            )?;
        }
        Ok(())
    }
}

pub fn resolve_backend(kind: RuntimeBackendKind) -> Box<dyn RuntimeBackend> {
    match kind {
        RuntimeBackendKind::Tmux => Box::new(TmuxRuntime),
        RuntimeBackendKind::Inline => Box::new(InlineRuntime),
        RuntimeBackendKind::Vm => Box::new(StubRuntime {
            kind: RuntimeBackendKind::Vm,
        }),
        RuntimeBackendKind::Ci => Box::new(StubRuntime {
            kind: RuntimeBackendKind::Ci,
        }),
    }
}

pub fn backend_for(record: &LaneRecord) -> Box<dyn RuntimeBackend> {
    resolve_backend(record.runtime)
}

fn append_log_event(log_path: &Path, event: serde_json::Value) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("open lane log {}", log_path.display()))?;
    writeln!(file, "{event}").with_context(|| format!("write lane log {}", log_path.display()))?;
    Ok(())
}

fn apply_worktree(record: &mut LaneRecord, spec: &LaneStartSpec) -> Result<Option<PathBuf>> {
    let Some(wt) = spec.worktree.as_ref() else {
        return Ok(spec.cwd.clone());
    };
    let provisioned = provision_worktree(wt)?;
    record.worktree_path = Some(provisioned.path.clone());
    record.branch = Some(provisioned.branch.clone());
    Ok(Some(provisioned.path))
}

/// Durable local tmux sessions + attach + stream-json log file.
#[derive(Debug, Default)]
pub struct TmuxRuntime;

impl RuntimeBackend for TmuxRuntime {
    fn kind(&self) -> RuntimeBackendKind {
        RuntimeBackendKind::Tmux
    }

    fn start(
        &self,
        registry: &LaneRegistry,
        record: &mut LaneRecord,
        spec: &LaneStartSpec,
    ) -> Result<()> {
        if spec.command.is_empty() {
            bail!("tmux runtime requires a non-empty command");
        }
        // tmux may be absent in CI; fall back to a dry-run session record when
        // CODEWHALE_LANE_TMUX_DRY_RUN=1 or tmux is missing (tests).
        let dry_run = std::env::var_os("CODEWHALE_LANE_TMUX_DRY_RUN").is_some()
            || Command::new("tmux")
                .arg("-V")
                .output()
                .map(|o| !o.status.success())
                .unwrap_or(true);

        let cwd = apply_worktree(record, spec)?;
        let session = format!("cw-{}", record.id);
        record.tmux_session = Some(session.clone());
        record.attach_target = Some(format!("tmux attach -t {session}"));

        append_log_event(
            &record.log_path,
            serde_json::json!({
                "type": "lane_started",
                "lane_id": record.id,
                "runtime": "tmux",
                "session": session,
                "workflow": record.workflow,
                "fleet": record.fleet,
                "issue": record.issue,
                "dry_run": dry_run,
            }),
        )?;

        if dry_run {
            append_log_event(
                &record.log_path,
                serde_json::json!({
                    "type": "lane_log",
                    "message": "tmux dry-run: session recorded without spawning process",
                    "command": spec.command,
                    "cwd": cwd.as_ref().map(|p| p.display().to_string()),
                }),
            )?;
            registry.mark_running(record)?;
            return Ok(());
        }

        // Detached session: run command, tee stdout into the lane log.
        let log_path = record.log_path.display().to_string();
        let shell_cmd = format!(
            "({}) 2>&1 | while IFS= read -r line; do printf '%s\\n' \"$line\" >> {}; done",
            shell_join(&spec.command),
            shell_escape(&log_path)
        );

        let mut cmd = Command::new("tmux");
        cmd.args(["new-session", "-d", "-s", &session]);
        if let Some(cwd) = cwd.as_ref() {
            cmd.arg("-c").arg(cwd);
        }
        cmd.arg(shell_cmd);
        let status = cmd
            .status()
            .with_context(|| format!("spawn tmux session {session}"))?;
        if !status.success() {
            record.status = LaneStatus::Failed;
            registry.save(record)?;
            bail!("tmux new-session failed with {status}");
        }

        registry.mark_running(record)?;
        Ok(())
    }

    fn attach_command(&self, record: &LaneRecord) -> Option<String> {
        record.attach_target.clone().or_else(|| {
            record
                .tmux_session
                .as_ref()
                .map(|s| format!("tmux attach -t {s}"))
        })
    }

    fn stop(&self, registry: &LaneRegistry, record: &mut LaneRecord) -> Result<()> {
        if let Some(session) = record.tmux_session.as_deref() {
            let dry_run = std::env::var_os("CODEWHALE_LANE_TMUX_DRY_RUN").is_some();
            if !dry_run {
                let _ = Command::new("tmux")
                    .args(["kill-session", "-t", session])
                    .status();
            }
            append_log_event(
                &record.log_path,
                serde_json::json!({
                    "type": "lane_stopped",
                    "lane_id": record.id,
                    "session": session,
                }),
            )?;
        }
        registry.mark_stopped(record, LaneStatus::Stopped)?;
        self.cleanup_worktree(record)?;
        Ok(())
    }
}

/// In-process / local command runtime (no tmux). Used for tests and headless.
#[derive(Debug, Default)]
pub struct InlineRuntime;

impl RuntimeBackend for InlineRuntime {
    fn kind(&self) -> RuntimeBackendKind {
        RuntimeBackendKind::Inline
    }

    fn start(
        &self,
        registry: &LaneRegistry,
        record: &mut LaneRecord,
        spec: &LaneStartSpec,
    ) -> Result<()> {
        if spec.command.is_empty() {
            bail!("inline runtime requires a non-empty command");
        }
        let cwd = apply_worktree(record, spec)?;
        append_log_event(
            &record.log_path,
            serde_json::json!({
                "type": "lane_started",
                "lane_id": record.id,
                "runtime": "inline",
                "command": spec.command,
            }),
        )?;

        let mut cmd = Command::new(&spec.command[0]);
        if spec.command.len() > 1 {
            cmd.args(&spec.command[1..]);
        }
        if let Some(cwd) = cwd.as_ref() {
            cmd.current_dir(cwd);
        }
        let output = cmd
            .output()
            .with_context(|| format!("run inline command {:?}", spec.command))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        for line in stdout.lines().chain(stderr.lines()) {
            append_log_event(
                &record.log_path,
                serde_json::json!({"type": "lane_log", "message": line}),
            )?;
        }
        if output.status.success() {
            append_log_event(
                &record.log_path,
                serde_json::json!({"type": "lane_completed", "lane_id": record.id}),
            )?;
            registry.mark_stopped(record, LaneStatus::Completed)?;
        } else {
            append_log_event(
                &record.log_path,
                serde_json::json!({
                    "type": "lane_failed",
                    "lane_id": record.id,
                    "status": format!("{}", output.status),
                }),
            )?;
            registry.mark_stopped(record, LaneStatus::Failed)?;
        }
        Ok(())
    }

    fn attach_command(&self, _record: &LaneRecord) -> Option<String> {
        None
    }

    fn stop(&self, registry: &LaneRegistry, record: &mut LaneRecord) -> Result<()> {
        if record.status.is_active() {
            registry.mark_stopped(record, LaneStatus::Stopped)?;
        }
        self.cleanup_worktree(record)?;
        Ok(())
    }
}

/// Placeholder for remote VM / CI backends (surface only in Phase 1).
#[derive(Debug)]
struct StubRuntime {
    kind: RuntimeBackendKind,
}

impl RuntimeBackend for StubRuntime {
    fn kind(&self) -> RuntimeBackendKind {
        self.kind
    }

    fn start(
        &self,
        registry: &LaneRegistry,
        record: &mut LaneRecord,
        _spec: &LaneStartSpec,
    ) -> Result<()> {
        append_log_event(
            &record.log_path,
            serde_json::json!({
                "type": "lane_started",
                "lane_id": record.id,
                "runtime": self.kind.as_str(),
                "message": format!("{} runtime is a Phase 1 stub", self.kind.as_str()),
            }),
        )?;
        registry.mark_running(record)?;
        Ok(())
    }

    fn attach_command(&self, _record: &LaneRecord) -> Option<String> {
        None
    }

    fn stop(&self, registry: &LaneRegistry, record: &mut LaneRecord) -> Result<()> {
        registry.mark_stopped(record, LaneStatus::Stopped)?;
        Ok(())
    }
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_escape(a))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn tmux_dry_run_start_attach_stop_roundtrip() {
        // SAFETY: test-only env toggle for tmux dry-run; single-threaded unit test.
        unsafe {
            std::env::set_var("CODEWHALE_LANE_TMUX_DRY_RUN", "1");
        }
        let dir = tempdir().unwrap();
        let reg = LaneRegistry::open(dir.path()).unwrap();
        let mut record = reg
            .create_pending(
                Some("stopship".into()),
                Some("v0868-stopship".into()),
                Some("4090".into()),
                None,
                RuntimeBackendKind::Tmux,
                None,
            )
            .unwrap();
        let backend = TmuxRuntime;
        backend
            .start(
                &reg,
                &mut record,
                &LaneStartSpec {
                    command: vec!["echo".into(), "hello-lane".into()],
                    cwd: None,
                    worktree: None,
                },
            )
            .unwrap();
        assert_eq!(record.status, LaneStatus::Running);
        assert!(record.tmux_session.is_some());
        let attach = backend.attach_command(&record).expect("attach");
        assert!(attach.contains("tmux attach"));
        let log = std::fs::read_to_string(&record.log_path).unwrap();
        assert!(log.contains("lane_started"));

        backend.stop(&reg, &mut record).unwrap();
        assert_eq!(record.status, LaneStatus::Stopped);
        let reloaded = reg.load(&record.id).unwrap();
        assert_eq!(reloaded.status, LaneStatus::Stopped);
        // SAFETY: paired cleanup of the test-only dry-run flag.
        unsafe {
            std::env::remove_var("CODEWHALE_LANE_TMUX_DRY_RUN");
        }
    }

    #[test]
    fn inline_runtime_writes_stream_json_log() {
        let dir = tempdir().unwrap();
        let reg = LaneRegistry::open(dir.path()).unwrap();
        let mut record = reg
            .create_pending(
                Some("demo".into()),
                None,
                None,
                Some("echo".into()),
                RuntimeBackendKind::Inline,
                None,
            )
            .unwrap();
        InlineRuntime
            .start(
                &reg,
                &mut record,
                &LaneStartSpec {
                    command: vec!["echo".into(), "inline-ok".into()],
                    cwd: None,
                    worktree: None,
                },
            )
            .unwrap();
        assert_eq!(record.status, LaneStatus::Completed);
        let log = std::fs::read_to_string(&record.log_path).unwrap();
        assert!(log.contains("inline-ok"), "log={log}");
        assert!(log.contains("lane_completed"));
    }
}
