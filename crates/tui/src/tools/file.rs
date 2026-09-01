//! File system engines for the lowercase `read`, `write`, and `edit` primitives
//! plus deferred workspace helpers such as `list_dir`. The older `File`,
//! `read_file`, `write_file`, and `edit_file` names remain registered but hidden
//! so saved sessions can replay their original schemas and behavior.
//!
//! These tools provide safe file system operations within the workspace,
//! with path validation to prevent escaping the workspace boundary.

use super::diff_format::make_unified_diff;
use super::spec::{
    ApprovalRequirement, RichToolResult, ToolCapability, ToolContext, ToolError, ToolResult,
    ToolSpec, lsp_diagnostics_for_paths, optional_str, optional_u64, required_str,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;
use unicode_normalization::UnicodeNormalization;

// === Content-hash edit guards (#3979) ===

/// Format a file snapshot's content hash as `sha256:<hex>`.
///
/// The prefixed shape (rather than a bare hex digest) is deliberate: it is
/// self-describing in the transcript, and it makes an accidentally-truncated or
/// hand-invented value fail the equality check instead of matching by luck.
///
/// A file's hash is taken over its raw bytes, always before any windowing,
/// truncation, or rendering, so the value `read` reports is the value `write`,
/// `edit`, and `patch` verify.
pub(super) fn content_hash(bytes: &[u8]) -> String {
    format!("sha256:{}", crate::hashing::sha256_hex(bytes))
}

/// Hash a file's bytes without holding the whole file in memory.
///
/// The read path streams a bounded window out of large files on purpose, so it
/// never has the full contents to hash. Digesting through a separate streaming
/// pass keeps that memory bound while still producing a hash over the entire
/// file — the only value an edit guard can verify against.
fn hash_file_streaming(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;

    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!(
        "sha256:{}",
        crate::hashing::hex_bytes(hasher.finalize())
    ))
}

/// The one-line header that reports a snapshot hash to the model.
///
/// This goes in `ToolResult::content`, not in `ToolResult::metadata`, and that
/// placement is the whole point. `metadata` never reaches the model: the wire
/// `ContentBlock::ToolResult` (`crates/core/src/request.rs`) has no field for
/// it, and the turn loop builds the tool message from `output.content` alone
/// (`crates/tui/src/core/engine/turn_loop.rs`). Metadata is for the TUI,
/// telemetry, and the approval/mutation receipts. A hash the model cannot read
/// is a guard the model cannot use, so it is rendered into the content — either
/// as an attribute on the `<file …>` envelope, or as this header line for the
/// unwrapped small-file read.
fn content_hash_header(hash: &str) -> String {
    format!("content_hash=\"{hash}\"\n")
}

/// Reject a mutation whose `expected_hash` does not describe the current file.
///
/// Callers must run this against the exact snapshot the mutation would be
/// applied to, and before anything is written. `None` (parameter absent) keeps
/// the pre-#3979 behavior untouched — the guard is opt-in.
fn verify_expected_hash(
    expected: Option<&str>,
    current_bytes: &[u8],
    action: &str,
    path_str: &str,
) -> Result<(), ToolError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = content_hash(current_bytes);
    if expected == actual {
        return Ok(());
    }
    Err(ToolError::execution_failed(format!(
        "File `{action}` refused: {path_str} changed since it was read. \
         expected_hash was {expected} but the file is now {actual}, so nothing was written. \
         Recovery: call File with action=\"read\" path=\"{path_str}\" to get the current contents \
         and its content_hash, then retry with the new hash."
    )))
}

/// Shared schema text for the optional guard parameter.
///
/// The hidden compatibility schema still has a byte budget, so this is only the
/// instruction the legacy caller needs: what to pass and what happens on a
/// mismatch. The rationale stays in doc comments rather than schema bytes.
pub(super) const EXPECTED_HASH_DESCRIPTION: &str = "The `content_hash` from a prior read; the write is refused and the file left unchanged if it changed since";

// === Cross-harness parameter aliases ===

/// Rewrite well-known parameter spellings from other coding harnesses onto the
/// names this tool actually implements.
///
/// Every mainstream harness names the same three file-edit arguments
/// differently — `old_string`/`new_string`, `old_str`/`new_str`,
/// `oldText`/`newText` — and models carry whichever spelling their training
/// saw most. CodeWhale's canonical `search`/`replace` is the odd one out, so a
/// model reaching for its prior used to burn a full turn on a rejection
/// (#5209) and then guess again. Translating an unambiguous synonym is
/// strictly better than refusing it: the edit the model asked for is the edit
/// that happens, and the schema still advertises exactly one canonical name so
/// there is no new ambiguity to learn.
///
/// This is deliberately *not* a silent-acceptance path. Only exact synonyms
/// are mapped, a synonym that disagrees with an explicitly supplied canonical
/// value is an error rather than a coin flip, and any parameter that is not a
/// known synonym still fails validation. The #5209 guarantee — no fabricated
/// "Replaced 1 occurrence" for an edit that never landed — is unchanged.
pub(super) struct ParamAlias {
    /// Spelling a model might emit.
    alias: &'static str,
    /// Parameter this tool implements.
    canonical: &'static str,
}

const fn alias(alias: &'static str, canonical: &'static str) -> ParamAlias {
    ParamAlias { alias, canonical }
}

/// Path spellings shared by every file action. `path` is CodeWhale's
/// canonical name and the most common one in the field, but `file_path` is
/// widespread enough in training data to be worth accepting everywhere.
pub(super) const PATH_ALIASES: &[ParamAlias] =
    &[alias("file_path", "path"), alias("filePath", "path")];

/// Edit-specific spellings. Ordered most- to least-common.
const EDIT_ALIASES: &[ParamAlias] = &[
    alias("old_string", "search"),
    alias("new_string", "replace"),
    alias("old_str", "search"),
    alias("new_str", "replace"),
    alias("oldText", "search"),
    alias("newText", "replace"),
    alias("old_text", "search"),
    alias("new_text", "replace"),
    alias("replacement", "replace"),
];

/// Read-window spellings. `offset`/`limit` and `line_offset`/`n_lines` both
/// name the same two numbers as CodeWhale's `start_line`/`max_lines` in widely
/// trained-on tool surfaces. A wrong guess here used to be ignored outright,
/// silently returning the head of the file instead of the window the model
/// asked for — a wrong answer shaped like a right one.
const READ_ALIASES: &[ParamAlias] = &[
    alias("offset", "start_line"),
    alias("line_offset", "start_line"),
    alias("limit", "max_lines"),
    alias("n_lines", "max_lines"),
    alias("num_lines", "max_lines"),
];

/// `search_name` spellings. The `File` wrapper advertises `max_results` for
/// both search actions, but only `search_content` implements that name; on
/// `search_name` the same number is spelled `limit`. Folding it here (rather
/// than copying it inside the wrapper) keeps one alias mechanism, so the
/// result-count cap a model asks for is the cap it gets whichever name it
/// reaches for, and a direct `file_search` call behaves the same way.
pub(super) const SEARCH_NAME_ALIASES: &[ParamAlias] = &[alias("max_results", "limit")];

/// `search_content` spellings, mirroring `SEARCH_NAME_ALIASES` in the other
/// direction: the wrapper advertises `query` and `limit` on the name-search
/// side, and a model that carries them across to a content search means
/// `pattern` and `max_results`.
pub(super) const SEARCH_CONTENT_ALIASES: &[ParamAlias] =
    &[alias("query", "pattern"), alias("limit", "max_results")];

/// Apply `aliases` to `input`, in place.
///
/// An alias is consumed only when the canonical key is absent. When both are
/// present and *equal* the alias is dropped as a harmless duplicate; when both
/// are present and disagree the call fails, because guessing which one the
/// model meant is exactly the fabrication this path exists to prevent.
pub(super) fn apply_param_aliases(
    input: &mut Value,
    aliases: &[ParamAlias],
    tool_label: &str,
) -> Result<(), ToolError> {
    let Some(obj) = input.as_object_mut() else {
        return Ok(());
    };

    for ParamAlias { alias, canonical } in aliases {
        let Some(alias_value) = obj.remove(*alias) else {
            continue;
        };
        match obj.get(*canonical) {
            None => {
                obj.insert((*canonical).to_string(), alias_value);
            }
            Some(existing) if existing == &alias_value => {}
            Some(_) => {
                return Err(ToolError::invalid_input(format!(
                    "{tool_label} received both `{canonical}` and its alias `{alias}` with different values, so the intended argument is ambiguous; nothing was changed. Pass only `{canonical}`."
                )));
            }
        }
    }

    Ok(())
}

// === Per-action parameter contracts ===

/// The parameter contract for one `File` action.
///
/// #5209 taught `edit` to refuse a parameter it does not implement instead of
/// dropping it and returning a success-shaped receipt. Only `edit` learned it.
/// Every other action kept silently discarding unknown keys, and for a reader
/// that is the same failure wearing a quieter costume: a misspelled
/// `start_line` on `read` is dropped, the head of the file comes back, and
/// nothing in the response says the requested window was never honored — a
/// wrong answer shaped like a right one.
///
/// One table, one error shape, every action.
pub(super) struct ActionParams {
    /// Action name as the model spells it on `File` (`read`, `write`, …).
    action: &'static str,
    /// Every parameter the action implements, canonical spellings only.
    /// Aliases are folded onto these by [`apply_param_aliases`] before
    /// validation runs, so they must not be listed here.
    allowed: &'static [&'static str],
    /// Parameters the action cannot run without.
    required: &'static [&'static str],
    /// `true` when exactly one of `required` is needed rather than all of
    /// them — `patch` accepts `patch`, `replace`, or `changes`.
    required_is_choice: bool,
}

const fn params(
    action: &'static str,
    allowed: &'static [&'static str],
    required: &'static [&'static str],
) -> ActionParams {
    ActionParams {
        action,
        allowed,
        required,
        required_is_choice: false,
    }
}

pub(super) const READ_PARAMS: ActionParams = params(
    "read",
    &["path", "start_line", "max_lines", "pages"],
    &["path"],
);

pub(super) const WRITE_PARAMS: ActionParams = params(
    "write",
    &["path", "content", "expected_hash"],
    &["path", "content"],
);

pub(super) const EDIT_PARAMS: ActionParams = params(
    "edit",
    &["path", "search", "replace", "expected_hash"],
    &["path", "search", "replace"],
);

pub(super) const LIST_PARAMS: ActionParams = params("list", &["path"], &[]);

pub(super) const SEARCH_NAME_PARAMS: ActionParams = params(
    "search_name",
    &["query", "path", "limit", "extensions", "exclude"],
    &["query"],
);

pub(super) const SEARCH_CONTENT_PARAMS: ActionParams = params(
    "search_content",
    &[
        "pattern",
        "path",
        "include",
        "exclude",
        "context_lines",
        "case_insensitive",
        "max_results",
    ],
    &["pattern"],
);

pub(super) const PATCH_PARAMS: ActionParams = ActionParams {
    action: "patch",
    allowed: &[
        "path",
        "patch",
        "replace",
        "changes",
        "fuzz",
        "create_if_missing",
        "expected_hash",
    ],
    required: &["patch", "replace", "changes"],
    required_is_choice: true,
};

