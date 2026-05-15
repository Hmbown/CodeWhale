//! `recall_archive` tool — search prior cycle archives (issue #127).
//!
//! Companion to the checkpoint-restart cycle architecture (#124). When the
//! agent's `<carry_forward>` briefing missed something, this tool scans the
//! on-disk JSONL archives at `~/.deepseek/sessions/<id>/cycles/*.jsonl` and
//! returns the top-N matching messages.
//!
//! ## Scoring
//!
//! v1: a simplified BM25 over tokenized message text. No external embedding
//! model, no cache — every call walks the archives. Acceptable because the
//! per-cycle archive is bounded by the 110K cycle threshold and most sessions
//! cross at most a handful of cycles. v2 (later) can add an
//! `~/.deepseek/embeddings/` cache built on archive write.

use std::collections::HashMap;
use std::fs::read_dir;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_u64, required_str,
};
use crate::cycle_manager::open_archive;
use crate::models::{ContentBlock, Message};

const DEFAULT_MAX_RESULTS: usize = 3;
const HARD_MAX_RESULTS: usize = 10;
const CONTEXT_WINDOW_CHARS: usize = 240;

/// BM25 hyper-parameters. Standard defaults from the literature.
const K1: f64 = 1.5;
const B: f64 = 0.75;

pub struct RecallArchiveTool;

#[derive(Debug, Clone, Serialize)]
struct RecallHit {
    cycle: u32,
    /// 0-based message index within the cycle.
    message_index: usize,
    role: String,
    score: f64,
    /// Short window around the best match, with `…` markers when truncated.
    excerpt: String,
}

#[async_trait]
impl ToolSpec for RecallArchiveTool {
    fn name(&self) -> &'static str {
        "recall_archive"
    }

    fn description(&self) -> &'static str {
        "Search prior context cycles for content not in your briefing. Use sparingly — \
         frequent recalls mean your briefing was too sparse; refine your next briefing."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. Tokenized and BM25-scored against archived messages."
                },
                "cycle": {
                    "type": "integer",
                    "description": "Optional: limit to a specific prior cycle number."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum hits to return (default 3, hard-capped at 10)."
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let query = required_str(&input, "query")?.trim().to_string();
        if query.is_empty() {
            return Err(ToolError::invalid_input("query cannot be empty"));
        }

        let max_results = (optional_u64(&input, "max_results", DEFAULT_MAX_RESULTS as u64)
            as usize)
            .clamp(1, HARD_MAX_RESULTS);
        let cycle_filter = input.get("cycle").and_then(Value::as_u64).map(|n| n as u32);

        let session_id = context.state_namespace.as_str();
        let archives = list_archives(session_id).map_err(|err| {
            ToolError::execution_failed(format!("Failed to enumerate cycle archives: {err}"))
        })?;

        if archives.is_empty() {
            return Ok(ToolResult::success(json!({
                "hits": [],
                "note": "No prior cycle archives exist. The session has not crossed a cycle boundary yet."
            }).to_string()));
        }

        let documents = load_messages(&archives, cycle_filter).map_err(|err| {
            ToolError::execution_failed(format!("Failed to read cycle archives: {err}"))
        })?;

        if documents.is_empty() {
            let note = match cycle_filter {
                Some(c) => format!("Cycle {c} has no messages in its archive."),
                None => "Cycle archives exist but contain no message text.".to_string(),
            };
            return Ok(ToolResult::success(
                json!({"hits": [], "note": note}).to_string(),
            ));
        }

        let query_tokens = tokenize(&query);
        if query_tokens.is_empty() {
            return Err(ToolError::invalid_input(
                "query has no scoring tokens after tokenization",
            ));
        }

        let hits = score_bm25(&documents, &query_tokens, max_results);

        let payload = json!({
            "query": query,
            "cycles_searched": archives.len(),
            "messages_scanned": documents.len(),
            "hits": hits,
        });

        Ok(ToolResult::success(payload.to_string()))
    }
}

