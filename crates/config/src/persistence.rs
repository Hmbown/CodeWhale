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

/// Hints that mark a config/JSON/env key as carrying a secret value.
///
/// Compound hints (`api_key`, `client_secret`) match as a substring of the
/// normalized key. Single-word hints (`token`, `secret`, `password`) match a
/// whole identifier segment so they describe a credential (`token`,
/// `api_token`) and not an English word (`tokens`, `tokenizer`).
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

/// Return a copy of a JSON value with secret-bearing data removed.
///
/// Object values whose key contains a sensitive hint are replaced wholesale,
/// while all other objects and arrays are traversed recursively. String leaves
/// still pass through [`redact_secrets`] so bare provider tokens and embedded
/// assignments remain covered without treating the serialized JSON document as
/// one flat keyed assignment.
#[must_use]
pub fn redact_json_secrets(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let value = if key_is_sensitive(key) {
                        serde_json::Value::String(REDACTED.to_string())
                    } else {
                        redact_json_secrets(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_json_secrets).collect())
        }
        serde_json::Value::String(text) => serde_json::Value::String(redact_secrets(text)),
        scalar => scalar.clone(),
    }
}

/// Redact secret-bearing values from arbitrary text so it is safe to put in a
/// setup report, log line, error message, or test snapshot.
///
/// Two passes, both dependency-free:
///
/// 1. **Keyed assignments.** Lines or whitespace-delimited inline tokens shaped
///    like `key = value`, `key: value`, or `key=value` whose key
///    (case-insensitively, ignoring quotes) matches a `SENSITIVE_KEY_HINTS`
///    credential identifier have their value replaced with [`REDACTED`]. The
///    spaced form (`key = value`) is matched anywhere on the line, not only
///    when the sensitive key owns the line's first separator — an `anyhow`
///    chain rendered with `{:#}` puts prose and its own `: ` separators in
///    front of the assignment, and that must not be a hole. Because such a
///    value can span several words (`authorization = Bearer <token>`),
///    everything from the value to the end of the line is dropped, exactly as
///    the whole-line form already does. Token *counts* in diagnostics
///    (`max tokens = 8192`) are not credentials and stay visible.
/// 2. **Bare tokens.** Whitespace-delimited words beginning with a known
///    `SECRET_TOKEN_PREFIXES` are replaced wholesale.
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

    // Inline-assignment / bare-token pass: mask any whitespace-delimited word
    // carrying a sensitive keyed value or a known bare secret prefix, plus the
    // spaced `key = value` form that `redact_keyed_assignment` above only sees
    // when the sensitive key owns the line's first separator.
    let mut changed = false;
    let mut spaced = SpacedAssignment::None;
    let mut masked: Vec<String> = Vec::new();
    for word in body.split(' ') {
        let trimmed = trim_word_punctuation(word);
        if spaced == SpacedAssignment::AwaitingValue && !trimmed.is_empty() {
            // The value may run to the end of the line, so drop the remainder
            // rather than masking one word and leaking the rest.
            masked.push(REDACTED.to_string());
            changed = true;
            break;
        }
        if let Some(redacted) = redact_inline_keyed_assignment(trimmed) {
            changed = true;
            masked.push(word.replace(trimmed, &redacted));
            spaced = SpacedAssignment::None;
        } else if !trimmed.is_empty() && looks_like_secret_token(trimmed) {
            changed = true;
            masked.push(word.replace(trimmed, REDACTED));
            spaced = SpacedAssignment::None;
        } else {
            masked.push(word.to_string());
            spaced = spaced.advance(trimmed);
        }
    }

    if changed {
        format!("{}{newline}", masked.join(" "))
    } else {
        format!("{body}{newline}")
    }
}

/// Progress through a `key <space> <sep> <space> value` assignment as the
/// word-level pass walks a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpacedAssignment {
    None,
    /// The previous word was a bare sensitive key awaiting its separator.
    SensitiveKey,
    /// A sensitive key and its separator are both behind us.
    AwaitingValue,
}

impl SpacedAssignment {
    fn advance(self, trimmed: &str) -> Self {
        // Runs of spaces produce empty words; they neither start nor cancel an
        // assignment.
        if trimmed.is_empty() {
            return self;
        }
        if matches!(trimmed, "=" | ":") {
            return if self == Self::SensitiveKey {
                Self::AwaitingValue
            } else {
                Self::None
            };
        }
        // `api_key=` / `api_key:` with the value in the next word. A word whose
        // separator is *not* final was already offered to
        // `redact_inline_keyed_assignment`, so it is not an assignment we own.
        if let Some(key) = trimmed
            .strip_suffix('=')
            .or_else(|| trimmed.strip_suffix(':'))
        {
            return if key_is_sensitive(key) {
                Self::AwaitingValue
            } else {
                Self::None
            };
        }
        if key_is_sensitive(trimmed) {
            return Self::SensitiveKey;
        }
        Self::None
    }
}

fn trim_word_punctuation(word: &str) -> &str {
    word.trim_matches(|c| matches!(c, '"' | '\'' | ',' | ';'))
}

/// Whether `raw`, normalized the way a config/env/JSON key is, matches a
/// [`SENSITIVE_KEY_HINTS`] credential identifier.
fn key_is_sensitive(raw: &str) -> bool {
    let key_norm = raw
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        .to_ascii_lowercase();
    !key_norm.is_empty()
        && SENSITIVE_KEY_HINTS
            .iter()
            .any(|hint| key_matches_sensitive_hint(&key_norm, hint))
}

