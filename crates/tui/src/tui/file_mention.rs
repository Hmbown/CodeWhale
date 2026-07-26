//! `@`-mention parsing, completion, and expansion for the composer.
//!
//! Two responsibilities live here:
//!
//! 1. **Tab-completion** at the cursor — `try_autocomplete_file_mention` is
//!    called by the composer's Tab handler. Walks the workspace, ranks
//!    candidates by prefix-then-substring match, and either splices the
//!    completion in directly (single match), extends to a shared prefix, or
//!    surfaces options in the status line.
//! 2. **Expansion before send** — when the user hits Enter on a message that
//!    contains `@<path>` references, `user_request_with_file_mentions`
//!    appends a "Local context from @mentions" block with the file contents
//!    (or directory listings, or media-attachment hints) so the model can see
//!    what the user pointed at. Capped per-message and per-file.
//!
//! The module is deliberately self-contained: nothing inside reaches into UI
//! widgets or rendering, so it stays unit-testable from `ui/tests.rs` and
//! from its own module-level tests.
//!
//! Pulled out of `ui.rs` to shrink the 5,500-line monolith and to give the
//! mention logic a single home that future maintainers can find without
//! grepping for `@` across half the codebase.

use std::fmt::Write;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tui::app::{App, MentionCompletionCache};
use crate::tui::git_mention::{self, GitMentionKind};
use crate::tui::mention_completion::{MentionDiscoveryBehavior, MentionDiscoveryKey};
use crate::working_set::Workspace;

/// Maximum number of `@`-mentions whose contents are inlined into one user
/// message. Beyond this we stop appending blocks but the raw `@token` text
/// remains in the message.
pub const MAX_FILE_MENTIONS_PER_MESSAGE: usize = 8;
/// Per-file byte ceiling when inlining mention contents.
pub const MAX_MENTION_FILE_BYTES: u64 = 128 * 1024;
/// Per-directory entry ceiling when inlining a directory listing.
pub const MAX_DIRECTORY_MENTION_ENTRIES: usize = 80;

/// Maximum file-mention completion candidates to consider per keypress. Caps
/// the cost of walking large workspaces; subsequent keystrokes narrow further.
const FILE_MENTION_COMPLETION_LIMIT: usize = 64;

/// Compact composer preview row for local context. `included=false` also
/// covers lexical `@` mentions whose exact inclusion is resolved on send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMentionPreview {
    pub kind: String,
    pub label: String,
    pub detail: Option<String>,
    pub included: bool,
    pub removable: bool,
}

