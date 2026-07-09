//! Transactional persistence, atomic writes, and secret redaction for the
//! v0.8.67 constitution-first setup lane (#3410).
//!
//! This is the safety layer under every setup step. A setup session may touch
//! several files (the setup-state sidecar, the user-global constitution, and —
//! through the existing comment-preserving `ConfigStore` — `config.toml`). The
//! contract this module guarantees:
//!
//! - **Preview writes nothing.** [`SetupTransaction::preview`] reports what
//!   would change without touching the filesystem.
//! - **Cancel leaves files unchanged.** A staged transaction that is dropped
//!   without [`SetupTransaction::commit`] never wrote anything.
//! - **Save is atomic.** Each file is written through a temp file + rename
//!   ([`atomic_write`]); a multi-file commit either fully applies or fully
//!   rolls back, so a partial failure never leaves a half-written file.
//! - **Secrets never leak.** [`redact_secrets`] masks secret-bearing values for
//!   any report, log line, or diagnostic that might echo config text.
//!
//! This module deliberately owns only the write / rollback / secret contract.
//! Each setup step owns *which* fields it writes; see [`crate::setup_state`] and
//! [`crate::user_constitution`].

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Restrictive file mode for setup-owned files (owner read/write only).
#[cfg(unix)]
const SETUP_FILE_MODE: u32 = 0o600;

/// Atomically write `bytes` to `path` via a sibling temp file + rename.
///
/// The temp file is created in the same directory as `path` so the final
/// `rename` is atomic on the same filesystem. On Unix the file is created with
/// `0o600` so setup-owned state never lands world-readable. Parent directories
/// are created as needed.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let dir = parent.unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("failed to create temp file in {}", dir.display()))?;

    use std::io::Write as _;
    tmp.write_all(bytes)
        .with_context(|| format!("failed to write temp file for {}", path.display()))?;
    tmp.flush()
        .with_context(|| format!("failed to flush temp file for {}", path.display()))?;

    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(SETUP_FILE_MODE);
        tmp.as_file()
            .set_permissions(perms)
            .with_context(|| format!("failed to set permissions for {}", path.display()))?;
    }

    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("failed to persist {}", path.display()))?;
    Ok(())
}

/// Atomically write `value` as pretty-printed JSON to `path`.
///
/// A trailing newline is appended so the file is well-formed for line-oriented
/// tooling and diffs.
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut body = serde_json::to_string_pretty(value)
        .with_context(|| format!("failed to serialize JSON for {}", path.display()))?;
    body.push('\n');
    atomic_write(path, body.as_bytes())
}

/// A staged multi-file write that either fully applies or fully rolls back.
///
/// Stage every file the setup step intends to write, then call [`commit`]. If
/// any single write fails, every already-applied write in the transaction is
/// restored to its pre-commit contents (or removed if it did not previously
/// exist), and the original error is returned. A transaction that is dropped
/// without committing leaves the filesystem untouched.
///
/// [`commit`]: SetupTransaction::commit
#[derive(Debug, Default)]
pub struct SetupTransaction {
    writes: Vec<StagedWrite>,
}

#[derive(Debug, Clone)]
struct StagedWrite {
    path: PathBuf,
    bytes: Vec<u8>,
}

/// A snapshot of a file's pre-commit state, captured so [`SetupTransaction`]
/// can restore it during rollback.
struct Snapshot {
    path: PathBuf,
    /// Original bytes, or `None` if the file did not exist before commit.
    original: Option<Vec<u8>>,
}

impl SetupTransaction {
    /// Create an empty transaction.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage `bytes` to be written to `path` on [`commit`](Self::commit).
    ///
    /// Staging touches nothing on disk. A later stage for the same path
    /// replaces an earlier one, so a step can revise its intended output before
    /// committing.
    pub fn stage(&mut self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) -> &mut Self {
        let path = path.into();
        let bytes = bytes.into();
        if let Some(existing) = self.writes.iter_mut().find(|w| w.path == path) {
            existing.bytes = bytes;
        } else {
            self.writes.push(StagedWrite { path, bytes });
        }
        self
    }

