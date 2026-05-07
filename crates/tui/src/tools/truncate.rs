//! Tool-output spillover writer (#422).
//!
//! When a tool produces output that's too large to land in the model's
//! context budget, we want two things at once:
//!
//! 1. The transcript / tool-cell renders a bounded preview so the UI
//!    stays scannable.
//! 2. The full original output is preserved on disk so the model can
//!    `read_file` it back if it later needs the elided tail, and so
//!    the user can open it in `$EDITOR`.
//!
//! This module owns the disk side. Files land in
//! `~/.deepseek/tool_outputs/<sanitised-id>.txt`. The id is the tool
//! call id the engine assigns; we sanitise it conservatively (ASCII
//! alphanumeric + `-`/`_`) so a hostile id can't escape the directory
//! via `..` or absolute-path tricks.
//!
//! Boot prune drops files whose mtime is older than [`SPILLOVER_MAX_AGE`]
//! (7 days). Prune failures are logged and never fatal — the user
//! shouldn't see startup wedge because of a stale tool-output file.
//!
//! ## Live callers
//!
//! * [`apply_spillover`] — invoked from the engine's tool-execution
//!   path (`turn_loop.rs`) so any successful tool result over
//!   [`SPILLOVER_THRESHOLD_BYTES`] spills to disk and the model
//!   receives a [`SPILLOVER_HEAD_BYTES`] head plus a pointer footer.
//! * Boot prune in `main.rs` deletes files older than
//!   [`SPILLOVER_MAX_AGE`].
//!
//! UI-side rendering of the inline `full output: <path>` annotation
//! is owned by `tui/history.rs::render_spillover_annotation`. The
//! tool-details pager opens the spillover file when the user
//! presses `Alt+V` (or plain `v` with empty composer) on a spilled
//! tool cell.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::tools::spec::ToolResult;

// `Path` is only referenced from helpers gated to test builds.
#[cfg(test)]
use std::path::Path;

/// Name of the spillover directory under `~/.deepseek/`.
pub const SPILLOVER_DIR_NAME: &str = "tool_outputs";

/// Default threshold above which a tool result is a candidate for
/// spillover. Mirrors the `MAX_MEMORY_SIZE` ceiling we use elsewhere
/// for "too large to inline" so the rules feel consistent. Wired
/// callers can pass a different value if a tool family has different
/// economics.
pub const SPILLOVER_THRESHOLD_BYTES: usize = 100 * 1024; // 100 KiB

/// Default boot-prune age. Older spillover files are deleted on
/// startup to keep `~/.deepseek/tool_outputs/` from growing without
/// bound. Mirrors the workspace-snapshot 7-day default.
pub const SPILLOVER_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[cfg(test)]
static TEST_SPILLOVER_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) static TEST_SPILLOVER_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolve `~/.deepseek/tool_outputs/`. Returns `None` if the home
/// directory can't be determined (CI containers occasionally hit
/// this). Callers should treat `None` as "spillover unavailable" and
/// degrade gracefully rather than fail the tool call.
#[must_use]
pub fn spillover_root() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(root) = TEST_SPILLOVER_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
    {
        return Some(root);
    }

    Some(dirs::home_dir()?.join(".deepseek").join(SPILLOVER_DIR_NAME))
}

/// Override the spillover root for tests without mutating `$HOME`.
#[cfg(test)]
pub(crate) fn set_test_spillover_root(root: Option<PathBuf>) -> Option<PathBuf> {
    let mut guard = TEST_SPILLOVER_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    std::mem::replace(&mut *guard, root)
}

/// Resolve the spillover-file path for a tool call id. Sanitises the
/// id so that a hostile value can't escape the storage directory.
/// Returns `None` for empty / fully-invalid ids; the caller should
/// treat that as "spillover unavailable" and skip the write.
#[must_use]
pub fn spillover_path(id: &str) -> Option<PathBuf> {
    let sanitised = sanitise_id(id)?;
    Some(spillover_root()?.join(format!("{sanitised}.txt")))
}