/// Durable, compact metadata for a user-visible context reference.
///
/// The transcript keeps the user's compact text (`@path` or `[Attached ...]`)
/// readable. This record preserves the exact target and inclusion state for
/// the context inspector and for session resume without leaking raw metadata
/// into the visible history cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextReference {
    pub kind: ContextReferenceKind,
    pub source: ContextReferenceSource,
    /// Short badge for terminal display, e.g. `file`, `dir`, `image`.
    pub badge: String,
    /// Compact display label from the transcript, without the leading `@`.
    pub label: String,
    /// Resolved target path or URI-equivalent string.
    pub target: String,
    pub included: bool,
    pub expanded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextReferenceKind {
    File,
    Directory,
    Missing,
    Unsupported,
    MediaMention,
    MediaAttachment,
    /// `@git` / `@diff` — curated git context rather than a path (#4067).
    GitContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextReferenceSource {
    AtMention,
    Attachment,
}

// ---------------------------------------------------------------------------
//  Tab-completion
// ---------------------------------------------------------------------------

/// If the cursor sits inside a `@<partial>` token in the input, return the
/// byte offset where the `@` starts (so we can splice in a completion) and
/// the partial path the user has typed so far. The token stops at whitespace
/// or the end of input. Returns `None` when the cursor is outside any mention
/// or the token is empty (`@` with nothing after it).
pub fn partial_file_mention_at_cursor(input: &str, cursor_chars: usize) -> Option<(usize, String)> {
    let chars: Vec<char> = input.chars().collect();
    if cursor_chars > chars.len() {
        return None;
    }
    // Walk left from the cursor until we find an `@` or a whitespace; if
    // whitespace comes first the cursor isn't inside a mention.
    let mut start_chars = cursor_chars;
    while start_chars > 0 {
        let prev = chars[start_chars - 1];
        if prev == '@' {
            start_chars -= 1;
            break;
        }
        if prev.is_whitespace() {
            return None;
        }
        start_chars -= 1;
    }
    if start_chars == cursor_chars || chars.get(start_chars) != Some(&'@') {
        return None;
    }
    // Confirm the `@` itself is at a valid mention boundary.
    if !is_file_mention_start(&chars, start_chars) {
        return None;
    }
    // Consume from the `@` to the next whitespace (the end of the token).
    let mut end_chars = start_chars + 1;
    while end_chars < chars.len() && !chars[end_chars].is_whitespace() {
        end_chars += 1;
    }
    let partial: String = chars[start_chars + 1..end_chars].iter().collect();
    let byte_start: usize = chars[..start_chars].iter().map(|c| c.len_utf8()).sum();
    Some((byte_start, partial))
}

/// Cwd-aware completion entry point. Shares its walker with the future
/// Ctrl+P fuzzy picker (#97); see [`Workspace::completions`] for the
/// ranking + display rules.
#[cfg(test)]
pub fn find_file_mention_completions(
    workspace: &Workspace,
    partial: &str,
    limit: usize,
) -> Vec<String> {
    let entries = workspace.completions(partial, limit);
    // #441: re-rank by frecency so files the user mentions a lot float up.
    // Never-mentioned candidates fall back to the workspace ranker's order.
    let entries = super::file_frecency::rerank_by_frecency(entries);
    tracing::debug!(
        target: "codewhale_tui::file_mention",
        partial = %partial,
        workspace = %workspace.root.display(),
        cwd = ?std::env::current_dir().ok(),
        match_count = entries.len(),
        "file mention completion walk",
    );
    entries
}

/// Resolve the `@`-mention completion popup contents for the current
/// composer state. Returns an empty `Vec` when:
///
/// - The popup is suppressed (`app.mention_menu_hidden`).
/// - The cursor is not inside an `@<partial>` token.
/// - The workspace walk produced no candidates.
///
/// Mirrors `visible_slash_menu_entries` so the composer widget can treat
/// both menus identically (one `Vec<String>` of entries, one selected index).
///
/// Once the composer widget is extended to render this as a popup, it will
/// pair with `apply_mention_menu_selection` for the Up/Down/Enter flow.
#[must_use]
pub fn visible_mention_menu_entries(app: &mut App, limit: usize) -> Vec<String> {
    if app.mention_menu_hidden {
        app.composer.mention_discovery.cancel();
        return Vec::new();
    }
    let Some((_byte_start, partial)) =
        partial_file_mention_at_cursor(&app.input, app.cursor_position)
    else {
        app.composer.mention_discovery.cancel();
        return Vec::new();
    };
    if limit == 0 {
        app.composer.mention_discovery.cancel();
        return Vec::new();
    }

    mention_menu_entries(app, &partial, limit).0
}

/// Drain a completed discovery result without waiting. The event loop calls
/// this once per tick so a finished scan repaints the popup even when the user
/// has stopped typing.
pub(crate) fn poll_background_mention_discovery(app: &mut App) -> bool {
    if app.composer.mention_discovery.poll() {
        app.composer.mention_completion_cache = None;
        return true;
    }
    false
}

/// Return `(entries, ready)`. `ready = false` means discovery is still in the
/// background; callers must not misreport that temporary empty result as "no
/// matches".
fn mention_menu_entries(app: &mut App, partial: &str, limit: usize) -> (Vec<String>, bool) {
    if poll_background_mention_discovery(app) {
        app.needs_redraw = true;
    }

    let workspace = app.workspace.clone();
    let cwd = app.composer.mention_cwd.clone();
    let walk_depth = app.mention_walk_depth;
    let behavior = app.mention_menu_behavior.clone();
    let follow_links = app.workspace_follow_symlinks;
    let discovery_key = if behavior == "browser" {
        MentionDiscoveryKey::browser(
            workspace.clone(),
            cwd.clone(),
            walk_depth,
            follow_links,
            partial.to_string(),
        )
    } else {
        MentionDiscoveryKey::fuzzy(workspace.clone(), cwd.clone(), walk_depth, follow_links)
    };
    app.composer
        .mention_discovery
        .ensure_requested(discovery_key.clone());

    if let Some(ref cache) = app.composer.mention_completion_cache
        && cache.workspace == workspace
        && cache.cwd == cwd
        && cache.partial == partial
        && cache.limit == limit
        && cache.walk_depth == walk_depth
        && cache.behavior == behavior
        && cache.follow_links == follow_links
    {
        return (cache.entries.clone(), true);
    }

    let Some(candidates) = app
        .composer
        .mention_discovery
        .cached_entries(&discovery_key)
    else {
        return (Vec::new(), false);
    };
    let entries = match &discovery_key.behavior {
        MentionDiscoveryBehavior::Fuzzy => {
            let ranked = crate::working_set::rank_completion_candidates(candidates, partial, limit);
            super::file_frecency::rerank_by_frecency(ranked)
        }
        MentionDiscoveryBehavior::Browser { .. } => {
            candidates.iter().take(limit).cloned().collect()
        }
    };

    let entries = with_git_mention_entries(entries, partial, limit);

    app.composer.mention_completion_cache = Some(MentionCompletionCache {
        workspace,
        cwd,
        partial: partial.to_string(),
        limit,
        walk_depth,
        behavior,
        follow_links,
        entries: entries.clone(),
    });

    (entries, true)
}

/// Prepend the `@git` / `@diff` tokens that prefix-match `partial` to the path
/// completions, so curated git context is discoverable from the same menu as
/// files (#4067). They lead because a two-entry prefix match is what the user
/// meant when they typed `gi` or `di`, and paths still fill the rest.
///
/// A bare `@` is deliberately left alone: that menu is the file picker, and
/// pushing two fixed tokens above every path would cost a slot on every
/// mention the user makes. The tokens appear as soon as a matching character
/// is typed.
fn with_git_mention_entries(entries: Vec<String>, partial: &str, limit: usize) -> Vec<String> {
    let needle = partial.trim().to_lowercase();
    if needle.is_empty() {
        return entries;
    }
    let matching: Vec<String> = GitMentionKind::iter_all()
        .filter(|kind| kind.token().starts_with(&needle))
        .map(|kind| kind.token().to_string())
        .collect();
    if matching.is_empty() {
        return entries;
    }
    let mut combined = matching;
    for entry in entries {
        if combined.len() >= limit {
            break;
        }
        if !combined.contains(&entry) {
            combined.push(entry);
        }
    }
    combined
}

/// Apply the currently selected `@`-mention popup entry to the composer
/// input, splicing it in place of the `@<partial>` token at the cursor.
/// Returns `true` if a substitution occurred.
///
/// Designed to be invoked by the same keybinding that drives
/// `apply_slash_menu_selection` (Enter / Tab); the caller is responsible
/// for choosing which menu is "active" based on cursor context.
pub fn apply_mention_menu_selection(app: &mut App, entries: &[String]) -> bool {
    if entries.is_empty() {
        return false;
    }
    let Some((byte_start, partial)) =
        partial_file_mention_at_cursor(&app.input, app.cursor_position)
    else {
        return false;
    };
    let selected_idx = app
        .mention_menu_selected
        .min(entries.len().saturating_sub(1));
    let replacement = &entries[selected_idx];
    // #441: bump this path's frecency before we splice it in. The store
    // persists asynchronously, so this never blocks input handling.
    super::file_frecency::record_mention(replacement);
    replace_file_mention(app, byte_start, &partial, replacement);
    app.mention_menu_hidden = false;
    app.status_message = Some(format!("Attached @{replacement}"));
    true
}

/// Tab-completion handler for `@file` mentions. Mirrors the slash-command
/// flow: a single match is applied directly; multiple matches with a longer
/// shared prefix extend the partial; otherwise the first few candidates are
/// surfaced via the status line. Returns true when the input was modified or
/// a suggestion was offered, so the caller can short-circuit other handlers.
pub fn try_autocomplete_file_mention(app: &mut App) -> bool {
    let Some((byte_start, partial)) =
        partial_file_mention_at_cursor(&app.input, app.cursor_position)
    else {
        return false;
    };
    let (candidates, ready) = mention_menu_entries(app, &partial, FILE_MENTION_COMPLETION_LIMIT);
    if !ready {
        return true;
    }
    if candidates.is_empty() {
        app.status_message = Some(no_file_mention_matches_status(
            &partial,
            app.mention_walk_depth,
        ));
        return true;
    }
    if candidates.len() == 1 {
        // #441: a unique-match completion is also a "mention" for ranking.
        super::file_frecency::record_mention(&candidates[0]);
        replace_file_mention(app, byte_start, &partial, &candidates[0]);
        app.status_message = Some(format!("Attached @{}", candidates[0]));
        return true;
    }
    let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    let shared = longest_common_prefix(&candidate_refs);
    if shared.len() > partial.len() {
        replace_file_mention(app, byte_start, &partial, shared);
        app.status_message = Some(format!("@{shared}…"));
        return true;
    }
    let preview = candidates
        .iter()
        .take(5)
        .map(|c| format!("@{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    app.status_message = Some(format!("Matches: {preview}"));
    true
}

fn no_file_mention_matches_status(partial: &str, walk_depth: usize) -> String {
    if path_partial_reaches_walk_depth(partial, walk_depth) {
        format!(
            "No files match @{partial} (mention_walk_depth={walk_depth}; use /config set mention_walk_depth 0 to search deeper)"
        )
    } else {
        format!("No files match @{partial}")
    }
}

fn path_partial_reaches_walk_depth(partial: &str, walk_depth: usize) -> bool {
    if walk_depth == 0 {
        return false;
    }
    let component_count = partial
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .count();
    component_count >= walk_depth
}

/// Splice a completion into the input, replacing the `@<partial>` token at
/// `byte_start` with `@<replacement>`. Cursor moves to the end of the new
/// token so further keystrokes extend (or escape via space) naturally.
fn replace_file_mention(app: &mut App, byte_start: usize, partial: &str, replacement: &str) {
    let original_token_len = '@'.len_utf8() + partial.len();
    let original_token_end = byte_start + original_token_len;
    let mut new_input =
        String::with_capacity(app.input.len() - original_token_len + 1 + replacement.len());
    new_input.push_str(&app.input[..byte_start]);
    new_input.push('@');
    new_input.push_str(replacement);
    if original_token_end < app.input.len() {
        new_input.push_str(&app.input[original_token_end..]);
    }
    let new_cursor_chars =
        app.input[..byte_start].chars().count() + 1 + replacement.chars().count();
    app.input = new_input;
    app.cursor_position = new_cursor_chars;
}

pub fn longest_common_prefix<'a>(values: &[&'a str]) -> &'a str {
    let Some(first) = values.first().copied() else {
        return "";
    };
    let mut end = first.len();

    for value in values.iter().skip(1) {
        while end > 0 && !value.starts_with(&first[..end]) {
            end -= 1;
            // Ensure we land on a valid UTF-8 char boundary.
            while end > 0 && !first.is_char_boundary(end) {
                end -= 1;
            }
        }
        if end == 0 {
            return "";
        }
    }

    &first[..end]
}

