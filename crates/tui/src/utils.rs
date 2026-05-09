//! Utility helpers shared across the `DeepSeek` CLI.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::models::{ContentBlock, Message};
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde_json::Value;

// === Project Mapping Helpers ===

/// Identify if a file is a "key" file for project identification.
#[must_use]
pub fn is_key_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    matches!(
        file_name.to_lowercase().as_str(),
        "cargo.toml"
            | "package.json"
            | "requirements.txt"
            | "build.gradle"
            | "pom.xml"
            | "readme.md"
            | "agents.md"
            | "claude.md"
            | "makefile"
            | "dockerfile"
            | "main.rs"
            | "lib.rs"
            | "index.js"
            | "index.ts"
            | "app.py"
    )
}

/// Generate a high-level summary of the project based on key files.
///
/// Output is byte-stable across calls: `WalkBuilder` doesn't sort siblings
/// (the OS readdir order leaks through), so the joined `key_files` list
/// would otherwise reorder run-to-run on filesystems that don't pre-sort.
/// Only matters when the workspace has no `AGENTS.md` / `CLAUDE.md`, since
/// the system prompt routes through `ProjectContext::as_system_block` first
/// and only falls back here when no project-context document exists.
#[must_use]
pub fn summarize_project(root: &Path) -> String {
    let mut key_files = Vec::new();

    let mut builder = WalkBuilder::new(root);
    builder.hidden(false).follow_links(false).max_depth(Some(2));
    let walker = builder.build();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.file_type().is_some_and(|ft| ft.is_symlink()) {
            continue;
        }
        if is_key_file(entry.path())
            && let Ok(rel) = entry.path().strip_prefix(root)
        {
            key_files.push(rel.to_string_lossy().to_string());
        }
    }

    key_files.sort();

    if key_files.is_empty() {
        return "Unknown project type".to_string();
    }

    let mut types = Vec::new();
    if key_files
        .iter()
        .any(|f| f.to_lowercase().contains("cargo.toml"))
    {
        types.push("Rust");
    }
    if key_files
        .iter()
        .any(|f| f.to_lowercase().contains("package.json"))
    {
        types.push("JavaScript/Node.js");
    }
    if key_files
        .iter()
        .any(|f| f.to_lowercase().contains("requirements.txt"))
    {
        types.push("Python");
    }

    if types.is_empty() {
        format!("Project with key files: {}", key_files.join(", "))
    } else {
        format!("A {} project", types.join(" and "))
    }
}

/// Generate a tree-like view of the project structure.
///
/// Sibling order is fixed by sorting collected paths — the underlying
/// `WalkBuilder` follows the OS readdir order, which is non-deterministic
/// across filesystems. Sorting by full path preserves the tree shape (a
/// directory still precedes its children because `"src" < "src/lib.rs"`)
/// while making the rendered output byte-stable across runs.
#[must_use]
pub fn project_tree(root: &Path, max_depth: usize) -> String {
    let mut entries: Vec<(PathBuf, bool)> = Vec::new();

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .max_depth(Some(max_depth + 1));

    for entry in builder.build().flatten() {
        if entry.file_type().is_some_and(|ft| ft.is_symlink()) {
            continue;
        }
        let depth = entry.depth();
        if depth == 0 || depth > max_depth {
            continue;
        }
        let rel_path = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_path_buf();
        let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
        entries.push((rel_path, is_dir));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut tree_lines = Vec::with_capacity(entries.len());
    for (rel_path, is_dir) in entries {
        let depth = rel_path.components().count();
        let indent = "  ".repeat(depth.saturating_sub(1));
        let prefix = if is_dir { "DIR: " } else { "FILE: " };
        tree_lines.push(format!(
            "{}{}{}",
            indent,
            prefix,
            rel_path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }

    tree_lines.join("\n")
}

// === Filesystem Helpers ===

/// Atomically write `contents` to `path` using a temporary file + fsync + rename.
///
/// 1. Creates a `NamedTempFile` in the same directory as `path` (same filesystem).
/// 2. Writes `contents` to the temp file.
/// 3. Calls `sync_all()` on the temp file for durability.
/// 4. Atomically renames (persists) the temp file over `path`.
///
/// On filesystems that support it (`ext4`, `apfs`, `ntfs`), the rename is
/// atomic — a concurrent reader sees either the old content or the new, never
/// a partial write. `sync_all` ensures the data is on stable storage before
/// the metadata change so an OS crash mid-rename doesn't lose data.
///
/// # Errors
/// Returns `io::Error` if the parent directory cannot be determined, the temp
/// file cannot be created, the write fails, or the rename fails.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", path.display()),
        )
    })?;
    // Use parent directory so the rename is on the same filesystem.
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut tmp, contents)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    Ok(())
}

