//! Tool-output spillover writer (#422).
//!
//! When a tool produces output that's too large to land in the model's
//! context budget, we want two things at once:
//!
//! 1. The transcript / tool-cell renders a bounded preview so the UI
//!    stays scannable.
//! 2. The full router input is preserved under its origin session so bounded
//!    retrieval and the raw-detail pager can inspect it without leaking a
//!    process-global filesystem path.
//!
//! The default adaptive path writes immutable artifacts under
//! `~/.codewhale/sessions/<session>/artifacts/`. The historical
//! `~/.codewhale/tool_outputs/<sanitised-id>.txt` directory remains only for
//! classic-routing compatibility, protected by a digest-bound origin sidecar.
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
//! UI-side rendering is owned by `tui/history.rs::render_spillover_annotation`;
//! it exposes a path-free receipt and the tool-details shortcut opens the
//! session artifact.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::tools::spec::ToolResult;

/// Name of the spillover directory under the CodeWhale home.
pub const SPILLOVER_DIR_NAME: &str = "tool_outputs";

const LEGACY_SPILLOVER_OWNER_SCHEMA_VERSION: u32 = 1;

/// Session proof for compatibility payloads kept in the historical global
/// `tool_outputs/` directory.
///
/// The payload remains in its legacy location so classic-routing rollback and
/// existing detail pagers keep working, but model retrieval is authorized only
/// when this sidecar names the active origin session and still matches the
/// immutable bytes being returned.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct LegacySpilloverOwnership {
    pub schema_version: u32,
    pub origin_session: String,
    pub digest: String,
    pub size_bytes: u64,
}

/// Default threshold above which a tool result is a candidate for
/// spillover. Mirrors the `MAX_MEMORY_SIZE` ceiling we use elsewhere
/// for "too large to inline" so the rules feel consistent. Wired
/// callers can pass a different value if a tool family has different
/// economics.
pub const SPILLOVER_THRESHOLD_BYTES: usize = 100 * 1024; // 100 KiB

/// Default boot-prune age. Older spillover files are deleted on
/// startup to keep `~/.codewhale/tool_outputs/` from growing without
/// bound. Mirrors the workspace-snapshot 7-day default.
pub const SPILLOVER_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[cfg(test)]
static TEST_SPILLOVER_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) static TEST_SPILLOVER_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolve `~/.codewhale/tool_outputs/`. Returns `None` if the home
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

    let home = dirs::home_dir()?;
    let primary = home.join(".codewhale").join(SPILLOVER_DIR_NAME);
    let legacy = home.join(".deepseek").join(SPILLOVER_DIR_NAME);
    if primary.exists() || !legacy.exists() {
        return Some(primary);
    }
    Some(legacy)
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

#[must_use]
pub(crate) fn legacy_spillover_ownership_path(payload_path: &Path) -> PathBuf {
    payload_path.with_extension("owner.json")
}

/// Publish the proof needed to retrieve a legacy-global spillover safely.
///
/// Payload publication happens first. If this atomic sidecar write fails, the
/// payload is deliberately left unowned and therefore inaccessible through
/// `retrieve_tool_result`; callers must not advertise a retrieval hint.
pub(crate) fn publish_legacy_spillover_ownership(
    payload_path: &Path,
    session_id: &str,
    bytes: &[u8],
) -> io::Result<PathBuf> {
    if session_id.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "legacy spillover ownership requires a session id",
        ));
    }
    let ownership = LegacySpilloverOwnership {
        schema_version: LEGACY_SPILLOVER_OWNER_SCHEMA_VERSION,
        origin_session: session_id.to_string(),
        digest: crate::hashing::sha256_hex(bytes),
        size_bytes: bytes.len().try_into().unwrap_or(u64::MAX),
    };
    let sidecar = legacy_spillover_ownership_path(payload_path);
    let encoded = serde_json::to_vec_pretty(&ownership)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    crate::utils::write_atomic(&sidecar, &encoded)?;
    Ok(sidecar)
}

pub(crate) fn read_legacy_spillover_ownership(
    payload_path: &Path,
) -> io::Result<LegacySpilloverOwnership> {
    let sidecar = legacy_spillover_ownership_path(payload_path);
    if std::fs::symlink_metadata(&sidecar)?
        .file_type()
        .is_symlink()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "legacy spillover ownership sidecar must not be a symlink",
        ));
    }
    let ownership = serde_json::from_slice::<LegacySpilloverOwnership>(&std::fs::read(sidecar)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if ownership.schema_version != LEGACY_SPILLOVER_OWNER_SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported legacy spillover ownership schema",
        ));
    }
    Ok(ownership)
}