// ---------------------------------------------------------------------------
//  Expansion at send-time
// ---------------------------------------------------------------------------

/// Append a "Local context from @mentions" block to the user's message when
/// any `@path` references are present. Returns the input unchanged when
/// there are none.
///
/// `cwd` carries the user's launch directory and drives the second
/// resolution pass (issue #101): relative `@<path>` mentions resolve under
/// `cwd` when `workspace.join(path)` doesn't exist, so the user's mental
/// anchor (their shell's pwd) wins when it diverges from `--workspace`.
/// Pass `None` to disable the cwd pass entirely (workspace-only).
///
/// Resolution here is deliberately exact. Fuzzy discovery belongs in the
/// background completion popup; silently resolving a manually typed basename
/// would require a tree walk on submit and could attach an arbitrary same-name
/// file from a nested directory.
pub fn user_request_with_file_mentions(
    input: &str,
    workspace: &Path,
    cwd: Option<PathBuf>,
) -> String {
    let Some(context) = local_context_from_file_mentions(input, workspace, cwd) else {
        return input.to_string();
    };
    format!("{input}\n\n---\n\nLocal context from @mentions:\n{context}")
}

#[must_use]
pub fn pending_context_previews(input: &str) -> Vec<FileMentionPreview> {
    let mut previews = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for mention in extract_file_mentions(input)
        .into_iter()
        .take(MAX_FILE_MENTIONS_PER_MESSAGE)
    {
        if !seen.insert(mention.clone()) {
            continue;
        }
        // Composer previews stay lexical (no git subprocess from the render
        // loop, same rule as #4365 for path stats); the payload is resolved
        // once at submit time.
        if let Some(kind) = git_mention::git_mention_kind(&mention) {
            previews.push(FileMentionPreview {
                kind: "git".to_string(),
                label: kind.label().to_string(),
                detail: Some("resolved on send".to_string()),
                included: false,
                removable: false,
            });
            continue;
        }
        let media = is_media_path(Path::new(&mention));
        previews.push(FileMentionPreview {
            kind: if media { "media" } else { "mention" }.to_string(),
            label: mention,
            detail: Some(if media {
                "use /attach for media bytes".to_string()
            } else {
                "resolved on send".to_string()
            }),
            // Lexical preview deliberately does not stat the path while the
            // user types. Exact inclusion/missing metadata is resolved once,
            // at submit time, rather than from the render loop (#4365).
            included: false,
            removable: false,
        });
    }

    for attachment in extract_media_attachment_references(input) {
        previews.push(FileMentionPreview {
            kind: attachment.kind,
            label: attachment.path,
            detail: Some("attached media".to_string()),
            included: true,
            removable: true,
        });
    }
    previews
}