/// Open or create a file for appending at `path`, optionally syncing after
/// every write. Use this for append-only logs like `audit.log`.
///
/// The returned `BufWriter<fs::File>` wraps the append handle. Call
/// `.flush()` followed by `.get_ref().sync_all()` after each batch.
pub fn open_append(path: &Path) -> std::io::Result<std::io::BufWriter<std::fs::File>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(std::io::BufWriter::new(file))
}

/// Flush a `BufWriter` wrapping a `File`, then `fsync` the underlying file.
pub fn flush_and_sync(writer: &mut std::io::BufWriter<std::fs::File>) -> std::io::Result<()> {
    writer.flush()?;
    writer.get_ref().sync_all()
}

/// Spawn a tokio task with panic supervision.
///
/// Wraps the future in `AssertUnwindSafe` + `catch_unwind`. On panic:
/// 1. Logs the panic with the task name and caller location via `tracing::error!`.
/// 2. Writes a crash dump to `~/.deepseek/crashes/<timestamp>-<name>.log`.
///
/// The returned `JoinHandle` resolves to `()` — the panic is caught and
/// handled internally so the parent process stays alive.
pub fn spawn_supervised<F>(
    name: &'static str,
    location: &'static std::panic::Location<'static>,
    future: F,
) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        use futures_util::FutureExt;
        let result = std::panic::AssertUnwindSafe(future).catch_unwind().await;
        if let Err(panic_info) = result {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            tracing::error!(
                target: "panic",
                "Task '{name}' panicked at {}: {msg}",
                location,
            );
            // Write crash dump (best-effort)
            let _ = write_panic_dump(name, location, &msg);
        }
    })
}

/// Write a panic dump file to `~/.deepseek/crashes/`.
///
/// Creates the directory if needed and writes a timestamped log
/// with the task name, caller location, and panic message.
/// Best-effort — failures are silently ignored.
fn write_panic_dump(
    name: &str,
    location: &std::panic::Location<'_>,
    message: &str,
) -> std::io::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "home directory not found")
    })?;
    let crash_dir = home.join(".deepseek").join("crashes");
    write_panic_dump_to(&crash_dir, name, location, message)
}

fn write_panic_dump_to(
    crash_dir: &Path,
    name: &str,
    location: &std::panic::Location<'_>,
    message: &str,
) -> std::io::Result<()> {
    use chrono::Utc;
    std::fs::create_dir_all(crash_dir)?;
    let timestamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let filename = format!("{timestamp}-{name}.log");
    let path = crash_dir.join(&filename);
    let contents =
        format!("Task: {name}\nLocation: {location}\nTimestamp: {timestamp}\nPanic: {message}\n");
    std::fs::write(&path, contents)?;
    Ok(())
}

/// Fire-and-forget `spawn_blocking` with panic dump protection.
///
/// In contrast to `spawn_supervised` (which wraps `tokio::spawn` for async
/// tasks), this helper wraps `tokio::task::spawn_blocking`.  Use it when a
/// CPU-bound or blocking-I/O task must run off the async runtime and its
/// completion is *not* awaited — for example a post-turn disk snapshot or a
/// file-tree build polled later via a shared data structure.  If the closure
/// panics, a crash dump is written to `~/.deepseek/crashes/` and the panic
/// is logged at ERROR level rather than being silently swallowed.
#[track_caller]
pub fn spawn_blocking_supervised<F>(name: &'static str, f: F) -> tokio::task::JoinHandle<()>
where
    F: FnOnce() + Send + 'static,
{
    let location = std::panic::Location::caller();
    tokio::task::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if let Err(panic_info) = result {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            tracing::error!(
                target: "panic",
                "Blocking task '{name}' panicked at {location}: {msg}",
            );
            let _ = write_panic_dump(name, location, &msg);
        }
    })
}

#[allow(dead_code)]
pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory: {}", path.display()))
}

/// Render JSON with pretty formatting, falling back to a compact string on error.
#[must_use]
#[allow(dead_code)]
pub fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Truncate a string to a maximum length, adding an ellipsis if truncated.
///
/// Uses char boundaries to avoid panicking on multi-byte UTF-8 characters.
#[must_use]
pub fn truncate_with_ellipsis(s: &str, max_len: usize, ellipsis: &str) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let budget = max_len.saturating_sub(ellipsis.len());
    // Find the last char boundary that fits within the byte budget.
    let safe_end = s
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= budget)
        .last()
        .unwrap_or(0);
    format!("{}{}", &s[..safe_end], ellipsis)
}