/// Render `names` as a backticked, comma-separated English list.
fn quoted_list(names: &[&str], conjunction: &str) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [] => "none".to_string(),
        [only] => only.clone(),
        [first, second] => format!("{first} {conjunction} {second}"),
        [head @ .., last] => format!("{}, {conjunction} {last}", head.join(", ")),
    }
}

impl ActionParams {
    /// Reject parameter names this action does not implement.
    ///
    /// Must run *after* [`apply_param_aliases`], exactly as the `edit` path
    /// does. The alias lane's reasoning stands: translating an unambiguous
    /// synonym is better than refusing it, so by the time this runs every
    /// spelling with a known meaning has already been folded onto its
    /// canonical name. What is left is a name with no known meaning, where
    /// continuing would mean guessing which argument was intended — so it
    /// hard-errors rather than dropping the argument and reporting success.
    pub(super) fn reject_unknown(&self, input: &Value) -> Result<(), ToolError> {
        let action = self.action;
        let required = if self.required_is_choice {
            format!("one of {}", quoted_list(self.required, "or"))
        } else {
            quoted_list(self.required, "and")
        };

        let Some(obj) = input.as_object() else {
            return Err(ToolError::invalid_input(format!(
                "File {action} input must be an object. Allowed parameters are {}. Required: {required}. The {action} was not performed.",
                quoted_list(self.allowed, "and"),
            )));
        };

        let unexpected: Vec<&str> = obj
            .keys()
            .map(String::as_str)
            .filter(|key| !self.allowed.contains(key))
            .collect();
        if !unexpected.is_empty() {
            return Err(ToolError::invalid_input(format!(
                "unexpected File {action} parameter(s): {}. Allowed parameters are {}. Required: {required}. The {action} was not performed.",
                unexpected.join(", "),
                quoted_list(self.allowed, "and"),
            )));
        }

        Ok(())
    }

    /// A required parameter that is not also allowed would make the refusal
    /// self-contradicting: it would name an argument the same check rejects.
    #[cfg(test)]
    pub(super) fn assert_required_is_allowed(&self) {
        for name in self.required {
            assert!(
                self.allowed.contains(name),
                "File {} requires `{name}` but does not allow it",
                self.action
            );
        }
    }
}

// === ReadFileTool ===

fn canonical_path_for_credential_guard(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

fn config_backup_path_for_credential_guard(config_path: &Path) -> PathBuf {
    let mut file_name = config_path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from(codewhale_config::CONFIG_FILE_NAME));
    file_name.push(".bak");
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file_name)
}

fn is_config_or_backup(candidate: &Path, config_path: &Path) -> bool {
    let config_path = canonical_path_for_credential_guard(config_path);
    let backup_path =
        canonical_path_for_credential_guard(&config_backup_path_for_credential_guard(&config_path));
    candidate == config_path || candidate == backup_path
}

/// Return whether `read_file` must refuse a CodeWhale-owned credential file.
///
/// This is deliberately scoped to the active config, the two conventional
/// config locations (including one-time backups), and CodeWhale's file-backed
/// secret-store directories. Other dotfiles remain readable. Model-bound
/// redaction is still required because shell tools can read these files and
/// arbitrary commands can print credentials without reading a file at all.
/// Refuse a read the sandbox read deny-list blocks (S1).
///
/// `read_file`, `read`, and `read_media` all run *in-process*: they call
/// `std::fs` inside the harness, so `sandbox-exec` and `bwrap` never see them
/// and the OS-level deny rules do not apply. This is the enforcement point for
/// those tools, and the refusal is always an explicit error — never an empty
/// result, which would read as "the file is empty" and invite the model to
/// probe siblings.
pub(crate) fn enforce_read_denylist(path: &Path, tool: &str) -> Result<(), ToolError> {
    match crate::sandbox::read_guard::active().check(path) {
        Ok(()) => Ok(()),
        Err(denial) => {
            let message = denial.message(tool);
            tracing::warn!(
                target: "codewhale::sandbox::read_guard",
                requested = %denial.requested.display(),
                via_symlink = denial.via_symlink,
                tool = tool,
                "sandbox read deny-list refused a read"
            );
            Err(ToolError::permission_denied(message))
        }
    }
}

pub(crate) fn is_codewhale_credential_path(path: &Path) -> bool {
    let candidate = canonical_path_for_credential_guard(path);

    if let Ok(active_config) = codewhale_config::resolve_config_path(None)
        && is_config_or_backup(&candidate, &active_config)
    {
        return true;
    }

    let roots = [
        codewhale_config::codewhale_home(),
        codewhale_config::legacy_deepseek_home(),
    ];
    for root in roots.into_iter().flatten() {
        if is_config_or_backup(&candidate, &root.join(codewhale_config::CONFIG_FILE_NAME)) {
            return true;
        }

        let secrets_dir = canonical_path_for_credential_guard(&root.join("secrets"));
        if candidate.starts_with(secrets_dir) {
            return true;
        }
    }

    false
}

// === small-contract-compatible primitive implementation helpers ===

const READ_MAX_LINES: usize = 2_000;
const READ_MAX_BYTES: usize = 50 * 1024;
const READ_RESULT_ABSOLUTE_MAX_BYTES: usize = 2 * 1024 * 1024;

fn effective_read_max_bytes() -> usize {
    crate::tools::large_output_router::WorkshopConfig::active_read_result_max_bytes()
        .map(|n| n.clamp(READ_MAX_BYTES, READ_RESULT_ABSOLUTE_MAX_BYTES))
        .unwrap_or(READ_MAX_BYTES)
}

fn effective_read_max_lines() -> usize {
    match crate::tools::large_output_router::WorkshopConfig::active_read_result_max_bytes() {
        Some(bytes) if bytes > READ_MAX_BYTES => (bytes / 80).clamp(READ_MAX_LINES, 20_000),
        _ => READ_MAX_LINES,
    }
}

type FileMutationMutex = AsyncMutex<()>;

/// File primitives can also be invoked outside the native engine's global
/// execution lock (for example by an embedded host). Keep writes to one path
/// ordered in those hosts without exposing any locking ceremony in the tool
/// schema or result.
fn file_mutation_lock(path: &Path) -> Result<Arc<FileMutationMutex>, ToolError> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<FileMutationMutex>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().map_err(|_| {
        ToolError::execution_failed(
            "file mutation queue is unavailable because its lock was poisoned",
        )
    })?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    Ok(lock)
}

async fn acquire_file_mutation(
    path: &Path,
    context: &ToolContext,
) -> Result<OwnedMutexGuard<()>, ToolError> {
    let lock = file_mutation_lock(path)?;
    if let Some(cancel) = context.cancel_token.as_ref() {
        tokio::select! {
            guard = lock.lock_owned() => Ok(guard),
            () = cancel.cancelled() => Err(ToolError::cancelled("Operation aborted")),
        }
    } else {
        Ok(lock.lock_owned().await)
    }
}

fn check_file_operation_cancelled(context: &ToolContext) -> Result<(), ToolError> {
    if context
        .cancel_token
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(ToolError::cancelled("Operation aborted"));
    }
    Ok(())
}

async fn contract_mutation_result(
    context: &ToolContext,
    file_path: &Path,
    requested_path: &str,
    before: &str,
    after: &str,
    outcome: &str,
    summary: String,
) -> ToolResult {
    let paths = [file_path.to_path_buf()];
    let diagnostics = lsp_diagnostics_for_paths(context, &paths).await;
    ToolResult::success(summary).with_metadata(json!({
        "event": "file.mutation",
        "lsp_diagnostics": diagnostics,
        "mutation": {
            "diff": make_unified_diff(requested_path, before, after),
            "files": [{ "path": requested_path, "outcome": outcome }],
            "renames": []
        }
    }))
}

fn reject_primitive_unknown(input: &Value, tool: &str, allowed: &[&str]) -> Result<(), ToolError> {
    let object = input
        .as_object()
        .ok_or_else(|| ToolError::invalid_input(format!("{tool} input must be an object")))?;
    let unexpected = object
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if unexpected.is_empty() {
        return Ok(());
    }
    Err(ToolError::invalid_input(format!(
        "unexpected {tool} parameter(s): {}",
        unexpected.join(", ")
    )))
}

fn contract_line_number(input: &Value, key: &str) -> Result<Option<usize>, ToolError> {
    let Some(value) = input.get(key) else {
        return Ok(None);
    };
    let number = value
        .as_u64()
        .ok_or_else(|| ToolError::invalid_input(format!("{key} must be a non-negative integer")))?;
    usize::try_from(number)
        .map(Some)
        .map_err(|_| ToolError::invalid_input(format!("{key} exceeds platform range")))
}

fn primitive_image_mime(bytes: &[u8]) -> Option<&'static str> {
    crate::image_attach::sniff_media_type(bytes)
        .or_else(|| bytes.starts_with(b"BM").then_some("image/bmp"))
}

fn contract_format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[derive(Debug)]
struct ContractReadWindow {
    content: String,
    shown_lines: usize,
    truncated_by_bytes: bool,
    truncated_by_lines: bool,
    first_line_too_large: bool,
}

/// Retain only complete lines from the head, stopping at its own independent
/// line and UTF-8 byte budgets. A terminal newline is content but does not add
/// a phantom line to the truncation counter.
fn contract_read_window(content: &str) -> ContractReadWindow {
    let mut lines = if content.is_empty() {
        Vec::new()
    } else {
        content.split('\n').collect::<Vec<_>>()
    };
    if content.ends_with('\n') {
        let _ = lines.pop();
    }
    let max_bytes = effective_read_max_bytes();
    let max_lines = effective_read_max_lines();
    if lines.first().is_some_and(|line| line.len() > max_bytes) {
        return ContractReadWindow {
            content: String::new(),
            shown_lines: 0,
            truncated_by_bytes: true,
            truncated_by_lines: false,
            first_line_too_large: true,
        };
    }

    if lines.len() <= max_lines && content.len() <= max_bytes {
        return ContractReadWindow {
            content: content.to_string(),
            shown_lines: lines.len(),
            truncated_by_bytes: false,
            truncated_by_lines: false,
            first_line_too_large: false,
        };
    }

    let mut kept = Vec::new();
    let mut bytes = 0usize;
    let mut truncated_by_bytes = false;
    for line in lines.iter().take(max_lines) {
        let next = line.len() + usize::from(!kept.is_empty());
        if bytes.saturating_add(next) > max_bytes {
            truncated_by_bytes = true;
            break;
        }
        kept.push(*line);
        bytes += next;
    }
    let shown_lines = kept.len();
    ContractReadWindow {
        content: kept.join("\n"),
        shown_lines,
        truncated_by_bytes,
        truncated_by_lines: !truncated_by_bytes,
        first_line_too_large: false,
    }
}

/// Tool for reading UTF-8 files from the workspace.
pub struct ReadFileTool;

