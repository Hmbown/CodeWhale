//! User-level memory file.
//!
//! v0.8.8 ships an MVP that lets the user keep a persistent personal
//! note file the model sees on every turn:
//!
//! - **Load** `~/.codewhale/memory.md` (path is configurable via
//!   `memory_path` in `config.toml` and `DEEPSEEK_MEMORY_PATH` env),
//!   wrap it in a `<user_memory>` block, and prepend it to the system
//!   prompt alongside the existing `<project_instructions>` block.
//! - **`# foo`** typed in the composer appends `foo` to the memory
//!   file as a timestamped bullet — fast capture without leaving the TUI.
//! - **`/memory`** shows the resolved file path and current contents, and
//!   **`/memory edit`** prints a copy-pasteable `$VISUAL` / `$EDITOR`
//!   command for opening the file yourself.
//! - **`remember` tool** lets the model itself append a bullet when it
//!   notices a durable preference or convention worth keeping across
//!   sessions.
//!
//! Default behavior is **opt-in**: load + use the memory file only when
//! `[memory] enabled = true` in `config.toml` or `DEEPSEEK_MEMORY=on`.
//! That keeps existing users on zero-overhead behavior and makes the
//! feature explicit.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use chrono::Utc;

/// Maximum size of the user memory file. Larger files are loaded but the
/// `<user_memory>` block carries a `<truncated bytes=N source="...">`
/// marker so the user knows the model only saw a slice. Mirrors
/// `project_context::MAX_CONTEXT_SIZE`.
const MAX_MEMORY_SIZE: usize = 100 * 1024;

/// Read the user memory file at `path`, returning `None` when the file
/// doesn't exist or is empty after trimming.
#[must_use]
pub fn load(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    Some(content)
}

/// Wrap memory content in a `<user_memory>` block ready to prepend to the
/// system prompt. The `source` value is rendered verbatim into a
/// `source="…"` attribute — pass the path so the model can see where the
/// memory came from. Returns `None` for empty content.
#[must_use]
pub fn as_system_block(content: &str, source: &Path) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    let display = source.display().to_string();
    let payload = if content.len() > MAX_MEMORY_SIZE {
        let cutoff = truncation_cutoff(content, &display);
        let omitted_bytes = content.len() - cutoff;
        let mut head = content[..cutoff].to_string();
        head.push_str(&truncation_marker(omitted_bytes, &display));
        head
    } else {
        trimmed.to_string()
    };

    Some(format!(
        "<user_memory source=\"{display}\">\n{payload}\n</user_memory>"
    ))
}

fn truncation_cutoff(content: &str, source: &str) -> usize {
    let mut cutoff = previous_char_boundary(content, MAX_MEMORY_SIZE);
    loop {
        let omitted_bytes = content.len() - cutoff;
        let max_head_len =
            MAX_MEMORY_SIZE.saturating_sub(truncation_marker(omitted_bytes, source).len());
        let next_cutoff = previous_char_boundary(content, cutoff.min(max_head_len));
        if next_cutoff == cutoff {
            return cutoff;
        }
        cutoff = next_cutoff;
    }
}

fn truncation_marker(omitted_bytes: usize, source: &str) -> String {
    format!("\n<truncated bytes={omitted_bytes} source=\"{source}\">")
}