/// Percent-encode a string for use in URL query parameters.
///
/// Encodes all characters except unreserved characters (A-Z, a-z, 0-9, `-`, `_`, `.`, `~`).
/// Spaces are encoded as `+`.
#[must_use]
pub fn url_encode(input: &str) -> String {
    let mut encoded = String::new();
    for ch in input.bytes() {
        match ch {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(ch as char)
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{ch:02X}")),
        }
    }
    encoded
}

/// Render a path for **user-facing display** with the home directory
/// contracted to `~`. Use this in the TUI, doctor/setup stdout, and any
/// other place a viewer might see the output (screenshot, video,
/// pasted-into-issue help). On macOS/Linux the absolute path
/// `/Users/<name>/...` or `/home/<name>/...` reveals the OS account name,
/// which is often the same as a public handle — undesirable for users
/// who share their terminal.
///
/// **Do not use** this for paths that get persisted (sessions, audit log)
/// or sent to the LLM provider — those want full fidelity so they
/// resolve correctly across processes.
#[must_use]
pub fn display_path(path: &Path) -> String {
    display_path_with_home(path, dirs::home_dir().as_deref())
}

/// Like [`display_path`] but takes an explicit home directory instead of
/// reading `$HOME` / `dirs::home_dir()`.  Used in tests and anywhere the
/// caller already has the home path available.
///
/// The home-relative suffix is rejoined with the platform separator
/// (`\` on Windows, `/` elsewhere) by walking the path's components, so
/// inputs that carried foreign separators don't leak through.
#[must_use]
pub fn display_path_with_home(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.display().to_string();
    };
    if let Ok(rest) = path.strip_prefix(home) {
        if rest.as_os_str().is_empty() {
            return "~".to_string();
        }
        let sep = std::path::MAIN_SEPARATOR_STR;
        let mut out = String::from("~");
        for component in rest.components() {
            out.push_str(sep);
            out.push_str(&component.as_os_str().to_string_lossy());
        }
        return out;
    }
    path.display().to_string()
}

/// Estimate the total character count across message content blocks.
#[must_use]
pub fn estimate_message_chars(messages: &[Message]) -> usize {
    let mut total = 0;
    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } => total += text.len(),
                ContentBlock::Thinking { thinking } => total += thinking.len(),
                ContentBlock::ToolUse { input, .. } => total += input.to_string().len(),
                ContentBlock::ToolResult { content, .. } => total += content.len(),
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. } => {}
            }
        }
    }
    total
}

// Tests use `display_path_with_home` so they never mutate the global `HOME`
// env var.  Mutating `HOME` via `std::env::set_var` is not thread-safe; Cargo
// runs tests in parallel by default and CI runners are multi-core, so any test
// that stomps `HOME` will race with tests that *read* it.  Using the injected
// helper avoids the race entirely and makes the tests portable to Windows
// without additional platform scaffolding.
#[cfg(test)]
mod tests {
    use super::display_path_with_home;
    use std::path::PathBuf;

    fn home(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    #[test]
    fn display_path_contracts_home_prefix() {
        let h = home("/Users/alice");
        assert_eq!(
            display_path_with_home(&PathBuf::from("/Users/alice/projects/foo"), h.as_deref()),
            format!(
                "~{}projects{}foo",
                std::path::MAIN_SEPARATOR,
                std::path::MAIN_SEPARATOR
            ),
        );
    }

    #[test]
    fn display_path_returns_bare_tilde_for_home_itself() {
        let h = home("/Users/alice");
        assert_eq!(
            display_path_with_home(&PathBuf::from("/Users/alice"), h.as_deref()),
            "~"
        );
    }

    #[test]
    fn display_path_leaves_unrelated_paths_alone() {
        let h = home("/Users/alice");
        // Different user — must not get rewritten or share the tilde.
        assert_eq!(
            display_path_with_home(&PathBuf::from("/Users/bob/Code"), h.as_deref()),
            "/Users/bob/Code".to_string()
        );
        // System path must stay absolute.
        assert_eq!(
            display_path_with_home(&PathBuf::from("/etc/hosts"), h.as_deref()),
            "/etc/hosts"
        );
    }

    #[test]
    fn display_path_does_not_match_username_prefix() {
        // Regression guard: a directory named like the user's home
        // *prefix* but not under it must not get rewritten.
        let h = home("/Users/alice");
        assert_eq!(
            display_path_with_home(&PathBuf::from("/Users/alice2/work"), h.as_deref()),
            "/Users/alice2/work"
        );
    }

    #[test]
    fn display_path_with_no_home_returns_full_path() {
        assert_eq!(
            display_path_with_home(&PathBuf::from("/some/path"), None),
            "/some/path"
        );
    }
}

#[cfg(test)]
mod atomic_write_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn write_atomic_writes_content() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("test.json");
        let content = b"hello atomic world";