#[must_use]
pub fn context_references_from_input(
    input: &str,
    workspace: &Path,
    cwd: Option<PathBuf>,
) -> Vec<ContextReference> {
    let mut references = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let ws = Workspace::with_cwd(workspace.to_path_buf(), cwd);

    for mention in extract_file_mentions(input)
        .into_iter()
        .take(MAX_FILE_MENTIONS_PER_MESSAGE)
    {
        // Git mentions resolve against the working tree, not the path index,
        // so the inspector reports their real size and budget (#4067).
        if let Some(kind) = git_mention::git_mention_kind(&mention) {
            let payload = git_mention::resolve_git_mention(kind, workspace);
            let detail = match payload.unavailable_reason.as_deref() {
                Some(reason) => format!("{}, {reason}", kind.label()),
                None if payload.truncated => format!(
                    "{}, {} bytes truncated at {} budget",
                    kind.label(),
                    payload.bytes,
                    kind.byte_budget()
                ),
                None => format!("{}, {} bytes", kind.label(), payload.bytes),
            };
            let reference = ContextReference {
                kind: ContextReferenceKind::GitContext,
                source: ContextReferenceSource::AtMention,
                badge: "git".to_string(),
                label: kind.token().to_string(),
                target: workspace.display().to_string(),
                included: payload.unavailable_reason.is_none(),
                expanded: false,
                detail: Some(detail),
            };
            if seen.insert(format!("git-mention:{}", kind.token())) {
                references.push(reference);
            }
            continue;
        }

        let (path, display_path, exists) = match ws.resolve_exact(&mention) {
            Ok(path) => {
                let display = path.display().to_string();
                (path, display, true)
            }
            Err(path) => {
                let display = path.display().to_string();
                (path, display, false)
            }
        };
        let reference = context_reference_for_mention(&mention, &path, &display_path, exists);
        if !seen.insert(format!(
            "{:?}:{:?}:{}:{}",
            reference.source, reference.kind, reference.target, reference.label
        )) {
            continue;
        }
        references.push(reference);
    }

    for reference in extract_media_attachment_references(input) {
        let context_reference = ContextReference {
            kind: ContextReferenceKind::MediaAttachment,
            source: ContextReferenceSource::Attachment,
            badge: reference.kind,
            label: reference.path.clone(),
            target: reference.path,
            included: true,
            expanded: false,
            detail: Some("attached media".to_string()),
        };
        if !seen.insert(format!(
            "{:?}:{:?}:{}:{}",
            context_reference.source,
            context_reference.kind,
            context_reference.target,
            context_reference.label
        )) {
            continue;
        }
        references.push(context_reference);
    }

    references
}

