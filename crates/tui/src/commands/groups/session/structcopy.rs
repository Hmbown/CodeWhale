//! `/structcopy` command — human-only structural copy (#2033).
//!
//! Copies exactly one bounded, human-selected session object (one transcript
//! item, one tool call+result pair, the current plan snapshot, or one
//! existing Workflow run projection) as deterministic, versioned canonical
//! JSON with a top-level receipt. The default target is the clipboard; an
//! explicit `stdout` argument is the only text-view path.
//!
//! Contract:
//! - Human-only. This is a slash command, never a model-visible tool, event,
//!   or authority, and it writes nothing back into App/session/plan/workflow
//!   state (see the registry/catalog contract test).
//! - Read-only projection over existing state. Redaction reuses the
//!   transcript/export seams (`export::redact_json` for values,
//!   `export::sanitize_text` for keys and status labels, which
//!   `redact_json` does not reach) plus a strict pass that strips URL
//!   userinfo/query/fragment entirely and folds the workspace and home
//!   prefixes to labels, removes other absolute paths, and handles generic
//!   authority URLs. The workflow object reuses the bounded
//!   `WorkflowRunSummary` projection.
//! - Hard caps on final encoded bytes, array items, string bytes, object key
//!   bytes, and nesting depth; grapheme-safe truncation; recursively sorted
//!   keys; exact full-tree original counts and exact retained counts in the
//!   receipt. If receipt metadata alone cannot fit the byte cap, the command
//!   fails closed and emits nothing.
//!
//! What this deliberately does **not** claim:
//! - It is not a general PII scrubber. Workspace/home paths retain a useful
//!   labelled suffix; other absolute POSIX, drive-letter, and UNC paths are
//!   replaced outright.
//! - Redaction is pattern-based (the export seam's private-key/bearer/JWT/
//!   URL/secret regexes plus this module's strict URL pass). A secret that
//!   matches none of those patterns and sits under a non-sensitive key is
//!   copied as-is.
//! - Delivery to the clipboard is not confirmed. Terminal-client transports
//!   (tmux / OSC 52) are queued on a background writer; the receipt says
//!   "queued", not "delivered".

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as FmtWrite;
use std::path::Path;

use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::{Locale, MessageId, tr};
use crate::models::{ContentBlock, Message};
use crate::tui::app::App;

use super::CommandResult;
use super::export::{is_internal_role, is_sensitive_key, redact_json, sanitize_text};

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "structcopy",
    aliases: &[],
    usage: "/structcopy <turn <n>|tool <call-id>|plan|workflow <run-id>> [stdout]",
    description_id: MessageId::CmdStructcopyDescription,
};

pub(in crate::commands) struct StructcopyCmd;

impl RegisterCommand for StructcopyCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        execute_structcopy(app, arg)
    }
}

/// Versioned envelope identity carried in every receipt.
const SCHEMA_ID: &str = "codewhale/structcopy/v1";
/// Redaction contract label so consumers can tell which seams ran.
const REDACTION_CONTRACT: &str = "export-sanitize/v1+typed-markers/v1+strict-url/v2+path-redact/v2";
/// Marker substituted for subtrees cut by the depth cap. Structural markers
/// are inserted after bounding and are intentionally exempt from
/// `max_string_bytes`; they are still counted as retained bytes.
const DEPTH_OMISSION_MARKER: &str = "omitted:depth_cap";
/// Marker substituted for a URL token that cannot be parsed and therefore
/// cannot be proven free of userinfo/query/fragment. Fail closed.
const URL_OMISSION_MARKER: &str = "redacted:url";
/// Marker substituted for an absolute filesystem path outside the labelled
/// workspace/home roots. Paths are privacy-bearing even when they contain no
/// conventional secret token.
const PATH_OMISSION_MARKER: &str = "redacted:absolute_path";
const BEARER_REDACTION_MARKER: &str = "redacted:bearer";
const SENSITIVE_VALUE_REDACTION_MARKER: &str = "redacted:sensitive_value";

/// Selectors are echoed into the receipt and into status messages, so they
/// get their own tight cap independent of the payload string cap.
const MAX_SELECTOR_BYTES: usize = 256;
/// Hard caps enforced on every emitted artifact. The byte cap stays well
/// under the OSC 52 clipboard ceiling (100 KiB) so the default clipboard
/// target always fits its weakest transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Caps {
    max_output_bytes: usize,
    max_array_items: usize,
    max_string_bytes: usize,
    max_depth: usize,
}

const DEFAULT_CAPS: Caps = Caps {
    max_output_bytes: 48 * 1024,
    max_array_items: 64,
    max_string_bytes: 2 * 1024,
    max_depth: 12,
};

/// Object keys are bounded separately from values: they are short by nature,
/// they participate in collision handling, and they are rewritten once during
/// redaction rather than per byte-cap retry.
const MAX_KEY_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
enum CopyKind {
    Turn(usize),
    Tool(String),
    Plan,
    Workflow(String),
}

impl CopyKind {
    fn display_label(&self, locale: Locale) -> String {
        let id = match self {
            CopyKind::Turn(_) => MessageId::CmdStructcopyKindTurn,
            CopyKind::Tool(_) => MessageId::CmdStructcopyKindTool,
            CopyKind::Plan => MessageId::CmdStructcopyKindPlan,
            CopyKind::Workflow(_) => MessageId::CmdStructcopyKindWorkflow,
        };
        tr(locale, id).into_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CopyRequest {
    kind: CopyKind,
    stdout: bool,
}

fn execute_structcopy(app: &mut App, arg: Option<&str>) -> CommandResult {
    let request = match parse_request(arg) {
        Ok(request) => request,
        Err(()) => {
            return CommandResult::error(
                tr(app.ui_locale, MessageId::CmdStructcopyUsageError)
                    .replace("{usage}", COMMAND_INFO.usage),
            );
        }
    };
    let label = request.kind.display_label(app.ui_locale);
    let json = match render_copy(app, &request.kind, &DEFAULT_CAPS) {
        Ok(json) => json,
        Err(err) => return CommandResult::error(err),
    };
    if request.stdout {
        // The text view exists only because a human explicitly asked for it;
        // the default clipboard path never prints the payload.
        return CommandResult::message(json);
    }
    // `requires_terminal_paste()` is true only for an SSH session with no
    // forwarded display, where the sole transport is the terminal client
    // itself. That write is queued on a background writer, so a successful
    // return means "accepted for transport", not "in the clipboard".
    let terminal_client = app.clipboard.requires_terminal_paste();
    let bytes = json.len();
    match app.clipboard.write_text(&json) {
        Ok(()) if terminal_client => CommandResult::message(
            tr(app.ui_locale, MessageId::CmdStructcopyClipboardQueued)
                .replace("{kind}", &label)
                .replace("{bytes}", &bytes.to_string()),
        ),
        Ok(()) => CommandResult::message(
            tr(app.ui_locale, MessageId::CmdStructcopyClipboardAccepted)
                .replace("{kind}", &label)
                .replace("{bytes}", &bytes.to_string()),
        ),
        Err(err) => CommandResult::error(
            tr(app.ui_locale, MessageId::CmdStructcopyClipboardFailed)
                .replace("{error}", &err.to_string()),
        ),
    }
}

fn parse_request(arg: Option<&str>) -> Result<CopyRequest, ()> {
    let raw = arg.unwrap_or("").trim();
    if raw.is_empty() {
        return Err(());
    }
    let mut tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut stdout = false;
    if tokens
        .last()
        .is_some_and(|last| last.eq_ignore_ascii_case("stdout"))
    {
        stdout = true;
        tokens.pop();
    }
    let kind = match tokens.as_slice() {
        ["plan"] => CopyKind::Plan,
        ["turn", index] => {
            let index = index
                .parse::<usize>()
                .ok()
                .filter(|index| *index >= 1)
                .ok_or(())?;
            CopyKind::Turn(index)
        }
        ["tool", call_id] => CopyKind::Tool((*call_id).to_string()),
        ["workflow", run_id] => CopyKind::Workflow((*run_id).to_string()),
        _ => return Err(()),
    };
    Ok(CopyRequest { kind, stdout })
}

// === Object selection (read-only; unavailable objects are reported, never
// fabricated) ===

fn build_payload(app: &App, kind: &CopyKind) -> Result<(&'static str, Value, Value), String> {
    match kind {
        CopyKind::Turn(index) => turn_payload(app, *index),
        CopyKind::Tool(call_id) => tool_payload(app, call_id),
        CopyKind::Plan => plan_payload(app),
        CopyKind::Workflow(run_id) => workflow_payload(app, run_id),
    }
}

fn turn_payload(app: &App, index: usize) -> Result<(&'static str, Value, Value), String> {
    if app.api_messages.is_empty() {
        return Err(unavailable_message(app, &CopyKind::Turn(index)));
    }
    let Some(message) = app.api_messages.get(index - 1) else {
        return Err(unavailable_message(app, &CopyKind::Turn(index)));
    };
    Ok(("turn", json!(index), message_payload(message, index)))
}

fn message_payload(message: &Message, index: usize) -> Value {
    if is_internal_role(message.role.as_str()) {
        return json!({
            "index": index,
            "role": message.role,
            "omission_code": "internal_context",
        });
    }
    let content: Vec<Value> = message.content.iter().map(block_payload).collect();
    json!({
        "index": index,
        "role": message.role,
        "content": content,
    })
}

/// JSON `null` is the only truthful encoding for an unknown tri-state flag.
/// Collapsing `None` to `false` would assert an outcome the session never
/// observed, so every optional boolean in this projection goes through here.
fn optional_bool(value: Option<bool>) -> Value {
    match value {
        Some(flag) => Value::Bool(flag),
        None => Value::Null,
    }
}

fn block_payload(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text { text, .. } => json!({
            "type": "text",
            "text": text,
        }),
        ContentBlock::Thinking { .. } => json!({
            "type": "thinking",
            "omission_code": "internal_reasoning_and_signature",
        }),
        ContentBlock::ToolUse {
            id,
            name,
            input,
            caller,
            ..
        } => json!({
            "type": "tool_use",
            "id": id,
            // `null` here means "no caller recorded", not "no caller".
            "caller_type": caller.as_ref().map(|caller| caller.caller_type.as_str()),
            "name": name,
            "input": input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            content_blocks,
        } => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "is_error": optional_bool(*is_error),
            "content": content,
            "content_blocks": crate::image_attach::safe_tool_result_content_blocks(content_blocks.as_deref()),
        }),
        ContentBlock::ImageUrl { image_url } => {
            if image_url.url.starts_with("http://") || image_url.url.starts_with("https://") {
                json!({
                    "type": "image",
                    "url": image_url.url,
                })
            } else {
                json!({
                    "type": "image",
                    "omission_code": "inline_or_local_image_payload",
                })
            }
        }
        ContentBlock::ServerToolUse { id, name, input } => json!({
            "type": "server_tool_use",
            "id": id,
            "name": name,
            "input": input,
        }),
        ContentBlock::ToolSearchToolResult {
            tool_use_id,
            content,
        } => json!({
            "type": "tool_search_tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
        }),
        ContentBlock::CodeExecutionToolResult {
            tool_use_id,
            content,
        } => json!({
            "type": "code_execution_tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
        }),
    }
}