        write_atomic(&path, content).expect("write_atomic");
        assert!(path.exists());
        let read = fs::read_to_string(&path).expect("read");
        assert_eq!(read.as_bytes(), content);
    }

    #[test]
    fn write_atomic_replaces_existing_file() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("existing.json");
        fs::write(&path, b"old content").expect("write old");
        write_atomic(&path, b"new content").expect("write_atomic");
        let read = fs::read_to_string(&path).expect("read");
        assert_eq!(read, "new content");
    }

    #[test]
    fn write_atomic_no_temp_left_behind_on_success() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("clean.json");
        write_atomic(&path, b"clean").expect("write_atomic");
        // List files in dir — there should be no .tmp files left
        let entries: Vec<_> = fs::read_dir(tmp.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .collect();
        let tmp_files: Vec<_> = entries
            .iter()
            .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with('.')))
            .collect();
        assert!(
            tmp_files.is_empty(),
            "temp files left behind: {tmp_files:?}"
        );
    }

    #[test]
    fn flush_and_sync_writes_and_syncs() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("append.log");
        {
            let mut writer = open_append(&path).expect("open_append");
            writeln!(writer, "line 1").expect("write");
            flush_and_sync(&mut writer).expect("flush_and_sync");
            writeln!(writer, "line 2").expect("write");
            flush_and_sync(&mut writer).expect("flush_and_sync");
        }
        let content = fs::read_to_string(&path).expect("read");
        assert_eq!(content, "line 1\nline 2\n");
    }
}

#[cfg(test)]
mod spawn_supervised_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A spawned task that panics does not propagate the panic to the
    /// parent task — `spawn_supervised` catches it. Verified in isolation
    /// from the on-disk crash-dump path so the test is portable across
    /// macOS / Linux / Windows (where `dirs::home_dir()` reads
    /// `USERPROFILE`, not `HOME`, so env-mutation tricks don't redirect
    /// the dump on Windows).
    #[tokio::test]
    async fn panicking_task_does_not_propagate_to_parent() {
        let parent_alive = Arc::new(AtomicBool::new(false));
        let parent_alive_clone = parent_alive.clone();

        let handle = spawn_supervised(
            "panic-test-fixture",
            std::panic::Location::caller(),
            async move {
                parent_alive_clone.store(true, Ordering::SeqCst);
                panic!("deliberate panic for catch-unwind test");
            },
        );

        let result = handle.await;
        assert!(
            result.is_ok(),
            "spawn_supervised must convert panic to a normal completion"
        );
        assert!(
            parent_alive.load(Ordering::SeqCst),
            "fixture task must have run before panicking"
        );
    }

    #[tokio::test]
    async fn panicking_blocking_task_does_not_propagate_to_parent() {
        let parent_alive = Arc::new(AtomicBool::new(false));
        let parent_alive_clone = parent_alive.clone();

        let handle = spawn_blocking_supervised("blocking-panic-test-fixture", move || {
            parent_alive_clone.store(true, Ordering::SeqCst);
            panic!("deliberate panic for spawn_blocking catch-unwind test");
        });

        let result = handle.await;
        assert!(
            result.is_ok(),
            "spawn_blocking_supervised must convert panic to a normal completion"
        );
        assert!(
            parent_alive.load(Ordering::SeqCst),
            "fixture blocking task must have run before panicking"
        );
    }

    /// `write_panic_dump_to` writes a properly-formatted crash log into
    /// the supplied directory. Tested separately from `spawn_supervised`
    /// because env-mutation redirection of `dirs::home_dir()` doesn't
    /// work on Windows.
    #[test]
    fn write_panic_dump_writes_named_log() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crash_dir = tmp.path().join("crashes");
        let location = std::panic::Location::caller();
        write_panic_dump_to(&crash_dir, "panic-fixture", location, "boom").expect("write dump");

        let entries: Vec<_> = std::fs::read_dir(&crash_dir)
            .expect("crashes dir exists")
            .flatten()
            .collect();
        assert_eq!(entries.len(), 1, "exactly one crash dump expected");
        let dump = std::fs::read_to_string(entries[0].path()).expect("read dump");
        assert!(
            dump.contains("panic-fixture"),
            "dump must include the task name; got: {dump}"
        );
        assert!(
            dump.contains("boom"),
            "dump must include the panic message; got: {dump}"
        );
    }
}