/// Resolve the spillover-file path for a SHA256 content hash. Separate
/// namespace (`sha_<hex>.txt`) from the tool-call-id files so legacy
/// SHA-addressed evidence can be recognized without colliding with
/// tool-call references. Retrieval still requires matching ownership
/// metadata. `sha` must be the raw 64-char lowercase hex digest —
/// case-insensitive matching is done by the caller.
#[must_use]
pub fn sha_spillover_path(sha: &str) -> Option<PathBuf> {
    let sha = sha.trim().to_ascii_lowercase();
    if !is_valid_sha256(&sha) {
        return None;
    }
    Some(spillover_root()?.join(format!("sha_{sha}.txt")))
}

/// True when `s` is a 64-character lowercase ASCII hex string. Used
/// to detect bare SHA refs the model might pass to retrieval and to
/// validate input to [`sha_spillover_path`].
#[must_use]
pub fn is_valid_sha256(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Write a legacy SHA-addressed spillover fixture for ownership tests.
#[cfg(test)]
pub fn write_sha_spillover(sha: &str, content: &str) -> io::Result<PathBuf> {
    let path = sha_spillover_path(sha).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sha must be a 64-char lowercase hex digest",
        )
    })?;
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::utils::write_atomic(&path, content.as_bytes())?;
    Ok(path)
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
/// Inline tail retained alongside the head so compiler summaries and final
/// test failures are not systematically hidden by truncation.
pub const SPILLOVER_TAIL_BYTES: usize = 8 * 1024;

fn retained_tail(content: &str, max_bytes: usize) -> &str {
    let floor = content.len().saturating_sub(max_bytes);
    let start = (floor..=content.len())
        .find(|&index| content.is_char_boundary(index))
        .unwrap_or(content.len());
    &content[start..]
}

/// Apply spillover to a tool result in place. If the result's
/// content exceeds [`SPILLOVER_THRESHOLD_BYTES`], writes the full
/// content to a sibling file under `~/.codewhale/tool_outputs/`,
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
#[allow(dead_code)]
pub fn apply_spillover(result: &mut ToolResult, tool_id: &str) -> Option<PathBuf> {
    apply_spillover_inner(result, tool_id, None)
}

/// Apply adaptive routing and publish session-scoped exact evidence.
///
/// The default path writes one immutable payload under the origin session and
/// replaces non-inline content with a calm, bounded receipt. The legacy dual
/// spillover behavior is reachable only through the classic rollback switch.
pub fn apply_spillover_with_artifact(
    result: &mut ToolResult,
    tool_id: &str,
    tool_name: &str,
    session_id: &str,
) -> Option<PathBuf> {
    apply_spillover_inner(
        result,
        tool_id,
        Some(ArtifactSpilloverContext {
            tool_name,
            session_id,
        }),
    )
}

#[derive(Clone, Copy)]
struct ArtifactSpilloverContext<'a> {
    tool_name: &'a str,
    session_id: &'a str,
}