fn tool_payload(app: &App, call_id: &str) -> Result<(&'static str, Value, Value), String> {
    let mut found_call: Option<(String, Value)> = None;
    let mut found_result: Option<(Option<bool>, String, Option<Vec<Value>>)> = None;
    for message in &app.api_messages {
        for block in &message.content {
            match block {
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    if id.as_str() == call_id {
                        found_call = Some((name.clone(), input.clone()));
                    }
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    content_blocks,
                } if tool_use_id.as_str() == call_id => {
                    found_result = Some((*is_error, content.clone(), content_blocks.clone()));
                }
                _ => {}
            }
        }
    }
    let Some((name, input)) = found_call else {
        return Err(unavailable_message(
            app,
            &CopyKind::Tool(call_id.to_string()),
        ));
    };
    let result = match found_result {
        Some((is_error, content, content_blocks)) => json!({
            "found": true,
            // `null` = the result carried no error flag, which is distinct
            // from `false` (an explicitly successful result).
            "is_error": optional_bool(is_error),
            "content": content,
            "content_blocks": crate::image_attach::safe_tool_result_content_blocks(content_blocks.as_deref()),
        }),
        None => json!({
            "found": false,
        }),
    };
    Ok((
        "tool",
        json!(call_id),
        json!({
            "call_id": call_id,
            "name": name,
            "input": input,
            "result": result,
        }),
    ))
}

fn plan_payload(app: &App) -> Result<(&'static str, Value, Value), String> {
    let snapshot = {
        let state = app
            .plan_state
            .try_lock()
            .map_err(|_| busy_message(app, &CopyKind::Plan))?;
        state.snapshot()
    };
    if snapshot.is_empty() {
        return Err(unavailable_message(app, &CopyKind::Plan));
    }
    let value = serde_json::to_value(&snapshot)
        .map_err(|err| prepare_failed_message(app, &CopyKind::Plan, &err.to_string()))?;
    Ok(("plan", Value::Null, value))
}

fn workflow_payload(app: &App, run_id: &str) -> Result<(&'static str, Value, Value), String> {
    match crate::tools::workflow::structcopy_run_projection(
        &app.workspace,
        run_id,
        app.current_session_id.as_deref(),
    ) {
        Some(value) => Ok(("workflow", json!(run_id), value)),
        None => Err(unavailable_message(
            app,
            &CopyKind::Workflow(run_id.to_string()),
        )),
    }
}

fn unavailable_message(app: &App, kind: &CopyKind) -> String {
    tr(app.ui_locale, MessageId::CmdStructcopyUnavailable)
        .replace("{kind}", &kind.display_label(app.ui_locale))
}

fn busy_message(app: &App, kind: &CopyKind) -> String {
    tr(app.ui_locale, MessageId::CmdStructcopyBusy)
        .replace("{kind}", &kind.display_label(app.ui_locale))
}

fn prepare_failed_message(app: &App, kind: &CopyKind, error: &str) -> String {
    tr(app.ui_locale, MessageId::CmdStructcopyPrepareFailed)
        .replace("{kind}", &kind.display_label(app.ui_locale))
        .replace("{error}", error)
}

// === Redaction (composed from existing central seams) ===

/// The strongest existing central redaction, applied before any bounding or
/// serialization and after key normalization.
///
/// [`redact_json`] replaces values under secret-shaped keys, and runs
/// [`sanitize_text`] over every string *value* — stripping ANSI/control
/// bytes and masking PEM blocks, `Bearer` tokens, JWTs, credential-bearing
/// URLs, and the config layer's known secret patterns. It does **not** touch
/// object *keys*, so this pass runs [`sanitize_text`] over keys as well,
/// then folds workspace/home prefixes to labels and strips URL
/// userinfo/query/fragment outright.
///
/// Keys are also sorted, bounded, and de-collided here. Original and retained
/// key counts are kept separately so omitted subtrees cannot inflate claims
/// about the emitted object.
fn redact_payload(value: &mut Value, labels: &PathLabels, keys: &mut KeyStats) {
    // Normalize keys first so ANSI/control obfuscation cannot hide a
    // sensitive-key hint from classification. `strict_strings` classifies
    // both the original and normalized key; the shared export pass then runs
    // over the normalized tree as defense in depth.
    let mut path = Vec::new();
    strict_strings(value, labels, keys, &mut path);
    redact_json(value, None);
    normalize_redaction_codes(value);
}

/// Prefix folding for useful filesystem paths. These prefixes are recognised:
/// the workspace root (both as configured and as canonicalized, which differ
/// on macOS where `/var` symlinks to `/private/var`) and `$HOME` /
/// `%USERPROFILE%`. The later strict pass removes every remaining absolute
/// POSIX, drive-letter, or UNC path.
struct PathLabels {
    /// `(prefix, label)` sorted longest-first so that a workspace nested
    /// inside `$HOME` folds to `<workspace>` rather than `<home>/…`.
    labels: Vec<(String, &'static str)>,
}

impl PathLabels {
    fn new(workspace: &Path) -> Self {
        let mut workspace_forms: Vec<String> = Vec::new();
        let literal = workspace.to_string_lossy().into_owned();
        if literal.len() > 1 {
            workspace_forms.push(literal);
        }
        // Read-only; `canonicalize` never creates state.
        if let Ok(canonical) = workspace.canonicalize() {
            let canonical = canonical.to_string_lossy().into_owned();
            if canonical.len() > 1 && !workspace_forms.contains(&canonical) {
                workspace_forms.push(canonical);
            }
        }
        let mut labels: Vec<(String, &'static str)> = workspace_forms
            .iter()
            .map(|form| (form.clone(), "<workspace>"))
            .collect();
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home = home.to_string_lossy().into_owned();
            if home.len() > 3 && !workspace_forms.contains(&home) {
                labels.push((home, "<home>"));
            }
        }
        labels.sort_by(|left, right| {
            right
                .0
                .len()
                .cmp(&left.0.len())
                .then_with(|| left.0.cmp(&right.0))
        });
        Self { labels }
    }

    fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (prefix, label) in &self.labels {
            out = replace_path_root(&out, prefix, label);
        }
        out
    }
}

