//! Post-edit LSP diagnostics hooks for engine tool execution.
//!
//! The turn loop only needs to ask "did a successful edit produce diagnostics?"
//! This module owns the tool-input path extraction and the synthetic diagnostic
//! message injection so the top-level engine module stays focused on session
//! orchestration.

use std::path::PathBuf;

use super::*;

/// #136: derive the file path(s) edited by a tool call. Returns the empty
/// vec for tools that don't modify files. We intentionally only handle the
/// three known edit tools — adding more (e.g. specialized refactor tools)
/// is a one-line change here.
pub(super) fn edited_paths_for_tool(tool_name: &str, input: &serde_json::Value) -> Vec<PathBuf> {
    match tool_name {
        "edit_file" | "write_file" => {
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                vec![PathBuf::from(path)]
            } else {
                Vec::new()
            }
        }
        "apply_patch" => {
            // `apply_patch` accepts either a `path` override or a list of
            // `files` (each `{path, content}`). We try both shapes.
            let mut out = Vec::new();
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                out.push(PathBuf::from(path));
            }
            if let Some(files) = input.get("files").and_then(|v| v.as_array()) {
                for entry in files {
                    if let Some(path) = entry.get("path").and_then(|v| v.as_str()) {
                        out.push(PathBuf::from(path));
                    }
                }
            }
            // Fallback: parse `---`/`+++` headers from a unified diff payload.
            if out.is_empty()
                && let Some(patch) = input.get("patch").and_then(|v| v.as_str())
            {
                out.extend(parse_patch_paths(patch));
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Lightweight parser for `+++ b/<path>` lines in a unified diff. Used as a
/// fallback when `apply_patch` is invoked with raw `patch` text and no
/// `path`/`files` override. We deliberately keep this dumb — the real
/// `apply_patch` tool already validates the patch shape; we only need a
/// best-effort hint for the LSP hook.
pub(super) fn parse_patch_paths(patch: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let trimmed = rest.trim();
            // Strip leading `b/` per git diff conventions.
            let path = trimmed.strip_prefix("b/").unwrap_or(trimmed);
            // Skip `/dev/null` (deletion).
            if path == "/dev/null" {
                continue;
            }
            out.push(PathBuf::from(path));
        }
    }
    out
}

impl Engine {
    /// #136: post-edit hook. Inspects the tool name + input, derives the
    /// edited file path, and asks the LSP manager for diagnostics. The
    /// rendered block is queued in `pending_lsp_blocks` and flushed to the
    /// session message stream just before the next API request. Failure is
    /// silent by design — a missing/crashing LSP server must never block
    /// the agent.
    pub(super) async fn run_post_edit_lsp_hook(
        &mut self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) {
        if !self.lsp_manager.config().enabled {
            return;
        }
        let paths = edited_paths_for_tool(tool_name, tool_input);
        for path in paths {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                self.session.workspace.join(&path)
            };
            // Use a short edit-sequence based on the existing turn counter so
            // log output stays correlated even though we do not currently
            // batch by sequence.
            let seq = self.turn_counter;
            if let Some(block) = self.lsp_manager.diagnostics_for(&absolute, seq).await {
                self.pending_lsp_blocks.push(block);
            }
        }
    }

    /// Drain `pending_lsp_blocks` into a single synthetic user message so the
    /// model sees the diagnostics on its next request. Skips when nothing is
    /// pending. The message uses the standard `text` content block shape
    /// (the same shape as the post-tool steer messages) so we don't need to
    /// invent a new envelope.
    pub(super) async fn flush_pending_lsp_diagnostics(&mut self) {
        if self.pending_lsp_blocks.is_empty() {
            return;
        }
        let blocks = std::mem::take(&mut self.pending_lsp_blocks);
        let rendered = crate::lsp::render_blocks(&blocks);
        if rendered.is_empty() {
            return;
        }
        self.add_session_message(self.user_text_message_with_turn_metadata(rendered))
            .await;
    }
}