fn apply_spillover_inner(
    result: &mut ToolResult,
    tool_id: &str,
    artifact_context: Option<ArtifactSpilloverContext<'_>>,
) -> Option<PathBuf> {
    if !crate::tools::large_output_router::classic_output_routing_enabled()
        && let Some(context) = artifact_context
    {
        return apply_adaptive_evidence_inner(result, tool_id, context);
    }
    if !result.success {
        return None;
    }
    if result.content.len() <= SPILLOVER_THRESHOLD_BYTES {
        return None;
    }
    let original_content = result.content.clone();
    let total = original_content.len();
    let outcome = match maybe_spillover(
        tool_id,
        &original_content,
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
    let tail = retained_tail(&original_content, SPILLOVER_TAIL_BYTES);
    let digest = crate::hashing::sha256_hex(original_content.as_bytes());
    let path_str = path.display().to_string();

    let legacy_owner_published = artifact_context.is_some_and(|context| {
        match publish_legacy_spillover_ownership(
            &path,
            context.session_id,
            original_content.as_bytes(),
        ) {
            Ok(_) => true,
            Err(err) => {
                tracing::warn!(
                    target: "spillover",
                    ?err,
                    tool_id,
                    "legacy spillover ownership publication failed"
                );
                false
            }
        }
    });

    let mut artifact_path = None;
    if let Some(context) = artifact_context {
        let artifact_id = crate::artifacts::artifact_id_for_tool_call(tool_id);
        match crate::artifacts::write_session_artifact(
            context.session_id,
            &artifact_id,
            &original_content,
        ) {
            Ok((absolute_path, relative_path)) => {
                let record = crate::artifacts::record_tool_output_artifact(
                    context.session_id,
                    tool_id,
                    context.tool_name,
                    relative_path.clone(),
                    &original_content,
                );
                let transcript_ref = crate::artifacts::TranscriptArtifactRef::from(&record);
                let reference = crate::artifacts::render_transcript_artifact_ref(&transcript_ref);
                result.content = format!(
                    "{reference}\n\n[retained head: {} bytes]\n{head}\n\n[retained tail: {} bytes]\n{tail}",
                    head.len(),
                    tail.len(),
                );
                artifact_path = Some((absolute_path, relative_path, record));
            }
            Err(err) => {
                tracing::warn!(
                    target: "spillover",
                    ?err,
                    tool_id,
                    "session artifact write failed; falling back to legacy spillover footer"
                );
            }
        }
    }

    if artifact_path.is_none() {
        let retrieval = if legacy_owner_published {
            format!(
                "Use `retrieve_tool_result ref={tool_id} mode=tail` or \
                 `retrieve_tool_result ref={tool_id} mode=query query=<text>` \
                 to inspect the retained evidence."
            )
        } else {
            "Exact retrieval is unavailable because session ownership could not be recorded."
                .to_string()
        };
        let footer = format!(
            "\n\n[Output truncated: {head_kib} KiB of {total_kib} KiB shown. {retrieval}]",
            head_kib = head.len() / 1024,
            total_kib = total / 1024,
        );
        result.content = format!(
            "{head}\n\n[retained tail: {} bytes]\n{tail}{footer}",
            tail.len()
        );
    }

    let metadata = result.metadata.get_or_insert_with(|| serde_json::json!({}));
    if let Some(obj) = metadata.as_object_mut() {
        if let Some((absolute_path, relative_path, record)) = artifact_path.as_ref() {
            obj.insert(
                "spillover_path".into(),
                serde_json::Value::String(absolute_path.display().to_string()),
            );
            obj.insert(
                "legacy_spillover_path".into(),
                serde_json::Value::String(path_str),
            );
            obj.insert(
                "artifact_id".into(),
                serde_json::Value::String(record.id.clone()),
            );
            obj.insert(
                "artifact_session_id".into(),
                serde_json::Value::String(record.session_id.clone()),
            );
            obj.insert(
                "artifact_relative_path".into(),
                serde_json::Value::String(crate::artifacts::format_artifact_relative_path(
                    relative_path,
                )),
            );
            obj.insert(
                "artifact_path".into(),
                serde_json::Value::String(absolute_path.display().to_string()),
            );
            obj.insert(
                "artifact_byte_size".into(),
                serde_json::Value::Number(serde_json::Number::from(record.byte_size)),
            );
            obj.insert(
                "artifact_preview".into(),
                serde_json::Value::String(record.preview.clone()),
            );
        } else {
            obj.insert("spillover_path".into(), serde_json::Value::String(path_str));
        }
    } else {
        // Pre-existing metadata that wasn't a JSON object (rare,
        // possibly an array). Replace with an object so we can
        // attach our key without losing prior data — wrap it under
        // a `_prior` field so callers that introspect can recover.
        let prior = std::mem::replace(metadata, serde_json::json!({}));
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert("_prior".into(), prior);
            if let Some((absolute_path, relative_path, record)) = artifact_path.as_ref() {
                obj.insert(
                    "spillover_path".into(),
                    serde_json::Value::String(absolute_path.display().to_string()),
                );
                obj.insert(
                    "legacy_spillover_path".into(),
                    serde_json::Value::String(path.display().to_string()),
                );
                obj.insert(
                    "artifact_id".into(),
                    serde_json::Value::String(record.id.clone()),
                );
                obj.insert(
                    "artifact_session_id".into(),
                    serde_json::Value::String(record.session_id.clone()),
                );
                obj.insert(
                    "artifact_relative_path".into(),
                    serde_json::Value::String(crate::artifacts::format_artifact_relative_path(
                        relative_path,
                    )),
                );
                obj.insert(
                    "artifact_path".into(),
                    serde_json::Value::String(absolute_path.display().to_string()),
                );
                obj.insert(
                    "artifact_byte_size".into(),
                    serde_json::Value::Number(serde_json::Number::from(record.byte_size)),
                );
                obj.insert(
                    "artifact_preview".into(),
                    serde_json::Value::String(record.preview.clone()),
                );
            } else {
                obj.insert(
                    "spillover_path".into(),
                    serde_json::Value::String(path.display().to_string()),
                );
            }
        }
    }
    if let Some(obj) = result
        .metadata
        .as_mut()
        .and_then(serde_json::Value::as_object_mut)
    {
        obj.insert("truncated".into(), serde_json::Value::Bool(true));
        obj.insert(
            "content_digest".into(),
            serde_json::Value::String(format!("sha256:{digest}")),
        );
        obj.insert(
            "original_byte_count".into(),
            serde_json::Value::Number(serde_json::Number::from(total as u64)),
        );
        obj.insert(
            "retained_head_bytes".into(),
            serde_json::Value::Number(serde_json::Number::from(head.len() as u64)),
        );
        obj.insert(
            "retained_tail_bytes".into(),
            serde_json::Value::Number(serde_json::Number::from(tail.len() as u64)),
        );
    }
    artifact_path
        .map(|(absolute_path, _, _)| absolute_path)
        .or(Some(path))
}