    /// Stage `value` serialized as pretty JSON (with trailing newline).
    pub fn stage_json<T: Serialize>(
        &mut self,
        path: impl Into<PathBuf>,
        value: &T,
    ) -> Result<&mut Self> {
        let path = path.into();
        let mut body = serde_json::to_string_pretty(value)
            .with_context(|| format!("failed to serialize JSON for {}", path.display()))?;
        body.push('\n');
        Ok(self.stage(path, body.into_bytes()))
    }

    /// The paths that [`commit`](Self::commit) would write, in staging order.
    /// Writes nothing — this is the preview surface.
    #[must_use]
    pub fn preview(&self) -> Vec<&Path> {
        self.writes.iter().map(|w| w.path.as_path()).collect()
    }

    /// True when nothing is staged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    /// Apply every staged write atomically.
    ///
    /// On success all files are updated. On the first failure, every write that
    /// already landed is rolled back to its captured pre-commit state and the
    /// original error is returned (rollback failures are attached as context).
    pub fn commit(self) -> Result<()> {
        let mut snapshots: Vec<Snapshot> = Vec::with_capacity(self.writes.len());

        for write in &self.writes {
            // Capture the pre-commit state before mutating, so we can restore it.
            let original = match fs::read(&write.path) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    rollback(&snapshots);
                    return Err(e).with_context(|| {
                        format!(
                            "failed to read existing {} before write; rolled back {} prior change(s)",
                            write.path.display(),
                            snapshots.len()
                        )
                    });
                }
            };

            match atomic_write(&write.path, &write.bytes) {
                Ok(()) => snapshots.push(Snapshot {
                    path: write.path.clone(),
                    original,
                }),
                Err(err) => {
                    // This write did not land (atomic_write is all-or-nothing),
                    // so roll back only the writes that came before it.
                    rollback(&snapshots);
                    return Err(err).with_context(|| {
                        format!(
                            "setup transaction failed writing {}; rolled back {} prior change(s)",
                            write.path.display(),
                            snapshots.len()
                        )
                    });
                }
            }
        }

        Ok(())
    }
}

/// Restore every snapshot to its captured pre-commit state. Best-effort: a
/// rollback error is logged but does not abort the remaining restores, because
/// leaving as many files as possible in their original state is the goal.
fn rollback(snapshots: &[Snapshot]) {
    for snap in snapshots.iter().rev() {
        let result = match &snap.original {
            Some(bytes) => atomic_write(&snap.path, bytes),
            None => match fs::remove_file(&snap.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        };
        if let Err(e) = result {
            tracing::error!(
                target: "config::persistence",
                "failed to roll back {} during setup transaction: {e:#}",
                snap.path.display()
            );
        }
    }
}

/// Substrings that mark a config/JSON/env key as carrying a secret value.
const SENSITIVE_KEY_HINTS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "token",
    "password",
    "passwd",
    "authorization",
    "auth_token",
    "access_key",
    "client_secret",
    "private_key",
];

/// Known opaque-token prefixes worth masking even when they appear bare (not as
/// `key = value`). Conservative on purpose: only well-known provider/key shapes.
const SECRET_TOKEN_PREFIXES: &[&str] = &["sk-", "sk_", "ghp_", "gho_", "xoxb-", "xoxp-", "pk-"];

/// The placeholder substituted for any redacted secret value.
pub const REDACTED: &str = "[redacted]";

