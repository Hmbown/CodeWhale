//! User-level memory file.
//!
//! v0.8.8 ships an MVP that lets the user keep a persistent personal
//! note file the model sees on every turn:
//!
//! - **Load** `~/.deepseek/memory.md` (path is configurable via
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

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use chrono::Utc;

/// Maximum size of the user memory file. Larger files are loaded but the
/// `<user_memory>` block carries a "(truncated)" marker so the user knows
/// the model only saw a slice. Mirrors `project_context::MAX_CONTEXT_SIZE`.
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

    let display = source.display();
    let payload = if content.len() > MAX_MEMORY_SIZE {
        let cutoff = previous_char_boundary(content, MAX_MEMORY_SIZE);
        let mut head = content[..cutoff].to_string();
        head.push_str("\n…(truncated, raise [memory].max_size or trim memory.md)");
        head
    } else {
        trimmed.to_string()
    };

    Some(format!(
        "<user_memory source=\"{display}\">\n{payload}\n</user_memory>"
    ))
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

/// Append `entry` to the memory file at `path`, creating it (and its
/// parent directory) if needed. The entry is timestamped so the user can
/// later see when each note was added. The leading `#` from a `# foo`
/// quick-add is stripped so the file stays as readable Markdown.
pub fn append_entry(path: &Path, entry: &str) -> io::Result<()> {
    let trimmed = entry.trim_start_matches('#').trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "memory entry is empty after stripping `#` prefix",
        ));
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let timestamp = Utc::now().format("%Y-%m-%d %H:%M UTC");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "- ({timestamp}) {trimmed}")?;
    Ok(())
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
        assert!(block.contains("(truncated"));
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
            .split_once("\n…(truncated, raise [memory].max_size or trim memory.md)")
            .unwrap();

        assert_eq!(head.len(), MAX_MEMORY_SIZE - 1);
        assert!(head.bytes().all(|byte| byte == b'x'));
        assert_eq!(marker, "");
    }

    #[test]
    fn as_system_block_truncates_emoji_at_char_boundary() {
        let mut content = "x".repeat(MAX_MEMORY_SIZE - 1);
        content.push('😀');
        content.push_str("tail");

        let block = as_system_block(&content, Path::new("/tmp/m.md")).unwrap();
        assert!(block.contains("…(truncated, raise [memory].max_size or trim memory.md)"));

        let payload = block
            .strip_prefix("<user_memory source=\"/tmp/m.md\">\n")
            .unwrap()
            .strip_suffix("\n</user_memory>")
            .unwrap();
        let head = payload
            .strip_suffix("\n…(truncated, raise [memory].max_size or trim memory.md)")
            .unwrap();

        assert!(head.len() <= MAX_MEMORY_SIZE);
        assert_eq!(head.len(), MAX_MEMORY_SIZE - 1);
        assert!(head.bytes().all(|byte| byte == b'x'));
    }

    #[test]
    fn append_entry_creates_file_and_writes_one_bullet() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "# remember the milk").unwrap();

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("remember the milk"), "{body}");
        assert!(
            body.starts_with("- ("),
            "should start with bullet + date: {body}"
        );
        assert!(body.trim_end().ends_with("remember the milk"));
    }

    #[test]
    fn append_entry_appends_subsequent_lines() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "# first").unwrap();
        append_entry(&path, "second").unwrap();
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
        let err = append_entry(&path, "###").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