#[cfg(test)]
mod project_mapping_tests {
    use super::{project_tree, summarize_project};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn project_tree_sorts_siblings_alphabetically() {
        // Cross-platform readdir doesn't guarantee alphabetical order — on
        // ext4 with htree it's hash order, on APFS it's roughly insertion
        // order, on ZFS it's storage-class dependent. The system prompt
        // embeds this string in the cached prefix when a workspace has no
        // AGENTS.md / CLAUDE.md, so the function has to be byte-stable
        // across runs regardless of host filesystem.
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        // Create files in a deliberately scrambled order to make the
        // hosting filesystem's pre-sort (if any) less likely to mask a
        // missing sort in our code.
        fs::write(root.join("zebra.txt"), "z").expect("write zebra");
        fs::write(root.join("apple.txt"), "a").expect("write apple");
        fs::write(root.join("mango.txt"), "m").expect("write mango");

        let tree = project_tree(root, 1);
        let lines: Vec<&str> = tree.lines().collect();
        let apple_pos = lines
            .iter()
            .position(|l| l.contains("apple.txt"))
            .expect("apple line");
        let mango_pos = lines
            .iter()
            .position(|l| l.contains("mango.txt"))
            .expect("mango line");
        let zebra_pos = lines
            .iter()
            .position(|l| l.contains("zebra.txt"))
            .expect("zebra line");

        assert!(apple_pos < mango_pos);
        assert!(mango_pos < zebra_pos);
    }

    #[test]
    fn project_tree_keeps_directory_before_its_children() {
        // Sorting siblings by full path is enough to preserve tree shape:
        // `"src" < "src/lib.rs"` because the shorter string compares less.
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let src = root.join("src");
        fs::create_dir_all(&src).expect("mkdir src");
        fs::write(src.join("lib.rs"), "lib").expect("write lib");
        fs::write(src.join("main.rs"), "main").expect("write main");

        let tree = project_tree(root, 2);
        let src_pos = tree.find("DIR: src").expect("src dir line");
        let lib_pos = tree.find("FILE: lib.rs").expect("lib file line");
        let main_pos = tree.find("FILE: main.rs").expect("main file line");

        assert!(src_pos < lib_pos, "directory must precede its children");
        assert!(lib_pos < main_pos, "siblings sorted by name");
    }

    #[test]
    fn project_tree_is_byte_stable_across_calls() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("z.txt"), "z").expect("write");
        fs::write(root.join("a.txt"), "a").expect("write");

        assert_eq!(project_tree(root, 1), project_tree(root, 1));
    }

    #[test]
    #[cfg(unix)]
    fn project_mapping_does_not_follow_symlinked_key_files() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&root).expect("mkdir workspace");
        fs::create_dir_all(&outside).expect("mkdir outside");
        let outside_file = outside.join("Cargo.toml");
        fs::write(&outside_file, "[package]\nname = \"outside\"\n").expect("write outside");
        std::os::unix::fs::symlink(&outside_file, root.join("Cargo.toml")).expect("symlink");

        assert_eq!(summarize_project(&root), "Unknown project type");
        assert!(!project_tree(&root, 1).contains("Cargo.toml"));
    }

    #[test]
    fn summarize_project_sorts_key_files_in_fallback() {
        // When `summarize_project` can't classify a project type it falls
        // back to listing the discovered key files. That joined list must
        // be deterministic so the system prompt that embeds it doesn't
        // drift between runs on filesystems that emit readdir in a
        // non-alphabetical order.
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        // Use key files that don't trigger any of the type detectors
        // (Cargo.toml / package.json / requirements.txt) so the function
        // hits the `Project with key files: …` branch.
        fs::write(root.join("Makefile"), "all:").expect("write makefile");
        fs::write(root.join("README.md"), "# x").expect("write readme");

        let summary = summarize_project(root);
        assert!(
            summary.starts_with("Project with key files: "),
            "expected fallback branch; got: {summary}"
        );
        let suffix = summary
            .strip_prefix("Project with key files: ")
            .expect("prefix");
        assert_eq!(suffix, "Makefile, README.md");
    }
}