/// Redact secret-bearing values from arbitrary text so it is safe to put in a
/// setup report, log line, error message, or test snapshot.
///
/// Two passes, both dependency-free:
///
/// 1. **Keyed assignments.** Lines shaped like `key = value`, `key: value`, or
///    `key=value` whose key (case-insensitively, ignoring quotes) contains a
///    [`SENSITIVE_KEY_HINTS`] substring have their value replaced with
///    [`REDACTED`].
/// 2. **Bare tokens.** Whitespace-delimited words beginning with a known
///    [`SECRET_TOKEN_PREFIXES`] are replaced wholesale.
///
/// The goal is defense in depth: setup state and reports are built from safe
/// summaries that never include secrets in the first place, and this is the
/// backstop for anything that echoes raw config text.
#[must_use]
pub fn redact_secrets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut first = true;
    for line in input.split_inclusive('\n') {
        if !first {
            // split_inclusive keeps the newline on the previous chunk, so we do
            // not need to re-add separators here.
        }
        first = false;
        out.push_str(&redact_line(line));
    }
    out
}

/// Redact a single line (which may include a trailing newline).
fn redact_line(line: &str) -> String {
    // Preserve any trailing newline so callers keep their line structure.
    let (body, newline) = match line.strip_suffix('\n') {
        Some(rest) => (rest, "\n"),
        None => (line, ""),
    };

    if let Some(redacted) = redact_keyed_assignment(body) {
        return format!("{redacted}{newline}");
    }

    // Bare-token pass: mask any whitespace-delimited word with a known prefix.
    let mut changed = false;
    let masked: Vec<String> = body
        .split(' ')
        .map(|word| {
            let trimmed = word.trim_matches(|c| matches!(c, '"' | '\'' | ',' | ';'));
            if !trimmed.is_empty() && looks_like_secret_token(trimmed) {
                changed = true;
                word.replace(trimmed, REDACTED)
            } else {
                word.to_string()
            }
        })
        .collect();

    if changed {
        format!("{}{newline}", masked.join(" "))
    } else {
        format!("{body}{newline}")
    }
}

/// If `body` is a `key <sep> value` assignment with a sensitive key, return the
/// line with the value redacted; otherwise `None`.
fn redact_keyed_assignment(body: &str) -> Option<String> {
    // Find the first `=` or `:` that separates a key from a value.
    let sep_idx = body.find(['=', ':'])?;
    let (raw_key, rest) = body.split_at(sep_idx);
    let sep = &rest[..1];
    let raw_value = &rest[1..];

    let key_norm = raw_key
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '[' | ']'))
        .to_ascii_lowercase();
    if key_norm.is_empty() || !SENSITIVE_KEY_HINTS.iter().any(|h| key_norm.contains(h)) {
        return None;
    }

    // Keep leading whitespace of the key and the original separator spacing so
    // the redacted line reads naturally.
    let key_lead_ws: String = raw_key.chars().take_while(|c| c.is_whitespace()).collect();
    let value_lead_ws: String = raw_value
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let value_rest = raw_value.trim_start();
    // If the value is empty, there is nothing to hide.
    if value_rest.is_empty() {
        return None;
    }
    // Preserve surrounding quotes so structured files stay parseable-looking.
    let quoted = value_rest.starts_with('"') || value_rest.starts_with('\'');
    let replacement = if quoted {
        format!("\"{REDACTED}\"")
    } else {
        REDACTED.to_string()
    };
    Some(format!(
        "{key_lead_ws}{}{sep}{value_lead_ws}{replacement}",
        raw_key.trim()
    ))
}