impl ReadFileTool {
    /// Execute the lowercase `read` primitive without leaking the hidden
    /// Codewhale hash/snapshot protocol into its small-contract-shaped model contract.
    pub(super) async fn execute_contract_read(
        input: Value,
        context: &ToolContext,
    ) -> Result<RichToolResult, ToolError> {
        reject_primitive_unknown(&input, "read", &["path", "offset", "limit"])?;
        let path_str = required_str(&input, "path")?;
        let offset = contract_line_number(&input, "offset")?;
        let limit = contract_line_number(&input, "limit")?;
        // S1/F2: check the caller's own spelling BEFORE `resolve_path`
        // canonicalizes it. A workspace symlink `notes.txt` -> a denied vault
        // file resolves to the secret's absolute location, and a denial raised
        // only on the resolved path would name that location in the error —
        // answering the very question ("where is the secret?") the read was
        // probing for. `read_guard::check` canonicalizes internally, so the
        // raw spelling still matches by its target; the resolved check after
        // `resolve_path` stays as defense in depth for callers whose process
        // cwd is not the workspace.
        enforce_read_denylist(Path::new(path_str), "read")?;
        let file_path = context.resolve_path(path_str)?;
        if is_codewhale_credential_path(&file_path) {
            return Err(ToolError::permission_denied(
                "read cannot expose Codewhale configuration or credential-store files; use `codewhale config list` or `codewhale auth status` for safe inspection",
            ));
        }
        enforce_read_denylist(&file_path, "read")?;
        check_file_operation_cancelled(context)?;
        let bytes = fs::read(&file_path).map_err(|error| {
            ToolError::execution_failed(format!("Failed to read {}: {error}", file_path.display()))
        })?;
        check_file_operation_cancelled(context)?;
        if let Some(mime_type) = primitive_image_mime(&bytes) {
            let prepared = crate::image_attach::prepare_tool_image_bytes(&bytes, mime_type);
            context.note_file_read(&file_path);
            return Ok(RichToolResult::with_content_blocks(
                ToolResult::success(prepared.note).with_metadata(json!({
                    "evidence_routing": "inline"
                })),
                prepared.block.into_iter().collect(),
            ));
        }

        // The small-contract reader decodes non-image buffers as UTF-8 text with replacement
        // characters instead of refusing the whole read on one invalid byte.
        let text = String::from_utf8_lossy(&bytes);
        let all_lines = text.split('\n').collect::<Vec<_>>();
        let requested_offset = offset.unwrap_or(1);
        let start = requested_offset.saturating_sub(1);
        if start >= all_lines.len() {
            return Err(ToolError::execution_failed(format!(
                "Offset {requested_offset} is beyond end of file ({} lines total)",
                all_lines.len()
            )));
        }

        let available = &all_lines[start..];
        let selected = match limit {
            Some(limit) => &available[..available.len().min(limit)],
            None => available,
        };
        let selected_content = selected.join("\n");
        let window = contract_read_window(&selected_content);
        let first_display = start + 1;
        let mut output = if window.first_line_too_large {
            let size = selected.first().map_or(0, |line| line.len());
            format!(
                "[Line {first_display} is {}, exceeds {} limit. Use bash: sed -n '{first_display}p' {path_str} | head -c {READ_MAX_BYTES}]",
                contract_format_size(size),
                contract_format_size(READ_MAX_BYTES)
            )
        } else {
            window.content
        };

        if !window.first_line_too_large && (window.truncated_by_bytes || window.truncated_by_lines)
        {
            let last_display = first_display + window.shown_lines.saturating_sub(1);
            let next_offset = last_display + 1;
            if window.truncated_by_bytes {
                output.push_str(&format!(
                    "\n\n[Showing lines {first_display}-{last_display} of {} (50KB limit). Use offset={next_offset} to continue.]",
                    all_lines.len()
                ));
            } else {
                output.push_str(&format!(
                    "\n\n[Showing lines {first_display}-{last_display} of {}. Use offset={next_offset} to continue.]",
                    all_lines.len()
                ));
            }
        } else if limit.is_some() {
            let consumed = selected.len();
            if start + consumed < all_lines.len() {
                let remaining = all_lines.len() - (start + consumed);
                let next_offset = start + consumed + 1;
                output.push_str(&format!(
                    "\n\n[{remaining} more lines in file. Use offset={next_offset} to continue.]"
                ));
            }
        }

        // This internal observation keeps hidden legacy edit replay working,
        // but no hash or read-before-edit ceremony reaches the lowercase
        // schema or result.
        context.note_file_read(&file_path);
        Ok(RichToolResult::plain(
            ToolResult::success(output).with_metadata(json!({
                "evidence_routing": "inline"
            })),
        ))
    }
}

#[async_trait]
impl ToolSpec for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 file from the workspace. Use this instead of `cat`, `head`, `tail`, or `sed -n '..p'` in `Bash` — it's faster, sandbox-aware, and skips the approval prompt. Plain text is returned as-is and records the file snapshot required before `edit` will make a narrow in-place edit. Text reads report the whole file's `content_hash=\"sha256:…\"`; pass that value back as `expected_hash` on a later `write`, `edit`, or `patch` to have the write refused if the file changed in between. Codewhale config files and file-backed credential stores cannot be read with this tool; use `codewhale config list` or `codewhale auth status` for safe inspection. PDFs are text-extracted when the optional `pdftotext` executable (Poppler) is installed. Image screenshots are OCR-extracted when local OCR is available. Cannot read other non-PDF binaries.\n\nFor large files, use `start_line` and `max_lines` to read in chunks. By default, returns up to 500 lines or 16KB, whichever comes first. If `truncated=\"true\"` and `next_start_line` is present, continue reading from there; a byte-limited window instead shows head + tail with a `[CONTENT TRUNCATED]` marker and its note says how to narrow the range. For PDFs, use `pages` instead — `start_line`/`max_lines` only apply to text files."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file (relative to workspace or absolute). Alias: `file_path`"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Starting line (1-based, default 1). Aliases: `offset`, `line_offset`"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum lines to return (default 500, max 500; a 16KB byte budget applies regardless). Aliases: `limit`, `n_lines`"
                },
                "pages": {
                    "type": "string",
                    "description": "PDF only: page range to extract, e.g. \"1-5\" or \"10\". Ignored for non-PDF files."
                }
            },
            "required": ["path"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut input = input;
        apply_param_aliases(&mut input, PATH_ALIASES, "File read")?;
        apply_param_aliases(&mut input, READ_ALIASES, "File read")?;
        READ_PARAMS.reject_unknown(&input)?;

        let path_str = required_str(&input, "path")?;
        // S1/F2: raw spelling first, resolved path after — see the matching
        // comment in `execute_contract_read`. Only the raw-spelling denial can
        // promise an error that never names the symlink target's location.
        enforce_read_denylist(Path::new(path_str), "read_file")?;
        let file_path = context.resolve_path(path_str)?;
        if is_codewhale_credential_path(&file_path) {
            return Err(ToolError::permission_denied(
                "File `read` cannot expose Codewhale configuration or credential-store files; use `codewhale config list` or `codewhale auth status` for safe inspection",
            ));
        }
        enforce_read_denylist(&file_path, "read_file")?;
        let pages = optional_str(&input, "pages")?;

        if let Some(result) = read_pdf_if_detected(
            &file_path,
            pages,
            super::pdf::PdfTextCommand::system(context.cancel_token.as_ref()),
        )
        .await?
        {
            return Ok(result);
        }
        if is_image_for_ocr(&file_path) {
            return read_image_via_ocr(&file_path, path_str);
        }

        // Open before parameter parsing so a missing file keeps the
        // historical "Failed to read …" error shape regardless of the other
        // arguments.
        let file = fs::File::open(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;
        let file_bytes = file.metadata().map(|meta| meta.len()).unwrap_or(u64::MAX);

        let explicit_range = input
            .get("start_line")
            .or_else(|| input.get("max_lines"))
            .is_some();

        // Small-file fast path. Only applies when the caller didn't pass an
        // explicit range — otherwise an explicit `start_line = 5` on a
        // tiny file would silently ignore the request.
        if !explicit_range && file_bytes <= SMALL_FILE_BYTES as u64 {
            drop(file);
            let contents = fs::read_to_string(&file_path).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to read {}: {}",
                    file_path.display(),
                    e
                ))
            })?;
            context.note_file_read(&file_path);

            let total_lines = contents.lines().count();
            if total_lines <= SMALL_FILE_LINES {
                // The whole file is in hand, so hash it directly rather than
                // re-reading it. Prefixed as a header line because this branch
                // returns the contents unwrapped — there is no `<file …>` tag
                // to hang the attribute on.
                let hash = content_hash(contents.as_bytes());
                let body = format!("{}{contents}", content_hash_header(&hash));
                return Ok(ToolResult::success(body).with_metadata(json!({
                    "evidence_routing": "inline",
                    "content_hash": hash
                })));
            }

            // Small in bytes but too many lines: render the default window
            // straight from the in-memory contents.
            let hash = content_hash(contents.as_bytes());
            let window: Vec<String> = contents
                .lines()
                .take(DEFAULT_READ_LINES)
                .map(str::to_string)
                .collect();
            return Ok(render_line_window(
                path_str,
                &window,
                total_lines,
                1,
                DEFAULT_READ_LINES,
                Some(hash.as_str()),
            ));
        }

        // Strict types (2026-08-04 review): a `start_line:"1200"` string or a
        // negative/float value used to silently fall back to the defaults —
        // returning the head of the file instead of the window the model
        // asked for, the exact wrong-answer-shaped-like-a-right-one this
        // action's alias/unknown-parameter hardening exists to prevent.
        let start_line = match optional_u64(&input, "start_line", 1)? {
            0 => {
                return Err(ToolError::invalid_input(
                    "start_line must be 1-based and greater than 0".to_string(),
                ));
            }
            v => usize::try_from(v).map_err(|_| {
                ToolError::invalid_input(
                    "start_line exceeds platform addressable range".to_string(),
                )
            })?,
        };

        let max_lines = match optional_u64(&input, "max_lines", DEFAULT_READ_LINES as u64)? {
            0 => {
                return Err(ToolError::invalid_input(
                    "max_lines must be greater than 0".to_string(),
                ));
            }
            v => {
                let converted = usize::try_from(v).map_err(|_| {
                    ToolError::invalid_input(
                        "max_lines exceeds platform addressable range".to_string(),
                    )
                })?;
                std::cmp::min(converted, HARD_MAX_READ_LINES)
            }
        };

        // Bounded read for ranged/large files: skip and take lines through a
        // BufReader instead of materializing the whole file. The stream still
        // runs to EOF so the total line count and whole-file UTF-8 validation
        // match the historical read_to_string behavior.
        let (window, total_lines) =
            read_window_streaming(file, start_line, max_lines).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to read {}: {}",
                    file_path.display(),
                    e
                ))
            })?;
        context.note_file_read(&file_path);

        // The window is a slice; the guard needs the whole file. A second
        // streaming pass digests the rest without ever materializing it. A
        // failure here only costs the guard — the read itself already
        // succeeded, so the window is still returned, just without a hash to
        // pass back to `edit`. Special files are skipped: reopening a FIFO or
        // device can block indefinitely (or re-consume a one-shot stream),
        // and a stream has no stable content an edit guard could pin.
        let hash = match fs::metadata(&file_path) {
            Ok(meta) if meta.is_file() => hash_file_streaming(&file_path).ok(),
            _ => None,
        };

        // `start_line > total_lines` is not an error — it lets the model
        // page past the end without raising. Returns an empty-content
        // sentinel so subsequent reads can stop.
        if start_line > total_lines {
            let hash_attr = hash
                .as_deref()
                .map(|hash| format!(" content_hash=\"{hash}\""))
                .unwrap_or_default();
            let output = format!(
                "<file path=\"{path_str}\" total_lines=\"{total_lines}\" shown_lines=\"none\" truncated=\"false\"{hash_attr}>\n\
                 \n\
                 [NO CONTENT] start_line {start_line} is beyond total_lines {total_lines}.\n\
                 </file>"
            );
            return Ok(ToolResult::success(output).with_metadata(json!({
                "evidence_routing": "inline",
                "content_hash": hash
            })));
        }

        Ok(render_line_window(
            path_str,
            &window,
            total_lines,
            start_line,
            max_lines,
            hash.as_deref(),
        ))
    }
}