/// One archived message + its provenance, ready to score.
struct ArchivedDoc {
    cycle: u32,
    message_index: usize,
    role: String,
    text: String,
    tokens: Vec<String>,
}

fn archive_root(session_id: &str) -> Result<PathBuf, std::io::Error> {
    let home = dirs::home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not resolve home directory for cycle archive root",
        )
    })?;
    Ok(home
        .join(".deepseek")
        .join("sessions")
        .join(session_id)
        .join("cycles"))
}

/// Enumerate all archive files for a session, sorted by cycle number ascending.
fn list_archives(session_id: &str) -> Result<Vec<(u32, PathBuf)>, std::io::Error> {
    let root = archive_root(session_id)?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut archives: Vec<(u32, PathBuf)> = Vec::new();
    for entry in read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let Ok(cycle_n) = stem.parse::<u32>() else {
            continue;
        };
        archives.push((cycle_n, path));
    }
    archives.sort_by_key(|(n, _)| *n);
    Ok(archives)
}

/// Read messages from each archive into a flat scoreable list.
fn load_messages(
    archives: &[(u32, PathBuf)],
    cycle_filter: Option<u32>,
) -> Result<Vec<ArchivedDoc>, anyhow::Error> {
    let mut docs: Vec<ArchivedDoc> = Vec::new();
    for (cycle_n, path) in archives {
        if let Some(filter) = cycle_filter
            && *cycle_n != filter
        {
            continue;
        }
        let (header, reader) = open_archive(path)?;
        for (idx, message_result) in reader.enumerate() {
            let message = message_result?;
            let text = message_text(&message);
            if text.trim().is_empty() {
                continue;
            }
            let tokens = tokenize(&text);
            if tokens.is_empty() {
                continue;
            }
            docs.push(ArchivedDoc {
                cycle: header.cycle,
                message_index: idx,
                role: message.role,
                text,
                tokens,
            });
        }
    }
    Ok(docs)
}

/// Concatenate all text-bearing content blocks of a message.
fn message_text(message: &Message) -> String {
    let mut out = String::new();
    let mut push = |s: &str| {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(s);
    };
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => push(text),
            ContentBlock::ToolUse { name, input, .. } => {
                push(&format!("[tool_use {name}] {input}"));
            }
            ContentBlock::ToolResult { content, .. } => {
                push(&format!("[tool_result] {content}"));
            }
            ContentBlock::Thinking { thinking } => {
                push(&format!("[thinking] {thinking}"));
            }
            ContentBlock::ServerToolUse { name, input, .. } => {
                push(&format!("[server_tool_use {name}] {input}"));
            }
            ContentBlock::ToolSearchToolResult { content, .. } => {
                push(&format!("[tool_search_result] {content}"));
            }
            ContentBlock::CodeExecutionToolResult { content, .. } => {
                push(&format!("[code_execution_result] {content}"));
            }
        }
    }
    out
}

/// Lower-case, split on non-alphanumerics, drop short tokens. Same recipe as
/// most lightweight BM25 implementations.
fn tokenize(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 2)
        .map(str::to_string)
        .collect()
}

/// Score documents against a query using BM25, return the top-N.
fn score_bm25(docs: &[ArchivedDoc], query_tokens: &[String], max_results: usize) -> Vec<RecallHit> {
    if docs.is_empty() || query_tokens.is_empty() {
        return Vec::new();
    }

    let n = docs.len() as f64;
    let avgdl: f64 = docs.iter().map(|d| d.tokens.len() as f64).sum::<f64>() / n.max(1.0);

    // Document frequency per query term.
    let mut df: HashMap<&str, u64> = HashMap::new();
    for token in query_tokens {
        let mut count = 0u64;
        for doc in docs {
            if doc.tokens.iter().any(|t| t == token) {
                count += 1;
            }
        }
        df.insert(token.as_str(), count);
    }

    let mut scored: Vec<(f64, &ArchivedDoc)> = docs
        .iter()
        .map(|doc| (bm25_doc_score(doc, query_tokens, &df, n, avgdl), doc))
        .filter(|(score, _)| *score > 0.0)
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_results);

    scored
        .into_iter()
        .map(|(score, doc)| RecallHit {
            cycle: doc.cycle,
            message_index: doc.message_index,
            role: doc.role.clone(),
            score: round_score(score),
            excerpt: best_window(&doc.text, query_tokens, CONTEXT_WINDOW_CHARS),
        })
        .collect()
}