/// Write `content` to the spillover file for `id`. Creates the
/// parent directory if needed. Returns the resolved path on success.
///
/// Atomic via `write` + filesystem rename guarantees from the
/// underlying OS — the file is created at a temp name first and
/// then renamed into place. Failures bubble up as `io::Error` so the
/// caller can decide whether to surface them.
pub fn write_spillover(id: &str, content: &str) -> io::Result<PathBuf> {
    let path = spillover_path(id).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "could not resolve spillover path (empty/invalid id or missing home directory)",
        )
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::utils::write_atomic(&path, content.as_bytes())?;
    Ok(path)
}

/// Drop spillover files older than `max_age`. Returns the number of
/// files removed. Non-fatal: directory-missing returns 0; per-file
/// errors are logged and skipped. Mirrors
/// [`crate::session_manager::prune_workspace_snapshots`].
pub fn prune_older_than(max_age: Duration) -> io::Result<usize> {
    let Some(root) = spillover_root() else {
        return Ok(0);
    };
    if !root.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut pruned = 0usize;
    for entry in fs::read_dir(&root)? {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(target: "spillover", ?err, "skipping unreadable dir entry");
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let modified = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(target: "spillover", ?err, ?path, "skipping unreadable mtime");
                continue;
            }
        };
        if modified < cutoff {
            if let Err(err) = fs::remove_file(&path) {
                tracing::warn!(target: "spillover", ?err, ?path, "spillover prune skipped a file");
                continue;
            }
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// Convenience for the common "too long? spill it." pattern. If
/// `content` is at or below `threshold` bytes, returns `None` and the
/// caller keeps the inline content. Above the threshold, writes the
/// full content to the spillover file and returns
/// `Some((head, path))` where `head` is the leading slice the caller
/// can show inline. The trailing tail isn't returned — `path` is the
/// canonical reference.
///
/// `head_bytes` controls how much inline content the caller wants to
/// keep. Pass `threshold` for "preserve as much as fits inline" or
/// a smaller value (e.g. `4 * 1024`) for "show a peek".
pub fn maybe_spillover(
    id: &str,
    content: &str,
    threshold: usize,
    head_bytes: usize,
) -> io::Result<Option<(String, PathBuf)>> {
    if content.len() <= threshold {
        return Ok(None);
    }
    let path = write_spillover(id, content)?;
    // Don't slice mid-utf8: walk back to a char boundary if needed.
    let cut = head_bytes.min(content.len());
    let cut = (0..=cut)
        .rev()
        .find(|&i| content.is_char_boundary(i))
        .unwrap_or(0);
    Ok(Some((content[..cut].to_string(), path)))
}

/// Inline head retained when [`apply_spillover`] truncates a tool
/// result. 32 KiB is large enough for the model to keep meaningful
/// context (a long stack trace, a `git diff` head, a directory
/// listing of typical depth) without consuming the lion's share of
/// the per-turn context budget. The full output is preserved on
/// disk; the model can `read_file` it back if it needs the tail.
pub const SPILLOVER_HEAD_BYTES: usize = 32 * 1024;

/// Apply spillover to a tool result in place. If the result's
/// content exceeds [`SPILLOVER_THRESHOLD_BYTES`], writes the full
/// content to a sibling file under `~/.deepseek/tool_outputs/`,
/// replaces `result.content` with a [`SPILLOVER_HEAD_BYTES`] head
/// plus a footer pointing the model at the spillover file, and
/// stamps `metadata.spillover_path` so the UI can render its
/// "full output: …" annotation.
///
/// Returns the spillover path on success, `None` if no spillover
/// happened (content small enough, error result, write failure).
/// Failures are logged but never bubble up — a tool that produced a
/// result shouldn't be marked failed because the spillover writer
/// couldn't reach disk; we degrade to no-op and the model gets the
/// original (large) content.
///
/// Error results (`success == false`) are skipped: error messages
/// are typically short, and turning them into a "see file" pointer
/// would just hide the error from the model's reasoning.
pub fn apply_spillover(result: &mut ToolResult, tool_id: &str) -> Option<PathBuf> {
    if !result.success {
        return None;
    }
    if result.content.len() <= SPILLOVER_THRESHOLD_BYTES {
        return None;
    }
    let total = result.content.len();
    let outcome = match maybe_spillover(
        tool_id,
        &result.content,
        SPILLOVER_THRESHOLD_BYTES,
        SPILLOVER_HEAD_BYTES,
    ) {
        Ok(Some(pair)) => pair,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(
                target: "spillover",
                ?err,
                tool_id,
                "spillover write failed; passing original content through"
            );
            return None;
        }
    };
    let (head, path) = outcome;
    let path_str = path.display().to_string();
    let footer = format!(
        "\n\n[Output truncated: {head_kib} KiB of {total_kib} KiB shown. \
         Full output saved to {path_str}. Use \
         `retrieve_tool_result ref={tool_id} mode=tail` or \
         `retrieve_tool_result ref={tool_id} mode=query query=<text>` \
         if you need the elided output.]",
        head_kib = head.len() / 1024,
        total_kib = total / 1024,
    );
    result.content = format!("{head}{footer}");
    let metadata = result.metadata.get_or_insert_with(|| serde_json::json!({}));
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert("spillover_path".into(), serde_json::Value::String(path_str));
    } else {
        // Pre-existing metadata that wasn't a JSON object (rare,
        // possibly an array). Replace with an object so we can
        // attach our key without losing prior data — wrap it under
        // a `_prior` field so callers that introspect can recover.
        let prior = std::mem::replace(metadata, serde_json::json!({}));
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert("_prior".into(), prior);
            obj.insert(
                "spillover_path".into(),
                serde_json::Value::String(path.display().to_string()),
            );
        }
    }
    Some(path)
}