// Bounded output for large files. The small-file fast path keeps the
// historical "return contents unchanged" behavior so existing flows
// (small configs, single source files, etc.) don't suddenly start
// seeing wrapped output. Once a file is large or the caller asks
// for an explicit range, we switch to a numbered, line-tagged
// window with continuation hints so the model can page through
// without re-loading the entire file on every turn. Harvested
// from PR #1451 by @Oliver-ZPLiu, closes part of #1450.
// One bound, not two competing ones. The real cost of a read is BYTES of
// context, and `MAX_VISIBLE_BYTES` already enforces that. A separate 200-line
// default fired long before the byte budget on any prose file — a 229-line,
// 12 KB document truncated at line 200 with a third of the budget unspent,
// costing a second round trip to fetch 29 lines. The line cap now only guards
// pathologically short lines, where 500 lines is still a small read.
const DEFAULT_READ_LINES: usize = HARD_MAX_READ_LINES;
const HARD_MAX_READ_LINES: usize = 500;
const MAX_VISIBLE_BYTES: usize = 16 * 1024;
const SMALL_FILE_LINES: usize = HARD_MAX_READ_LINES;
const SMALL_FILE_BYTES: usize = 16 * 1024;

/// Stream a line window out of `file`: skip `start_line - 1` lines, collect
/// up to `max_lines`, then keep counting (and validating UTF-8) to EOF.
/// Returns the collected window plus the total line count. Only the window
/// is ever held in memory.
fn read_window_streaming(
    file: fs::File,
    start_line: usize,
    max_lines: usize,
) -> std::io::Result<(Vec<String>, usize)> {
    use std::io::BufRead;

    let mut reader = std::io::BufReader::new(file);
    let mut raw: Vec<u8> = Vec::new();
    let mut window: Vec<String> = Vec::new();
    let mut total_lines = 0usize;
    let start_idx = start_line - 1;

    loop {
        raw.clear();
        let n = reader.read_until(b'\n', &mut raw)?;
        if n == 0 {
            break;
        }
        // Mirror `str::lines`: strip the trailing '\n', and a '\r' only when
        // it directly precedes that '\n'.
        let mut end = raw.len();
        if raw[..end].ends_with(b"\n") {
            end -= 1;
            if raw[..end].ends_with(b"\r") {
                end -= 1;
            }
        }
        // Validate every line so invalid UTF-8 anywhere in the file fails
        // exactly like the previous whole-file read_to_string did.
        let line = std::str::from_utf8(&raw[..end]).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })?;
        if total_lines >= start_idx && window.len() < max_lines {
            window.push(line.to_string());
        }
        total_lines += 1;
    }

    Ok((window, total_lines))
}

/// Marker placed between the retained head and tail when a read window is
/// truncated by the byte budget. Mirrors qwen-code's truncation style so the
/// model sees both ends of the range.
const BYTE_TRUNCATION_SEPARATOR: &str = "\n\n---\n... [CONTENT TRUNCATED] ...\n---\n\n";

/// Split `content` into a head of at most `head_budget` bytes and a tail that
/// fills the remainder of `total_budget` (separator accounted for). Never
/// overlaps and never splits mid-codepoint. Style matches qwen-code:
/// `head_budget = total_budget / 5`.
fn head_tail_for_budget(content: &str, total_budget: usize) -> (String, String) {
    let head_budget = (total_budget / 5).max(1);
    let head_end = (0..=head_budget.min(content.len()))
        .rev()
        .find(|&i| content.is_char_boundary(i))
        .unwrap_or(0);
    let sep_len = BYTE_TRUNCATION_SEPARATOR.len();
    let tail_budget = total_budget
        .saturating_sub(head_end)
        .saturating_sub(sep_len)
        .max(1);
    let tail_floor = content.len().saturating_sub(tail_budget).max(head_end);
    let tail_start = (tail_floor..=content.len())
        .find(|&i| content.is_char_boundary(i))
        .unwrap_or(content.len());
    (
        content[..head_end].to_string(),
        content[tail_start..].to_string(),
    )
}

/// Render a collected line window into the `<file …>` wrapper used for
/// ranged/large reads. `window` must hold the lines for
/// `start_line..start_line + max_lines` (clamped to EOF).
fn render_line_window(
    path_str: &str,
    window: &[String],
    total_lines: usize,
    start_line: usize,
    max_lines: usize,
    content_hash: Option<&str>,
) -> ToolResult {
    let zero_based_start = start_line - 1;
    let zero_based_end = std::cmp::min(zero_based_start + max_lines, total_lines);
    let shown_first = start_line;
    let shown_last = zero_based_end; // 1-based inclusive line number of the last shown line

    let mut numbered = String::new();
    for (offset, line) in window.iter().enumerate() {
        let line_no = start_line + offset;
        numbered.push_str(&format!("{line_no:>6}│ {line}\n"));
    }

    // UTF-8-safe byte truncation of the rendered range. Qwen-style: keep a
    // short head (budget/5) plus the matching tail so the model sees both
    // ends of a long range. The full file already lives at `path_str` — the
    // recovery note names that absolute/workspace path for a re-read.
    let visible_bytes =
        crate::tools::large_output_router::WorkshopConfig::active_read_result_max_bytes()
            .map(|n| n.clamp(MAX_VISIBLE_BYTES, READ_RESULT_ABSOLUTE_MAX_BYTES))
            .unwrap_or(MAX_VISIBLE_BYTES);
    let truncated_by_bytes = numbered.len() > visible_bytes;
    let shown_content = if truncated_by_bytes {
        let (head, tail) = head_tail_for_budget(&numbered, visible_bytes);
        format!("{head}{BYTE_TRUNCATION_SEPARATOR}{tail}")
    } else {
        numbered
    };

    let truncated_by_lines = zero_based_end < total_lines;
    let truncated = truncated_by_lines || truncated_by_bytes;
    let next_start = zero_based_end + 1;

    let mut attrs = format!(
        "path=\"{path_str}\" total_lines=\"{total_lines}\" shown_lines=\"{shown_first}-{shown_last}\" truncated=\"{truncated}\""
    );
    if truncated_by_lines {
        attrs.push_str(&format!(" next_start_line=\"{next_start}\""));
    }
    // Hashes the whole file, not the shown window — a partial read still
    // yields a guard the model can pass to `edit`/`patch`.
    if let Some(hash) = content_hash {
        attrs.push_str(&format!(" content_hash=\"{hash}\""));
    }

    let mut output = format!("<file {attrs}>\n{shown_content}");
    if truncated_by_lines {
        output.push_str(&format!(
            "\n[TRUNCATED] Showing lines {shown_first}-{shown_last} of {total_lines}. To continue, call read with path=\"{path_str}\" offset={next_start} limit={max_lines}\n"
        ));
    }
    if truncated_by_bytes {
        if shown_first == shown_last {
            // One line alone exceeds the byte budget: no start_line/max_lines
            // combination can ever reveal the elided middle, so the note must
            // not pretend otherwise — name the escape hatch that works.
            output.push_str(&format!(
                "\n[TRUNCATED] Line {shown_first} alone exceeds 50KB; showing its head + tail. No line window can reveal the middle of one line — use a searched shell slice when needed.\n"
            ));
        } else {
            let narrower = (shown_last - shown_first).div_ceil(2).max(1);
            output.push_str(&format!(
                "\n[TRUNCATED] The selected range exceeded 50KB; showing head + tail of lines {shown_first}-{shown_last}. Re-read narrower windows to see the middle, e.g. offset={shown_first} limit={narrower}, then advance offset.\n"
            ));
        }
    }
    output.push_str("</file>");

    // The file tool self-bounds at 50 KiB and carries its own continuation
    // contract (`next_start_line`), so the large-output spillover envelope
    // must never re-wrap a read result with a second, weaker truncation.
    ToolResult::success(output).with_metadata(json!({
        "evidence_routing": "inline",
        "content_hash": content_hash
    }))
}

fn read_image_via_ocr(path: &Path, requested_path: &str) -> Result<ToolResult, ToolError> {
    let text = crate::tools::image_ocr::ocr_image_path(path)?;
    Ok(ToolResult::success(format!(
        "<image_ocr path=\"{requested_path}\">\n{text}\n</image_ocr>"
    )))
}

/// Detect an existing PDF by extension or by sniffing `%PDF` magic bytes.
fn is_pdf(path: &Path) -> Result<bool, ToolError> {
    let extension_matches = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    let mut file = fs::File::open(path).map_err(|error| {
        ToolError::execution_failed(format!("Failed to read {}: {error}", path.display()))
    })?;
    if extension_matches {
        return Ok(true);
    }
    let mut buf = [0u8; 4];
    use std::io::Read;
    Ok(file.read_exact(&mut buf).is_ok() && &buf == b"%PDF")
}

fn is_image_for_ocr(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp"
            )
        })
}

fn parse_pages_arg(spec: &str) -> Option<(u32, u32)> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((a, b)) = trimmed.split_once('-') {
        let start: u32 = a.trim().parse().ok()?;
        let end: u32 = b.trim().parse().ok()?;
        if start == 0 || end < start {
            return None;
        }
        Some((start, end))
    } else {
        let n: u32 = trimmed.parse().ok()?;
        if n == 0 {
            return None;
        }
        Some((n, n))
    }
}

/// Clean PDF-extracted text for TUI display: collapse consecutive blank
/// lines (more than 1 becomes 1), replace NUL bytes with U+FFFD, replace
/// non-breaking spaces with regular spaces, and trim trailing whitespace
/// on each line. Produces output that won't clutter the transcript with
/// vertical gaps or invisible control characters.
fn clean_pdf_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank_run = 0usize;
    let mut any_content = false;
    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run = blank_run.saturating_add(1);
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            any_content = true;
            // Push cleaned characters directly — avoids a per-line
            // temporary String allocation.
            for c in trimmed.chars() {
                match c {
                    '\0' => out.push('\u{FFFD}'),
                    '\u{A0}' => out.push(' '),
                    other => out.push(other),
                }
            }
            out.push('\n');
        }
    }
    // Trim leading blank lines only — don't use str::trim() which
    // would also strip intentional indentation (e.g. centred titles).
    if any_content {
        let start = out.find(|c: char| c != '\n').unwrap_or(0);
        // Walk back from end to find the last non-newline character.
        let end = out.rfind(|c: char| c != '\n').map_or(out.len(), |i| {
            i + out[i..].chars().next().map_or(1, |c| c.len_utf8())
        });
        out[start..end].to_string()
    } else {
        String::new()
    }
}