fn bm25_doc_score(
    doc: &ArchivedDoc,
    query_tokens: &[String],
    df: &HashMap<&str, u64>,
    n: f64,
    avgdl: f64,
) -> f64 {
    let dl = doc.tokens.len() as f64;
    if dl == 0.0 {
        return 0.0;
    }
    let mut score = 0.0;
    for token in query_tokens {
        let tf = doc.tokens.iter().filter(|t| *t == token).count() as f64;
        if tf == 0.0 {
            continue;
        }
        let df_t = df.get(token.as_str()).copied().unwrap_or(0) as f64;
        let idf = ((n - df_t + 0.5) / (df_t + 0.5) + 1.0).ln();
        let denom = tf + K1 * (1.0 - B + B * (dl / avgdl.max(1.0)));
        score += idf * (tf * (K1 + 1.0)) / denom.max(f64::EPSILON);
    }
    score
}

fn round_score(score: f64) -> f64 {
    (score * 1000.0).round() / 1000.0
}

/// Find the substring of `text` of at most `window_chars` characters that
/// contains the densest cluster of query tokens. Returns it with `…` markers
/// when truncated. Falls back to a head-of-text excerpt when no tokens hit.
fn best_window(text: &str, query_tokens: &[String], window_chars: usize) -> String {
    let lower = text.to_ascii_lowercase();
    let mut hit_positions: Vec<usize> = Vec::new();
    for token in query_tokens {
        let mut start = 0usize;
        while let Some(pos) = lower[start..].find(token.as_str()) {
            hit_positions.push(start + pos);
            start += pos + token.len();
        }
    }
    if hit_positions.is_empty() {
        return head_excerpt(text, window_chars);
    }
    hit_positions.sort_unstable();

    // Greedy: center the window on the first hit, walk forward as long as
    // additional hits fit in the window.
    let center = hit_positions[0];
    let half = window_chars / 2;
    let start = center.saturating_sub(half);
    let end = (start + window_chars).min(text.len());
    let start = align_char_boundary(text, start, false);
    let end = align_char_boundary(text, end, true);
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < text.len() { "…" } else { "" };
    format!("{prefix}{}{suffix}", &text[start..end])
}

fn head_excerpt(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let cut = align_char_boundary(text, max_chars, true);
    format!("{}…", &text[..cut])
}