fn apply_adaptive_evidence_inner(
    result: &mut ToolResult,
    tool_id: &str,
    context: ArtifactSpilloverContext<'_>,
) -> Option<PathBuf> {
    use crate::tools::large_output_router::{
        DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS, EVIDENCE_RETENTION_SECS, EvidenceArtifact,
        EvidenceRetentionState, EvidenceRouting, estimate_tokens, publish_evidence_metadata,
        unix_millis_now,
    };

    let estimated_tokens = estimate_tokens(&result.content);
    let threshold = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("evidence_threshold_tokens"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS);
    let routing = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("evidence_routing"))
        .cloned()
        .and_then(|value| serde_json::from_value::<EvidenceRouting>(value).ok())
        .unwrap_or_else(|| EvidenceRouting::from_token_estimate(estimated_tokens, threshold));
    if routing == EvidenceRouting::Inline {
        return None;
    }

    let original = result.content.clone();
    let artifact_id = crate::artifacts::artifact_id_for_tool_call(tool_id);
    let relative_path = crate::artifacts::session_artifact_relative_path(&artifact_id);
    let digest = crate::hashing::sha256_hex(original.as_bytes());
    let now_ms = unix_millis_now();
    let proposed_artifact = EvidenceArtifact {
        handle: artifact_id.clone(),
        digest: digest.clone(),
        size_bytes: original.len().try_into().unwrap_or(u64::MAX),
        content_type: if serde_json::from_str::<serde_json::Value>(&original).is_ok() {
            "application/json".to_string()
        } else {
            "text/plain".to_string()
        },
        tool_name: context.tool_name.to_string(),
        call_id: tool_id.to_string(),
        origin_session: context.session_id.to_string(),
        generation: 1,
        redacted: false,
        encoding: "utf-8".to_string(),
        retention_state: EvidenceRetentionState::Live,
        created_at_unix_ms: now_ms,
        retain_until_unix_ms: now_ms.saturating_add(EVIDENCE_RETENTION_SECS * 1_000),
        storage_path: relative_path.clone(),
    };
    let artifact = match crate::tools::large_output_router::read_evidence_metadata(
        context.session_id,
        &artifact_id,
    ) {
        Ok(existing)
            if existing.digest == proposed_artifact.digest
                && existing.size_bytes == proposed_artifact.size_bytes
                && existing.call_id == proposed_artifact.call_id
                && existing.origin_session == proposed_artifact.origin_session =>
        {
            existing
        }
        Ok(_) => {
            tracing::warn!(target: "evidence", tool_id, "adaptive evidence replay conflicts with immutable metadata");
            return None;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Err(err) = publish_evidence_metadata(context.session_id, &proposed_artifact) {
                tracing::warn!(target: "evidence", ?err, tool_id, "adaptive evidence metadata publication failed");
                return None;
            }
            proposed_artifact
        }
        Err(err) => {
            tracing::warn!(target: "evidence", ?err, tool_id, "adaptive evidence metadata validation failed");
            return None;
        }
    };

    // Seal the ownership/integrity record before publishing predictable
    // `art_<call>.txt` bytes. If metadata publication fails, no payload exists
    // for a guessed handle to retrieve without the generation, redaction,
    // retention, size, and digest checks above. A metadata-only interruption
    // is safe: the handle is never advertised and a retry can idempotently
    // publish the matching bytes.
    let (absolute_path, relative_path) = match crate::artifacts::write_session_artifact_immutable(
        context.session_id,
        &artifact_id,
        original.as_bytes(),
    ) {
        Ok(paths) => paths,
        Err(err) => {
            tracing::warn!(target: "evidence", ?err, tool_id, "adaptive evidence content publication failed");
            return None;
        }
    };

    let record = crate::artifacts::record_tool_output_artifact(
        context.session_id,
        tool_id,
        context.tool_name,
        relative_path.clone(),
        &original,
    );
    let head_limit = if routing == EvidenceRouting::Hybrid {
        8 * 1024
    } else {
        2 * 1024
    };
    let tail_limit = if routing == EvidenceRouting::Hybrid {
        2 * 1024
    } else {
        512
    };
    let head_end = (0..=head_limit.min(original.len()))
        .rev()
        .find(|index| original.is_char_boundary(*index))
        .unwrap_or(0);
    let tail = retained_tail(&original, tail_limit);
    result.content = format!(
        "[Exact evidence retained · {} · inspect with `retrieve_tool_result ref={}`]\n\n{}\n\n[final excerpt]\n{}",
        crate::artifacts::format_byte_size(original.len().try_into().unwrap_or(u64::MAX)),
        artifact_id,
        &original[..head_end],
        tail,
    );
    let metadata = result.metadata.get_or_insert_with(|| serde_json::json!({}));
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "spillover_path".into(),
            absolute_path.display().to_string().into(),
        );
        object.insert("artifact_id".into(), artifact_id.into());
        object.insert("artifact_session_id".into(), context.session_id.into());
        object.insert(
            "artifact_relative_path".into(),
            crate::artifacts::format_artifact_relative_path(&relative_path).into(),
        );
        object.insert("artifact_byte_size".into(), artifact.size_bytes.into());
        object.insert("artifact_digest".into(), digest.into());
        object.insert("artifact_generation".into(), artifact.generation.into());
        object.insert("artifact_encoding".into(), artifact.encoding.into());
        object.insert("artifact_retention_state".into(), "live".into());
        object.insert("evidence_available".into(), true.into());
        object.insert("truncated".into(), true.into());
        object.insert("original_byte_count".into(), artifact.size_bytes.into());
        object.insert("retained_head_bytes".into(), head_end.into());
        object.insert("retained_tail_bytes".into(), tail.len().into());
        object.insert(
            "artifact_preview".into(),
            original.chars().take(200).collect::<String>().into(),
        );
        object.insert(
            "artifact_record".into(),
            serde_json::to_value(record).unwrap_or(serde_json::Value::Null),
        );
    }
    Some(absolute_path)
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