async fn read_pdf_if_detected(
    path: &Path,
    pages: Option<&str>,
    command: super::pdf::PdfTextCommand<'_>,
) -> Result<Option<ToolResult>, ToolError> {
    if !is_pdf(path)? {
        return Ok(None);
    }
    // Validate the `pages` spec once, up front, so both extractor paths
    // surface the same error shape on bad input.
    let page_range = match pages {
        Some(spec) => match parse_pages_arg(spec) {
            Some((start, end)) => Some((start, end)),
            None => {
                return Err(ToolError::invalid_input(format!(
                    "invalid `pages` value `{spec}` (expected `N` or `N-M`, e.g. `1-5`)"
                )));
            }
        },
        None => None,
    };

    read_pdf_with_command(path, page_range, command)
        .await
        .map(Some)
}

async fn read_pdf_with_command(
    path: &Path,
    page_range: Option<(u32, u32)>,
    command: super::pdf::PdfTextCommand<'_>,
) -> Result<ToolResult, ToolError> {
    let text = super::pdf::extract_path(path, page_range, command)
        .await
        .map_err(super::pdf::into_tool_error)?;
    Ok(ToolResult::success(clean_pdf_text(&text)))
}

// === WriteFileTool ===

/// Tool for writing UTF-8 files to the workspace.
pub struct WriteFileTool;