fn previous_char_boundary(value: &str, mut index: usize) -> usize {
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Compose the `<user_memory>` block for the system prompt, honouring the
/// opt-in toggle. Returns `None` when the feature is disabled or the file
/// is missing / empty so the caller doesn't have to check both conditions.
///
/// Callers that hold a `&Config` should pass `config.memory_enabled()` and
/// `config.memory_path()` directly. The split keeps this module
/// `Config`-free so it can be reused from sub-agent / engine boundaries
/// where the high-level `Config` isn't available.
#[must_use]
pub fn compose_block(enabled: bool, path: &Path) -> Option<String> {
    if !enabled {
        return None;
    }
    let content = load(path)?;
    as_system_block(&content, path)
}

/// Parse `#tag` hashtags from a text string, returning them in order of
/// appearance. Duplicates are preserved as-is; the caller should deduplicate
/// if needed.
pub fn extract_tags(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .filter(|w| w.starts_with('#') && w.len() > 1 && !w[1..].starts_with('#'))
        .collect()
}

/// Remove `#tag` hashtags from a text string, returning the cleaned text.
/// This is used to separate tags from the note body before storage.
fn strip_tags(text: &str) -> String {
    text.split_whitespace()
        .filter(|w| !(w.starts_with('#') && w.len() > 1 && !w[1..].starts_with('#')))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Append `entry` to the memory file at `path`, creating it (and its
/// parent directory) if needed. The entry is timestamped so the user can
/// later see when each note was added. The leading `#` from a `# foo`
/// quick-add is stripped so the file stays as readable Markdown.
///
/// Tags are extracted from two sources:
/// 1. `#tag` hashtags found inline in the entry text
/// 2. The explicit `extra_tags` parameter
///
/// All tags are deduplicated and appended as `#tag` suffixes on the bullet.
pub fn append_entry(path: &Path, entry: &str, extra_tags: &[&str]) -> io::Result<()> {
    let trimmed = entry.trim_start_matches('#').trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "memory entry is empty after stripping `#` prefix",
        ));
    }

    // Extract inline tags from the entry, then strip them from the body
    let inline_tags = extract_tags(trimmed);
    let body = strip_tags(trimmed);
    let body = body.trim();
    if body.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "memory entry has only tags, no content",
        ));
    }

    // Merge and deduplicate tags
    let mut all_tags: Vec<&str> = Vec::new();
    for t in inline_tags.into_iter().chain(extra_tags.iter().copied()) {
        let tag = t.trim_start_matches('#');
        if !tag.is_empty() && !all_tags.contains(&tag) {
            all_tags.push(tag);
        }
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let timestamp = Utc::now().format("%Y-%m-%d %H:%M UTC");
    let tag_str = if all_tags.is_empty() {
        String::new()
    } else {
        format!(" {}", all_tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "))
    };
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "- ({timestamp}) {body}{tag_str}")?;
    Ok(())
}

/// A parsed memory entry with structured fields: timestamp, body text,
/// and a deduplicated list of tags (without leading `#`).
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub timestamp: String,
    pub body: String,
    pub tags: Vec<String>,
    #[allow(dead_code)]
    pub raw: String,
}

/// Parse a single memory line into structured components.
///
/// Format: `- (2026-06-22 10:30 UTC) body text #tag1 #tag2`
///
/// Returns `None` for lines that don't match the expected format (blank
/// lines, non-bullet text, free-form markdown, etc.).
pub fn parse_entry(line: &str) -> Option<MemoryEntry> {
    let line = line.trim();
    if !line.starts_with("- (") {
        return None;
    }
    let close_paren = line.find(')')?;
    let timestamp = line[3..close_paren].to_string();
    let rest = line[close_paren + 1..].trim();
    if rest.is_empty() {
        return None;
    }
    let tag_strs = extract_tags(rest);
    let body = strip_tags(rest);
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let mut seen = Vec::new();
    let tags: Vec<String> = tag_strs
        .iter()
        .map(|t| t.trim_start_matches('#').to_string())
        .filter(|t| {
            if seen.contains(t) {
                false
            } else {
                seen.push(t.clone());
                true
            }
        })
        .collect();
    Some(MemoryEntry {
        timestamp,
        body: body.to_string(),
        tags,
        raw: line.to_string(),
    })
}

/// Parse all bullet entries from memory file content. Non-bullet lines
/// (blank lines, free-form markdown) are silently skipped.
pub fn parse_all(content: &str) -> Vec<MemoryEntry> {
    content.lines().filter_map(parse_entry).collect()
}

/// List all unique tags with their occurrence counts, sorted by frequency
/// (most frequent first). Tags are returned without the leading `#`.
pub fn list_tags(content: &str) -> Vec<(String, usize)> {
    let entries = parse_all(content);
    let mut counts: HashMap<String, usize> = HashMap::new();
    for entry in &entries {
        for tag in &entry.tags {
            *counts.entry(tag.clone()).or_insert(0) += 1;
        }
    }
    let mut result: Vec<_> = counts.into_iter().collect();
    result.sort_by(|a, b| b.1.cmp(&a.1));
    result
}

/// Filter entries that match any of the given tags (OR logic). Tag
/// matching is case-insensitive and supports both `#tag` and `tag` forms.
pub fn search_by_tags<'a>(entries: &'a [MemoryEntry], tags: &[&str]) -> Vec<&'a MemoryEntry> {
    if tags.is_empty() {
        return entries.iter().collect();
    }
    let normalized: Vec<String> = tags
        .iter()
        .map(|t| t.trim_start_matches('#').to_lowercase())
        .collect();
    entries
        .iter()
        .filter(|e| normalized.iter().any(|t| e.tags.iter().any(|et| et.to_lowercase() == *t)))
        .collect()
}