/// Sanitise a tool call id for use as a filename. Keeps ASCII
/// alphanumerics, `-`, and `_`; rejects `.` to keep `..` traversal
/// out, rejects empty results. Returns `None` if the input contains
/// no acceptable characters.
fn sanitise_id(id: &str) -> Option<String> {
    let cleaned: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Override the spillover root for tests so they don't pollute the
/// user's real `~/.deepseek/` directory. Wraps the body with a
/// temporary `HOME` override that gets restored on drop.
#[cfg(test)]
fn with_test_home<F, R>(home: &Path, f: F) -> R
where
    F: FnOnce() -> R,
{
    // SAFETY: tests in this module serialize through `TEST_GUARD`
    // because they share process-wide `$HOME`. Without the guard,
    // parallel tests could observe each other's overrides.
    let prior = std::env::var_os("HOME");
    // SAFETY: caller holds the test guard.
    unsafe {
        std::env::set_var("HOME", home);
    }
    let out = f();
    // SAFETY: caller holds the test guard.
    unsafe {
        if let Some(p) = prior {
            std::env::set_var("HOME", p);
        } else {
            std::env::remove_var("HOME");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Tests in this module serialize through this guard because
    /// they mutate process-global `$HOME`. Without it, cargo's
    /// parallel runner would observe interleaved overrides.
    fn setup() -> std::sync::MutexGuard<'static, ()> {
        super::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn sanitise_id_keeps_safe_chars_and_drops_dangerous() {
        assert_eq!(super::sanitise_id("abc-123_x"), Some("abc-123_x".into()));
        // `.` is dropped to keep `..` out of the path.
        assert_eq!(super::sanitise_id("../etc"), Some("etc".into()));
        assert_eq!(super::sanitise_id("/etc/passwd"), Some("etcpasswd".into()));
        // Empty-after-sanitise → None.
        assert!(super::sanitise_id("...").is_none());
        assert!(super::sanitise_id("").is_none());
    }

    #[test]
    fn write_spillover_creates_directory_and_writes_file() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let path = write_spillover("call-abc", "hello world").expect("write");
            assert!(path.exists(), "{path:?} missing");
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body, "hello world");
            // Directory landed under `<HOME>/.deepseek/tool_outputs/`.
            // Compare components instead of a substring on `to_string_lossy`
            // — Windows uses `\` as the separator so a `/` substring match
            // would falsely fail there.
            let components: Vec<&str> = path
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            assert!(
                components.contains(&".deepseek") && components.contains(&"tool_outputs"),
                "spillover path missing expected `.deepseek/tool_outputs/...` segments: {path:?}"
            );
        });
    }

    #[test]
    fn write_spillover_rejects_empty_id() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let err = write_spillover("...", "x").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        });
    }

    #[test]
    fn maybe_spillover_returns_none_below_threshold() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let out = maybe_spillover("call-1", "tiny content", 100 * 1024, 4 * 1024).expect("ok");
            assert!(out.is_none());
        });
    }

    #[test]
    fn maybe_spillover_writes_and_returns_head_above_threshold() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // Content larger than the threshold.
            let big = "A".repeat(2_000);
            let (head, path) = maybe_spillover("call-2", &big, 1_000, 256)
                .expect("ok")
                .expect("should have spilled");
            // Head is bounded.
            assert_eq!(head.len(), 256);
            // Full content on disk.
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body.len(), 2_000);
        });
    }

    #[test]
    fn maybe_spillover_does_not_split_inside_a_codepoint() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // 4 byte chars; ask for 3 bytes of head → walks back to
            // the previous char boundary (0).
            let s = "🐳🐳🐳🐳"; // 4 × 4-byte codepoints
            assert_eq!(s.len(), 16);
            let (head, _) = maybe_spillover("call-3", s, 1, 3)
                .expect("ok")
                .expect("spilled");
            // 3 isn't a char boundary in this string; walk back → 0.
            assert_eq!(head, "");
            // Asking for 4 bytes lands on the first char boundary.
            let (head, _) = maybe_spillover("call-3b", s, 1, 4)
                .expect("ok")
                .expect("spilled");
            assert_eq!(head, "🐳");
        });
    }

    #[test]
    fn prune_older_than_handles_missing_root() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // Nothing has ever written; root doesn't exist; that's fine.
            let count = prune_older_than(SPILLOVER_MAX_AGE).expect("ok");
            assert_eq!(count, 0);
        });
    }

    // The mtime backdate uses utimensat (Unix-only). On Windows the
    // filetime_set_modified helper is a no-op, so the prune wouldn't see
    // any stale files. Gate the whole test on `cfg(unix)` instead of
    // testing a no-op path that can't fail meaningfully.
    #[test]
    #[cfg(unix)]
    fn prune_older_than_keeps_fresh_files_drops_stale_ones() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let fresh = write_spillover("fresh", "x").unwrap();
            let stale = write_spillover("stale", "y").unwrap();

            // Backdate `stale` to 30 days ago.
            let thirty_days = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
            filetime_set_modified(&stale, thirty_days);

            let pruned = prune_older_than(SPILLOVER_MAX_AGE).unwrap();
            assert_eq!(pruned, 1);
            assert!(fresh.exists());
            assert!(!stale.exists());
        });
    }

    /// Set the mtime on a file. The workspace doesn't pull the
    /// `filetime` crate, so we reach for `utimensat` directly on
    /// Unix. Windows is a no-op — the prune semantics are the same
    /// and the per-cycle stress test lives on the Unix path.
    #[cfg(unix)]
    fn filetime_set_modified(path: &Path, when: SystemTime) {
        let secs = when
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as libc::time_t;
        let times = [
            libc::timespec {
                tv_sec: secs,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: secs,
                tv_nsec: 0,
            },
        ];
        let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: path_c is a valid CString; times is a 2-element array
        // matching utimensat's signature.
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, path_c.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(
            rc,
            0,
            "utimensat failed: {}",
            std::io::Error::last_os_error()
        );
    }

    // Windows stub removed in v0.8.8 — the only caller of
    // `filetime_set_modified` is `prune_older_than_keeps_fresh_files_drops_stale_ones`,
    // which is now `#[cfg(unix)]` because mtime backdating requires
    // `utimensat` and a Windows no-op stub can't make the assertion pass
    // anyway. Keeping the stub triggered `-D dead-code` on Windows builds
    // (the prune test was the only caller) and broke `Test (windows-latest)`.

    #[test]
    fn apply_spillover_is_noop_below_threshold() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let mut result = ToolResult::success("small payload");
            let path = apply_spillover(&mut result, "call-small");
            assert!(path.is_none());
            assert_eq!(result.content, "small payload");
            assert!(result.metadata.is_none());
        });
    }

    #[test]
    fn apply_spillover_is_noop_for_error_results() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // Even very large error messages are passed through —
            // truncating an error would hide it from the model.
            let big_err = "boom\n".repeat(50_000);
            let mut result = ToolResult::error(big_err.clone());
            let path = apply_spillover(&mut result, "call-err");
            assert!(path.is_none());
            assert_eq!(result.content, big_err);
        });
    }

    #[test]
    fn apply_spillover_truncates_and_stamps_metadata_above_threshold() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // 200 KiB body — well above the 100 KiB threshold.
            let big = "X".repeat(200 * 1024);
            let mut result = ToolResult::success(big.clone());
            let path = apply_spillover(&mut result, "call-big").expect("should spill");

            // Inline content shrunk to head + footer.
            assert!(result.content.len() < big.len());
            assert!(
                result.content.contains("Output truncated:"),
                "footer missing: {}",
                &result.content[result.content.len().saturating_sub(200)..]
            );
            assert!(result.content.contains("retrieve_tool_result ref=call-big"));

            // Full bytes are on disk at the returned path.
            assert!(path.exists(), "spillover file missing: {path:?}");
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body.len(), 200 * 1024);

            // metadata.spillover_path stamped for the UI to find.
            let metadata = result.metadata.expect("metadata stamped");
            let stamped = metadata
                .get("spillover_path")
                .and_then(serde_json::Value::as_str)
                .expect("spillover_path key present");
            assert_eq!(stamped, path.display().to_string());
        });
    }

    #[test]
    fn apply_spillover_preserves_existing_metadata() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let big = "Y".repeat(200 * 1024);
            let mut result = ToolResult::success(big)
                .with_metadata(serde_json::json!({"prior_key": "prior_value"}));
            let path = apply_spillover(&mut result, "call-meta").expect("should spill");

            let metadata = result.metadata.expect("metadata present");
            // Prior keys survive.
            assert_eq!(
                metadata
                    .get("prior_key")
                    .and_then(serde_json::Value::as_str),
                Some("prior_value")
            );
            // New key added alongside.
            assert_eq!(
                metadata
                    .get("spillover_path")
                    .and_then(serde_json::Value::as_str),
                Some(path.display().to_string().as_str())
            );
        });
    }

    #[test]
    fn apply_spillover_wraps_non_object_metadata_under_prior_key() {
        // Defends against a tool whose `metadata` is something
        // other than a JSON object (rare — most use the `json!({})`
        // pattern — but legal per `serde_json::Value`). The
        // spillover writer must add `spillover_path` without losing
        // the prior payload.
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let big = "Z".repeat(200 * 1024);
            let mut result = ToolResult::success(big).with_metadata(serde_json::json!([
                "unexpected",
                "array",
                "payload"
            ]));
            let path = apply_spillover(&mut result, "call-arr").expect("should spill");

            let metadata = result.metadata.expect("metadata stamped");
            // Prior payload re-homed under `_prior`.
            let prior = metadata.get("_prior").expect("_prior wrap key present");
            assert_eq!(
                prior,
                &serde_json::json!(["unexpected", "array", "payload"]),
                "prior array should round-trip under _prior"
            );
            // New key alongside.
            assert_eq!(
                metadata
                    .get("spillover_path")
                    .and_then(serde_json::Value::as_str),
                Some(path.display().to_string().as_str())
            );
        });
    }
}