impl WriteFileTool {
    /// Execute the small-contract-shaped lowercase writer. Compatibility-only hash
    /// arguments remain on the hidden `write_file`/`File` paths.
    pub(super) async fn execute_contract_write(
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        reject_primitive_unknown(&input, "write", &["path", "content"])?;
        let path_str = required_str(&input, "path")?;
        let file_content = required_str(&input, "content")?;
        let file_path = context.resolve_path(path_str)?;
        let mutation_guard = acquire_file_mutation(&file_path, context).await?;
        check_file_operation_cancelled(context)?;

        let existed_before = file_path.exists();
        let prior_bytes = if existed_before {
            fs::read(&file_path).unwrap_or_default()
        } else {
            Vec::new()
        };
        let prior_contents = String::from_utf8_lossy(&prior_bytes);

        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ToolError::execution_failed(format!(
                    "Failed to create directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        check_file_operation_cancelled(context)?;
        crate::utils::write_atomic_workspace(&file_path, file_content.as_bytes()).map_err(
            |error| {
                ToolError::execution_failed(format!(
                    "Failed to write {}: {error}",
                    file_path.display()
                ))
            },
        )?;
        check_file_operation_cancelled(context)?;
        context.note_file_read(&file_path);
        drop(mutation_guard);

        let outcome = if existed_before { "updated" } else { "created" };
        let utf16_units = file_content.encode_utf16().count();
        Ok(contract_mutation_result(
            context,
            &file_path,
            path_str,
            prior_contents.as_ref(),
            file_content,
            outcome,
            format!("Successfully wrote {utf16_units} bytes to {path_str}"),
        )
        .await)
    }
}

#[async_trait]
impl ToolSpec for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Write content to a UTF-8 file in the workspace. Use this instead of heredocs (`cat <<EOF > file`) or `echo > file` in `Bash` — diffs render inline and approval is handled cleanly. Creates or overwrites; parent directories are auto-created. Pass `expected_hash` (the `content_hash` from a prior `read`) to have the overwrite refused if the file changed since that read."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file. Alias: `file_path`"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                },
                "expected_hash": {
                    "type": "string",
                    "description": EXPECTED_HASH_DESCRIPTION
                }
            },
            "required": ["path", "content"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Suggest
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut input = input;
        apply_param_aliases(&mut input, PATH_ALIASES, "File write")?;
        WRITE_PARAMS.reject_unknown(&input)?;

        let path_str = required_str(&input, "path")?;
        let file_content = required_str(&input, "content")?;
        let expected_hash = optional_str(&input, "expected_hash")?;

        let file_path = context.resolve_path(path_str)?;

        // Snapshot the existing contents (if any) before we overwrite — used
        // to render an inline diff in the tool result.
        let existed_before = file_path.exists();
        let prior_contents = if existed_before {
            fs::read_to_string(&file_path).unwrap_or_default()
        } else {
            String::new()
        };

        // Content-hash guard (#3979), checked against the same snapshot the
        // diff is rendered from and before any directory or file is touched.
        if let Some(expected) = expected_hash {
            if !existed_before {
                // A hash describes a file that was read. Guarding a create is
                // a contradiction, and silently creating the file anyway would
                // defeat the guard the caller asked for — fail closed.
                return Err(ToolError::execution_failed(format!(
                    "File `write` refused: expected_hash was supplied but {path_str} does not exist, so there is no snapshot to verify and nothing was written. Recovery: drop `expected_hash` to create the file, or read the intended path first."
                )));
            }
            verify_expected_hash(Some(expected), prior_contents.as_bytes(), "write", path_str)?;
        }

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                ToolError::execution_failed(format!(
                    "Failed to create directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        crate::utils::write_atomic_workspace(&file_path, file_content.as_bytes()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;
        context.note_file_read(&file_path);

        let display = file_path.display().to_string();
        let diff = make_unified_diff(&display, &prior_contents, file_content);
        let summary = if existed_before {
            format!("Wrote {} bytes to {}", file_content.len(), display)
        } else {
            format!("Created {} ({} bytes)", display, file_content.len())
        };
        let body = if diff.is_empty() {
            format!("{summary}\n(no changes)")
        } else {
            format!("{diff}\n{summary}")
        };

        // Append LSP diagnostics for the written file when enabled (#428).
        let diag_block = lsp_diagnostics_for_paths(context, &[file_path]).await;
        let full_body = if diag_block.is_empty() {
            body
        } else {
            format!("{body}\n{diag_block}")
        };

        let outcome = if existed_before { "updated" } else { "created" };
        // Keep the execution-owned receipt workspace-relative even though the
        // legacy model-facing output above retains its resolved-path wording.
        let receipt_diff = make_unified_diff(path_str, &prior_contents, file_content);
        Ok(ToolResult::success(full_body).with_metadata(json!({
            "event": "file.mutation",
            "mutation": {
                "diff": receipt_diff,
                "files": [{ "path": path_str, "outcome": outcome }],
                "renames": []
            }
        })))
    }
}

// === EditFileTool ===

/// Tool for search/replace editing of files.
pub struct EditFileTool;

#[derive(Clone, Debug)]
struct ContractEdit {
    index: usize,
    old_text: String,
    new_text: String,
}

#[derive(Clone, Debug)]
struct ResolvedContractEdit {
    index: usize,
    start: usize,
    end: usize,
    replacement: String,
}

fn normalize_contract_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn contract_line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(index) if index > 0 && text.as_bytes()[index - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

fn restore_contract_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// Fallback matching view used only after a literal match fails. It follows
/// The small-contract normalization categories while leaving the public schema as
/// exact-text replacement rather than teaching a second edit mode.
fn normalize_contract_fuzzy(text: &str) -> String {
    let compatible = text.nfkc().collect::<String>();
    compatible
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .map(|ch| match ch {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

fn text_matches(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    haystack
        .match_indices(needle)
        .map(|(start, matched)| (start, start + matched.len()))
        .collect()
}

fn contract_edit_not_found(path: &str, index: usize, total: usize) -> ToolError {
    if total == 1 {
        ToolError::execution_failed(format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        ))
    } else {
        ToolError::execution_failed(format!(
            "Could not find edits[{index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        ))
    }
}

fn contract_edit_duplicate(path: &str, index: usize, total: usize, matches: usize) -> ToolError {
    if total == 1 {
        ToolError::execution_failed(format!(
            "Found {matches} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        ))
    } else {
        ToolError::execution_failed(format!(
            "Found {matches} occurrences of edits[{index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        ))
    }
}

fn prepare_contract_edit_input(mut input: Value) -> Result<Value, ToolError> {
    let object = input
        .as_object_mut()
        .ok_or_else(|| ToolError::invalid_input("edit input must be an object"))?;
    if let Some(Value::String(encoded)) = object.get("edits")
        && let Ok(decoded) = serde_json::from_str::<Value>(encoded)
        && decoded.is_array()
    {
        object.insert("edits".to_string(), decoded);
    }

    let legacy_old = object
        .get("oldText")
        .and_then(Value::as_str)
        .map(str::to_string);
    let legacy_new = object
        .get("newText")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let (Some(old_text), Some(new_text)) = (legacy_old, legacy_new) {
        let legacy = json!({"oldText": old_text, "newText": new_text});
        let mut edits = object
            .get("edits")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        edits.push(legacy);
        object.insert("edits".to_string(), Value::Array(edits));
        object.remove("oldText");
        object.remove("newText");
    }
    Ok(input)
}

fn parse_contract_edits(input: &Value) -> Result<Vec<ContractEdit>, ToolError> {
    let raw = input
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::invalid_input("edits must be an array"))?;
    if raw.is_empty() {
        return Err(ToolError::invalid_input(
            "edit requires at least one replacement in edits",
        ));
    }
    raw.iter()
        .enumerate()
        .map(|(index, edit)| {
            reject_primitive_unknown(edit, &format!("edits[{index}]"), &["oldText", "newText"])?;
            let old_text = required_str(edit, "oldText")?;
            let new_text = required_str(edit, "newText")?;
            if old_text.is_empty() {
                return Err(ToolError::invalid_input(format!(
                    "edits[{index}].oldText must not be empty"
                )));
            }
            Ok(ContractEdit {
                index,
                old_text: normalize_contract_line_endings(old_text),
                new_text: normalize_contract_line_endings(new_text),
            })
        })
        .collect()
}

fn apply_resolved_edits(base: &str, edits: &[ResolvedContractEdit], offset: usize) -> String {
    let mut updated = base.to_string();
    for edit in edits.iter().rev() {
        updated.replace_range(
            edit.start.saturating_sub(offset)..edit.end.saturating_sub(offset),
            &edit.replacement,
        );
    }
    updated
}

fn lines_with_endings(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split_inclusive('\n').collect()
    }
}

fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut offset = 0usize;
    lines_with_endings(text)
        .into_iter()
        .map(|line| {
            let span = (offset, offset + line.len());
            offset = span.1;
            span
        })
        .collect()
}

fn touched_line_range(
    spans: &[(usize, usize)],
    edit: &ResolvedContractEdit,
) -> Result<(usize, usize), ToolError> {
    let start = spans
        .iter()
        .position(|(line_start, line_end)| edit.start >= *line_start && edit.start < *line_end)
        .ok_or_else(|| ToolError::execution_failed("edit match fell outside the file"))?;
    let mut end = start;
    while end < spans.len() && spans[end].1 < edit.end {
        end += 1;
    }
    if end >= spans.len() {
        return Err(ToolError::execution_failed(
            "edit match fell outside the file",
        ));
    }
    Ok((start, end + 1))
}

fn apply_fuzzy_edits_preserving_other_lines(
    original: &str,
    normalized: &str,
    edits: &[ResolvedContractEdit],
) -> Result<String, ToolError> {
    let original_lines = lines_with_endings(original);
    let spans = line_spans(normalized);
    if original_lines.len() != spans.len() {
        return Err(ToolError::execution_failed(
            "fuzzy edit could not preserve the file's untouched lines",
        ));
    }

    #[derive(Debug)]
    struct Group {
        start_line: usize,
        end_line: usize,
        edits: Vec<ResolvedContractEdit>,
    }

    let mut groups: Vec<Group> = Vec::new();
    for edit in edits {
        let (start_line, end_line) = touched_line_range(&spans, edit)?;
        if let Some(group) = groups.last_mut()
            && start_line < group.end_line
        {
            group.end_line = group.end_line.max(end_line);
            group.edits.push(edit.clone());
        } else {
            groups.push(Group {
                start_line,
                end_line,
                edits: vec![edit.clone()],
            });
        }
    }

    let mut result = String::new();
    let mut original_line = 0usize;
    for group in groups {
        for line in &original_lines[original_line..group.start_line] {
            result.push_str(line);
        }
        let group_start = spans[group.start_line].0;
        let group_end = spans[group.end_line - 1].1;
        result.push_str(&apply_resolved_edits(
            &normalized[group_start..group_end],
            &group.edits,
            group_start,
        ));
        original_line = group.end_line;
    }
    for line in &original_lines[original_line..] {
        result.push_str(line);
    }
    Ok(result)
}

fn apply_contract_edits(
    base: &str,
    edits: &[ContractEdit],
    path: &str,
) -> Result<String, ToolError> {
    let fuzzy_base = normalize_contract_fuzzy(base);
    let initial = edits
        .iter()
        .map(|edit| {
            if base.contains(&edit.old_text) {
                Ok(false)
            } else if fuzzy_base.contains(&normalize_contract_fuzzy(&edit.old_text)) {
                Ok(true)
            } else {
                Err(contract_edit_not_found(path, edit.index, edits.len()))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let use_fuzzy = initial.into_iter().any(|used| used);
    let replacement_base = if use_fuzzy { fuzzy_base.as_str() } else { base };

    let mut resolved = Vec::with_capacity(edits.len());
    for edit in edits {
        let exact = text_matches(replacement_base, &edit.old_text);
        let fuzzy_old = normalize_contract_fuzzy(&edit.old_text);
        let fuzzy_occurrences = text_matches(&fuzzy_base, &fuzzy_old).len();
        if fuzzy_occurrences > 1 {
            return Err(contract_edit_duplicate(
                path,
                edit.index,
                edits.len(),
                fuzzy_occurrences,
            ));
        }
        let matches = if exact.is_empty() {
            text_matches(replacement_base, &fuzzy_old)
        } else {
            exact
        };
        let Some(&(start, end)) = matches.first() else {
            return Err(contract_edit_not_found(path, edit.index, edits.len()));
        };
        if matches.len() > 1 {
            return Err(contract_edit_duplicate(
                path,
                edit.index,
                edits.len(),
                matches.len(),
            ));
        }
        resolved.push(ResolvedContractEdit {
            index: edit.index,
            start,
            end,
            replacement: edit.new_text.clone(),
        });
    }

    resolved.sort_by_key(|edit| (edit.start, edit.end));
    for pair in resolved.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(ToolError::execution_failed(format!(
                "edits[{}] and edits[{}] overlap in {path}; merge them or target separate regions",
                pair[0].index, pair[1].index
            )));
        }
    }

    let updated = if use_fuzzy {
        apply_fuzzy_edits_preserving_other_lines(base, replacement_base, &resolved)?
    } else {
        apply_resolved_edits(replacement_base, &resolved, 0)
    };
    if updated == base {
        return Err(ToolError::execution_failed(format!(
            "No changes made to {path}; the replacement produced identical content."
        )));
    }
    Ok(updated)
}

impl EditFileTool {
    pub(super) async fn execute_contract_edits(
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let input = prepare_contract_edit_input(input)?;
        reject_primitive_unknown(&input, "edit", &["path", "edits"])?;
        let path_str = required_str(&input, "path")?;
        let edits = parse_contract_edits(&input)?;
        let file_path = context.resolve_path(path_str)?;
        let mutation_guard = acquire_file_mutation(&file_path, context).await?;
        check_file_operation_cancelled(context)?;

        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&file_path)
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "Could not edit file {path_str}: target must be readable and writable ({error})"
                ))
            })?;
        check_file_operation_cancelled(context)?;
        let raw_bytes = fs::read(&file_path).map_err(|error| {
            ToolError::execution_failed(format!("Could not edit file {path_str}: {error}"))
        })?;
        check_file_operation_cancelled(context)?;
        let raw = String::from_utf8_lossy(&raw_bytes).into_owned();
        let (bom, without_bom) = raw
            .strip_prefix('\u{FEFF}')
            .map_or(("", raw.as_str()), |text| ("\u{FEFF}", text));
        let ending = contract_line_ending(without_bom);
        let normalized = normalize_contract_line_endings(without_bom);
        let updated = apply_contract_edits(&normalized, &edits, path_str)?;
        check_file_operation_cancelled(context)?;
        let final_content = format!("{bom}{}", restore_contract_line_endings(&updated, ending));

        crate::utils::write_atomic_workspace(&file_path, final_content.as_bytes()).map_err(
            |error| {
                ToolError::execution_failed(format!(
                    "Failed to write {}: {error}",
                    file_path.display()
                ))
            },
        )?;
        check_file_operation_cancelled(context)?;
        context.note_file_read(&file_path);
        drop(mutation_guard);

        Ok(contract_mutation_result(
            context,
            &file_path,
            path_str,
            &raw,
            &final_content,
            "updated",
            format!(
                "Successfully replaced {} block(s) in {path_str}.",
                edits.len()
            ),
        )
        .await)
    }
}

#[async_trait]
impl ToolSpec for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Replace text in a single file via exact search/replace after the file has been read with File `read` in this session. Use this instead of `sed -i` in `Bash` for one unambiguous in-place edit. `search` must match exactly one location by default; when no exact match is found the tool retries with leading-whitespace-tolerant fuzzy matching automatically. Returns a compact unified diff, not the full file. Pass `expected_hash` (the `content_hash` from that `read`) to have the edit refused, with the file untouched, if it changed in between. For structural, multi-block, or cross-file changes, use File `patch` or `write` instead."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file. Alias: `file_path`"
                },
                "search": {
                    "type": "string",
                    "description": "Exact text to search for, including whitespace, indentation, and newlines. Aliases: `old_string`, `old_str`, `oldText`"
                },
                "replace": {
                    "type": "string",
                    "description": "Text to replace with. Aliases: `new_string`, `new_str`, `newText`"
                },
                "expected_hash": {
                    "type": "string",
                    "description": EXPECTED_HASH_DESCRIPTION
                }
            },
            "required": ["path", "search", "replace"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Suggest
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        // Translate known cross-harness spellings (`old_string`/`new_string`,
        // `old_str`/`new_str`, …) onto `search`/`replace` first, then reject
        // whatever is left that we do not implement. #5209 required that a
        // mis-named edit never produce a success-shaped receipt for a file
        // that did not change; performing the edit the model unambiguously
        // asked for satisfies that more directly than refusing it did.
        let mut input = input;
        apply_param_aliases(&mut input, PATH_ALIASES, "File edit")?;
        apply_param_aliases(&mut input, EDIT_ALIASES, "File edit")?;
        EDIT_PARAMS.reject_unknown(&input)?;

        let path_str = required_str(&input, "path")?;
        let search = required_str(&input, "search")?;
        let replace = required_str(&input, "replace")?;
        let expected_hash = optional_str(&input, "expected_hash")?;

        if search == replace {
            // #5003 — long-text edits repeatedly failed here because the model
            // generated a `replace` identical to `search`. A bare "no change"
            // message gave no hint of the root cause, so the model retried the
            // same broken call. Spell out the failure and the recovery path.
            let char_count = search.chars().count();
            let line_count = search.lines().count();
            return Err(ToolError::invalid_input(format!(
                "search and replace are identical ({char_count} chars, {line_count} lines), so no change is possible. This usually means `replace` was copied verbatim from `search` instead of carrying the intended edits. Recovery: re-read the file with File action=\"read\", then retry with a `replace` that is genuinely different from `search`; for large multi-line rewrites prefer apply_patch with a unified diff."
            )));
        }
        if search.is_empty() {
            return Err(ToolError::invalid_input("search must not be empty"));
        }
        if let Some(reason) = edit_payload_looks_corrupted(search, replace) {
            return Err(ToolError::invalid_input(format!(
                "edit_file refused corrupted payload: {reason}. Recovery: re-read the file and retry with a complete replace (or use apply_patch for brace-heavy multi-line edits)."
            )));
        }

        let file_path = context.resolve_path(path_str)?;
        context.require_fresh_file_read(&file_path, path_str)?;

        let contents = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::execution_failed(format!("Failed to read {}: {}", file_path.display(), e))
        })?;

        // Content-hash guard (#3979). Verified against `contents` — the exact
        // snapshot every match below is computed from and that the write is
        // derived from — and before any search/replace work, so a stale hash
        // can never reach the filesystem regardless of what the search would
        // have matched.
        verify_expected_hash(expected_hash, contents.as_bytes(), "edit", path_str)?;

        // Models provide LF newlines even when the file on disk uses CRLF.
        // Match in a newline-normalized view, while retaining the sparse
        // positions where CR bytes were removed so only the original span is
        // replaced and the rest of the file stays byte-for-byte untouched.
        let (normalized_contents, crlf_positions) = normalize_crlf_with_positions(&contents);
        let normalized_search = normalize_crlf(search);
        let mut exact_ranges = normalized_contents
            .match_indices(normalized_search.as_ref())
            .map(|(start, matched)| (start, start + matched.len()));
        let first_exact_match = exact_ranges
            .next()
            .map(|range| map_normalized_range(range, crlf_positions.as_deref()));
        let exact_count = usize::from(first_exact_match.is_some()) + exact_ranges.count();

        let ((match_start, match_end), fuzz_kind) = if exact_count == 0 {
            // First fallback: tolerate indentation differences.
            let indent_matches = map_normalized_ranges(
                leading_whitespace_fuzzy_matches(
                    normalized_contents.as_ref(),
                    normalized_search.as_ref(),
                ),
                crlf_positions.as_deref(),
            );
            match indent_matches.as_slice() {
                [(start, end)] => ((*start, *end), Some("indentation")),
                [] => {
                    // Second fallback: tolerate typographic-punctuation
                    // drift (smart quotes, em-dashes, NBSP). Picks up the
                    // copy-paste failure mode where a browser/chat client
                    // silently substituted Unicode punctuation in for the
                    // ASCII the file actually contains.
                    let punct_matches = map_normalized_ranges(
                        punctuation_normalized_matches(
                            normalized_contents.as_ref(),
                            normalized_search.as_ref(),
                        ),
                        crlf_positions.as_deref(),
                    );
                    match punct_matches.as_slice() {
                        [] => {
                            // #5003 — the model could not tell why its search
                            // missed; show the first lines of the search text
                            // so it can compare against the file's contents.
                            return Err(ToolError::execution_failed(format!(
                                "Search string not found in {}. The search text starts with:\n{}\nRecovery: call File with action=\"read\" path=\"{path_str}\" to inspect the current contents, then retry with a search string copied from the file.",
                                file_path.display(),
                                preview_search_for_error(search),
                            )));
                        }
                        [(start, end)] => ((*start, *end), Some("punctuation")),
                        _ => {
                            return Err(ToolError::execution_failed(format!(
                                "File `edit` search is non-unique after punctuation normalization: matched {} locations in {}. Recovery: call File with action=\"read\" path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                                punct_matches.len(),
                                file_path.display()
                            )));
                        }
                    }
                }
                _ => {
                    return Err(ToolError::execution_failed(format!(
                        "File `edit` search is non-unique after indentation normalization: matched {} locations in {}. Recovery: call File with action=\"read\" path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                        indent_matches.len(),
                        file_path.display()
                    )));
                }
            }
        } else if exact_count > 1 {
            return Err(ToolError::execution_failed(format!(
                "File `edit` search is non-unique: matched {} locations in {}. \
                 Recovery: call File with action=\"read\" path=\"{path_str}\" and retry with surrounding lines that make the search unique.",
                exact_count,
                file_path.display()
            )));
        } else {
            let Some((start, end)) = first_exact_match else {
                return Err(ToolError::execution_failed(
                    "edit_file internal range accounting failed — refusing write",
                ));
            };
            let fuzz_kind = (&contents[start..end] != search).then_some("line endings");
            ((start, end), fuzz_kind)
        };

        let effective_replace =
            normalize_replacement_line_endings(replace, crlf_positions.is_some());
        let mut updated = contents.clone();
        updated.replace_range(match_start..match_end, &effective_replace);
        if updated == contents {
            return Err(ToolError::invalid_input(
                "search and replace resolve to identical file contents after line-ending normalization, no change intended",
            ));
        }

        if let Some(reason) = invalid_preprocessor_edit(&file_path, &contents, &updated) {
            return Err(ToolError::invalid_input(format!(
                "edit_file refused corrupted payload: {reason}. Recovery: re-read the file and retry with a complete replace (or use apply_patch for brace-heavy multi-line edits)."
            )));
        }

        // Fidelity: the intended replace text must appear in the updated buffer
        // (empty replace is a valid deletion). Catches host/tool bridges that
        // claim success after mangling the payload.
        if !effective_replace.is_empty() && !updated.contains(&effective_replace) {
            return Err(ToolError::execution_failed(
                "edit_file internal fidelity check failed: replace text missing from updated buffer — refusing write",
            ));
        }

        crate::utils::write_atomic_workspace(&file_path, updated.as_bytes()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write {}: {}", file_path.display(), e))
        })?;

        // #5209 — never emit a success receipt unless the on-disk write
        // actually applied. A fabricated "Replaced 1 occurrence" + diff is
        // worse than a hard error: models trust it and re-edit the same
        // span 3–5× before noticing nothing changed.
        let on_disk = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::execution_failed(format!(
                "Failed to verify write to {}: {}",
                file_path.display(),
                e
            ))
        })?;
        if on_disk != updated {
            return Err(ToolError::execution_failed(format!(
                "edit_file write verification failed for {}: on-disk contents do not match the applied edit — refusing success receipt",
                file_path.display()
            )));
        }

        context.note_file_read(&file_path);

        let display = file_path.display().to_string();
        let diff = make_unified_diff(&display, &contents, &updated);
        let fuzz_note = match fuzz_kind {
            Some("indentation") => " (fuzzy indentation match)",
            Some("punctuation") => {
                " (fuzzy punctuation match — typographic quotes/dashes normalized)"
            }
            Some("line endings") => " (CRLF/LF-normalized match)",
            Some(other) => other,
            None => "",
        };
        let summary = format!("Replaced 1 occurrence in {display}{fuzz_note}");
        let body = if diff.is_empty() {
            format!("{summary}\n(no textual changes)")
        } else {
            format!("{diff}\n{summary}")
        };

        // Append LSP diagnostics for the edited file when enabled (#428).
        let diag_block = lsp_diagnostics_for_paths(context, &[file_path]).await;
        let full_body = if diag_block.is_empty() {
            body
        } else {
            format!("{body}\n{diag_block}")
        };

        // The structured receipt uses the requested workspace path instead of
        // the resolved host path retained by the legacy model-facing body.
        let receipt_diff = make_unified_diff(path_str, &contents, &updated);
        Ok(ToolResult::success(full_body).with_metadata(json!({
            "event": "file.mutation",
            "mutation": {
                "diff": receipt_diff,
                "files": [{ "path": path_str, "outcome": "updated" }],
                "renames": []
            }
        })))
    }
}