fn context_reference_for_mention(
    raw: &str,
    path: &Path,
    display_path: &str,
    exists: bool,
) -> ContextReference {
    if !exists {
        return ContextReference {
            kind: ContextReferenceKind::Missing,
            source: ContextReferenceSource::AtMention,
            badge: "missing".to_string(),
            label: raw.to_string(),
            target: display_path.to_string(),
            included: false,
            expanded: false,
            detail: Some("not found".to_string()),
        };
    }
    if path.is_dir() {
        return ContextReference {
            kind: ContextReferenceKind::Directory,
            source: ContextReferenceSource::AtMention,
            badge: "dir".to_string(),
            label: raw.to_string(),
            target: display_path.to_string(),
            included: true,
            expanded: true,
            detail: Some("directory listing".to_string()),
        };
    }
    if !path.is_file() {
        return ContextReference {
            kind: ContextReferenceKind::Unsupported,
            source: ContextReferenceSource::AtMention,
            badge: "skipped".to_string(),
            label: raw.to_string(),
            target: display_path.to_string(),
            included: false,
            expanded: false,
            detail: Some("unsupported path".to_string()),
        };
    }
    if is_media_path(path) {
        return ContextReference {
            kind: ContextReferenceKind::MediaMention,
            source: ContextReferenceSource::AtMention,
            badge: "media".to_string(),
            label: raw.to_string(),
            target: display_path.to_string(),
            included: false,
            expanded: false,
            detail: Some("use /attach for media bytes".to_string()),
        };
    }

    let detail = match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_MENTION_FILE_BYTES => {
            Some("included truncated".to_string())
        }
        Ok(_) => Some("included".to_string()),
        Err(err) => Some(format!("metadata: {err}")),
    };

    ContextReference {
        kind: ContextReferenceKind::File,
        source: ContextReferenceSource::AtMention,
        badge: "file".to_string(),
        label: raw.to_string(),
        target: display_path.to_string(),
        included: true,
        expanded: true,
        detail: detail.or_else(|| Some(display_path.to_string())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAttachmentReference {
    pub kind: String,
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

pub fn media_attachment_references(input: &str) -> Vec<MediaAttachmentReference> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for line in input.split_inclusive('\n') {
        let start_byte = offset;
        let end_byte = offset + line.len();
        offset = end_byte;
        let trimmed = line.trim();
        let Some(body) = trimmed
            .strip_prefix("[Attached ")
            .and_then(|value| value.strip_suffix(']'))
        else {
            continue;
        };
        let Some((kind, rest)) = body.split_once(": ") else {
            continue;
        };
        let path = rest
            .rsplit_once(" at ")
            .map_or(rest, |(_, path)| path)
            .trim();
        if !path.is_empty() {
            out.push(MediaAttachmentReference {
                kind: kind.trim().to_string(),
                path: path.to_string(),
                start_byte,
                end_byte,
            });
        }
    }
    out
}

fn extract_media_attachment_references(input: &str) -> Vec<MediaAttachmentReference> {
    media_attachment_references(input)
}

fn local_context_from_file_mentions(
    input: &str,
    workspace: &Path,
    cwd: Option<PathBuf>,
) -> Option<String> {
    let mentions = extract_file_mentions(input);
    if mentions.is_empty() {
        return None;
    }

    let mut blocks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let ws = Workspace::with_cwd(workspace.to_path_buf(), cwd);

    for mention in mentions.into_iter().take(MAX_FILE_MENTIONS_PER_MESSAGE) {
        // `@git` / `@diff` resolve to curated git context, not to a path, so
        // they short-circuit before any workspace path resolution (#4067).
        if let Some(kind) = git_mention::git_mention_kind(&mention) {
            if !seen.insert(format!("git-mention:{}", kind.token())) {
                continue;
            }
            blocks.push(git_mention::resolve_git_mention(kind, workspace).block);
            continue;
        }

        // `Workspace::resolve_exact` already returns absolute paths when the root
        // is absolute (TUI always runs from an absolute workspace), so we
        // skip `canonicalize()` here — it's per-mention I/O on the
        // message-send hot path. Accept the rare symlink-aliasing dedup
        // miss as the cost of avoiding a syscall (Gemini code-review).
        let (path, display_path, exists) = match ws.resolve_exact(&mention) {
            Ok(p) => {
                let d = p.display().to_string();
                (p, d, true)
            }
            Err(p) => {
                let d = p.display().to_string();
                (p, d, false)
            }
        };
        tracing::debug!(
            target: "codewhale_tui::file_mention",
            raw_typed = %mention,
            workspace = %workspace.display(),
            cwd = ?std::env::current_dir().ok(),
            resolved = %display_path,
            exists,
            "file mention resolution",
        );

        // Gate every block — including <missing-file> — through the dedup
        // set so a user typing the same non-existent file twice doesn't
        // waste tokens on duplicate missing-file blocks (Devin code-review).
        if !seen.insert(display_path.clone()) {
            continue;
        }

        if exists {
            blocks.push(render_file_mention_context(&mention, &path, &display_path));
        } else {
            blocks.push(format!(
                "<missing-file mention=\"@{mention}\" path=\"{display_path}\" />"
            ));
        }
    }

    if blocks.is_empty() {
        None
    } else {
        Some(blocks.join("\n\n"))
    }
}

fn extract_file_mentions(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut mentions = Vec::new();
    let mut idx = 0;

    while idx < chars.len() {
        if chars[idx] != '@' || !is_file_mention_start(&chars, idx) {
            idx += 1;
            continue;
        }

        let Some(next) = chars.get(idx + 1).copied() else {
            break;
        };
        if next.is_whitespace() {
            idx += 1;
            continue;
        }

        if matches!(next, '"' | '\'') {
            let quote = next;
            let mut end = idx + 2;
            let mut raw = String::new();
            while end < chars.len() && chars[end] != quote {
                raw.push(chars[end]);
                end += 1;
            }
            if !raw.trim().is_empty() {
                mentions.push(raw.trim().to_string());
            }
            idx = end.saturating_add(1);
            continue;
        }

        let mut end = idx + 1;
        let mut raw = String::new();
        while end < chars.len() && !chars[end].is_whitespace() {
            raw.push(chars[end]);
            end += 1;
        }
        let trimmed = trim_unquoted_mention(&raw);
        if !trimmed.is_empty() {
            mentions.push(trimmed.to_string());
        }
        idx = end;
    }

    mentions
}

fn is_file_mention_start(chars: &[char], idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    chars
        .get(idx.saturating_sub(1))
        .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '<' | '"' | '\''))
}

fn trim_unquoted_mention(raw: &str) -> &str {
    let mut trimmed = raw.trim();
    while trimmed.chars().count() > 1
        && trimmed
            .chars()
            .last()
            .is_some_and(|ch| matches!(ch, ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}'))
    {
        trimmed = &trimmed[..trimmed.len() - trimmed.chars().last().unwrap().len_utf8()];
    }
    trimmed
}

fn render_file_mention_context(raw: &str, path: &Path, display_path: &str) -> String {
    if !path.exists() {
        return format!("<missing-file mention=\"@{raw}\" path=\"{display_path}\" />");
    }
    if path.is_dir() {
        return render_directory_mention_context(raw, path, display_path);
    }
    if !path.is_file() {
        return format!("<unsupported-path mention=\"@{raw}\" path=\"{display_path}\" />");
    }
    if is_media_path(path) {
        return format!(
            "<media-file mention=\"@{raw}\" path=\"{display_path}\">\nUse /attach {raw} when the intent is to attach this image or video to the next message.\n</media-file>"
        );
    }

    match read_text_prefix(path) {
        Ok((text, truncated)) => {
            let truncated_attr = if truncated { " truncated=\"true\"" } else { "" };
            format!(
                "<file mention=\"@{raw}\" path=\"{display_path}\"{truncated_attr}>\n{text}\n</file>"
            )
        }
        Err(err) => {
            format!(
                "<unreadable-file mention=\"@{raw}\" path=\"{display_path}\">\n{err}\n</unreadable-file>"
            )
        }
    }
}

fn render_directory_mention_context(raw: &str, path: &Path, display_path: &str) -> String {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) => {
            return format!(
                "<unreadable-directory mention=\"@{raw}\" path=\"{display_path}\">\n{err}\n</unreadable-directory>"
            );
        }
    };

    let mut names = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let marker = entry
                .file_type()
                .ok()
                .filter(|ty| ty.is_dir())
                .map_or("", |_| "/");
            format!("{}{}", entry.file_name().to_string_lossy(), marker)
        })
        .collect::<Vec<_>>();
    names.sort();
    let total = names.len();
    names.truncate(MAX_DIRECTORY_MENTION_ENTRIES);
    let mut body = names.join("\n");
    if total > MAX_DIRECTORY_MENTION_ENTRIES {
        let omitted = total - MAX_DIRECTORY_MENTION_ENTRIES;
        let _ = write!(body, "\n... {omitted} more entries");
    }
    format!("<directory mention=\"@{raw}\" path=\"{display_path}\">\n{body}\n</directory>")
}