/// Walk left or right until `idx` lands on a UTF-8 char boundary.
fn align_char_boundary(text: &str, mut idx: usize, walk_right: bool) -> usize {
    if idx >= text.len() {
        return text.len();
    }
    while idx > 0 && idx < text.len() && !text.is_char_boundary(idx) {
        if walk_right {
            idx += 1;
        } else {
            idx -= 1;
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle_manager::archive_cycle;
    use crate::models::{ContentBlock, Message};
    use chrono::Utc;
    use tempfile::TempDir;

    fn user_msg(text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn asst_msg(text: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    /// Guard that points `dirs::home_dir()` at a tempdir for the test's
    /// lifetime and restores the original on drop. On Unix this means
    /// `HOME`; on Windows it means `USERPROFILE`. We set both so the same
    /// guard works portably. Holds process-wide lock to serialize.
    struct HomeGuard {
        _tmp: TempDir,
        original_home: Option<String>,
        original_userprofile: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl HomeGuard {
        fn new() -> Self {
            let lock = crate::test_support::lock_test_env();
            let tmp = TempDir::new().expect("tempdir");
            let original_home = std::env::var("HOME").ok();
            let original_userprofile = std::env::var("USERPROFILE").ok();
            // SAFETY: serialized by process-wide lock; only this thread mutates the
            // env vars for the duration of the guard.
            unsafe {
                std::env::set_var("HOME", tmp.path());
                std::env::set_var("USERPROFILE", tmp.path());
            }
            Self {
                _tmp: tmp,
                original_home,
                original_userprofile,
                _lock: lock,
            }
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: still holding HOME_LOCK.
            unsafe {
                match self.original_home.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
                match self.original_userprofile.take() {
                    Some(v) => std::env::set_var("USERPROFILE", v),
                    None => std::env::remove_var("USERPROFILE"),
                }
            }
        }
    }

    fn fresh_session_id() -> String {
        format!("test-{}", uuid::Uuid::new_v4())
    }

    fn ctx_for_session(workspace: &std::path::Path, session_id: &str) -> ToolContext {
        ToolContext::new(workspace).with_state_namespace(session_id.to_string())
    }

    #[test]
    fn tokenize_lowers_splits_drops_short() {
        // Filter is `len >= 2`, so "a" and "0" drop; "42" stays.
        let toks = tokenize("Hello, World! a 42 OAuth-2.0");
        assert_eq!(toks, vec!["hello", "world", "42", "oauth"]);
    }

    #[test]
    fn message_text_concatenates_blocks() {
        let m = Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "first".to_string(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: "second".to_string(),
                    cache_control: None,
                },
            ],
        };
        assert_eq!(message_text(&m), "first\nsecond");
    }

    #[test]
    fn list_archives_handles_missing_dir() {
        let _home = HomeGuard::new();
        let sid = fresh_session_id();
        let archives = list_archives(&sid).expect("list_archives");
        assert!(archives.is_empty());
    }

    #[test]
    fn list_archives_sorts_by_cycle_number() {
        let _home = HomeGuard::new();
        let sid = fresh_session_id();
        let now = Utc::now();
        archive_cycle(&sid, 3, &[user_msg("c3")], "deepseek-v4-pro", now).unwrap();
        archive_cycle(&sid, 1, &[user_msg("c1")], "deepseek-v4-pro", now).unwrap();
        archive_cycle(&sid, 2, &[user_msg("c2")], "deepseek-v4-pro", now).unwrap();
        let archives = list_archives(&sid).unwrap();
        let cycles: Vec<u32> = archives.iter().map(|(n, _)| *n).collect();
        assert_eq!(cycles, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn execute_returns_empty_when_no_archives() {
        let _home = HomeGuard::new();
        let sid = fresh_session_id();
        let workspace = TempDir::new().unwrap();
        let ctx = ctx_for_session(workspace.path(), &sid);
        let tool = RecallArchiveTool;
        let result = tool
            .execute(json!({"query": "anything"}), &ctx)
            .await
            .unwrap();
        assert!(result.content.contains("No prior cycle archives"));
    }

    #[tokio::test]
    async fn execute_finds_matching_messages() {
        let _home = HomeGuard::new();
        let sid = fresh_session_id();
        let workspace = TempDir::new().unwrap();
        let ctx = ctx_for_session(workspace.path(), &sid);
        let now = Utc::now();
        let messages = vec![
            user_msg("How does the cycle restart strategy work?"),
            asst_msg("It archives messages to JSONL when crossing the 110K threshold."),
            user_msg("What happens if briefing is too short?"),
            asst_msg("Use recall_archive to retrieve specific past content from JSONL files."),
        ];
        archive_cycle(&sid, 1, &messages, "deepseek-v4-pro", now).unwrap();

        let tool = RecallArchiveTool;
        let result = tool
            .execute(
                json!({"query": "JSONL archive briefing", "max_results": 3}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            result.content.contains("\"cycle\":1"),
            "got: {}",
            result.content
        );
        assert!(
            result.content.contains("\"hits\""),
            "got: {}",
            result.content
        );
        assert!(result.content.contains("JSONL"), "got: {}", result.content);
    }

    #[tokio::test]
    async fn execute_filters_by_cycle() {
        let _home = HomeGuard::new();
        let sid = fresh_session_id();
        let workspace = TempDir::new().unwrap();
        let ctx = ctx_for_session(workspace.path(), &sid);
        let now = Utc::now();
        archive_cycle(
            &sid,
            1,
            &[user_msg("alpha pattern")],
            "deepseek-v4-pro",
            now,
        )
        .unwrap();
        archive_cycle(
            &sid,
            2,
            &[user_msg("alpha pattern")],
            "deepseek-v4-pro",
            now,
        )
        .unwrap();

        let tool = RecallArchiveTool;
        let result = tool
            .execute(
                json!({"query": "alpha", "cycle": 2, "max_results": 5}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            result.content.contains("\"cycle\":2"),
            "got: {}",
            result.content
        );
        assert!(
            !result.content.contains("\"cycle\":1"),
            "got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_caps_max_results_at_hard_max() {
        let _home = HomeGuard::new();
        let sid = fresh_session_id();
        let workspace = TempDir::new().unwrap();
        let ctx = ctx_for_session(workspace.path(), &sid);
        let now = Utc::now();
        let mut messages: Vec<Message> = Vec::new();
        for i in 0..30 {
            messages.push(user_msg(&format!("alpha message number {i}")));
        }
        archive_cycle(&sid, 1, &messages, "deepseek-v4-pro", now).unwrap();

        let tool = RecallArchiveTool;
        let result = tool
            .execute(json!({"query": "alpha", "max_results": 999}), &ctx)
            .await
            .unwrap();
        let count = result.content.matches("\"message_index\":").count();
        assert!(count <= HARD_MAX_RESULTS, "got {count} hits");
    }

    #[tokio::test]
    async fn execute_rejects_empty_query() {
        let _home = HomeGuard::new();
        let sid = fresh_session_id();
        let workspace = TempDir::new().unwrap();
        let ctx = ctx_for_session(workspace.path(), &sid);
        let tool = RecallArchiveTool;
        let err = tool
            .execute(json!({"query": "   "}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }));
    }

    #[test]
    fn best_window_centers_on_first_hit() {
        let text = "lorem ipsum dolor sit amet, the quick brown fox jumps over the lazy dog";
        let win = best_window(text, &["fox".to_string()], 30);
        assert!(win.contains("fox"), "got: {win}");
    }

    #[test]
    fn best_window_falls_back_to_head_when_no_hits() {
        let text = "the quick brown fox jumps";
        let win = best_window(text, &["zzz".to_string()], 10);
        assert!(win.starts_with("the quick"), "got: {win}");
    }

    #[test]
    fn align_char_boundary_handles_multibyte() {
        let text = "héllo world";
        // Index 2 is mid-byte for `é` (UTF-8 encoded as 2 bytes).
        let aligned = align_char_boundary(text, 2, true);
        assert!(text.is_char_boundary(aligned), "boundary check");
    }

    #[test]
    fn bm25_returns_relevant_docs_drops_irrelevant() {
        // BM25 length normalization can let very short matching docs outrank
        // longer ones with higher term-frequency, so we only assert the
        // weak invariant: matching docs are returned, non-matching docs are
        // filtered out.
        let docs = vec![
            ArchivedDoc {
                cycle: 1,
                message_index: 0,
                role: "user".to_string(),
                text: "cat dog cat dog cat".to_string(),
                tokens: tokenize("cat dog cat dog cat"),
            },
            ArchivedDoc {
                cycle: 1,
                message_index: 1,
                role: "user".to_string(),
                text: "fish bird".to_string(),
                tokens: tokenize("fish bird"),
            },
            ArchivedDoc {
                cycle: 1,
                message_index: 2,
                role: "user".to_string(),
                text: "cat sleeps".to_string(),
                tokens: tokenize("cat sleeps"),
            },
        ];
        let hits = score_bm25(&docs, &["cat".to_string()], 3);
        let indices: Vec<usize> = hits.iter().map(|h| h.message_index).collect();
        assert!(indices.contains(&0), "doc 0 (3x cat) should appear");
        assert!(indices.contains(&2), "doc 2 (1x cat) should appear");
        assert!(!indices.contains(&1), "zero-score doc filtered");
        assert!(hits[0].score > 0.0, "top hit has positive score");
    }
}