/// Detect catastrophic argument corruption of brace-structured edits.
///
/// Models (and some host XML/JSON bridges) occasionally deliver a `replace`
/// payload where a multi-line `{ ... }` block collapsed to empty `[]` or `{}`
/// while `search` still contains the full structured original. Writing that
/// would brick Rust match arms / JSON objects. Fail closed with recovery text
/// instead of applying the mangled payload (dogfood 2026-07-24).
///
/// Unbalanced-to-unbalanced edits with the **same** brace/bracket delta are
/// legitimate (e.g. adding `});` inside a nested fragment). Only a *change*
/// in balance is treated as truncation/mangling. Empty-bracket collapse and
/// extreme-shrinkage guards remain.
fn edit_payload_looks_corrupted(search: &str, replace: &str) -> Option<&'static str> {
    let search_curly_open = search.matches('{').count();
    let search_curly_close = search.matches('}').count();
    let replace_curly_open = replace.matches('{').count();
    let replace_curly_close = replace.matches('}').count();
    let search_square_open = search.matches('[').count();
    let search_square_close = search.matches(']').count();
    let replace_square_open = replace.matches('[').count();
    let replace_square_close = replace.matches(']').count();

    let search_curly_delta = search_curly_open as i32 - search_curly_close as i32;
    let replace_curly_delta = replace_curly_open as i32 - replace_curly_close as i32;
    let search_square_delta = search_square_open as i32 - search_square_close as i32;
    let replace_square_delta = replace_square_open as i32 - replace_square_close as i32;

    // Same delta on both sides (including both unbalanced the same way) is
    // normal for fragment edits. Divergent deltas usually mean truncation.
    if search_curly_delta != replace_curly_delta {
        return Some(
            "search/replace change `{`/`}` brace balance — the tool-call arguments were likely truncated or mangled before apply",
        );
    }
    if search_square_delta != replace_square_delta {
        return Some(
            "search/replace change `[`/`]` bracket balance — the tool-call arguments were likely truncated or mangled before apply",
        );
    }

    // Dogfood 2026-07-24: multi-line Rust `{ ... }` search collapsed into an
    // empty `[ ... ]` placeholder (host/XML arg bridge ate the brace body).
    // Count non-whitespace, non-bracket payload chars; a near-empty bracket
    // husk with a tiny tail like `=> {},` is the signature of that failure.
    if search_curly_open >= 1 && replace_square_open >= 1 {
        let significant = replace
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '[' && *c != ']')
            .count();
        if significant <= 12 {
            return Some(
                "replace collapsed a brace-structured search block into an empty/placeholder bracket span — refusing to brick the file; re-send the full replace text (prefer apply_patch for multi-line match arms)",
            );
        }
    }

    // Extreme shrinkage with lost braces (e.g. 200-char match arm -> tiny stub).
    // Balanced-to-balanced nesting changes that shrink hard still look like
    // mangling; keep this guard even when deltas match.
    if search.len() >= 80
        && replace.len() * 8 < search.len()
        && search_curly_open >= 1
        && replace_curly_open < search_curly_open
    {
        return Some(
            "replace is drastically shorter than search and lost brace structure — likely argument mangling; refuse apply",
        );
    }

    None
}

const PREPROCESSOR_CONDITIONAL_ERROR: &str = "replace would change the C/C++ preprocessor conditional balance (#if/#ifdef/#ifndef vs #endif) — the search or replace text is missing a matching directive; copy the complete block including both its opening and closing directives";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PreprocessorConditionalDebt {
    orphaned_closes: usize,
    unclosed_opens: usize,
}

impl PreprocessorConditionalDebt {
    fn total(self) -> usize {
        self.orphaned_closes + self.unclosed_opens
    }
}

/// Reject an edit only when it introduces new conditional-structure damage in
/// a file whose extension identifies it as C-family source. The whole file is
/// checked before and after the edit: complete block insertion/removal is safe,
/// while an orphaned opener or closer increases the structural debt. Existing
/// debt may be preserved or reduced so this guard never prevents a repair.
fn invalid_preprocessor_edit(path: &Path, before: &str, after: &str) -> Option<&'static str> {
    if !is_c_family_source(path) {
        return None;
    }

    let before_debt = preprocessor_conditional_debt(before);
    let after_debt = preprocessor_conditional_debt(after);
    let safe = after_debt == before_debt
        || after_debt.total() == 0
        || after_debt.total() < before_debt.total();

    (!safe).then_some(PREPROCESSOR_CONDITIONAL_ERROR)
}