fn key_matches_sensitive_hint(key_norm: &str, hint: &str) -> bool {
    if key_norm == hint {
        return true;
    }
    // Compound hints already name a credential (`api_key`, `client_secret`).
    // Substring is the right match: `openai_api_key` contains `api_key`.
    if hint.contains('_') || hint.contains('-') {
        return key_norm.contains(hint);
    }
    // Single-word hints must be a whole identifier segment so `token`
    // redacts `token` / `api_token` and not English `tokens`.
    key_norm.split(['_', '-']).any(|segment| segment == hint)
}

fn redact_inline_keyed_assignment(word: &str) -> Option<String> {
    let sep_idx = word.find(['=', ':'])?;
    let (raw_key, rest) = word.split_at(sep_idx);
    let raw_value = &rest[1..];
    if raw_value.is_empty() {
        return None;
    }
    if !key_is_sensitive(raw_key) {
        return None;
    }
    Some(format!("{}{}{}", raw_key, &rest[..1], REDACTED))
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
        .trim_matches(|c| matches!(c, '"' | '\'' | '[' | ']'));
    if !key_is_sensitive(key_norm) {
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

    fn synthetic_secret_fixture() -> String {
        ["abc123", "def456", "ghi"].concat()
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
        let synthetic_secret = synthetic_secret_fixture();
        let input = format!(
            "\
api_key = \"sk-supersecretvalue123\"
provider = \"openai\"
  \"token\": \"{synthetic_secret}\",
model = \"mimo-ultraspeed\"
PASSWORD=hunter2hunter2"
        );
        let out = redact_secrets(&input);
        assert!(!out.contains("sk-supersecretvalue123"), "{out}");
        assert!(!out.contains(&synthetic_secret), "{out}");
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
    fn redact_masks_inline_sensitive_assignments_after_prose_prefixes() {
        let out = redact_secrets(
            "Decision: use token=plain-secret-value and api_key:another-secret-value",
        );
        assert!(!out.contains("plain-secret-value"), "{out}");
        assert!(!out.contains("another-secret-value"), "{out}");
        assert_eq!(out.matches(REDACTED).count(), 2, "{out}");
        assert!(out.starts_with("Decision: use "), "{out}");
    }

    #[test]
    fn redact_masks_spaced_assignment_that_is_not_the_first_separator() {
        // The shape `redact_secrets(&format!("{error:#}"))` produces: an
        // anyhow chain puts prose and its own `: ` separators in front of the
        // assignment, so the sensitive key never owns the line's first
        // separator and the whole-line pass declines the line.
        let out = redact_secrets("request failed: api_key = AIzaSyDeadBeefLeak");
        assert!(!out.contains("AIzaSyDeadBeefLeak"), "{out}");
        assert!(out.contains(REDACTED), "{out}");

        let synthetic_secret = synthetic_secret_fixture();
        let out = redact_secrets(&format!("note: the token = {synthetic_secret}"));
        assert!(!out.contains(&synthetic_secret), "{out}");
        assert!(out.contains(REDACTED), "{out}");
    }

    #[test]
    fn redact_masks_whole_multi_word_value_of_a_spaced_assignment() {
        // Assemble the placeholder at runtime so secret scanners do not
        // mistake a redaction fixture for a committed credential.
        let bearer = ["Bear", "er"].concat();
        let credential = ["abc123", "def456", "ghi"].concat();
        let out = redact_secrets(&format!(
            "mcp call failed: authorization = {bearer} {credential}"
        ));
        assert!(!out.contains(&credential), "{out}");
        assert!(!out.contains(&bearer), "{out}");
        assert!(
            out.starts_with("mcp call failed: authorization = "),
            "{out}"
        );
    }

    #[test]
    fn redact_spaced_pass_leaves_ordinary_prose_alone() {
        // No sensitive key, so the spaced-assignment state machine must not
        // start swallowing the rest of the line.
        let input = "the quick brown fox = jumps over the lazy dog";
        assert_eq!(redact_secrets(input), input);
        let input = "note: the model = deepseek-v4-pro and the seed = 7";
        assert_eq!(redact_secrets(input), input);
    }

    #[test]
    fn redact_leaves_token_count_diagnostics_intact() {
        // "tokens" is the English plural of a usage metric, not a credential
        // key. The spaced-assignment pass used to treat the "token" hint as a
        // substring and then drop the rest of the line, which made the exact
        // class of error people paste into issues unreadable.
        for input in [
            "stream error: max tokens = 8192 but budget = 4096",
            "error: token expired",
            "request failed: token count: 4096 exceeds the model limit",
            "http 401: authorization header rejected",
            "warning: password policy requires 12 characters",
            "note: secret scanning found 3 issues",
        ] {
            assert_eq!(redact_secrets(input), input, "{input}");
        }
    }

    #[test]
    fn redact_still_masks_a_bearer_token_assignment() {
        // Counterpart of the diagnostic test above: a real credential keyed
        // as `token` (or `api_token`) must still be dropped, including a
        // multi-word Bearer value that is not a known bare-token prefix.
        // The JWT is assembled at runtime so no scanner-shaped literal sits
        // in the source tree — same precedent as the AWS fixture in
        // `crates/workflow/src/redaction.rs`.
        let jwt = ["eyJhbGciOiJIUzI1NiJ9", "e30", "c2lnbmF0dXJl"].join(".");
        let out = redact_secrets(&format!("stream error: token = Bearer {jwt}"));
        assert!(!out.contains(&jwt), "{out}");
        assert!(!out.contains("Bearer"), "{out}");
        assert!(out.starts_with("stream error: token = "), "{out}");
        assert!(out.contains(REDACTED), "{out}");

        let synthetic_secret = synthetic_secret_fixture();
        let out = redact_secrets(&format!("note: api_token = {synthetic_secret}"));
        assert!(!out.contains(&synthetic_secret), "{out}");
        assert!(out.contains(REDACTED), "{out}");
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