fn read_text_prefix(path: &Path) -> std::io::Result<(String, bool)> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.by_ref()
        .take(MAX_MENTION_FILE_BYTES + 1)
        .read_to_end(&mut buffer)?;
    let truncated = buffer.len() as u64 > MAX_MENTION_FILE_BYTES;
    if truncated {
        buffer.truncate(MAX_MENTION_FILE_BYTES as usize);
        // Round down to the nearest valid UTF-8 character boundary so a
        // multi-byte sequence (CJK, emoji, etc.) is never split at the cut point.
        // Only adjust when error_len() is None — that means truncation landed
        // mid-sequence (incomplete tail).  A Some(_) error_len means the file
        // genuinely contains invalid UTF-8 bytes; leave the buffer intact so
        // the from_utf8 call below returns the correct "file is not UTF-8" error.
        if let Err(e) = std::str::from_utf8(&buffer)
            && e.error_len().is_none()
        {
            buffer.truncate(e.valid_up_to());
        }
    }
    if buffer.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file appears to be binary",
        ));
    }
    let text = std::str::from_utf8(&buffer)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "file is not UTF-8"))?
        .to_string();
    Ok((text, truncated))
}

fn is_media_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "bmp"
            | "tif"
            | "tiff"
            | "ppm"
            | "mp4"
            | "mov"
            | "m4v"
            | "webm"
            | "avi"
            | "mkv"
    )
}