fn is_c_family_source(path: &Path) -> bool {
    const EXTENSIONS: &[&str] = &[
        "c", "cc", "cp", "cpp", "cxx", "h", "h++", "hh", "hpp", "hxx", "inl", "ipp", "ixx", "m",
        "mm", "tpp", "cu", "cuh", "cppm",
    ];

    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// Measure unmatched preprocessor conditionals across an entire source file.
/// Tracking nesting (instead of comparing span-level tuple counts) also catches
/// an `#endif` moved before its opener. Whitespace between `#` and the directive
/// name is accepted, as it is by C preprocessors.
fn preprocessor_conditional_debt(text: &str) -> PreprocessorConditionalDebt {
    let mut depth = 0usize;
    let mut orphaned_closes = 0usize;

    for line in text.lines() {
        match preprocessor_directive(line) {
            Some("if" | "ifdef" | "ifndef") => depth += 1,
            Some("endif") if depth == 0 => orphaned_closes += 1,
            Some("endif") => depth -= 1,
            _ => {}
        }
    }

    PreprocessorConditionalDebt {
        orphaned_closes,
        unclosed_opens: depth,
    }
}

fn preprocessor_directive(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let name_end = rest
        .find(|character: char| !character.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    (name_end > 0).then_some(&rest[..name_end])
}

/// Build a short, line-truncated preview of a (possibly very long) search
/// payload for error messages, so the model can compare what it searched for
/// against the file's actual contents without the error message ballooning.
fn preview_search_for_error(search: &str) -> String {
    const MAX_PREVIEW_LINES: usize = 3;
    const MAX_PREVIEW_LINE_LEN: usize = 80;
    search
        .lines()
        .take(MAX_PREVIEW_LINES)
        .map(|line| {
            if line.chars().count() > MAX_PREVIEW_LINE_LEN {
                let mut truncated: String = line.chars().take(MAX_PREVIEW_LINE_LEN).collect();
                truncated.push_str("...");
                truncated
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalize Windows CRLF pairs to LF while retaining the normalized byte
/// positions where a `\r` was removed. Lone carriage returns are preserved.
/// Inputs without CRLF are borrowed and use identity offsets.
///
/// A normalized boundary maps back to the original by adding the number of
/// removed CR bytes strictly before it. At the normalized newline itself that
/// excludes the current CR, so the start maps to `\r`; after the newline (or
/// at EOF) it includes that CR and spans the full pair.
fn normalize_crlf(input: &str) -> Cow<'_, str> {
    if input.contains("\r\n") {
        Cow::Owned(input.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(input)
    }
}

fn normalize_crlf_with_positions(input: &str) -> (Cow<'_, str>, Option<Vec<usize>>) {
    if !input.contains("\r\n") {
        return (Cow::Borrowed(input), None);
    }

    let mut normalized = String::with_capacity(input.len());
    let mut crlf_positions = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some((_, ch)) = chars.next() {
        if ch == '\r' && matches!(chars.peek(), Some((_, '\n'))) {
            let _ = chars.next();
            crlf_positions.push(normalized.len());
            normalized.push('\n');
            continue;
        }

        normalized.push(ch);
    }

    (Cow::Owned(normalized), Some(crlf_positions))
}

fn map_normalized_range(
    (start, end): (usize, usize),
    crlf_positions: Option<&[usize]>,
) -> (usize, usize) {
    let Some(crlf_positions) = crlf_positions else {
        return (start, end);
    };
    let map_boundary =
        |offset| offset + crlf_positions.partition_point(|position| *position < offset);
    (map_boundary(start), map_boundary(end))
}

fn map_normalized_ranges(
    ranges: impl IntoIterator<Item = (usize, usize)>,
    crlf_positions: Option<&[usize]>,
) -> Vec<(usize, usize)> {
    ranges
        .into_iter()
        .map(|range| map_normalized_range(range, crlf_positions))
        .collect()
}

/// Convert model-provided replacement newlines to the base file's convention.
/// Fold CRLF first so an already-CRLF payload never becomes `\r\r\n`.
fn normalize_replacement_line_endings(replace: &str, use_crlf: bool) -> String {
    let lf = replace.replace("\r\n", "\n");
    if use_crlf {
        lf.replace('\n', "\r\n")
    } else {
        lf
    }
}

fn strip_line_leading_whitespace_with_map(input: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(input.len());
    let mut byte_map = Vec::with_capacity(input.len());
    let mut at_line_start = true;
    for (idx, ch) in input.char_indices() {
        if at_line_start && matches!(ch, ' ' | '\t') {
            continue;
        }
        normalized.push(ch);
        for _ in 0..ch.len_utf8() {
            byte_map.push(idx);
        }
        at_line_start = ch == '\n';
    }
    (normalized, byte_map)
}

fn line_start_before(input: &str, idx: usize) -> usize {
    input[..idx]
        .rfind('\n')
        .map_or(0, |newline| newline.saturating_add(1))
}

fn next_char_boundary(input: &str, idx: usize) -> usize {
    if idx >= input.len() {
        return input.len();
    }

    let mut next = idx.saturating_add(1);
    while next < input.len() && !input.is_char_boundary(next) {
        next = next.saturating_add(1);
    }
    next
}

fn leading_whitespace_fuzzy_matches(contents: &str, search: &str) -> Vec<(usize, usize)> {
    let (normalized_contents, byte_map) = strip_line_leading_whitespace_with_map(contents);
    let (normalized_search, _) = strip_line_leading_whitespace_with_map(search);
    if normalized_search.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut cursor = 0;
    while let Some(rel_idx) = normalized_contents[cursor..].find(&normalized_search) {
        let norm_start = cursor + rel_idx;
        let norm_end = norm_start + normalized_search.len();
        let Some(&mapped_start) = byte_map.get(norm_start) else {
            break;
        };
        // Use the actual match start position, expanding to line start only
        // when the match begins at a line boundary in the normalized text.
        // This prevents destroying preceding text on the same line when
        // the match starts mid-line after whitespace stripping.
        let original_start =
            if norm_start == 0 || normalized_contents.as_bytes()[norm_start - 1] == b'\n' {
                // Match starts at a line boundary — use line start for full-line replacement.
                line_start_before(contents, mapped_start)
            } else {
                // Match starts mid-line — use the exact mapped position.
                mapped_start
            };
        let original_end = byte_map.get(norm_end).copied().unwrap_or(contents.len());
        matches.push((original_start, original_end));
        cursor = next_char_boundary(&normalized_contents, norm_start);
    }
    matches
}

/// Normalize typographic punctuation to its ASCII counterpart:
///
/// * `"` `"` / U+201C U+201D → `"`
/// * `'` `'` / U+2018 U+2019 → `'`
/// * `–` `—` / U+2013 U+2014 → `-`
/// * U+00A0 (non-breaking space) → ASCII space
///
/// Returns the normalized string plus a byte-map sized to
/// `normalized.len()` whose i-th entry is the original byte offset of
/// the character that produced normalized byte i. Used to recover the
/// original-byte range after finding a match in normalized space.
fn punctuation_normalized_with_map(input: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(input.len());
    let mut byte_map = Vec::with_capacity(input.len());
    for (idx, ch) in input.char_indices() {
        let replacement: Option<char> = match ch {
            '\u{201C}' | '\u{201D}' => Some('"'),
            '\u{2018}' | '\u{2019}' => Some('\''),
            '\u{2013}' | '\u{2014}' => Some('-'),
            '\u{00A0}' => Some(' '),
            _ => None,
        };
        let written = replacement.unwrap_or(ch);
        normalized.push(written);
        for _ in 0..written.len_utf8() {
            byte_map.push(idx);
        }
    }
    (normalized, byte_map)
}

/// Try to find `search` inside `contents` after normalizing typographic
/// punctuation in both. Catches the copy-paste failure mode where a
/// browser, word processor, or chat client silently converted ASCII
/// quotes/dashes to their Unicode "pretty" forms.
fn punctuation_normalized_matches(contents: &str, search: &str) -> Vec<(usize, usize)> {
    let (norm_contents, byte_map) = punctuation_normalized_with_map(contents);
    let (norm_search, _) = punctuation_normalized_with_map(search);
    if norm_search.is_empty() {
        return Vec::new();
    }
    // If normalization didn't change anything, the exact-match pass
    // already considered this case — skip to avoid double-reporting.
    if norm_contents == contents && norm_search == search {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut cursor = 0;
    while let Some(rel_idx) = norm_contents[cursor..].find(&norm_search) {
        let norm_start = cursor + rel_idx;
        let norm_end = norm_start + norm_search.len();
        let Some(&original_start) = byte_map.get(norm_start) else {
            break;
        };
        let original_end = byte_map.get(norm_end).copied().unwrap_or(contents.len());
        matches.push((original_start, original_end));
        cursor = next_char_boundary(&norm_contents, norm_start);
    }
    matches
}

// === ListDirTool ===

/// Tool for listing directory contents.
pub struct ListDirTool;

const LIST_DIR_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap on entries returned by a single `list_dir` call so a huge directory
/// (node_modules, build output, photo dumps) can't balloon the tool result.
/// Mirrors the bounded-output idiom of `read_file`'s `HARD_MAX_READ_LINES`.
/// Directories at or under the cap keep the historical plain-array response;
/// larger ones return an object with truncation metadata.
const LIST_DIR_MAX_ENTRIES: usize = 500;

#[async_trait]
impl ToolSpec for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn model_visible(&self) -> bool {
        true
    }

    fn description(&self) -> &'static str {
        "List entries in a workspace directory. This bounded, sandbox-aware tool is searchable when the core read/write/edit/bash toolbox is not enough."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path (default: .)"
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut input = input;
        apply_param_aliases(&mut input, PATH_ALIASES, "File list")?;
        LIST_PARAMS.reject_unknown(&input)?;

        let path_str = optional_str(&input, "path")?.unwrap_or(".");
        // S1: enumerating a denied directory is a read of it — `list_dir ~/.ssh`
        // hands back the key file names. Seatbelt's `deny file-read*` blocks
        // readdir of denied dirs, so refusing here matches the OS layer. The
        // raw spelling is checked first (F2) so the refusal names the caller's
        // path, never a symlink target it might resolve to.
        enforce_read_denylist(Path::new(path_str), "list_dir")?;
        let dir_path = context.resolve_path(path_str)?;
        enforce_read_denylist(&dir_path, "list_dir")?;

        let entries =
            list_dir_entries_async(dir_path, context.cancel_token.clone(), LIST_DIR_TIMEOUT)
                .await?;

        ToolResult::json(&entries).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

async fn list_dir_entries_async(
    dir_path: PathBuf,
    cancel_token: Option<CancellationToken>,
    timeout: Duration,
) -> Result<Value, ToolError> {
    let worker_cancel_token = cancel_token.clone();
    run_blocking_list_dir(timeout, cancel_token, move || {
        list_dir_entries(&dir_path, worker_cancel_token.as_ref())
    })
    .await
}

async fn run_blocking_list_dir<F>(
    timeout: Duration,
    cancel_token: Option<CancellationToken>,
    list_dir: F,
) -> Result<Value, ToolError>
where
    F: FnOnce() -> Result<Value, ToolError> + Send + 'static,
{
    if cancel_token
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(list_dir_cancelled());
    }

    let task = tokio::task::spawn_blocking(list_dir);
    let result = match cancel_token {
        Some(token) => {
            tokio::select! {
                biased;
                () = token.cancelled() => return Err(list_dir_cancelled()),
                result = tokio::time::timeout(timeout, task) => result,
            }
        }
        None => tokio::time::timeout(timeout, task).await,
    };

    let joined = result.map_err(|_| list_dir_timeout(timeout))?;
    joined.map_err(|err| {
        ToolError::execution_failed(format!("list_dir worker failed before completion: {err}"))
    })?
}

fn list_dir_entries(
    dir_path: &Path,
    cancel_token: Option<&CancellationToken>,
) -> Result<Value, ToolError> {
    check_list_dir_cancelled(cancel_token)?;

    let mut entries = Vec::new();
    let mut total_entries = 0usize;

    for entry in fs::read_dir(dir_path).map_err(|e| {
        ToolError::execution_failed(format!(
            "Failed to read directory {}: {}",
            dir_path.display(),
            e
        ))
    })? {
        check_list_dir_cancelled(cancel_token)?;

        let entry = entry.map_err(|e| ToolError::execution_failed(e.to_string()))?;
        total_entries += 1;
        // Past the cap, keep counting for the truncation metadata but stop
        // materializing entries.
        if entries.len() >= LIST_DIR_MAX_ENTRIES {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        entries.push(json!({
            "name": entry.file_name().to_string_lossy().to_string(),
            "is_dir": file_type.is_dir(),
        }));
    }

    if total_entries > entries.len() {
        Ok(json!({
            "entries": entries,
            "listed_entries": LIST_DIR_MAX_ENTRIES,
            "total_entries": total_entries,
            "truncated": true,
        }))
    } else {
        Ok(Value::Array(entries))
    }
}

fn check_list_dir_cancelled(cancel_token: Option<&CancellationToken>) -> Result<(), ToolError> {
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(list_dir_cancelled());
    }
    Ok(())
}

fn list_dir_cancelled() -> ToolError {
    ToolError::cancelled("list_dir cancelled before completion")
}

fn list_dir_timeout(timeout: Duration) -> ToolError {
    ToolError::Timeout {
        seconds: timeout.as_secs().max(1),
    }
}

// === Unit Tests ===

#[cfg(test)]
#[path = "file/tests.rs"]
mod pdf_tests;

#[cfg(test)]
#[path = "file/tests/tools.rs"]
mod tests;