fn looks_like_secret_token(word: &str) -> bool {
    SECRET_TOKEN_PREFIXES
        .iter()
        .any(|p| word.len() > p.len() + 6 && word.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn atomic_write_creates_parent_dirs_and_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/state.json");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(read(&path), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_uses_owner_only_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        atomic_write(&path, b"x").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, SETUP_FILE_MODE);
    }

    #[test]
    fn atomic_write_replaces_existing_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(read(&path), "new");
        // No stray temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != "state.json")
            .collect();
        assert!(leftovers.is_empty(), "stray temp files: {leftovers:?}");
    }

    #[test]
    fn transaction_preview_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.json");
        let b = tmp.path().join("b.json");
        let mut tx = SetupTransaction::new();
        tx.stage(a.clone(), b"1".to_vec())
            .stage(b.clone(), b"2".to_vec());
        let preview = tx.preview();
        assert_eq!(preview, vec![a.as_path(), b.as_path()]);
        assert!(!a.exists());
        assert!(!b.exists());
    }

    #[test]
    fn dropped_transaction_leaves_files_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.json");
        {
            let mut tx = SetupTransaction::new();
            tx.stage(a.clone(), b"staged".to_vec());
            // tx dropped here without commit
        }
        assert!(!a.exists());
    }

    #[test]
    fn transaction_commit_applies_all() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.json");
        let b = tmp.path().join("sub/b.json");
        let mut tx = SetupTransaction::new();
        tx.stage(a.clone(), b"A".to_vec())
            .stage(b.clone(), b"B".to_vec());
        tx.commit().unwrap();
        assert_eq!(read(&a), "A");
        assert_eq!(read(&b), "B");
    }

    #[test]
    fn transaction_rolls_back_on_partial_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("good.json");
        fs::write(&good, "ORIGINAL").unwrap();

        // Second target is unwritable: a path whose parent is an existing file.
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "i am a file").unwrap();
        let bad = blocker.join("child.json"); // parent is a file → create_dir_all fails

        let mut tx = SetupTransaction::new();
        tx.stage(good.clone(), b"UPDATED".to_vec())
            .stage(bad.clone(), b"NOPE".to_vec());
        let err = tx.commit().unwrap_err();
        assert!(format!("{err:#}").contains("rolled back"));

        // The first file must be restored to its original contents.
        assert_eq!(read(&good), "ORIGINAL");
        assert!(!bad.exists());
    }

    #[test]
    fn transaction_rollback_removes_newly_created_file() {
        let tmp = tempfile::tempdir().unwrap();
        let fresh = tmp.path().join("fresh.json"); // did not exist before
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "file").unwrap();
        let bad = blocker.join("child.json");

        let mut tx = SetupTransaction::new();
        tx.stage(fresh.clone(), b"created".to_vec())
            .stage(bad, b"x".to_vec());
        assert!(tx.commit().is_err());
        // The newly created file must be removed on rollback, not left behind.
        assert!(!fresh.exists());
    }

    #[test]
    fn redact_masks_keyed_secrets_toml_and_json() {
        let input = "\
api_key = \"sk-supersecretvalue123\"
provider = \"openai\"
  \"token\": \"abc123def456ghi\",
model = \"mimo-ultraspeed\"
PASSWORD=hunter2hunter2";
        let out = redact_secrets(input);
        assert!(!out.contains("sk-supersecretvalue123"), "{out}");
        assert!(!out.contains("abc123def456ghi"), "{out}");
        assert!(!out.contains("hunter2hunter2"), "{out}");
        // Non-secret values survive untouched.
        assert!(out.contains("provider = \"openai\""));
        assert!(out.contains("model = \"mimo-ultraspeed\""));
        assert!(out.matches(REDACTED).count() >= 3, "{out}");
    }

    #[test]
    fn redact_masks_bare_token_prefixes() {
        let out = redact_secrets("the leaked key sk-abcdef1234567890 appeared in a log");
        assert!(!out.contains("sk-abcdef1234567890"), "{out}");
        assert!(out.contains(REDACTED));
        assert!(out.contains("appeared in a log"));
    }

    #[test]
    fn redact_preserves_line_structure() {
        let input = "line1\nsecret = \"xyzsecretvalue\"\nline3";
        let out = redact_secrets(input);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line1");
        assert_eq!(lines[2], "line3");
        assert!(lines[1].contains(REDACTED));
    }

    #[test]
    fn redact_leaves_plain_text_untouched() {
        let input = "the quick brown fox = jumps over";
        // `fox` key has no sensitive hint → unchanged.
        assert_eq!(redact_secrets(input), input);
    }
}