// ---------------------------------------------------------------------------
//  #101 regression repros
// ---------------------------------------------------------------------------
//
// The bug being guarded: typing `@<some/file>` resolved under `--workspace`,
// not the user's launch CWD. When the two diverged (the canonical case is
// `--workspace=/repo` with `pwd=/repo/sub`), every relative `@` token routed
// to the wrong root and the prompt got `<missing-file>` blocks.
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// #101 regression — workspace-vs-cwd divergence: `@bar.txt` typed from
    /// the cwd `<root>/sub` MUST resolve to `<root>/sub/bar.txt`, never to
    /// `<root>/bar.txt` (which doesn't exist).
    #[test]
    fn cwd_pass_resolves_when_workspace_pass_misses() {
        let tmp = TempDir::new().expect("tempdir");
        let sub = tmp.path().join("sub");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let bar = sub.join("bar.txt");
        std::fs::write(&bar, "hello bar").expect("write bar");

        let content =
            user_request_with_file_mentions("look at @bar.txt", tmp.path(), Some(sub.clone()));

        // The block must reference the cwd-rooted path with the file's body —
        // and crucially it must NOT collapse to <missing-file>.
        assert!(
            content.contains("hello bar"),
            "expected file body to be inlined; got: {content}",
        );
        assert!(
            !content.contains("<missing-file"),
            "must not surface <missing-file> for a path that exists under cwd; got: {content}",
        );
        let bar_disp = bar.display().to_string();
        assert!(
            content.contains(&bar_disp),
            "expected resolved path {bar_disp} in content; got: {content}",
        );
        // Belt-and-suspenders: the workspace-rooted path doesn't exist and
        // must not appear in the rendered <file path="..."> attribute.
        let wrong = tmp.path().join("bar.txt").display().to_string();
        assert!(
            !content.contains(&format!("path=\"{wrong}\"")),
            "should NOT have routed to {wrong}; got: {content}",
        );
    }

    /// #101 regression — nested workspace path: `@nested/deep/file.md` with
    /// the file at workspace root resolves through the workspace pass.
    #[test]
    fn workspace_pass_resolves_nested_path() {
        let tmp = TempDir::new().expect("tempdir");
        let nested = tmp.path().join("nested/deep");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let file_md = nested.join("file.md");
        std::fs::write(&file_md, "# nested deep").expect("write file_md");

        // Cwd is irrelevant; an unrelated tempdir would do. Pass `None` so we
        // are unambiguously testing the workspace-pass path.
        let content = user_request_with_file_mentions("see @nested/deep/file.md", tmp.path(), None);

        assert!(content.contains("# nested deep"), "got: {content}");
        assert!(!content.contains("<missing-file"), "got: {content}");
        // Path-separator-portable check: the resolved path's filename is the
        // most reliable cross-platform anchor (Windows mixes `/` and `\` when
        // join() preserves user-typed separators).
        let basename = file_md
            .file_name()
            .and_then(|n| n.to_str())
            .expect("file_name utf-8");
        assert!(
            content.contains(basename),
            "basename {basename} not in path; got: {content}",
        );
    }

    /// Snapshot-style check: the rendered `<file>` block for a resolvable
    /// mention must include the expected attributes and contents, and must
    /// NOT contain `<missing-file>`.
    #[test]
    fn resolvable_mention_renders_file_block_not_missing_file() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("guide.md"), "# Guide\nUse the fast path.\n")
            .expect("write");

        let content = user_request_with_file_mentions("read @guide.md", tmp.path(), None);

        // Header + tag presence.
        assert!(content.contains("Local context from @mentions:"));
        assert!(content.contains("<file mention=\"@guide.md\""));
        assert!(content.contains("# Guide\nUse the fast path."));
        assert!(content.ends_with("</file>"), "got: {content}");
        // The bug fingerprint MUST be absent.
        assert!(!content.contains("<missing-file"), "got: {content}");
    }

    /// Negative test: a truly missing path still produces `<missing-file>`
    /// so the user gets an explicit signal instead of silent failure.
    #[test]
    fn truly_missing_mention_still_renders_missing_file() {
        let tmp = TempDir::new().expect("tempdir");

        let content = user_request_with_file_mentions(
            "huh @does/not/exist.txt",
            tmp.path(),
            Some(tmp.path().to_path_buf()),
        );

        assert!(
            content.contains("<missing-file mention=\"@does/not/exist.txt\""),
            "got: {content}",
        );
    }

    #[test]
    fn pending_context_preview_is_lexical_and_does_not_probe_paths() {
        let previews = pending_context_previews("read @guide.md and @missing.md");

        assert_eq!(previews.len(), 2);
        assert_eq!(previews[0].kind, "mention");
        assert_eq!(previews[0].label, "guide.md");
        assert!(!previews[0].included);
        assert_eq!(previews[0].detail.as_deref(), Some("resolved on send"));
        assert_eq!(previews[1].kind, "mention");
        assert_eq!(previews[1].label, "missing.md");
        assert!(!previews[1].included);
        assert_eq!(previews[1].detail.as_deref(), Some("resolved on send"));
    }

    #[test]
    fn pending_context_preview_distinguishes_attach_media_from_at_media() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("photo.png"), b"png").expect("write");
        let attached = tmp.path().join("photo.png").display().to_string();
        let input = format!("inspect @photo.png\n[Attached image: {attached}]");

        let previews = pending_context_previews(&input);

        assert!(
            previews
                .iter()
                .any(|item| item.kind == "media" && !item.included),
            "at-mention media should be hint-only: {previews:?}"
        );
        assert!(
            previews
                .iter()
                .any(|item| item.kind == "image" && item.included),
            "/attach media should be included: {previews:?}"
        );
    }

    #[test]
    fn manually_typed_basename_does_not_fuzzy_attach_nested_file() {
        let tmp = TempDir::new().expect("tempdir");
        let nested = tmp.path().join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(nested.join("guide.md"), "nested secret").expect("write");

        let content = user_request_with_file_mentions("read @guide.md", tmp.path(), None);

        assert!(
            content.contains("<missing-file mention=\"@guide.md\""),
            "a manually typed basename should remain exact: {content}",
        );
        assert!(
            !content.contains("nested secret"),
            "exact resolution must not silently attach a fuzzy nested match: {content}",
        );
    }

    #[test]
    fn media_attachment_references_include_removable_line_ranges() {
        let input = "before\n[Attached image: 8x4 PNG at /tmp/pasted.png]\nafter";

        let references = media_attachment_references(input);

        assert_eq!(references.len(), 1);
        let reference = &references[0];
        assert_eq!(reference.kind, "image");
        assert_eq!(reference.path, "/tmp/pasted.png");
        assert_eq!(
            &input[reference.start_byte..reference.end_byte],
            "[Attached image: 8x4 PNG at /tmp/pasted.png]\n"
        );
    }

    #[test]
    fn context_references_preserve_exact_targets_and_roundtrip() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
        std::fs::write(tmp.path().join("src/main.rs"), "fn main() {}").expect("write");
        let input = "read @src/main.rs";

        let references =
            context_references_from_input(input, tmp.path(), Some(tmp.path().to_path_buf()));

        assert_eq!(references.len(), 1);
        let reference = &references[0];
        assert_eq!(reference.kind, ContextReferenceKind::File);
        assert_eq!(reference.source, ContextReferenceSource::AtMention);
        assert_eq!(reference.label, "src/main.rs");
        assert!(reference.target.ends_with("src/main.rs"));
        assert!(reference.included);
        assert!(reference.expanded);

        let encoded = serde_json::to_string(reference).expect("serialize");
        let decoded: ContextReference = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(&decoded, reference);
    }

    /// Regression test for #1441: truncating at MAX_MENTION_FILE_BYTES must not
    /// split a multi-byte UTF-8 sequence, which previously produced U+FFFD
    /// replacement characters in the TUI output.
    #[test]
    fn read_text_prefix_truncation_respects_utf8_char_boundary() {
        use std::io::Write;

        // Build a file that is MAX_MENTION_FILE_BYTES - 1 ASCII bytes followed
        // by a 3-byte CJK character (U+4E2D, '中'). The naive truncate at
        // MAX_MENTION_FILE_BYTES cuts after the first byte of '中', producing
        // an invalid sequence.
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("cjk.txt");
        let mut f = std::fs::File::create(&path).expect("create");
        let padding = vec![b'a'; MAX_MENTION_FILE_BYTES as usize - 1];
        f.write_all(&padding).expect("write padding");
        f.write_all("中".as_bytes()).expect("write CJK");

        let (text, truncated) = read_text_prefix(&path).expect("should succeed");
        assert!(
            truncated,
            "file exceeds limit so should be marked truncated"
        );
        assert!(
            !text.contains('\u{FFFD}'),
            "truncated text must not contain replacement characters; got: {text:?}",
        );
    }
    // ---------------------------------------------------------------------
    //  #4067 — @git / @diff composer mentions
    // ---------------------------------------------------------------------

    fn init_test_repo(dir: &Path) {
        for args in [
            vec!["init", "--initial-branch=main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
        ] {
            let out = std::process::Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .expect("git available in tests");
            assert!(out.status.success(), "git {args:?} failed");
        }
    }

    fn commit_test_repo(dir: &Path) {
        for args in [vec!["add", "-A"], vec!["commit", "-m", "initial"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .expect("git available in tests");
        }
    }

    #[test]
    fn git_and_diff_mentions_inline_curated_context_not_paths() {
        let tmp = TempDir::new().expect("tempdir");
        init_test_repo(tmp.path());
        std::fs::write(tmp.path().join("a.txt"), "one\n").expect("write");
        commit_test_repo(tmp.path());
        std::fs::write(tmp.path().join("a.txt"), "two\n").expect("write");

        let expanded = user_request_with_file_mentions(
            "look at @git and @diff",
            tmp.path(),
            Some(tmp.path().to_path_buf()),
        );

        assert!(expanded.contains("<git-status"), "{expanded}");
        assert!(expanded.contains("<git-diff"), "{expanded}");
        assert!(expanded.contains("a.txt"), "{expanded}");
        // The tokens are not treated as paths, so no missing-file block.
        assert!(!expanded.contains("<missing-file"), "{expanded}");
    }

    #[test]
    fn git_mentions_outside_a_repository_say_so_explicitly() {
        let tmp = TempDir::new().expect("tempdir");
        let expanded = user_request_with_file_mentions(
            "status? @git",
            tmp.path(),
            Some(tmp.path().to_path_buf()),
        );
        assert!(expanded.contains("<git-unavailable"), "{expanded}");
        assert!(expanded.contains("not a git repository"), "{expanded}");
    }

    #[test]
    fn git_mention_is_deduplicated_within_one_message() {
        let tmp = TempDir::new().expect("tempdir");
        init_test_repo(tmp.path());
        std::fs::write(tmp.path().join("a.txt"), "one\n").expect("write");
        commit_test_repo(tmp.path());
        std::fs::write(tmp.path().join("a.txt"), "two\n").expect("write");

        let expanded = user_request_with_file_mentions(
            "@diff and again @diff",
            tmp.path(),
            Some(tmp.path().to_path_buf()),
        );
        assert_eq!(expanded.matches("<git-diff").count(), 1, "{expanded}");
    }

    #[test]
    fn paths_that_merely_start_with_git_stay_file_mentions() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("diff.txt"), "plain file").expect("write");

        let expanded = user_request_with_file_mentions(
            "see @diff.txt",
            tmp.path(),
            Some(tmp.path().to_path_buf()),
        );
        assert!(expanded.contains("plain file"), "{expanded}");
        assert!(!expanded.contains("<git-diff"), "{expanded}");
    }

    #[test]
    fn large_diff_is_truncated_and_the_inspector_reports_the_budget() {
        let tmp = TempDir::new().expect("tempdir");
        init_test_repo(tmp.path());
        std::fs::write(tmp.path().join("big.txt"), "seed\n").expect("write");
        commit_test_repo(tmp.path());
        let bulk: String = (0..40_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(tmp.path().join("big.txt"), bulk).expect("write");

        let expanded =
            user_request_with_file_mentions("@diff", tmp.path(), Some(tmp.path().to_path_buf()));
        assert!(
            expanded.contains("truncated=\"true\""),
            "expected truncation marker"
        );

        let references =
            context_references_from_input("@diff", tmp.path(), Some(tmp.path().to_path_buf()));
        let git_ref = references
            .iter()
            .find(|r| r.kind == ContextReferenceKind::GitContext)
            .expect("git reference present in the inspector");
        assert_eq!(git_ref.label, "diff");
        assert!(git_ref.included);
        let detail = git_ref.detail.clone().unwrap_or_default();
        assert!(detail.contains("truncated at"), "{detail}");
        assert!(
            detail.contains(&crate::tui::git_mention::MAX_GIT_DIFF_MENTION_BYTES.to_string()),
            "{detail}"
        );
    }

    #[test]
    fn empty_repository_reference_is_visible_but_not_included() {
        let tmp = TempDir::new().expect("tempdir");
        init_test_repo(tmp.path());
        std::fs::write(tmp.path().join("a.txt"), "one\n").expect("write");
        commit_test_repo(tmp.path());

        let references =
            context_references_from_input("@diff", tmp.path(), Some(tmp.path().to_path_buf()));
        let git_ref = references
            .iter()
            .find(|r| r.kind == ContextReferenceKind::GitContext)
            .expect("git reference present even when there is nothing to show");
        assert!(!git_ref.included);
        assert!(
            git_ref
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("no working-tree changes")),
            "{:?}",
            git_ref.detail
        );
    }

    #[test]
    fn composer_preview_lists_git_mentions_without_running_git() {
        let previews = pending_context_previews("@git @diff");
        let kinds: Vec<&str> = previews.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(kinds, vec!["git", "git"]);
        assert!(previews.iter().all(|p| !p.included));
    }

    #[test]
    fn completion_offers_git_and_diff_alongside_paths() {
        let paths = vec!["src/main.rs".to_string(), "docs/guide.md".to_string()];
        // A bare `@` stays the file picker.
        assert_eq!(with_git_mention_entries(paths.clone(), "", 8), paths);

        let narrowed = with_git_mention_entries(paths.clone(), "di", 8);
        assert_eq!(narrowed.first().map(String::as_str), Some("diff"));
        assert!(narrowed.contains(&"src/main.rs".to_string()));

        let git_only = with_git_mention_entries(paths.clone(), "g", 8);
        assert_eq!(git_only.first().map(String::as_str), Some("git"));

        // A partial that matches no token leaves path completion untouched.
        assert_eq!(with_git_mention_entries(paths.clone(), "src", 8), paths);
    }
}
