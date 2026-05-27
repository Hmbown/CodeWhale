//! Lightweight audit logging for sensitive operations.

use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde_json::{Value, json};

use crate::utils::{flush_and_sync, open_append};

/// Append an audit event to `~/.deepseek/audit.log`.
///
/// This helper is best-effort by design: callers should not fail critical flows
/// if audit persistence fails.
pub fn log_sensitive_event(event: &str, details: Value) {
    if let Err(err) = append_event(event, details) {
        crate::logging::warn(format!("audit log write failed: {err}"));
    }
}

fn append_event(event: &str, details: Value) -> anyhow::Result<()> {
    let path = default_audit_path()?;
    let parent = path.parent().map(|p| p.to_path_buf());
    if let Some(ref parent) = parent {
        fs::create_dir_all(parent)?;
    }
    // Open for append with a BufWriter for buffered I/O, then flush + fsync
    // after each event so the record is durably on disk.
    let mut writer = open_append(&path)?;
    let record = json!({
        "ts": Utc::now().to_rfc3339(),
        "event": event,
        "details": details,
    });
    let line = serde_json::to_string(&record)?;
    use std::io::Write;
    writeln!(writer, "{line}")?;
    flush_and_sync(&mut writer)?;
    Ok(())
}

fn default_audit_path() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory not found"))?;
    Ok(home.join(".codewhale").join("audit.log"))
}