/// Replace a configured root only when it ends on a path-component boundary.
/// A lexical prefix such as `/opt/app` must not label `/opt/application`; the
/// latter remains foreign and is removed by the absolute-path scrubber.
fn replace_path_root(text: &str, root: &str, label: &str) -> String {
    if root.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find(root) {
        let start = cursor + offset;
        let end = start + root.len();
        out.push_str(&text[cursor..start]);
        let component_boundary = text[end..]
            .chars()
            .next()
            .is_none_or(|ch| matches!(ch, '/' | '\\'));
        if component_boundary {
            out.push_str(label);
        } else {
            out.push_str(root);
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Per-object-key accounting. Computed once during redaction and reported in
/// the receipt so a renamed or truncated key is never silent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct KeyStats {
    entries: BTreeMap<Vec<String>, KeyFlags>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct KeyFlags {
    truncated: bool,
    deduped: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RetainedKeyStats {
    total: u64,
    truncated: u64,
    deduped: u64,
}

impl KeyStats {
    fn original_total(&self) -> u64 {
        u64::try_from(self.entries.len()).unwrap_or(u64::MAX)
    }
}

fn strict_strings(
    value: &mut Value,
    labels: &PathLabels,
    keys: &mut KeyStats,
    path: &mut Vec<String>,
) {
    match value {
        Value::String(text) => *text = scrub_string(text, labels),
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                path.push(format!("i:{index}"));
                strict_strings(item, labels, keys, path);
                path.pop();
            }
        }
        Value::Object(map) => {
            // Take the map, rewrite each key, and reinsert. Entries are
            // processed in sorted original-key order so collision suffixes
            // are assigned deterministically regardless of insertion order.
            let mut entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, mut item) in entries {
                let scrubbed = flatten_ws(&scrub_string(&key, labels));
                let (bounded, was_truncated) =
                    truncate_string_grapheme_safe(&scrubbed, MAX_KEY_BYTES);
                let (unique, collision_truncated) = unique_object_key(map, &bounded);
                let sensitive = is_sensitive_key(&key) || is_sensitive_key(&scrubbed);
                if sensitive {
                    item = Value::String("[redacted]".to_string());
                } else {
                    path.push(key_path_segment(&unique));
                    strict_strings(&mut item, labels, keys, path);
                    path.pop();
                }
                path.push(key_path_segment(&unique));
                keys.entries.insert(
                    path.clone(),
                    KeyFlags {
                        truncated: was_truncated || collision_truncated,
                        deduped: unique != bounded,
                    },
                );
                path.pop();
                map.insert(unique, item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn key_path_segment(key: &str) -> String {
    format!("k:{}:{key}", key.len())
}

fn collect_retained_key_stats(
    value: &Value,
    original: &KeyStats,
    path: &mut Vec<String>,
    retained: &mut RetainedKeyStats,
) {
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(format!("i:{index}"));
                collect_retained_key_stats(item, original, path, retained);
                path.pop();
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                path.push(key_path_segment(key));
                if let Some(flags) = original.entries.get(path) {
                    retained.total += 1;
                    if flags.truncated {
                        retained.truncated += 1;
                    }
                    if flags.deduped {
                        retained.deduped += 1;
                    }
                }
                collect_retained_key_stats(item, original, path, retained);
                path.pop();
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Deterministic collision handling for keys that collapsed onto each other
/// after scrubbing or truncation.
///
/// Termination is structural rather than hopeful: the numeric reserve is
/// sized for the largest suffix this call can produce, so `base` is fixed and
/// the `map.len() + 1` candidates `base~2 … base~(len+2)` are pairwise
/// distinct. A map holding `len` keys cannot occupy all of them.
///
/// When `MAX_KEY_BYTES` is smaller than the reserve the suffix still wins:
/// losing a key to a silent overwrite is worse than exceeding a key cap by a
/// few bytes, and the per-key flags record that it happened.
fn unique_object_key(map: &serde_json::Map<String, Value>, requested: &str) -> (String, bool) {
    if !map.contains_key(requested) {
        return (requested.to_string(), false);
    }
    let highest = map.len().saturating_add(2);
    let reserve = 1 + decimal_width(highest);
    let base_cap = MAX_KEY_BYTES.saturating_sub(reserve);
    let (base, collision_truncated) = truncate_string_grapheme_safe(requested, base_cap);
    for index in 2..=highest {
        let candidate = format!("{base}~{index}");
        if !map.contains_key(&candidate) {
            return (candidate, collision_truncated);
        }
    }
    unreachable!(
        "map of {} keys cannot occupy {} distinct candidates",
        map.len(),
        highest - 1
    )
}

fn decimal_width(mut value: usize) -> usize {
    let mut width = 1;
    while value >= 10 {
        value /= 10;
        width += 1;
    }
    width
}

/// Collapse every run of whitespace to a single space. Used for object keys,
/// where control layout is a structural hazard rather than data.
fn flatten_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn scrub_string(text: &str, labels: &PathLabels) -> String {
    // `sanitize_text` first: it strips ANSI and control bytes, so the URL
    // scan below cannot be fooled by an escape sequence spliced into a
    // scheme. It is idempotent, so re-running it over values that
    // `redact_json` already sanitized is safe.
    let sanitized = sanitize_text(text);
    let bearer_safe = redact_loose_bearers(&sanitized);
    let labelled = labels.apply(&bearer_safe);
    scrub_paths(&scrub_urls(&labelled))
}

/// Convert the prose placeholders owned by the shared export seam into stable
/// language-neutral codes. Structural JSON is a machine artifact and must not
/// change with the UI locale.
fn normalize_redaction_codes(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = text
                .replace("[redacted private key]", "redacted:private_key")
                .replace("Bearer [redacted]", BEARER_REDACTION_MARKER)
                .replace("[redacted token]", "redacted:token")
                .replace("[redacted]", SENSITIVE_VALUE_REDACTION_MARKER);
        }
        Value::Array(items) => {
            for item in items {
                normalize_redaction_codes(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                normalize_redaction_codes(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_loose_bearers(text: &str) -> String {
    // Selectors cannot carry the whitespace used by a conventional
    // `Bearer <token>` header. Delimiter variants are still secret-shaped;
    // redact their entire line tail so token punctuation cannot terminate a
    // regex early and expose the remainder.
    let lowered = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(offset) = ["bearer-", "bearer_", "bearer:", "bearer="]
        .iter()
        .filter_map(|prefix| lowered[cursor..].find(prefix))
        .min()
    {
        let start = cursor + offset;
        out.push_str(&text[cursor..start]);
        let end = text[start..]
            .find('\n')
            .map(|line_end| start + line_end)
            .unwrap_or(text.len());
        out.push_str(BEARER_REDACTION_MARKER);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Trailing characters that are punctuation or wrappers around a URL rather
/// than part of it. Trimming generously is safe in both directions: the
/// trimmed tail is re-appended verbatim and can hold no credential, while a
/// tail left attached would be swallowed by the query/fragment strip.
const URL_TRAILING_PUNCTUATION: &[char] = &[
    '.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'', '`', '*', '_', '\\',
];

/// Strip URL userinfo, query, and fragment entirely, leaving a
/// `scheme://host[:port]/path` label.
///
/// The export seam has already masked credentials in URLs it recognised;
/// this pass enforces the stricter structural-copy contract that no
/// userinfo, query string, or fragment may survive at all — including for
/// URLs that are punctuation-wrapped (`(https://…)`, `<https://…>`,
/// `"https://…"`), embedded mid-token, or uppercased. A token that starts
/// with a syntactically valid `scheme://` prefix but does not parse is replaced outright rather than
/// passed through, because an unparseable URL cannot be proven credential
/// free.
fn scrub_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(offset) = next_url_start(&text[cursor..]) {
        let start = cursor + offset;
        out.push_str(&text[cursor..start]);
        let rest = &text[start..];
        // A scheme prefix contains no whitespace, so `end` is always > 0 and
        // the cursor strictly advances.
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        out.push_str(&scrub_url_token(&rest[..end]));
        cursor = start + end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn next_url_start(text: &str) -> Option<usize> {
    for (separator, _) in text.match_indices("://") {
        let before = &text[..separator];
        let start = before
            .char_indices()
            .rev()
            .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
            .map(|(index, _)| index)
            .last()
            .unwrap_or(separator);
        let scheme = &text[start..separator];
        if scheme
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
        {
            return Some(start);
        }
    }
    None
}

fn scrub_url_token(token: &str) -> String {
    let trimmed = token.trim_end_matches(URL_TRAILING_PUNCTUATION);
    let suffix = &token[trimmed.len()..];
    let Ok(mut parsed) = reqwest::Url::parse(trimmed) else {
        return format!("{URL_OMISSION_MARKER}{suffix}");
    };
    // `set_username`/`set_password` only fail for cannot-be-a-base URLs.
    // Failing closed keeps the "no userinfo survives" claim literally true.
    if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
        return format!("{URL_OMISSION_MARKER}{suffix}");
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    format!("{parsed}{suffix}")
}

fn scrub_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(start) = next_absolute_path_start(text, cursor) {
        out.push_str(&text[cursor..start]);
        // An unquoted absolute path can legally contain spaces. Stop at the
        // line boundary rather than risk leaking the tail of such a path;
        // losing adjacent prose is safer than emitting a customer/user name.
        let end = text[start..]
            .find('\n')
            .map(|offset| start + offset)
            .unwrap_or(text.len());
        out.push_str(PATH_OMISSION_MARKER);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn next_absolute_path_start(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        let boundary = index == 0
            || text[..index]
                .chars()
                .next_back()
                .is_some_and(|ch| !ch.is_alphanumeric() && !matches!(ch, '_' | '/' | '\\'));
        if boundary {
            let labelled_root =
                text[..index].ends_with("<workspace>") || text[..index].ends_with("<home>");
            let url_separator = index > 0
                && index + 1 < bytes.len()
                && bytes[index - 1] == b':'
                && bytes[index + 1] == b'/';
            let previous_is_slash = index > 0 && bytes[index - 1] == b'/';
            let posix =
                bytes[index] == b'/' && !previous_is_slash && !url_separator && !labelled_root;
            let drive = index + 2 < bytes.len()
                && bytes[index].is_ascii_alphabetic()
                && bytes[index + 1] == b':'
                && matches!(bytes[index + 2], b'/' | b'\\');
            let unc = index + 1 < bytes.len() && bytes[index] == b'\\' && bytes[index + 1] == b'\\';
            if posix || drive || unc {
                return Some(index);
            }
        }
        index += text[index..].chars().next()?.len_utf8();
    }
    None
}

// === Bounding (hard caps + exact accounting) ===

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct BoundStats {
    /// Strings present in the full redacted tree, at every depth.
    strings_total: u64,
    /// Strings actually present in the emitted payload, including the
    /// structural markers substituted for depth-omitted subtrees.
    strings_retained: u64,
    strings_truncated: u64,
    string_bytes_original: u64,
    string_bytes_retained: u64,
    /// Array elements present in the full redacted tree, at every depth —
    /// including elements inside subtrees that the depth cap later omits.
    array_items_original: u64,
    array_items_retained: u64,
    depth_omissions: u64,
}

/// Exact full-tree original counts. Deliberately depth-unbounded: the
/// receipt's `*_original` numbers describe the whole redacted object, so
/// that a subtree removed by the depth cap still shows up in the difference
/// between original and retained.
fn collect_original_counts(value: &Value, stats: &mut BoundStats) {
    match value {
        Value::String(text) => {
            stats.strings_total += 1;
            stats.string_bytes_original += text.len() as u64;
        }
        Value::Array(items) => {
            stats.array_items_original += items.len() as u64;
            for item in items {
                collect_original_counts(item, stats);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_original_counts(item, stats);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn bound_value(
    value: &mut Value,
    caps: &Caps,
    stats: &mut BoundStats,
    reasons: &mut BTreeSet<&'static str>,
    depth: usize,
) {
    match value {
        Value::String(text) => {
            let (truncated, was_truncated) =
                truncate_string_grapheme_safe(text, caps.max_string_bytes);
            if was_truncated {
                *text = truncated;
                stats.strings_truncated += 1;
                reasons.insert("string_bytes_cap");
            }
            stats.strings_retained += 1;
            stats.string_bytes_retained += text.len() as u64;
        }
        Value::Array(items) => {
            if depth >= caps.max_depth {
                omit_for_depth(value, stats, reasons);
                return;
            }
            if items.len() > caps.max_array_items {
                items.truncate(caps.max_array_items);
                reasons.insert("array_items_cap");
            }
            stats.array_items_retained += items.len() as u64;
            for item in items {
                bound_value(item, caps, stats, reasons, depth + 1);
            }
        }
        Value::Object(map) => {
            if depth >= caps.max_depth {
                omit_for_depth(value, stats, reasons);
                return;
            }
            for item in map.values_mut() {
                bound_value(item, caps, stats, reasons, depth + 1);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Replace a too-deep subtree with the structural marker. The marker is a
/// string that really is emitted, so it counts toward the retained totals —
/// otherwise `string_bytes_retained` would understate the artifact it
/// describes.
fn omit_for_depth(value: &mut Value, stats: &mut BoundStats, reasons: &mut BTreeSet<&'static str>) {
    stats.depth_omissions += 1;
    reasons.insert("depth_cap");
    *value = Value::String(DEPTH_OMISSION_MARKER.to_string());
    stats.strings_retained += 1;
    stats.string_bytes_retained += DEPTH_OMISSION_MARKER.len() as u64;
}

/// UTF-8/grapheme-safe truncation: never splits a grapheme cluster, and the
/// retained bytes (including the ellipsis marker) never exceed the cap.
///
/// When `max_bytes` is below the ellipsis's own 3 bytes there is no way to
/// emit both content and a truncation marker inside the cap. The honest
/// answer is the empty string plus `true`: the caller records a truncation,
/// and no partial content escapes under a cap it does not fit.
fn truncate_string_grapheme_safe(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    if max_bytes < '…'.len_utf8() {
        return (String::new(), true);
    }
    let budget = max_bytes - '…'.len_utf8();
    let mut out = String::new();
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        if out.len() + grapheme.len() > budget {
            break;
        }
        out.push_str(grapheme);
    }
    out.push('…');
    (out, true)
}

// === Canonical serialization (deterministic, recursively sorted keys) ===

fn canonical_string(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            let _ = write!(out, "{number}");
        }
        Value::String(text) => {
            let encoded = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
            out.push_str(&encoded);
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let encoded = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                out.push_str(&encoded);
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
    }
}

// === Envelope assembly ===

fn render_copy(app: &App, kind: &CopyKind, caps: &Caps) -> Result<String, String> {
    let (kind_label, mut selector, mut payload) = build_payload(app, kind)?;
    let labels = PathLabels::new(&app.workspace);

    // The selector is echoed verbatim into the receipt, so it goes through
    // the same redaction as the payload and gets its own tight byte bound.
    let mut selector_keys = KeyStats::default();
    redact_payload(&mut selector, &labels, &mut selector_keys);
    bound_selector(&mut selector);

    let mut keys = KeyStats::default();
    redact_payload(&mut payload, &labels, &mut keys);

    // Fit the byte cap by tightening the content caps before ever
    // considering a payload omission.
    let mut effective = *caps;
    for _ in 0..4 {
        let encoded = encode_attempt(
            kind_label, &selector, &payload, &effective, caps, &keys, false,
        );
        if encoded.len() <= caps.max_output_bytes {
            return Ok(encoded);
        }
        effective.max_string_bytes = (effective.max_string_bytes / 2).max(64);
        effective.max_array_items = (effective.max_array_items / 2).max(1);
        effective.max_depth = effective.max_depth.saturating_sub(2).max(2);
    }

    // Last resort: emit receipt metadata only. If even that exceeds the cap,
    // fail closed rather than emit an over-cap artifact.
    let encoded = encode_attempt(
        kind_label, &selector, &payload, &effective, caps, &keys, true,
    );
    if encoded.len() <= caps.max_output_bytes {
        return Ok(encoded);
    }
    Err(tr(app.ui_locale, MessageId::CmdStructcopyReceiptTooLarge)
        .replace("{bytes}", &caps.max_output_bytes.to_string()))
}

/// Bound the selector independently of the payload caps. Selectors are
/// scalars, so this only has to handle the string case.
fn bound_selector(selector: &mut Value) {
    if let Value::String(text) = selector {
        let (bounded, _) = truncate_string_grapheme_safe(text, MAX_SELECTOR_BYTES);
        *text = bounded;
    }
}

fn encode_attempt(
    kind_label: &str,
    selector: &Value,
    payload: &Value,
    effective: &Caps,
    hard: &Caps,
    keys: &KeyStats,
    omit_payload: bool,
) -> String {
    let mut candidate = payload.clone();
    let mut stats = BoundStats::default();
    let mut reasons = BTreeSet::new();
    collect_original_counts(&candidate, &mut stats);
    bound_value(&mut candidate, effective, &mut stats, &mut reasons, 0);
    let mut retained_keys = RetainedKeyStats::default();
    collect_retained_key_stats(&candidate, keys, &mut Vec::new(), &mut retained_keys);
    if effective != hard {
        reasons.insert("caps_tightened_output_bytes_cap");
    }
    if retained_keys.truncated > 0 {
        reasons.insert("object_key_bytes_cap");
    }
    if retained_keys.deduped > 0 {
        reasons.insert("object_key_collision");
    }
    let emitted = if omit_payload {
        // Nothing from the bounding pass was emitted, so every retained
        // counter and every bounding reason would be a claim about an
        // artifact that does not exist. Originals stay; the rest resets.
        reasons.clear();
        reasons.insert("payload_omitted_output_bytes_cap");
        stats.strings_retained = 0;
        stats.strings_truncated = 0;
        stats.string_bytes_retained = 0;
        stats.array_items_retained = 0;
        stats.depth_omissions = 0;
        retained_keys = RetainedKeyStats::default();
        Value::Null
    } else {
        candidate
    };
    let envelope = assemble_envelope(
        kind_label,
        selector,
        &emitted,
        &stats,
        keys,
        &retained_keys,
        &reasons,
        effective,
        hard,
    );
    canonical_string(&envelope)
}

#[allow(clippy::too_many_arguments)]
fn assemble_envelope(
    kind: &str,
    selector: &Value,
    payload: &Value,
    stats: &BoundStats,
    original_keys: &KeyStats,
    retained_keys: &RetainedKeyStats,
    reasons: &BTreeSet<&'static str>,
    effective: &Caps,
    hard: &Caps,
) -> Value {
    json!({
        "object": payload,
        "receipt": {
            "schema": SCHEMA_ID,
            "human_only": true,
            "kind": kind,
            "selector": selector,
            "redaction": REDACTION_CONTRACT,
            // `caps` is the declared contract; `applied_caps` is what this
            // artifact was actually bounded with. They differ whenever the
            // output-byte cap forced a tightening pass.
            "caps": caps_value(hard),
            "applied_caps": caps_value(effective),
            "counts": {
                "strings_total": stats.strings_total,
                "strings_retained": stats.strings_retained,
                "strings_truncated": stats.strings_truncated,
                "string_bytes_original": stats.string_bytes_original,
                "string_bytes_retained": stats.string_bytes_retained,
                "array_items_original": stats.array_items_original,
                "array_items_retained": stats.array_items_retained,
                "depth_omissions": stats.depth_omissions,
                "object_keys_original": original_keys.original_total(),
                "object_keys_retained": retained_keys.total,
                "object_keys_truncated": retained_keys.truncated,
                "object_keys_deduped": retained_keys.deduped,
                "payload_bytes": canonical_string(payload).len(),
            },
            "reasons": reasons.iter().copied().collect::<Vec<_>>(),
        }
    })
}

fn caps_value(caps: &Caps) -> Value {
    json!({
        "max_output_bytes": caps.max_output_bytes,
        "max_array_items": caps.max_array_items,
        "max_string_bytes": caps.max_string_bytes,
        "max_key_bytes": MAX_KEY_BYTES,
        "max_depth": caps.max_depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::Role;
    use crate::models::{ImageUrlContent, ToolCaller};
    use crate::tools::plan::{PlanItemArg, StepStatus, UpdatePlanArgs};
    use crate::tui::app::TuiOptions;
    use crate::tui::clipboard::ClipboardHandler;
    use tempfile::TempDir;

    fn test_app(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            ..crate::test_support::test_tui_options(tmpdir.path())
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = Locale::En;
        app
    }

    fn stdout_json(result: &CommandResult) -> String {
        assert!(!result.is_error, "{:?}", result.message);
        result.message.clone().expect("stdout payload")
    }

    fn parsed(json: &str) -> Value {
        serde_json::from_str(json).expect("structcopy output must be valid JSON")
    }

    fn no_labels() -> PathLabels {
        PathLabels { labels: Vec::new() }
    }

    fn seed_transcript(app: &mut App) {
        app.api_messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "please run the fetch".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "private chain of thought".to_string(),
                        signature: Some("signature-secret".to_string()),
                        state: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call-7".to_string(),
                        name: "fetch_url".to_string(),
                        input: json!({
                            "url": "https://alice:hunter2@example.com/path?token=abc123&ok=1#frag",
                            "api_key": "literal-api-secret",
                        }),
                        caller: Some(ToolCaller {
                            caller_type: "code_execution_20250825".to_string(),
                            tool_id: None,
                        }),
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-7".to_string(),
                    content: "Authorization: Bearer result-secret-token\nfetch ok".to_string(),
                    is_error: Some(false),
                    content_blocks: None,
                }],
            },
        ];
    }

    #[test]
    fn turn_copy_projects_one_item_and_redacts() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        seed_transcript(&mut app);

        let json = stdout_json(&execute_structcopy(&mut app, Some("turn 2 stdout")));
        let value = parsed(&json);
        assert_eq!(value["receipt"]["schema"], json!(SCHEMA_ID));
        assert_eq!(value["receipt"]["kind"], json!("turn"));
        assert_eq!(value["receipt"]["selector"], json!(2));
        assert_eq!(value["object"]["role"], json!("assistant"));
        let content = value["object"]["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], json!("thinking"));
        assert!(content[0].get("thinking").is_none());
        assert_eq!(
            content[0]["omission_code"],
            json!("internal_reasoning_and_signature")
        );
        assert_eq!(content[1]["type"], json!("tool_use"));
        assert_eq!(content[1]["caller_type"], json!("code_execution_20250825"));
        for forbidden in [
            "private chain of thought",
            "signature-secret",
            "literal-api-secret",
            "hunter2",
            "abc123",
            "frag",
        ] {
            assert!(!json.contains(forbidden), "leaked {forbidden:?}: {json}");
        }
        // URL userinfo/query/fragment are stripped outright.
        assert!(json.contains("https://example.com/path"), "{json}");
        assert!(json.contains(SENSITIVE_VALUE_REDACTION_MARKER), "{json}");
        for prose in [
            "internal context omitted",
            "internal reasoning and signature omitted",
            "inline or local image payload omitted",
            "[redacted private key]",
            "Bearer [redacted]",
            "[redacted token]",
            "[redacted]",
        ] {
            assert!(!json.contains(prose), "prose marker {prose:?}: {json}");
        }
    }

    #[test]
    fn generated_omissions_are_language_neutral_codes() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.api_messages = vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "must not be copied".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ImageUrl {
                    image_url: ImageUrlContent {
                        url: "data:image/png;base64,private".to_string(),
                    },
                }],
            },
        ];

        let internal = parsed(&stdout_json(&execute_structcopy(
            &mut app,
            Some("turn 1 stdout"),
        )));
        assert_eq!(
            internal["object"]["omission_code"],
            json!("internal_context")
        );
        assert!(internal["object"].get("omitted").is_none());

        let image = parsed(&stdout_json(&execute_structcopy(
            &mut app,
            Some("turn 2 stdout"),
        )));
        assert_eq!(
            image["object"]["content"][0]["omission_code"],
            json!("inline_or_local_image_payload")
        );
        assert!(image["object"]["content"][0].get("omitted").is_none());

        let english = stdout_json(&execute_structcopy(&mut app, Some("turn 2 stdout")));
        app.ui_locale = Locale::ZhHans;
        let chinese_ui = stdout_json(&execute_structcopy(&mut app, Some("turn 2 stdout")));
        assert_eq!(
            english, chinese_ui,
            "machine payload must not vary with the UI locale"
        );
    }

    #[test]
    fn tool_copy_pairs_call_and_result() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        seed_transcript(&mut app);

        let json = stdout_json(&execute_structcopy(&mut app, Some("tool call-7 stdout")));
        let value = parsed(&json);
        assert_eq!(value["receipt"]["kind"], json!("tool"));
        assert_eq!(value["receipt"]["selector"], json!("call-7"));
        assert_eq!(value["object"]["name"], json!("fetch_url"));
        assert_eq!(value["object"]["result"]["found"], json!(true));
        assert_eq!(value["object"]["result"]["is_error"], json!(false));
        assert!(!json.contains("result-secret-token"), "{json}");

        // A call without a result is honest, not fabricated.
        app.api_messages[1].content.push(ContentBlock::ToolUse {
            id: "call-lonely".to_string(),
            name: "view_image".to_string(),
            input: json!({}),
            caller: None,
            thought_signature: None,
        });
        let json = stdout_json(&execute_structcopy(
            &mut app,
            Some("tool call-lonely stdout"),
        ));
        let value = parsed(&json);
        assert_eq!(value["object"]["result"]["found"], json!(false));
    }

    /// An unknown `Option<bool>` must serialize as JSON `null`. Collapsing it
    /// to `false` would assert an outcome nothing observed.
    #[test]
    fn unknown_optional_booleans_stay_null_and_are_not_dropped() {
        assert_eq!(optional_bool(None), Value::Null);
        assert_eq!(optional_bool(Some(false)), Value::Bool(false));
        assert_eq!(optional_bool(Some(true)), Value::Bool(true));

        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.api_messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call-unknown".to_string(),
                    name: "exec_command".to_string(),
                    input: json!({}),
                    // No caller recorded: also an unknown, also null.
                    caller: None,
                    thought_signature: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-unknown".to_string(),
                    content: "no error flag was recorded".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];

        // Tool-pair projection.
        let json = stdout_json(&execute_structcopy(
            &mut app,
            Some("tool call-unknown stdout"),
        ));
        let value = parsed(&json);
        let result = value["object"]["result"].as_object().expect("result");
        assert!(
            result.contains_key("is_error"),
            "the unknown flag must be present, not dropped: {json}"
        );
        assert_eq!(result["is_error"], Value::Null);
        assert_ne!(result["is_error"], json!(false));

        // Turn projection of the same result block, plus the unknown caller.
        let json = stdout_json(&execute_structcopy(&mut app, Some("turn 2 stdout")));
        let value = parsed(&json);
        let block = &value["object"]["content"][0];
        assert!(
            block.as_object().expect("block").contains_key("is_error"),
            "{json}"
        );
        assert_eq!(block["is_error"], Value::Null);

        let json = stdout_json(&execute_structcopy(&mut app, Some("turn 1 stdout")));
        let value = parsed(&json);
        let block = &value["object"]["content"][0];
        assert!(
            block
                .as_object()
                .expect("block")
                .contains_key("caller_type"),
            "{json}"
        );
        assert_eq!(block["caller_type"], Value::Null);
    }

    #[test]
    fn plan_copy_snapshots_current_plan() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        {
            let mut state = app.plan_state.try_lock().expect("plan lock");
            state.update(UpdatePlanArgs {
                title: Some("Ship structcopy".to_string()),
                plan: vec![
                    PlanItemArg {
                        step: "Read seams".to_string(),
                        status: StepStatus::Completed,
                    },
                    PlanItemArg {
                        step: "Copy exactly one object".to_string(),
                        status: StepStatus::InProgress,
                    },
                ],
                ..Default::default()
            });
        }

        let json = stdout_json(&execute_structcopy(&mut app, Some("plan stdout")));
        let value = parsed(&json);
        assert_eq!(value["receipt"]["kind"], json!("plan"));
        assert_eq!(value["object"]["title"], json!("Ship structcopy"));
        let items = value["object"]["items"].as_array().expect("items");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["status"], json!("in_progress"));
    }

    #[test]
    fn workflow_copy_projects_existing_run_without_side_effects() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.current_session_id = Some("structcopy-workflow-test-session".to_string());

        // Unknown run, no state: honest error, and the read must not create
        // the workflow journal on disk.
        let missing = execute_structcopy(&mut app, Some("workflow nope stdout"));
        assert!(missing.is_error);
        assert!(
            missing
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unavailable"),
            "{:?}",
            missing.message
        );
        assert!(
            !tmpdir.path().join(".codewhale").exists(),
            "read-only copy must not create the workflow journal"
        );

        crate::tools::workflow::structcopy_test_seed_run(
            tmpdir.path(),
            "structcopy-test-run-alpha",
            app.current_session_id
                .as_deref()
                .expect("test session identity"),
        );
        let json = stdout_json(&execute_structcopy(
            &mut app,
            Some("workflow structcopy-test-run-alpha stdout"),
        ));
        let value = parsed(&json);
        assert_eq!(value["receipt"]["kind"], json!("workflow"));
        assert_eq!(
            value["object"]["run_id"],
            json!("structcopy-test-run-alpha")
        );
        assert_eq!(value["object"]["status"], json!("running"));
        assert_eq!(value["object"]["leaf_count"], Value::Null);
        assert_eq!(value["object"]["branch_count"], Value::Null);
        assert_eq!(value["object"]["control_count"], Value::Null);
        assert!(
            value["object"].get("source_path").is_none(),
            "filesystem paths must not leave the projection: {json}"
        );

        let unknown = execute_structcopy(&mut app, Some("workflow nope stdout"));
        assert!(unknown.is_error);
        let message = unknown.message.as_deref().unwrap_or_default();
        assert!(message.contains("unavailable"), "{message}");
        assert!(
            !message.contains("structcopy-test-run-alpha"),
            "unavailable errors must not enumerate private run ids: {message}"
        );
    }

    #[test]
    fn unavailable_selectors_are_reported_not_fabricated() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);

        let empty_turn = execute_structcopy(&mut app, Some("turn 1 stdout"));
        assert!(empty_turn.is_error);
        assert!(
            empty_turn
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unavailable"),
            "{:?}",
            empty_turn.message
        );

        let empty_plan = execute_structcopy(&mut app, Some("plan stdout"));
        assert!(empty_plan.is_error);
        assert!(
            empty_plan
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unavailable"),
            "{:?}",
            empty_plan.message
        );

        seed_transcript(&mut app);
        let out_of_range = execute_structcopy(&mut app, Some("turn 99 stdout"));
        assert!(out_of_range.is_error);
        assert!(
            out_of_range
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unavailable"),
            "{:?}",
            out_of_range.message
        );

        let missing_tool = execute_structcopy(&mut app, Some("tool call-nope stdout"));
        assert!(missing_tool.is_error);
        assert!(
            missing_tool
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unavailable"),
            "{:?}",
            missing_tool.message
        );

        for bad in [
            None,
            Some(""),
            Some("turn 0"),
            Some("turn x"),
            Some("turn -1"),
            Some("turn 99999999999999999999999999"),
            Some("plan extra"),
            Some("tool"),
            Some("workflow"),
            Some("stdout"),
            Some("   "),
        ] {
            let result = execute_structcopy(&mut app, bad);
            assert!(result.is_error, "{bad:?}: {:?}", result.message);
        }
    }

    #[test]
    fn command_feedback_uses_the_active_locale() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.ui_locale = Locale::ZhHans;

        let invalid = execute_structcopy(&mut app, Some("unknown"));
        assert!(invalid.is_error);
        let expected = tr(Locale::ZhHans, MessageId::CmdStructcopyUsageError)
            .replace("{usage}", COMMAND_INFO.usage);
        assert!(
            invalid
                .message
                .as_deref()
                .is_some_and(|message| message.ends_with(&expected)),
            "{:?}",
            invalid.message
        );

        let unavailable = execute_structcopy(&mut app, Some("plan stdout"));
        assert!(unavailable.is_error);
        let expected = tr(Locale::ZhHans, MessageId::CmdStructcopyUnavailable).replace(
            "{kind}",
            &tr(Locale::ZhHans, MessageId::CmdStructcopyKindPlan),
        );
        assert!(
            unavailable
                .message
                .as_deref()
                .is_some_and(|message| message.ends_with(&expected)),
            "{:?}",
            unavailable.message
        );
    }

    /// An unavailable selector is never echoed. An available selector is
    /// scrubbed and bounded in both the receipt and copied object.
    #[test]
    fn hostile_selectors_are_redacted_and_bounded_everywhere() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        let workspace = tmpdir.path().to_string_lossy().into_owned();
        seed_transcript(&mut app);

        // Unavailable selector: no attacker-influenced bytes are echoed.
        let hostile = format!(
            "\u{1b}[31mred\u{1b}[0m-Bearer-abcdef1234567890-https://u:p@evil.test/x?k=v#f-{workspace}-{}",
            "A".repeat(4096)
        );
        let result = execute_structcopy(&mut app, Some(&format!("tool {hostile} stdout")));
        assert!(result.is_error);
        let message = result.message.as_deref().unwrap_or_default();
        assert!(message.len() < 400, "status message unbounded: {message}");
        for forbidden in [
            "\u{1b}[31m",
            "abcdef1234567890",
            "u:p@evil.test",
            "k=v",
            workspace.as_str(),
        ] {
            assert!(
                !message.contains(forbidden),
                "leaked {forbidden:?}: {message}"
            );
        }
        assert!(!message.contains('\n'), "status label must be one line");
        assert!(message.contains("unavailable"), "{message}");

        // Receipt path: a long but *available* selector is bounded too.
        let long_id = format!("call-{}", "z".repeat(4096));
        app.api_messages[1].content.push(ContentBlock::ToolUse {
            id: long_id.clone(),
            name: "exec_command".to_string(),
            input: json!({}),
            caller: None,
            thought_signature: None,
        });
        let json = stdout_json(&execute_structcopy(
            &mut app,
            Some(&format!("tool {long_id} stdout")),
        ));
        let value = parsed(&json);
        let selector = value["receipt"]["selector"].as_str().expect("selector");
        assert!(
            selector.len() <= MAX_SELECTOR_BYTES,
            "selector {} bytes exceeds the {MAX_SELECTOR_BYTES}-byte cap",
            selector.len()
        );
        assert!(selector.ends_with('…'), "{selector}");

        // Composer selectors cannot contain a whitespace-delimited `Bearer`
        // header, so delimiter-shaped bearer tokens are scrubbed too.
        for bearer_id in [
            "call-Bearer-abcdef1234567890",
            "call-Bearer=zyxwvutsrqponmlk",
        ] {
            app.api_messages[1].content.push(ContentBlock::ToolUse {
                id: bearer_id.to_string(),
                name: "exec_command".to_string(),
                input: json!({}),
                caller: None,
                thought_signature: None,
            });
            let json = stdout_json(&execute_structcopy(
                &mut app,
                Some(&format!("tool {bearer_id} stdout")),
            ));
            assert!(!json.contains("abcdef1234567890"), "{json}");
            assert!(!json.contains("zyxwvutsrqponmlk"), "{json}");
            assert!(json.contains(BEARER_REDACTION_MARKER), "{json}");
        }
    }

    /// Object keys are attacker-influenced too (a model can name a tool-input
    /// field anything). Keys must be sanitized, bounded, and de-collided
    /// deterministically without dropping a value.
    #[test]
    fn hostile_object_keys_are_scrubbed_bounded_and_deduped_deterministically() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        let workspace = tmpdir.path().to_string_lossy().into_owned();

        // Three keys that collapse onto the same bounded form, one key with
        // ANSI + newlines, and one key carrying a workspace path.
        let long_a = format!("k{}A", "x".repeat(MAX_KEY_BYTES));
        let long_b = format!("k{}B", "x".repeat(MAX_KEY_BYTES));
        let long_c = format!("k{}C", "x".repeat(MAX_KEY_BYTES));
        let input = json!({
            long_a.clone(): 1,
            long_b.clone(): 2,
            long_c.clone(): 3,
            "\u{1b}[31mansi\u{1b}[0m\nkey": 4,
            format!("at {workspace}/src"): 5,
        });
        app.api_messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-keys".to_string(),
                name: "exec_command".to_string(),
                input,
                caller: None,
                thought_signature: None,
            }],
        }];

        let first = stdout_json(&execute_structcopy(&mut app, Some("tool call-keys stdout")));
        let second = stdout_json(&execute_structcopy(&mut app, Some("tool call-keys stdout")));
        assert_eq!(
            first, second,
            "key collision handling must be deterministic"
        );

        let value = parsed(&first);
        let object = value["object"]["input"].as_object().expect("input");
        // No value is lost to a collision.
        assert_eq!(object.len(), 5, "{object:?}");
        let mut values: Vec<u64> = object
            .values()
            .map(|item| item.as_u64().expect("number"))
            .collect();
        values.sort_unstable();
        assert_eq!(values, vec![1, 2, 3, 4, 5]);

        for key in object.keys() {
            assert!(
                key.len() <= MAX_KEY_BYTES,
                "key {} bytes exceeds the {MAX_KEY_BYTES}-byte cap",
                key.len()
            );
            assert!(!key.contains('\u{1b}'), "ANSI survived in key {key:?}");
            assert!(!key.contains('\n'), "newline survived in key {key:?}");
            assert!(!key.contains(&workspace), "workspace path in key {key:?}");
        }
        assert!(
            object.keys().any(|key| key.contains("<workspace>")),
            "{object:?}"
        );

        let counts = &value["receipt"]["counts"];
        assert_eq!(
            counts["object_keys_original"],
            counts["object_keys_retained"]
        );
        assert_eq!(counts["object_keys_truncated"], json!(3));
        assert!(
            counts["object_keys_deduped"].as_u64().expect("deduped") >= 2,
            "{counts}"
        );
        let reasons = value["receipt"]["reasons"].as_array().expect("reasons");
        assert!(
            reasons.contains(&json!("object_key_bytes_cap")),
            "{reasons:?}"
        );
        assert!(
            reasons.contains(&json!("object_key_collision")),
            "{reasons:?}"
        );
    }

    #[test]
    fn sensitive_keys_are_classified_after_control_and_ansi_normalization() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.api_messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-obfuscated-keys".to_string(),
                name: "exec_command".to_string(),
                input: json!({
                    "api\u{1b}[31m_key": "plain-value-that-must-not-leak",
                    "pass\u{7}word": "another-plain-value-that-must-not-leak",
                }),
                caller: None,
                thought_signature: None,
            }],
        }];

        let json = stdout_json(&execute_structcopy(
            &mut app,
            Some("tool call-obfuscated-keys stdout"),
        ));
        assert!(!json.contains("plain-value-that-must-not-leak"), "{json}");
        assert!(
            !json.contains("another-plain-value-that-must-not-leak"),
            "{json}"
        );
        let value = parsed(&json);
        assert_eq!(
            value["object"]["input"]["api_key"],
            json!(SENSITIVE_VALUE_REDACTION_MARKER)
        );
        assert_eq!(
            value["object"]["input"]["password"],
            json!(SENSITIVE_VALUE_REDACTION_MARKER)
        );
    }

    /// `unique_object_key` must terminate and preserve every value even when
    /// the key cap leaves no room at all for a base.
    #[test]
    fn key_dedup_terminates_under_a_degenerate_cap() {
        let mut map = serde_json::Map::new();
        for _ in 0..12 {
            let (key, _) = unique_object_key(&map, "");
            assert!(!map.contains_key(&key), "reused key {key:?}");
            map.insert(key, Value::Null);
        }
        assert_eq!(map.len(), 12, "every insert must survive");

        // Deterministic across runs with the same inputs.
        let mut replay = serde_json::Map::new();
        for _ in 0..12 {
            let (key, _) = unique_object_key(&replay, "");
            replay.insert(key, Value::Null);
        }
        let left: Vec<&String> = map.keys().collect();
        let right: Vec<&String> = replay.keys().collect();
        assert_eq!(left, right);

        assert_eq!(decimal_width(0), 1);
        assert_eq!(decimal_width(9), 1);
        assert_eq!(decimal_width(10), 2);
        assert_eq!(decimal_width(999), 3);
        assert_eq!(decimal_width(1000), 4);
    }

    #[test]
    fn collision_suffix_reserve_reports_its_own_truncation() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        let exact = "x".repeat(MAX_KEY_BYTES);
        let same_after_flatten = format!("{exact}\n");
        app.api_messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-reserve".to_string(),
                name: "exec_command".to_string(),
                input: json!({exact: 1, same_after_flatten: 2}),
                caller: None,
                thought_signature: None,
            }],
        }];

        let json = stdout_json(&execute_structcopy(
            &mut app,
            Some("tool call-reserve stdout"),
        ));
        let value = parsed(&json);
        let input = value["object"]["input"].as_object().expect("input");
        assert_eq!(input.len(), 2);
        assert!(input.keys().all(|key| key.len() <= MAX_KEY_BYTES));
        let counts = &value["receipt"]["counts"];
        assert_eq!(counts["object_keys_deduped"], json!(1));
        assert_eq!(counts["object_keys_truncated"], json!(1));
        let reasons = value["receipt"]["reasons"].as_array().expect("reasons");
        assert!(
            reasons.contains(&json!("object_key_collision")),
            "{reasons:?}"
        );
        assert!(
            reasons.contains(&json!("object_key_bytes_cap")),
            "{reasons:?}"
        );
    }

    #[test]
    fn output_is_deterministic_with_recursively_sorted_keys() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        seed_transcript(&mut app);

        let first = stdout_json(&execute_structcopy(&mut app, Some("tool call-7 stdout")));
        let second = stdout_json(&execute_structcopy(&mut app, Some("tool call-7 stdout")));
        assert_eq!(first, second, "output must be byte-for-byte deterministic");

        let value = parsed(&first);
        let top: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(top, ["object", "receipt"]);
        let receipt: Vec<&String> = value["receipt"]
            .as_object()
            .expect("receipt")
            .keys()
            .collect();
        let mut sorted = receipt.clone();
        sorted.sort();
        assert_eq!(receipt, sorted, "receipt keys must be sorted");
        let counts: Vec<&String> = value["receipt"]["counts"]
            .as_object()
            .expect("counts")
            .keys()
            .collect();
        let mut sorted_counts = counts.clone();
        sorted_counts.sort();
        assert_eq!(counts, sorted_counts, "counts keys must be sorted");
        let object: Vec<&String> = value["object"]
            .as_object()
            .expect("object")
            .keys()
            .collect();
        let mut sorted_object = object.clone();
        sorted_object.sort();
        assert_eq!(object, sorted_object, "object keys must be sorted");
    }

    #[test]
    fn hostile_content_is_redacted_before_serialization() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        let workspace = tmpdir.path().to_string_lossy().into_owned();
        app.api_messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!(
                    "escaped \\\"api_key\\\": \\\"sk-escapedsecret99\\\"\n\
                     bearer: Bearer abcdef1234567890\n\
                     jwt eyJhbGciOiJIUzI1NiIsFAKE.eyJGQUtFIjoiZml4dHVyZSJ9.FAKEFIXTURESIGNATUREnotasecret000\n\
                     url https://bob:s3cret@example.com/deep?session_token=xyz&ok=1#section\n\
                     path {workspace}/src/main.rs"
                ),
                cache_control: None,
            }],
        }];

        let json = stdout_json(&execute_structcopy(&mut app, Some("turn 1 stdout")));
        for forbidden in [
            "sk-escapedsecret99",
            "abcdef1234567890",
            "eyJhbGciOiJIUzI1NiIs",
            "s3cret",
            "session_token=xyz",
            "section",
            workspace.as_str(),
        ] {
            assert!(!json.contains(forbidden), "leaked {forbidden:?}: {json}");
        }
        assert!(json.contains("https://example.com/deep"), "{json}");
        assert!(json.contains("<workspace>/src/main.rs"), "{json}");
        assert!(parsed(&json).is_object());
    }

    /// URLs do not arrive as tidy whitespace-delimited tokens. Wrapped,
    /// embedded, uppercased, and malformed forms must all lose their
    /// userinfo, query, and fragment.
    #[test]
    fn urls_lose_userinfo_query_and_fragment_in_hostile_shapes() {
        let labels = no_labels();
        let cases = [
            "(https://u:p@host.test/a?q=1#f)",
            "<https://u:p@host.test/a?q=1#f>",
            "\"https://u:p@host.test/a?q=1#f\"",
            "'https://u:p@host.test/a?q=1#f'",
            "see https://u:p@host.test/a?q=1#f.",
            "see https://u:p@host.test/a?q=1#f, then",
            "[link](https://u:p@host.test/a?q=1#f)",
            "prefixhttps://u:p@host.test/a?q=1#f",
            "HTTPS://U:P@HOST.TEST/a?q=1#f",
            "ws://u:p@host.test/a?q=1#f",
            "ftp://u:p@host.test/a?q=1#f",
            "postgres://u:p@host.test/db?sslkey=secret#f",
            "mongodb://u:p@host.test/db?authSource=admin#f",
            "redis://u:p@host.test/0?token=secret#f",
            "amqp://u:p@host.test/vhost?token=secret#f",
            "ssh://u:p@host.test/repo?identity=secret#f",
            "socks5://u:p@host.test/path?token=secret#f",
            "trailing`https://u:p@host.test/a?q=1#f`",
            "a=https://u:p@host.test/a?q=1#f&b=2",
        ];
        for case in cases {
            let scrubbed = scrub_string(case, &labels);
            for forbidden in [
                "u:p@",
                "q=1",
                "#f",
                "P@HOST",
                "sslkey=secret",
                "authSource=admin",
                "token=secret",
                "identity=secret",
            ] {
                assert!(
                    !scrubbed.contains(forbidden),
                    "{case:?} kept {forbidden:?}: {scrubbed}"
                );
            }
            assert!(
                scrubbed.contains("host.test") || scrubbed.contains(URL_OMISSION_MARKER),
                "{case:?} -> {scrubbed}"
            );
        }

        // Two URLs in one string: both are scrubbed, order preserved.
        let both = scrub_string(
            "first https://a:b@one.test/x?y=1#z then https://c:d@two.test/w?v=2#u end",
            &labels,
        );
        assert!(both.contains("one.test"), "{both}");
        assert!(both.contains("two.test"), "{both}");
        assert!(both.starts_with("first "), "{both}");
        assert!(both.ends_with(" end"), "{both}");
        for forbidden in ["a:b@", "c:d@", "y=1", "v=2", "#z", "#u"] {
            assert!(!both.contains(forbidden), "kept {forbidden:?}: {both}");
        }

        // Unparseable but scheme-prefixed: fail closed, do not pass through.
        for hostile in [
            "https://",
            "https://[not-an-ipv6:1]/x?token=leak#f",
            "http://user:pw@:99999/x?token=leak",
        ] {
            let scrubbed = scrub_string(hostile, &labels);
            assert!(!scrubbed.contains("token=leak"), "{hostile} -> {scrubbed}");
            assert!(!scrubbed.contains("user:pw@"), "{hostile} -> {scrubbed}");
        }

        // An ANSI escape spliced into a scheme must not hide the URL from
        // the scanner: `sanitize_text` runs first.
        let hidden = scrub_string("htt\u{1b}[0mps://u:p@host.test/a?q=1#f", &labels);
        assert!(!hidden.contains("u:p@"), "{hidden}");
        assert!(!hidden.contains("q=1"), "{hidden}");

        // Text with no URL is untouched.
        assert_eq!(
            scrub_string("plain text, no url", &labels),
            "plain text, no url"
        );
    }

    /// Workspace/home paths retain useful labels. Every other absolute POSIX,
    /// drive-letter, and UNC path is removed from copied values.
    #[test]
    fn path_labels_preserve_known_roots_and_scrub_every_other_absolute_path() {
        let tmpdir = TempDir::new().expect("tempdir");
        let workspace = tmpdir.path().to_path_buf();
        let labels = PathLabels::new(&workspace);
        let literal = workspace.to_string_lossy().into_owned();

        let folded = labels.apply(&format!("open {literal}/src/main.rs now"));
        assert_eq!(folded, "open <workspace>/src/main.rs now");
        assert!(!folded.contains(&literal));
        assert_eq!(
            scrub_string(&format!("open {literal}/src/main.rs now"), &labels),
            "open <workspace>/src/main.rs now"
        );

        // The canonical form folds too (macOS /var -> /private/var).
        if let Ok(canonical) = workspace.canonicalize() {
            let canonical = canonical.to_string_lossy().into_owned();
            let folded = labels.apply(&format!("open {canonical}/src/main.rs"));
            assert_eq!(folded, "open <workspace>/src/main.rs");
        }

        // Repeated occurrences all fold, not just the first.
        let folded = labels.apply(&format!("{literal}/a and {literal}/b"));
        assert_eq!(folded, "<workspace>/a and <workspace>/b");

        // Prefix folding itself only handles known roots; the composed scrub
        // removes every foreign absolute path before serialization.
        let foreign = "/opt/other/place/file.txt";
        assert_eq!(labels.apply(foreign), foreign);
        assert_eq!(scrub_string(foreign, &labels), PATH_OMISSION_MARKER);
        assert_eq!(
            scrub_string(r"C:\Users\customer\secret.txt", &labels),
            PATH_OMISSION_MARKER
        );
        assert_eq!(
            scrub_string(r"\\server\private\customer.txt", &labels),
            PATH_OMISSION_MARKER
        );
        let spaced = scrub_string(
            "open /Volumes/Client Name/private file.txt then continue\nsecond line",
            &labels,
        );
        assert_eq!(spaced, format!("open {PATH_OMISSION_MARKER}\nsecond line"));

        // A workspace nested inside $HOME folds to <workspace>, not <home>.
        if let Some(home) = std::env::var_os("HOME") {
            let home = home.to_string_lossy().into_owned();
            if home.len() > 3 {
                let nested = PathLabels::new(Path::new(&format!("{home}/nested/ws")));
                let folded = nested.apply(&format!("{home}/nested/ws/src"));
                assert_eq!(folded, "<workspace>/src");
                assert_eq!(
                    nested.apply(&format!("{home}/elsewhere")),
                    "<home>/elsewhere"
                );
                assert_eq!(
                    scrub_string(&format!("{home}/elsewhere/file.rs"), &nested),
                    "<home>/elsewhere/file.rs"
                );
            }
        }
    }

    #[test]
    fn path_labels_require_component_boundaries_and_preserve_repeated_roots() {
        let labels = PathLabels {
            labels: vec![
                ("/opt/app".to_string(), "<workspace>"),
                ("/Users/alice".to_string(), "<home>"),
            ],
        };

        assert_eq!(labels.apply("/opt/app"), "<workspace>");
        assert_eq!(labels.apply("/opt/app/src"), "<workspace>/src");
        assert_eq!(labels.apply(r"/opt/app\src"), r"<workspace>\src");
        assert_eq!(
            labels.apply("/opt/app/a and /opt/app/b"),
            "<workspace>/a and <workspace>/b"
        );
        assert_eq!(labels.apply("/Users/alice"), "<home>");
        assert_eq!(
            labels.apply("/Users/alice/project and /Users/alice/other"),
            "<home>/project and <home>/other"
        );

        for collision in [
            "/opt/application/customer",
            "/opt/app-old/customer",
            "/Users/alice-old/private",
            "/Users/alice2/private",
        ] {
            assert_eq!(
                labels.apply(collision),
                collision,
                "near-prefix path must not receive a trusted label"
            );
            assert_eq!(
                scrub_string(collision, &labels),
                PATH_OMISSION_MARKER,
                "near-prefix path must remain foreign and be redacted"
            );
        }
    }

    #[test]
    fn absolute_paths_are_scrubbed_from_values_keys_and_selectors() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        let call_id = "call=/opt/customer/private-id";
        app.api_messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: call_id.to_string(),
                name: "exec_command".to_string(),
                input: json!({
                    "/Volumes/ClientSecret/source.rs": "open C:\\Users\\customer\\secret.txt",
                    "unc": r"\\server\private\customer.txt",
                }),
                caller: None,
                thought_signature: None,
            }],
        }];

        let json = stdout_json(&execute_structcopy(
            &mut app,
            Some(&format!("tool {call_id} stdout")),
        ));
        for forbidden in [
            "/opt/customer/private-id",
            "/Volumes/ClientSecret/source.rs",
            r"C:\Users\customer\secret.txt",
            r"\\server\private\customer.txt",
            "ClientSecret",
            "customer",
        ] {
            assert!(!json.contains(forbidden), "leaked {forbidden:?}: {json}");
        }
        assert!(json.contains(PATH_OMISSION_MARKER), "{json}");
    }

    #[test]
    fn string_bytes_cap_truncates_grapheme_safely() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.api_messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "emoji cluster test: 👨‍👩‍👧‍👦🏳️‍🌈 repeated many times over".repeat(20),
                cache_control: None,
            }],
        }];
        let caps = Caps {
            max_string_bytes: 40,
            ..DEFAULT_CAPS
        };
        let json = render_copy(&app, &CopyKind::Turn(1), &caps).expect("render");
        let value = parsed(&json);
        let text = value["object"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.ends_with('…'), "{text}");
        assert!(text.len() <= 40, "{} bytes", text.len());
        assert_eq!(value["receipt"]["counts"]["strings_truncated"], json!(1));
        assert_eq!(value["receipt"]["reasons"], json!(["string_bytes_cap"]));
        let original = value["receipt"]["counts"]["string_bytes_original"]
            .as_u64()
            .expect("original");
        let retained = value["receipt"]["counts"]["string_bytes_retained"]
            .as_u64()
            .expect("retained");
        assert!(original > retained);
    }

    /// A cap below the ellipsis's own 3 bytes has no representable
    /// "truncated" form. It must stay in-bounds and stay honest rather than
    /// panic, overflow, or emit partial content.
    #[test]
    fn string_cap_below_the_ellipsis_is_safe() {
        for max_bytes in 0..=4usize {
            for text in ["", "a", "ab", "abc", "abcd", "é", "👨‍👩‍👧‍👦", "héllo wörld"]
            {
                let (out, truncated) = truncate_string_grapheme_safe(text, max_bytes);
                assert!(
                    out.len() <= max_bytes.max(text.len()),
                    "cap {max_bytes} text {text:?} -> {out:?}"
                );
                if text.len() <= max_bytes {
                    assert!(!truncated);
                    assert_eq!(out, text);
                } else {
                    assert!(truncated, "cap {max_bytes} text {text:?}");
                    assert!(
                        out.len() <= max_bytes,
                        "cap {max_bytes} text {text:?} -> {} bytes",
                        out.len()
                    );
                    if max_bytes < 3 {
                        assert!(
                            out.is_empty(),
                            "no partial content may escape below the marker size: {out:?}"
                        );
                    } else {
                        assert!(out.ends_with('…'), "cap {max_bytes} -> {out:?}");
                    }
                }
                assert!(std::str::from_utf8(out.as_bytes()).is_ok());
            }
        }

        // End to end: the whole pipeline survives a sub-ellipsis cap and the
        // receipt still reports the truncation.
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.api_messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "a longer body that cannot fit".to_string(),
                cache_control: None,
            }],
        }];
        let caps = Caps {
            max_string_bytes: 1,
            ..DEFAULT_CAPS
        };
        let json = render_copy(&app, &CopyKind::Turn(1), &caps).expect("render");
        let value = parsed(&json);
        assert_eq!(value["object"]["content"][0]["text"], json!(""));
        assert!(
            value["receipt"]["counts"]["strings_truncated"]
                .as_u64()
                .expect("truncated")
                >= 1
        );
    }

    #[test]
    fn array_items_cap_counts_original_and_retained_exactly() {
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir);
        {
            let mut state = app.plan_state.try_lock().expect("plan lock");
            state.update(UpdatePlanArgs {
                plan: (0..10)
                    .map(|index| PlanItemArg {
                        step: format!("step {index}"),
                        status: StepStatus::Pending,
                    })
                    .collect(),
                ..Default::default()
            });
        }
        let caps = Caps {
            max_array_items: 3,
            ..DEFAULT_CAPS
        };
        let json = render_copy(&app, &CopyKind::Plan, &caps).expect("render");
        let value = parsed(&json);
        assert_eq!(value["object"]["items"].as_array().expect("items").len(), 3);
        assert_eq!(
            value["receipt"]["counts"]["array_items_original"],
            json!(10)
        );
        assert_eq!(value["receipt"]["counts"]["array_items_retained"], json!(3));
        assert_eq!(value["receipt"]["reasons"], json!(["array_items_cap"]));
    }

    #[test]
    fn depth_cap_omits_deep_subtrees() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.api_messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-deep".to_string(),
                name: "exec_command".to_string(),
                input: json!({"a": {"b": {"c": {"d": {"e": "too deep"}}}}}),
                caller: None,
                thought_signature: None,
            }],
        }];
        let caps = Caps {
            max_depth: 3,
            ..DEFAULT_CAPS
        };
        let json =
            render_copy(&app, &CopyKind::Tool("call-deep".to_string()), &caps).expect("render");
        let value = parsed(&json);
        assert!(json.contains(DEPTH_OMISSION_MARKER), "{json}");
        assert!(!json.contains("too deep"), "{json}");
        let omissions = value["receipt"]["counts"]["depth_omissions"]
            .as_u64()
            .expect("omissions");
        assert!(omissions >= 1, "{omissions}");
        assert!(
            value["receipt"]["reasons"]
                .as_array()
                .expect("reasons")
                .contains(&json!("depth_cap"))
        );
    }

    /// The original counts describe the full redacted tree; the retained
    /// counts describe exactly what was emitted, marker strings included.
    /// Both must be checkable against the artifact itself.
    #[test]
    fn counts_stay_exact_across_a_depth_omission() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        // Two strings and two array items live below the depth cut, plus one
        // string and one array item above it.
        app.api_messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-counts".to_string(),
                name: "exec_command".to_string(),
                input: json!({
                    "shallow": ["kept"],
                    "deep": {"one": {"two": ["cut-a", "cut-b"]}},
                }),
                caller: None,
                thought_signature: None,
            }],
        }];
        let caps = Caps {
            max_depth: 3,
            ..DEFAULT_CAPS
        };
        let json =
            render_copy(&app, &CopyKind::Tool("call-counts".to_string()), &caps).expect("render");
        let value = parsed(&json);
        let counts = &value["receipt"]["counts"];

        // Independently recount the emitted object and compare.
        let mut emitted = BoundStats::default();
        collect_original_counts(&value["object"], &mut emitted);
        assert_eq!(
            counts["strings_retained"].as_u64().expect("retained"),
            emitted.strings_total,
            "retained string count must match the emitted artifact: {json}"
        );
        assert_eq!(
            counts["string_bytes_retained"]
                .as_u64()
                .expect("retained bytes"),
            emitted.string_bytes_original,
            "retained bytes must include the depth marker: {json}"
        );
        assert_eq!(
            counts["array_items_retained"].as_u64().expect("items"),
            emitted.array_items_original,
            "{json}"
        );

        // Originals cover the *whole* tree, including the omitted subtree.
        assert!(
            counts["strings_total"].as_u64().expect("total")
                > counts["strings_retained"].as_u64().expect("retained"),
            "originals must count strings under the depth cut: {counts}"
        );
        assert!(
            counts["array_items_original"].as_u64().expect("original")
                > counts["array_items_retained"].as_u64().expect("retained"),
            "originals must count array items under the depth cut: {counts}"
        );
        assert_eq!(counts["depth_omissions"], json!(1));
        assert!(
            counts["object_keys_original"]
                .as_u64()
                .expect("original keys")
                > counts["object_keys_retained"]
                    .as_u64()
                    .expect("retained keys"),
            "keys under the depth cut must be original-only: {counts}"
        );
    }

    #[test]
    fn omitted_key_transformations_do_not_claim_emitted_reasons() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        let long_a = format!("{}A", "private-key-name-".repeat(32));
        let long_b = format!("{}B", "private-key-name-".repeat(32));
        app.api_messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call-deep-keys".to_string(),
                name: "exec_command".to_string(),
                input: json!({"deep": {"one": {long_a: 1, long_b: 2}}}),
                caller: None,
                thought_signature: None,
            }],
        }];
        let caps = Caps {
            max_depth: 3,
            ..DEFAULT_CAPS
        };
        let json = render_copy(&app, &CopyKind::Tool("call-deep-keys".to_string()), &caps)
            .expect("render");
        let value = parsed(&json);
        let counts = &value["receipt"]["counts"];
        assert!(
            counts["object_keys_original"].as_u64().expect("original")
                > counts["object_keys_retained"].as_u64().expect("retained"),
            "{counts}"
        );
        assert_eq!(counts["object_keys_truncated"], json!(0));
        assert_eq!(counts["object_keys_deduped"], json!(0));
        let reasons = value["receipt"]["reasons"].as_array().expect("reasons");
        assert!(
            !reasons.contains(&json!("object_key_bytes_cap")),
            "{reasons:?}"
        );
        assert!(
            !reasons.contains(&json!("object_key_collision")),
            "{reasons:?}"
        );
    }

    #[test]
    fn output_bytes_cap_omits_payload_then_fails_closed() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        {
            let mut state = app.plan_state.try_lock().expect("plan lock");
            state.update(UpdatePlanArgs {
                title: Some("large plan".to_string()),
                plan: (0..60)
                    .map(|index| PlanItemArg {
                        step: format!("step {index}: {}", "padding ".repeat(40)),
                        status: StepStatus::Pending,
                    })
                    .collect(),
                ..Default::default()
            });
        }

        // Tight byte cap: payload must be omitted while the receipt survives.
        let caps = Caps {
            max_output_bytes: 2 * 1024,
            ..DEFAULT_CAPS
        };
        let json = render_copy(&app, &CopyKind::Plan, &caps).expect("render");
        assert!(json.len() <= 2 * 1024, "{} bytes", json.len());
        let value = parsed(&json);
        assert_eq!(value["object"], Value::Null);
        let reasons = value["receipt"]["reasons"].as_array().expect("reasons");
        assert!(
            reasons.contains(&json!("payload_omitted_output_bytes_cap")),
            "{reasons:?}"
        );
        // Nothing was emitted, so no retained counter and no bounding reason
        // may claim otherwise.
        for retained in [
            "array_items_retained",
            "string_bytes_retained",
            "strings_retained",
            "strings_truncated",
            "depth_omissions",
            "object_keys_retained",
            "object_keys_truncated",
            "object_keys_deduped",
        ] {
            assert_eq!(
                value["receipt"]["counts"][retained],
                json!(0),
                "{retained} must be zero when nothing was emitted: {json}"
            );
        }
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert_eq!(
            value["receipt"]["counts"]["array_items_original"],
            json!(60)
        );
        assert!(
            value["receipt"]["counts"]["object_keys_original"]
                .as_u64()
                .expect("original keys")
                > 0
        );

        // Below the metadata floor the command fails closed and emits nothing.
        let tiny = Caps {
            max_output_bytes: 64,
            ..DEFAULT_CAPS
        };
        let err = render_copy(&app, &CopyKind::Plan, &tiny).expect_err("must fail closed");
        assert!(err.contains("refusing to emit"), "{err}");
        let result = execute_structcopy(&mut app, Some("plan stdout"));
        assert!(!result.is_error, "default caps fit: {:?}", result.message);
    }

    /// When the byte cap forces tighter caps than the declared contract, the
    /// receipt must say so instead of advertising caps that never ran.
    #[test]
    fn receipt_reports_the_caps_that_actually_ran() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        {
            let mut state = app.plan_state.try_lock().expect("plan lock");
            state.update(UpdatePlanArgs {
                title: Some("padded plan".to_string()),
                plan: (0..40)
                    .map(|index| PlanItemArg {
                        step: format!("step {index}: {}", "padding ".repeat(30)),
                        status: StepStatus::Pending,
                    })
                    .collect(),
                ..Default::default()
            });
        }
        let caps = Caps {
            max_output_bytes: 6 * 1024,
            ..DEFAULT_CAPS
        };
        let json = render_copy(&app, &CopyKind::Plan, &caps).expect("render");
        let value = parsed(&json);
        assert_eq!(
            value["receipt"]["caps"]["max_output_bytes"],
            json!(6 * 1024)
        );
        let applied = &value["receipt"]["applied_caps"];
        assert!(
            applied["max_array_items"].as_u64().expect("items")
                <= DEFAULT_CAPS.max_array_items as u64
        );
        if applied != &value["receipt"]["caps"] {
            assert!(
                value["receipt"]["reasons"]
                    .as_array()
                    .expect("reasons")
                    .contains(&json!("caps_tightened_output_bytes_cap")),
                "{json}"
            );
        }

        // The unconstrained case declares no tightening.
        let json = stdout_json(&execute_structcopy(&mut app, Some("plan stdout")));
        let value = parsed(&json);
        assert_eq!(value["receipt"]["applied_caps"], value["receipt"]["caps"]);
        assert!(
            !value["receipt"]["reasons"]
                .as_array()
                .expect("reasons")
                .contains(&json!("caps_tightened_output_bytes_cap"))
        );
    }

    #[test]
    fn clipboard_is_default_and_stdout_is_explicit() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        seed_transcript(&mut app);

        // Default: clipboard target; the payload never appears in the message.
        let default = execute_structcopy(&mut app, Some("turn 1"));
        assert!(!default.is_error, "{:?}", default.message);
        let message = default.message.as_deref().unwrap_or_default();
        assert!(message.contains("handed to the clipboard"), "{message}");
        // The receipt must not overclaim delivery.
        assert!(
            !message.contains("copied to the local clipboard"),
            "{message}"
        );
        assert!(!message.contains("\"receipt\""), "{message}");
        let payload = app
            .clipboard
            .last_written_text()
            .expect("clipboard payload");
        assert!(payload.contains("\"receipt\""));

        // Explicit stdout: payload in the message, clipboard untouched.
        let mut app = test_app(&tmpdir);
        seed_transcript(&mut app);
        let stdout = execute_structcopy(&mut app, Some("turn 1 stdout"));
        assert!(
            stdout
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("\"receipt\"")
        );
        assert!(app.clipboard.last_written_text().is_none());
    }

    /// The terminal-client path queues a background write; the message must
    /// not claim the copy landed, and must not claim a transport the session
    /// does not have.
    #[test]
    fn terminal_client_receipt_says_queued_not_delivered() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        seed_transcript(&mut app);
        app.clipboard = ClipboardHandler::for_test(true, false);
        assert!(app.clipboard.requires_terminal_paste());

        let result = execute_structcopy(&mut app, Some("turn 1"));
        assert!(!result.is_error, "{:?}", result.message);
        let message = result.message.as_deref().unwrap_or_default();
        assert!(message.contains("queued"), "{message}");
        assert!(message.contains("not confirmed"), "{message}");
        assert!(
            !message.contains("copied to"),
            "must not claim delivery: {message}"
        );
    }

    #[test]
    fn clipboard_failure_is_honest_and_suggests_stdout() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        seed_transcript(&mut app);
        app.clipboard = ClipboardHandler::unavailable_for_test(false);

        let failed = execute_structcopy(&mut app, Some("turn 1"));
        assert!(failed.is_error);
        let message = failed.message.as_deref().unwrap_or_default();
        assert!(message.contains("Nothing was written"), "{message}");
        assert!(message.contains("stdout"), "{message}");
        assert!(app.clipboard.last_written_text().is_none());
    }

    #[test]
    fn copy_does_not_mutate_session_state() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        seed_transcript(&mut app);
        {
            let mut state = app.plan_state.try_lock().expect("plan lock");
            state.update(UpdatePlanArgs {
                title: Some("immutable".to_string()),
                ..Default::default()
            });
        }
        let plan_before = app.plan_state.try_lock().expect("plan lock").snapshot();
        let messages_before = app.api_messages.clone();
        let history_before = app.history.len();
        let work_before = app.work_state_snapshot().expect("Work snapshot");

        for arg in [
            "turn 1 stdout",
            "turn 2",
            "tool call-7 stdout",
            "plan stdout",
            "turn 99 stdout",
            "tool call-nope stdout",
            "workflow nope stdout",
        ] {
            let _ = execute_structcopy(&mut app, Some(arg));
        }

        assert_eq!(app.api_messages, messages_before);
        assert_eq!(app.history.len(), history_before);
        assert_eq!(
            app.plan_state.try_lock().expect("plan lock").snapshot(),
            plan_before
        );
        assert_eq!(
            app.work_state_snapshot().expect("Work snapshot after copy"),
            work_before,
            "structcopy must not mutate Work"
        );
    }

    #[test]
    fn structcopy_is_registered_human_only_and_absent_from_model_catalog() {
        // Registered as a human slash command.
        assert!(
            crate::commands::command_infos()
                .iter()
                .any(|info| info.name == "structcopy"),
            "structcopy must be a registered slash command"
        );

        // Never a model-visible tool: neither in the native tool catalog nor
        // in the legacy tool registry surface sent to providers.
        assert!(
            !crate::core::engine::default_active_native_tool_names().contains(&"structcopy"),
            "structcopy must not be a native tool"
        );
        let tmpdir = TempDir::new().expect("tempdir");
        let context = crate::tools::spec::ToolContext::new(tmpdir.path().to_path_buf());
        let registry = crate::tools::ToolRegistryBuilder::new()
            .with_file_tools()
            .with_read_only_file_tools()
            .with_shell_tools()
            .with_search_tools()
            .with_git_tools()
            .with_git_history_tools()
            .with_diagnostics_tool()
            .with_skill_tools()
            .with_validation_tools()
            .with_project_tools()
            .with_test_runner_tool()
            .with_tool_result_retrieval_tool()
            .with_web_tools()
            .with_finance_tool()
            .build(context);
        let names: Vec<String> = registry
            .to_api_tools()
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert!(
            !names.is_empty(),
            "builder surface must register model tools for this contract to be meaningful"
        );
        assert!(
            !names.iter().any(|name| name.contains("structcopy")),
            "no model tool may reference structcopy: {names:?}"
        );
    }
}