/// Search entries by text content (case-insensitive substring match against
/// both body and tags).
pub fn search_text<'a>(entries: &'a [MemoryEntry], query: &str) -> Vec<&'a MemoryEntry> {
    let q = query.to_lowercase();
    entries
        .iter()
        .filter(|e| {
            e.body.to_lowercase().contains(&q)
                || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
        })
        .collect()
}

/// Simple auto-tagging for entries that have no explicit tags. Extracts
/// capitalized words (potential proper nouns / technical terms) and
/// words containing special characters (camelCase, snake_case, etc.)
/// as candidate tags. Returns at most `max_tags` tags, sorted by quality.
pub fn auto_tag(text: &str, max_tags: usize) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for word in text.split_whitespace() {
        let clean = word.trim_matches(|c: char| c.is_ascii_punctuation());
        if clean.len() < 3 || clean.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // Capitalized words (proper nouns / technical terms)
        if clean.starts_with(|c: char| c.is_uppercase()) {
            let tag = clean.to_lowercase();
            if seen.insert(tag.clone()) {
                candidates.push(tag);
            }
        } else if clean.contains(|c: char| !c.is_alphanumeric() && c != '\'') {
            let tag = clean.to_lowercase();
            if seen.insert(tag.clone()) {
                candidates.push(tag);
            }
        }
    }
    candidates.truncate(max_tags);
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_returns_none_for_missing_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("never-existed.md");
        assert!(load(&path).is_none());
    }

    #[test]
    fn load_returns_none_for_whitespace_only_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        fs::write(&path, "   \n   \n").unwrap();
        assert!(load(&path).is_none());
    }

    #[test]
    fn load_returns_content_for_real_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        fs::write(&path, "remember the milk").unwrap();
        assert_eq!(load(&path).as_deref(), Some("remember the milk"));
    }

    #[test]
    fn as_system_block_produces_xml_wrapper() {
        let block = as_system_block("note 1", Path::new("/tmp/m.md")).unwrap();
        assert!(block.contains("<user_memory source=\"/tmp/m.md\">"));
        assert!(block.contains("note 1"));
        assert!(block.ends_with("</user_memory>"));
    }

    #[test]
    fn as_system_block_returns_none_for_empty_content() {
        assert!(as_system_block("   ", Path::new("/tmp/m.md")).is_none());
    }

    #[test]
    fn as_system_block_truncates_oversize_input() {
        let big = "x".repeat(MAX_MEMORY_SIZE + 100);
        let block = as_system_block(&big, Path::new("/tmp/m.md")).unwrap();
        let payload = user_memory_payload(&block);
        assert_eq!(payload.len(), MAX_MEMORY_SIZE);
        assert!(payload.ends_with("<truncated bytes=141 source=\"/tmp/m.md\">"));
    }

    #[test]
    fn as_system_block_truncates_non_ascii_at_char_boundary() {
        let mut content = "x".repeat(MAX_MEMORY_SIZE - 1);
        content.push('é');
        content.push_str("tail");

        let block = as_system_block(&content, Path::new("/tmp/m.md")).unwrap();
        let payload = block
            .strip_prefix("<user_memory source=\"/tmp/m.md\">\n")
            .unwrap()
            .strip_suffix("\n</user_memory>")
            .unwrap();
        let (head, marker) = payload
            .split_once("\n<truncated bytes=45 source=\"/tmp/m.md\">")
            .unwrap();

        assert_eq!(payload.len(), MAX_MEMORY_SIZE);
        assert_eq!(head.len(), MAX_MEMORY_SIZE - 40);
        assert!(head.bytes().all(|byte| byte == b'x'));
        assert_eq!(marker, "");
    }

    #[test]
    fn as_system_block_truncates_emoji_at_char_boundary() {
        let mut content = "x".repeat(MAX_MEMORY_SIZE - 1);
        content.push('😀');
        content.push_str("tail");

        let block = as_system_block(&content, Path::new("/tmp/m.md")).unwrap();
        assert!(block.contains("<truncated bytes=47 source=\"/tmp/m.md\">"));

        let payload = block
            .strip_prefix("<user_memory source=\"/tmp/m.md\">\n")
            .unwrap()
            .strip_suffix("\n</user_memory>")
            .unwrap();
        let head = payload
            .strip_suffix("\n<truncated bytes=47 source=\"/tmp/m.md\">")
            .unwrap();

        assert_eq!(payload.len(), MAX_MEMORY_SIZE);
        assert!(head.len() <= MAX_MEMORY_SIZE);
        assert_eq!(head.len(), MAX_MEMORY_SIZE - 40);
        assert!(head.bytes().all(|byte| byte == b'x'));
    }

    fn user_memory_payload(block: &str) -> &str {
        block
            .strip_prefix("<user_memory source=\"/tmp/m.md\">\n")
            .unwrap()
            .strip_suffix("\n</user_memory>")
            .unwrap()
    }

    #[test]
    fn append_entry_creates_file_and_writes_one_bullet() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "# remember the milk", &[]).unwrap();

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("remember the milk"), "{body}");
        assert!(
            body.starts_with("- ("),
            "should start with bullet + date: {body}"
        );
        assert!(body.trim_end().ends_with("remember the milk"));
        // No tags appended
        assert!(!body.contains('#'), "no tags expected: {body}");
    }

    #[test]
    fn append_entry_appends_subsequent_lines() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "# first", &[]).unwrap();
        append_entry(&path, "second", &[]).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("first"));
        assert!(body.contains("second"));
        // Two bullets means two lines of `- (date) entry`.
        assert_eq!(body.matches("- (").count(), 2);
    }

    #[test]
    fn append_entry_rejects_empty_after_strip() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        let err = append_entry(&path, "###", &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn append_entry_stores_inline_tags() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "# use 4 spaces #indentation #rust", &[]).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("use 4 spaces"), "{body}");
        assert!(body.contains("#indentation"), "{body}");
        assert!(body.contains("#rust"), "{body}");
        // Tags appear as suffix after body, not inline within the body text
        assert!(
            body.contains("use 4 spaces #indentation"),
            "tags should be appended as suffix: {body}"
        );
    }

    #[test]
    fn append_entry_merges_extra_tags_with_inline_tags() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "use tabs #preference", &["editor", "preference"]).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("use tabs"), "{body}");
        assert!(body.contains("#preference"), "{body}");
        assert!(body.contains("#editor"), "{body}");
    }

    #[test]
    fn append_entry_deduplicates_tags() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "note #dupe", &["dupe", "unique"]).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        // "#dupe" should appear only once
        assert_eq!(body.matches("#dupe").count(), 1, "duplicate tag: {body}");
        assert!(body.contains("#unique"), "{body}");
    }

    #[test]
    fn append_entry_rejects_only_tags_no_body() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        let err = append_entry(&path, "# #tag #only", &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn extract_tags_parses_hashtags() {
        let tags = extract_tags("hello #world this #is #a test");
        assert_eq!(tags, vec!["#world", "#is", "#a"]);
    }

    #[test]
    fn extract_tags_ignores_double_hash() {
        let tags = extract_tags("hello ##world #valid");
        assert_eq!(tags, vec!["#valid"]);
    }

    #[test]
    fn extract_tags_returns_empty_for_no_tags() {
        let tags = extract_tags("hello world");
        assert!(tags.is_empty());
    }

    // === parse_entry / parse_all ===

    #[test]
    fn parse_entry_parses_standard_bullet() {
        let entry = parse_entry("- (2026-06-22 10:30 UTC) remember the milk #chore").unwrap();
        assert_eq!(entry.timestamp, "2026-06-22 10:30 UTC");
        assert_eq!(entry.body, "remember the milk");
        assert_eq!(entry.tags, vec!["chore"]);
    }

    #[test]
    fn parse_entry_returns_none_for_non_bullet() {
        assert!(parse_entry("free form text").is_none());
        assert!(parse_entry("").is_none());
        assert!(parse_entry("  ").is_none());
    }

    #[test]
    fn parse_entry_handles_multi_tag() {
        let entry =
            parse_entry("- (2026-06-22 10:30 UTC) use 4 spaces #indentation #rust #style").unwrap();
        assert_eq!(entry.body, "use 4 spaces");
        assert_eq!(entry.tags, vec!["indentation", "rust", "style"]);
    }

    #[test]
    fn parse_entry_deduplicates_tags() {
        let entry =
            parse_entry("- (2026-06-22 10:30 UTC) note #dupe #unique #dupe").unwrap();
        assert_eq!(entry.tags, vec!["dupe", "unique"]);
    }

    #[test]
    fn parse_entry_handles_no_tags() {
        let entry = parse_entry("- (2026-06-22 10:30 UTC) plain note").unwrap();
        assert_eq!(entry.body, "plain note");
        assert!(entry.tags.is_empty());
    }

    #[test]
    fn parse_all_skips_non_bullet_lines() {
        let content = "\
- (2026-06-22 10:30 UTC) first #tag1
some free text
- (2026-06-22 11:00 UTC) second #tag2

- (2026-06-22 12:00 UTC) third #tag3";
        let entries = parse_all(content);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].body, "first");
        assert_eq!(entries[1].body, "second");
        assert_eq!(entries[2].body, "third");
    }

    #[test]
    fn parse_all_returns_empty_for_empty_content() {
        assert!(parse_all("").is_empty());
        assert!(parse_all("   \n\n  ").is_empty());
    }

    // === list_tags ===

    #[test]
    fn list_tags_returns_sorted_counts() {
        let content = "\
- (2026-06-22 10:00 UTC) a #rust #cli
- (2026-06-22 11:00 UTC) b #rust #web
- (2026-06-22 12:00 UTC) c #cli";
        let tags = list_tags(content);
        assert_eq!(tags.len(), 3);
        // Most frequent first
        assert!(tags[0].0 == "rust" || tags[0].0 == "cli");
        assert_eq!(tags.iter().find(|(t, _)| t == "rust").unwrap().1, 2);
        assert_eq!(tags.iter().find(|(t, _)| t == "cli").unwrap().1, 2);
        assert_eq!(tags.iter().find(|(t, _)| t == "web").unwrap().1, 1);
    }

    #[test]
    fn list_tags_returns_empty_when_no_entries() {
        assert!(list_tags("").is_empty());
    }

    // === search_by_tags ===

    #[test]
    fn search_by_tags_finds_matching_entries() {
        let entries = parse_all(
            "\
- (2026-06-22 10:00 UTC) first #rust
- (2026-06-22 11:00 UTC) second #python
- (2026-06-22 12:00 UTC) third #rust #web",
        );
        let results = search_by_tags(&entries, &["rust"]);
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|e| e.body == "first"));
        assert!(results.iter().any(|e| e.body == "third"));
    }

    #[test]
    fn search_by_tags_accepts_hash_prefix() {
        let entries = parse_all("- (2026-06-22 10:00 UTC) note #mytag");
        let results = search_by_tags(&entries, &["#mytag"]);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_by_tags_or_logic() {
        let entries = parse_all(
            "\
- (2026-06-22 10:00 UTC) first #rust
- (2026-06-22 11:00 UTC) second #python",
        );
        let results = search_by_tags(&entries, &["rust", "python"]);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_by_tags_returns_all_when_empty() {
        let entries = parse_all(
            "\
- (2026-06-22 10:00 UTC) first #rust
- (2026-06-22 11:00 UTC) second #python",
        );
        let results = search_by_tags(&entries, &[]);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_by_tags_no_match() {
        let entries = parse_all("- (2026-06-22 10:00 UTC) note #rust");
        let results = search_by_tags(&entries, &["nonexistent"]);
        assert!(results.is_empty());
    }

    // === search_text ===

    #[test]
    fn search_text_case_insensitive() {
        let entries = parse_all("- (2026-06-22 10:00 UTC) Use Four Spaces");
        let results = search_text(&entries, "four");
        assert_eq!(results.len(), 1);
        let results = search_text(&entries, "FOUR");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_text_matches_tags() {
        let entries = parse_all("- (2026-06-22 10:00 UTC) note #indentation");
        let results = search_text(&entries, "indentation");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_text_no_match() {
        let entries = parse_all("- (2026-06-22 10:00 UTC) note #rust");
        let results = search_text(&entries, "python");
        assert!(results.is_empty());
    }

    // === auto_tag ===

    #[test]
    fn auto_tag_extracts_capitalized_words() {
        let tags = auto_tag("use DeepSeek V4 in CodeWhale", 5);
        assert!(tags.contains(&"deepseek".to_string()));
        assert!(tags.contains(&"codewhale".to_string()));
    }

    #[test]
    fn auto_tag_handles_snake_case() {
        let tags = auto_tag("check the memory_manager config", 5);
        assert!(tags.contains(&"memory_manager".to_string()));
    }

    #[test]
    fn auto_tag_respects_max_tags() {
        let tags = auto_tag("Foo Bar Baz Qux Quux", 3);
        assert_eq!(tags.len(), 3);
    }

    #[test]
    fn auto_tag_returns_empty_for_no_candidates() {
        let tags = auto_tag("a be in it", 5);
        assert!(tags.is_empty());
    }
}