/// Override the storage roots for tests so they don't pollute the
/// user's real `~/.codewhale/` directory. This uses explicit test hooks instead
/// of `$HOME` because Windows home-dir resolution can ignore environment
/// overrides and return the runner profile directory.
#[cfg(test)]
fn with_test_home<F, R>(home: &Path, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _artifact_guard = crate::artifacts::TEST_ARTIFACT_SESSIONS_GUARD
        .lock()
        .unwrap_or_else(|err| err.into_inner());

    struct StorageRootOverride {
        prior_spillover: Option<PathBuf>,
        prior_artifacts: Option<PathBuf>,
    }

    impl Drop for StorageRootOverride {
        fn drop(&mut self) {
            set_test_spillover_root(self.prior_spillover.take());
            crate::artifacts::set_test_artifact_sessions_root(self.prior_artifacts.take());
        }
    }

    // Tests in this module serialize spillover through `TEST_GUARD`; the
    // artifact guard above protects the session-artifact root shared with
    // artifacts.rs tests.
    let prior_spillover =
        set_test_spillover_root(Some(home.join(".codewhale").join(SPILLOVER_DIR_NAME)));
    let prior_artifacts = crate::artifacts::set_test_artifact_sessions_root(Some(
        home.join(".codewhale").join("sessions"),
    ));
    let _restore = StorageRootOverride {
        prior_spillover,
        prior_artifacts,
    };
    f()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Tests in this module serialize through this guard because they mutate
    /// process-global test storage roots. Without it, cargo's parallel runner
    /// would observe interleaved overrides.
    fn setup() -> std::sync::MutexGuard<'static, ()> {
        super::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn with_test_home_overrides_storage_roots_without_home_resolution() {
        let _g = setup();
        let tmp = tempdir().unwrap();

        with_test_home(tmp.path(), || {
            assert_eq!(
                spillover_root().as_deref(),
                Some(tmp.path().join(".codewhale").join("tool_outputs").as_path())
            );
            assert_eq!(
                crate::artifacts::session_artifact_absolute_path(
                    "session-123",
                    &PathBuf::from("artifacts").join("art_call-big.txt")
                )
                .as_deref(),
                Some(
                    tmp.path()
                        .join(".codewhale")
                        .join("sessions")
                        .join("session-123")
                        .join("artifacts")
                        .join("art_call-big.txt")
                        .as_path()
                )
            );
        });
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
            // Directory landed under `<HOME>/.codewhale/tool_outputs/`.
            // Compare components instead of a substring on `to_string_lossy`
            // — Windows uses `\` as the separator so a `/` substring match
            // would falsely fail there.
            let components: Vec<&str> = path
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            assert!(
                components.contains(&".codewhale") && components.contains(&"tool_outputs"),
                "spillover path missing expected `.codewhale/tool_outputs/...` segments: {path:?}"
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
            assert!(
                result
                    .content
                    .contains("Exact retrieval is unavailable because session ownership")
            );
            assert!(!result.content.contains("retrieve_tool_result"));

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
            assert_eq!(metadata["truncated"], true);
            assert_eq!(metadata["original_byte_count"], 200 * 1024);
            assert_eq!(metadata["retained_head_bytes"], SPILLOVER_HEAD_BYTES);
            assert_eq!(metadata["retained_tail_bytes"], SPILLOVER_TAIL_BYTES);
            assert!(
                metadata["content_digest"]
                    .as_str()
                    .is_some_and(|digest| digest.starts_with("sha256:"))
            );
        });
    }

    #[test]
    fn apply_spillover_with_artifact_writes_session_file_and_ref_block() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let big = "checking crate ... error[E0425]: cannot find value\n".repeat(4_000);
            let mut result = ToolResult::success(big.clone());
            let path =
                apply_spillover_with_artifact(&mut result, "call-big", "exec_shell", "session-123")
                    .expect("should spill");

            let session_artifact = tmp
                .path()
                .join(".codewhale")
                .join("sessions")
                .join("session-123")
                .join("artifacts")
                .join("art_call-big.txt");
            assert_eq!(path, session_artifact);
            assert_eq!(fs::read_to_string(&session_artifact).unwrap(), big);
            assert!(
                !tmp.path()
                    .join(".codewhale/tool_outputs/call-big.txt")
                    .exists(),
                "adaptive evidence stores one exact origin-session copy"
            );
            assert!(result.content.starts_with("[Exact evidence retained"));
            assert!(
                result
                    .content
                    .contains("retrieve_tool_result ref=art_call-big")
            );
            assert!(!result.content.contains("artifacts/art_call-big.txt"));
            assert!(
                session_artifact
                    .with_file_name("art_call-big.evidence.json")
                    .exists()
            );

            let metadata = result.metadata.expect("metadata stamped");
            assert_eq!(
                metadata
                    .get("artifact_id")
                    .and_then(serde_json::Value::as_str),
                Some("art_call-big")
            );
            assert_eq!(
                metadata
                    .get("artifact_relative_path")
                    .and_then(serde_json::Value::as_str),
                Some("artifacts/art_call-big.txt")
            );
            assert_eq!(
                metadata
                    .get("artifact_session_id")
                    .and_then(serde_json::Value::as_str),
                Some("session-123")
            );
            assert_eq!(metadata["original_byte_count"], big.len());
            assert!(metadata["retained_head_bytes"].as_u64().unwrap_or(0) <= 2 * 1024);
            assert!(metadata["retained_tail_bytes"].as_u64().unwrap_or(0) <= 512);
        });
    }

    #[test]
    fn adaptive_evidence_keeps_success_and_failure_exact_distinct_and_out_of_context() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let sentinel = "DEEP_RAW_SENTINEL";
            let success_raw = format!(
                "{}{}{}",
                "head\n".repeat(2_000),
                sentinel,
                "tail\n".repeat(2_000)
            );
            let failure_raw = format!("{}{}", "failure\n".repeat(3_000), "FAILURE_END");
            let mut success = ToolResult::success(success_raw.clone());
            let mut failure = ToolResult::error(failure_raw.clone());

            let success_path = apply_spillover_with_artifact(
                &mut success,
                "call-success",
                "exec_shell",
                "session-a",
            )
            .expect("success evidence");
            let failure_path = apply_spillover_with_artifact(
                &mut failure,
                "call-failure",
                "mcp_fixture",
                "session-a",
            )
            .expect("failure evidence");

            assert_ne!(success_path, failure_path);
            assert_eq!(
                std::fs::read(&success_path).unwrap(),
                success_raw.as_bytes()
            );
            assert_eq!(
                std::fs::read(&failure_path).unwrap(),
                failure_raw.as_bytes()
            );
            assert!(!success.content.contains(sentinel));
            assert!(success.content.len() < 4 * 1024);
            let success_meta = success.metadata.as_ref().unwrap();
            let failure_meta = failure.metadata.as_ref().unwrap();
            assert_ne!(
                success_meta["artifact_digest"],
                failure_meta["artifact_digest"]
            );
            assert_eq!(success_meta["artifact_session_id"], "session-a");
            assert_eq!(failure_meta["artifact_session_id"], "session-a");

            let mut replay = ToolResult::success(success_raw);
            let replay_path = apply_spillover_with_artifact(
                &mut replay,
                "call-success",
                "exec_shell",
                "session-a",
            )
            .expect("idempotent replay");
            assert_eq!(replay_path, success_path);
        });
    }

    #[test]
    fn adaptive_evidence_publication_failure_emits_no_handle_or_details_hint() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let session_dir = tmp
                .path()
                .join(".codewhale")
                .join("sessions")
                .join("session-blocked");
            std::fs::create_dir_all(&session_dir).unwrap();
            std::fs::write(session_dir.join("artifacts"), b"block artifact directory").unwrap();

            let raw = format!(
                "{}{}{}",
                "publication failure head\n".repeat(1_500),
                "DEEP_FAILURE_SENTINEL",
                "publication failure tail\n".repeat(1_500),
            );
            let mut result = ToolResult::error(raw.clone());
            let path = apply_spillover_with_artifact(
                &mut result,
                "call-failed-publish",
                "mcp_fixture",
                "session-blocked",
            );

            assert!(path.is_none());
            assert_eq!(result.content, raw);
            assert!(!result.content.contains("Exact evidence retained"));
            assert!(!result.content.contains("retrieve_tool_result"));
            assert!(
                result
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("evidence_available"))
                    .is_none()
            );
            assert!(
                !session_dir
                    .join("artifacts/art_call-failed-publish.txt")
                    .exists()
            );
        });
    }

    #[test]
    fn adaptive_evidence_metadata_atomic_failure_leaves_payload_unadvertised() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let artifact_dir = tmp
                .path()
                .join(".codewhale")
                .join("sessions")
                .join("session-metadata-blocked")
                .join("artifacts");
            std::fs::create_dir_all(artifact_dir.join("art_call-failed-metadata.evidence.json"))
                .unwrap();

            let raw = format!(
                "{}{}{}",
                "metadata failure head\n".repeat(1_500),
                "DEEP_METADATA_FAILURE_SENTINEL",
                "metadata failure tail\n".repeat(1_500),
            );
            let mut result = ToolResult::success(raw.clone());
            let path = apply_spillover_with_artifact(
                &mut result,
                "call-failed-metadata",
                "exec_shell",
                "session-metadata-blocked",
            );

            assert!(path.is_none());
            assert_eq!(result.content, raw);
            assert!(!result.content.contains("Exact evidence retained"));
            assert!(!result.content.contains("retrieve_tool_result"));
            assert!(
                !artifact_dir.join("art_call-failed-metadata.txt").exists(),
                "metadata failure must leave no payload behind a guessable handle"
            );
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
