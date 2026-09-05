//! Utility helpers shared across the `DeepSeek` CLI.

use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::models::{ContentBlock, Message};
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::io;

/// A writer that counts bytes written without storing them.
pub(crate) struct CountingWriter {
    count: usize,
}

impl CountingWriter {
    pub(crate) fn new() -> Self {
        Self { count: 0 }
    }

    pub(crate) fn count(&self) -> usize {
        self.count
    }
}

impl io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.count += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const LOG_FINGERPRINT_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const LOG_FINGERPRINT_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Return a stable, non-reversible log label for an identifier.
///
/// This is meant for correlation in diagnostics where the raw value may be a
/// session token, remote protocol session id, or other bearer-like handle.
#[must_use]
pub fn redacted_identifier_for_log(identifier: &str) -> String {
    if identifier.is_empty() {
        return "<redacted:empty>".to_string();
    }

    let mut hash = LOG_FINGERPRINT_OFFSET_BASIS;
    for byte in identifier.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(LOG_FINGERPRINT_PRIME);
    }
    hash ^= identifier.len() as u64;
    hash = hash.wrapping_mul(LOG_FINGERPRINT_PRIME);

    format!("<redacted:{hash:016x}>")
}

#[cfg(windows)]
pub(crate) fn suppress_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn suppress_console_window(_cmd: &mut Command) {}

#[cfg(windows)]
pub(crate) fn suppress_tokio_console_window(cmd: &mut tokio::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn suppress_tokio_console_window(_cmd: &mut tokio::process::Command) {}

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
pub fn project_tree(root: &Path, max_depth: usize, follow_symlinks: bool) -> String {
    let mut entries: Vec<(PathBuf, bool)> = Vec::new();

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(follow_symlinks)
        .max_depth(Some(max_depth + 1));

    for entry in builder.build().flatten() {
        if entry.file_type().is_some_and(|ft| ft.is_symlink()) && !follow_symlinks {
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

/// Permission policy for atomic writes.
///
/// - [`AtomicWritePermissions::Private`]: keep tempfile's owner-only defaults
///   (used for CodeWhale internal persistence such as session/history/trust).
/// - [`AtomicWritePermissions::Workspace`]: match ordinary workspace file
///   semantics — new files request mode `0666` (kernel applies umask); existing
///   files retain ordinary `rwx` bits (not setuid/setgid/sticky).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomicWritePermissions {
    Private,
    Workspace,
}

/// Atomically write `contents` to `path` using a temporary file + fsync + rename.
///
/// Uses a **private** permission policy (Unix tempfile default `0600`). Prefer
/// [`write_atomic_workspace`] for user workspace source/config files.
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
    write_atomic_with_permissions(path, contents, AtomicWritePermissions::Private)
}

/// Atomically write `contents` to a **user workspace** path.
///
/// On Unix:
/// - New files request creation mode `0666`; the OS applies the process umask
///   (same candidate mode as ordinary `std::fs::write`).
/// - Existing files keep ordinary permission bits (`mode & 0o777`), including
///   executable bits. setuid/setgid/sticky are intentionally not restored.
///
/// On Windows this matches [`write_atomic`] (no POSIX mode simulation).
///
/// # Errors
/// Same failure modes as [`write_atomic`].
pub fn write_atomic_workspace(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    // Hard-link guard (issue #5569): a workspace path that shares its inode
    // with another name cannot be proven to stay inside the writable root by
    // path checks. Atomic rename would replace the directory entry (leaving
    // the outside link on the old inode), but that silently splits the pair
    // and would not block a future non-atomic writer. Fail closed on both
    // platforms that can count links.
    if let Some(links) = hard_link_count(path)
        && links > 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "refusing to rewrite {}: the file has {links} hard links and path checks cannot prove the other links stay inside the workspace; copy it to a new name to break the link",
                path.display(),
            ),
        ));
    }
    write_atomic_with_permissions(path, contents, AtomicWritePermissions::Workspace)
}

/// Hard-link count for an existing regular file, or `None` when the platform
/// cannot answer or the path is not a regular file.
///
/// `None` means "unknown", never "one". A caller guarding against link
/// escapes must treat an unknown count as unguarded, not as safe.
#[cfg(unix)]
fn hard_link_count(path: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    metadata.is_file().then(|| metadata.nlink())
}

/// Windows counts links too — `std` does not expose it, but the Win32 call
/// that does is already used for the workspace `.env` guard in `lib.rs`.
/// Leaving this side unguarded meant a hard-linked file outside the
/// workspace was rewritable on Windows and refused on Unix, which is the
/// worse half of the platform to leave open.
///
/// The handle is opened read-only and closed by `File`'s Drop, so this adds
/// one open/close on a path that is about to be rewritten anyway.
#[cfg(windows)]
fn hard_link_count(path: &Path) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` owns a live kernel handle for the duration of the call
    // and `information` stays writable across it. The call is synchronous and
    // performs no path lookup or re-open.
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information).ok()?;
    }
    Some(u64::from(information.nNumberOfLinks))
}

#[cfg(not(any(unix, windows)))]
fn hard_link_count(_path: &Path) -> Option<u64> {
    None
}

fn write_atomic_with_permissions(
    path: &Path,
    contents: &[u8],
    #[cfg_attr(not(unix), allow(unused_variables))] permission_policy: AtomicWritePermissions,
) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", path.display()),
        )
    })?;

    // Capture ordinary rwx bits before replacement. Use symlink_metadata so we
    // do not follow links: an inaccessible or dangling symlink target must not
    // abort the write — rename still replaces the directory entry, matching
    // the pre-#4606 private write_atomic behavior. Symlink entries themselves
    // are treated as "no mode to preserve" (new ordinary file after rename);
    // only regular-file modes are restored. Mask with 0o777 so setuid/setgid/
    // sticky are never restored after rewriting content.
    #[cfg(unix)]
    let existing_workspace_mode = if permission_policy == AtomicWritePermissions::Workspace {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => None,
            Ok(metadata) => {
                use std::os::unix::fs::PermissionsExt;
                Some(metadata.permissions().mode() & 0o777)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err),
        }
    } else {
        None
    };

    // Use parent directory so the rename is on the same filesystem.
    #[cfg(unix)]
    let mut builder = tempfile::Builder::new();
    #[cfg(not(unix))]
    let builder = tempfile::Builder::new();

    // New workspace files should behave like ordinary files opened with
    // creation mode 0666. The kernel applies the inherited process umask.
    // Do NOT chmod after create: set_permissions bypasses umask.
    #[cfg(unix)]
    if permission_policy == AtomicWritePermissions::Workspace && existing_workspace_mode.is_none() {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o666));
    }

    // Reclaim our own strays before adding another (see the function docs).
    // Private permission policy is also used for user-chosen destinations
    // such as `/save <path>`; only sweep Codewhale-owned state/config dirs.
    if permission_policy == AtomicWritePermissions::Private && is_codewhale_owned_state_dir(parent)
    {
        sweep_stale_atomic_write_temps(parent);
    }

    let mut tmp = builder.tempfile_in(parent)?;
    std::io::Write::write_all(&mut tmp, contents)?;

    // Atomic replacement creates a new inode. Restore ordinary access /
    // executable bits of an existing workspace file before persisting.
    #[cfg(unix)]
    if let Some(mode) = existing_workspace_mode {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }

    tmp.as_file().sync_all()?;
    #[cfg(windows)]
    {
        // Windows can briefly deny replacement while Defender, indexing, or a
        // concurrent reader still holds the destination without delete sharing.
        // Keep the already-synced tempfile and retry only the transient Win32
        // sharing/lock failures; permanent permission errors still surface.
        const MAX_PERSIST_ATTEMPTS: usize = 6;
        let mut pending = tmp;
        for attempt in 0..MAX_PERSIST_ATTEMPTS {
            match pending.persist(path) {
                Ok(_) => break,
                Err(err) => {
                    let retryable = err.error.kind() == std::io::ErrorKind::PermissionDenied
                        || matches!(err.error.raw_os_error(), Some(5 | 32 | 33));
                    if !retryable || attempt + 1 == MAX_PERSIST_ATTEMPTS {
                        return Err(err.error);
                    }
                    pending = err.file;
                    std::thread::sleep(std::time::Duration::from_millis(
                        10u64.saturating_mul(1u64 << attempt),
                    ));
                }
            }
        }
    }
    #[cfg(not(windows))]
    tmp.persist(path)?;
    // Fsync the parent directory so the rename (the new directory entry) is
    // itself durable — otherwise a power loss right after the rename can lose
    // it even though the file data was synced, silently dropping a
    // crash-recovery checkpoint. Best-effort: not all platforms permit
    // opening a directory for sync, so a failure here is not fatal.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// True when `dir` is under `$CODEWHALE_HOME` / `~/.codewhale`, or the ambient
/// `~/.deepseek` legacy root when that root is still in play.
fn is_codewhale_owned_state_dir(dir: &Path) -> bool {
    if dir.as_os_str().is_empty() {
        return false;
    }
    let primary = codewhale_paths::codewhale_home().ok().flatten();
    let legacy = (!codewhale_paths::codewhale_home_is_explicit())
        .then(codewhale_paths::legacy_deepseek_home)
        .flatten();
    [primary, legacy].into_iter().flatten().any(|root| {
        if root.as_os_str().is_empty() {
            return false;
        }
        let Ok(physical_root) = std::fs::canonicalize(root) else {
            return false;
        };
        let Ok(physical_dir) = std::fs::canonicalize(dir) else {
            return false;
        };
        physical_dir.starts_with(physical_root)
    })
}

/// Remove `.tmpXXXXXX` files this writer stranded in `dir` on an earlier run.
///
/// `NamedTempFile` deletes itself on drop, so an ordinary failure — or an
/// ordinary exit — leaves nothing behind. A `SIGKILL` between `tempfile_in`
/// and `persist` cannot run a destructor, so the partial file survives, and
/// nothing ever collected it: five such strays (46 KB each, mode 0600) were
/// sitting in a real `~/.codewhale/` from a single day three weeks earlier.
/// They accumulate silently in the user's config directory forever.
///
/// Deliberately conservative, because this deletes files under `$HOME`:
///
/// - **Product directories only** — parent must be under `$CODEWHALE_HOME`
///   (or `~/.codewhale`) or the ambient `~/.deepseek` legacy root. User-chosen
///   destinations such as `/save <path>` keep the private permission policy
///   but are not swept (enforced at the call site).
/// - **Exact shape only** — `tempfile`'s default naming is the literal prefix
///   `.tmp` followed by exactly six alphanumerics and nothing else. A user file
///   called `.tmp`, `.tmpfile`, or `.tmp-backup` does not match.
/// - **Older than an hour** — so a concurrent write by another Codewhale
///   process is never raced. Same threshold and reasoning as
///   `shell_dispatcher::sweep_stale_temp_ps1`.
/// - **Best effort** — every failure is ignored; this must never turn a
///   successful write into an error.
fn sweep_stale_atomic_write_temps(dir: &Path) {
    const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60);
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_stray_atomic_write_temp_name(name) {
            continue;
        }
        // Only regular files; never follow or remove a symlink or directory.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_AFTER);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// `tempfile`'s default name: `.tmp` + exactly six ASCII alphanumerics.
fn is_stray_atomic_write_temp_name(name: &str) -> bool {
    let Some(random) = name.strip_prefix(".tmp") else {
        return false;
    };
    random.len() == 6 && random.chars().all(|c| c.is_ascii_alphanumeric())
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

/// Open a URL in the system's default browser.
///
/// Dispatches to the platform-appropriate opener:
/// - macOS: `open`
/// - Linux / BSD: `xdg-open`
/// - Windows: `cmd /C start ""`
/// - Other: returns an error.
///
/// This is the single entry point for URL opening — every call site in
/// the codebase should use this instead of hardcoding `Command::new("open")`,
/// `Command::new("xdg-open")`, or `Command::new("cmd")`.
pub fn open_url(url: &str) -> Result<()> {
    let mut command = browser_open_command(url)?;
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("failed to launch browser command: {e}"))
}

fn browser_open_command(url: &str) -> Result<Command> {
    if url.trim().is_empty() {
        return Err(anyhow::anyhow!("browser URL cannot be empty"));
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg(url);
        Ok(command)
    }

    #[cfg(any(
        all(target_os = "linux", not(target_env = "ohos")),
        target_os = "netbsd",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        Ok(command)
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        Ok(cmd)
    }

    #[cfg(not(any(
        target_os = "macos",
        all(target_os = "linux", not(target_env = "ohos")),
        target_os = "windows",
        target_os = "netbsd",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )))]
    Err(anyhow::anyhow!(
        "browser opening is unsupported on this platform"
    ))
}

/// Spawn a tokio task with panic supervision.
///
/// Wraps the future in `AssertUnwindSafe` + `catch_unwind`. On panic:
/// 1. Logs the panic with the task name and caller location via `tracing::error!`.
/// 2. Writes a crash dump to `~/.codewhale/crashes/<timestamp>-<name>.log`.
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
            let msg = panic_message(&*panic_info);
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

/// Extract a human-readable message from a caught panic payload (the `Err`
/// value of `catch_unwind`). Mirrors how the panic hook formats `&str` and
/// `String` payloads so crash dumps stay consistent across call sites.
#[must_use]
pub fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Record a panic that was caught at a call site (via `catch_unwind`) rather
/// than by a task supervisor. Logs it on the `panic` target and writes a
/// best-effort crash dump to `~/.codewhale/crashes/`, so diagnostics land in
/// the same place `spawn_supervised` writes them even when the caller recovers
/// and keeps running.
#[track_caller]
pub fn record_caught_panic(name: &'static str, message: &str) {
    let location = std::panic::Location::caller();
    tracing::error!(target: "panic", "Task '{name}' panicked at {location}: {message}");
    let _ = write_panic_dump(name, location, message);
    // A caught panic is still a panic. The site is allowlist-reduced to
    // `crates/…` or the literal `<dep>`, and `message` is deliberately not
    // read: a slicing panic embeds the entire string being sliced. The exit
    // class is left alone — the caller recovered, so this process is not
    // ending here. A no-op unless this process was armed.
    codewhale_telemetry::record_blocking(codewhale_telemetry::Event::Panic {
        site: codewhale_telemetry::reduce_panic_site(
            location.file(),
            location.line(),
            location.column(),
        ),
    });
}

/// Write a panic dump file to `~/.codewhale/crashes/`.
///
/// Creates the directory if needed and writes a timestamped log
/// with the task name, caller location, and panic message.
/// Best-effort — failures are silently ignored.
fn write_panic_dump(
    name: &str,
    location: &std::panic::Location<'_>,
    message: &str,
) -> std::io::Result<()> {
    let home = crate::config::effective_home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "home directory not found")
    })?;
    // Prefer .codewhale, fall back to .deepseek
    let crash_dir = home.join(".codewhale").join("crashes");
    if !crash_dir.exists() {
        // Try legacy path for reading, but prefer new for writing
        let _ = std::fs::create_dir_all(&crash_dir);
    }
    let crash_dir = if crash_dir.exists() {
        crash_dir
    } else {
        home.join(".deepseek").join("crashes")
    };
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
/// panics, a crash dump is written to `~/.codewhale/crashes/` and the panic
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
            let msg = panic_message(&*panic_info);
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
    display_path_with_home(path, crate::config::effective_home_dir().as_deref())
}

/// Like [`display_path`] but takes an explicit home directory instead of
/// reading `$HOME` / `crate::config::effective_home_dir()`.  Used in tests and anywhere the
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
                ContentBlock::Thinking { thinking, .. } => total += thinking.len(),
                ContentBlock::ToolUse { input, .. } => {
                    let mut cw = CountingWriter::new();
                    let _ = serde_json::to_writer(&mut cw, input);
                    total += cw.count();
                }
                ContentBlock::ToolResult { content, .. } => total += content.len(),
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. }
                | ContentBlock::ImageUrl { .. } => {}
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
    use super::{display_path_with_home, redacted_identifier_for_log};
    use std::path::PathBuf;

    fn home(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    #[test]
    fn redacted_identifier_for_log_hides_value_and_stays_stable() {
        let identifier = "session-secret-1234567890";
        let redacted = redacted_identifier_for_log(identifier);

        assert!(redacted.starts_with("<redacted:"));
        assert!(redacted.ends_with('>'));
        assert!(!redacted.contains(identifier));
        assert_eq!(redacted, redacted_identifier_for_log(identifier));
        assert_ne!(redacted, redacted_identifier_for_log("another-session"));
    }

    #[test]
    fn redacted_identifier_for_log_marks_empty_values() {
        assert_eq!(redacted_identifier_for_log(""), "<redacted:empty>");
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

    #[cfg(windows)]
    #[test]
    fn write_atomic_retries_windows_replace_contention() {
        use std::os::windows::fs::OpenOptionsExt;

        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("contended.json");
        fs::write(&path, b"old content").expect("write old");

        // FILE_SHARE_READ | FILE_SHARE_WRITE deliberately omits
        // FILE_SHARE_DELETE, reproducing the short-lived handle contention
        // that makes MoveFileExW report access denied during replacement.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&path)
            .expect("hold destination without delete sharing");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(held);
        });

        write_atomic(&path, b"new content").expect("retry contended atomic replacement");
        release.join().expect("release destination handle");
        assert_eq!(fs::read(&path).expect("read replacement"), b"new content");
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

    #[cfg(any(unix, windows))]
    fn assert_workspace_hard_link_is_refused() {
        let dir = tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&outside).expect("create outside directory");
        let outside = outside.join("outside.txt");
        fs::write(&outside, b"outside").expect("outside state");
        let linked = workspace.join("linked.txt");
        fs::hard_link(&outside, &linked).expect("hard link");

        let err = write_atomic_workspace(&linked, b"new").expect_err("must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("hard links"),
            "the refusal must name the hard-link reason: {err}"
        );
        assert_eq!(
            fs::read_to_string(&outside).expect("outside read"),
            "outside",
            "the outside file must stay untouched"
        );
        assert_eq!(
            fs::read_to_string(&linked).expect("linked read"),
            "outside",
            "the workspace entry must stay untouched too"
        );
        // Breaking the link re-enables normal writes.
        fs::remove_file(&linked).expect("break link");
        write_atomic_workspace(&linked, b"fresh").expect("single-link write");
        assert_eq!(fs::read_to_string(&linked).expect("fresh read"), "fresh");
        assert_eq!(
            fs::read_to_string(&outside).expect("outside read"),
            "outside"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_workspace_refuses_a_hard_linked_target() {
        assert_workspace_hard_link_is_refused();
    }

    #[cfg(windows)]
    #[test]
    fn windows_write_atomic_workspace_refuses_a_hard_linked_target() {
        assert_workspace_hard_link_is_refused();
    }

    #[cfg(windows)]
    #[test]
    fn windows_hard_link_count_detects_multiple_links() {
        let dir = tempdir().expect("tempdir");
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        fs::write(&first, b"linked").expect("write fixture");
        fs::hard_link(&first, &second).expect("hard link");

        assert_eq!(hard_link_count(&second), Some(2));
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_workspace_new_file_matches_standard_creation_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let control = dir.path().join("control.txt");
        let actual = dir.path().join("actual.txt");

        fs::write(&control, b"control").expect("write control");
        write_atomic_workspace(&actual, b"actual").expect("atomic workspace write");

        let control_mode = fs::metadata(&control)
            .expect("control metadata")
            .permissions()
            .mode()
            & 0o777;
        let actual_mode = fs::metadata(&actual)
            .expect("actual metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(actual_mode, control_mode);
        assert_eq!(fs::read(&actual).expect("read"), b"actual");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_workspace_preserves_existing_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("shared.txt");
        fs::write(&path, b"before").expect("initial write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o664))
            .expect("set shared permissions");

        write_atomic_workspace(&path, b"after").expect("atomic workspace write");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o664);
        assert_eq!(fs::read(&path).expect("read"), b"after");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_workspace_preserves_executable_bits() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("script.sh");
        fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("initial write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("set executable permissions");

        write_atomic_workspace(&path, b"#!/bin/sh\nexit 1\n").expect("atomic workspace write");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        assert_eq!(fs::read(&path).expect("read"), b"#!/bin/sh\nexit 1\n");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_workspace_does_not_restore_special_bits() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("special.sh");
        fs::write(&path, b"#!/bin/sh\n").expect("initial write");
        // Request sticky + setgid + rwxr-xr-x. Filesystems may clear some
        // special bits; we only assert that after rewrite we never keep
        // bits outside the ordinary 0o777 mask.
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o6755));
        let before = fs::metadata(&path)
            .expect("metadata before")
            .permissions()
            .mode();
        let expected_ordinary = before & 0o777;

        write_atomic_workspace(&path, b"#!/bin/sh\necho rewritten\n")
            .expect("atomic workspace write");

        let after = fs::metadata(&path)
            .expect("metadata after")
            .permissions()
            .mode();
        assert_eq!(after & 0o777, expected_ordinary);
        // `PermissionsExt::mode()` also contains the regular-file type bit on
        // macOS/BSD. Check only the Unix special permission bits rather than
        // treating every non-rwx bit as a restored permission.
        assert_eq!(after & 0o7000, 0, "special bits must not be restored");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_workspace_replaces_symlink_without_following_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, b"target-body").expect("write target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .expect("lock down target mode");
        symlink(&target, &link).expect("create symlink");

        write_atomic_workspace(&link, b"replaced-link")
            .expect("workspace write must replace symlink directory entry");

        let link_meta = fs::symlink_metadata(&link).expect("link metadata");
        assert!(
            link_meta.file_type().is_file() && !link_meta.file_type().is_symlink(),
            "rename should replace the symlink with a regular file"
        );
        assert_eq!(fs::read(&link).expect("read link path"), b"replaced-link");
        // Target inode must remain untouched (old private write_atomic semantics).
        assert_eq!(fs::read(&target).expect("read target"), b"target-body");
        assert_eq!(
            fs::metadata(&target)
                .expect("target metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_workspace_replaces_self_referential_symlink_without_following() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let link = dir.path().join("self-link.txt");
        symlink(&link, &link).expect("create self-referential symlink");

        assert!(
            fs::symlink_metadata(&link)
                .expect("lstat self-referential symlink")
                .file_type()
                .is_symlink()
        );
        let follow_error = fs::metadata(&link).expect_err("following the symlink must fail");
        assert_ne!(
            follow_error.kind(),
            std::io::ErrorKind::NotFound,
            "the fixture must catch a metadata-following regression"
        );

        let result = write_atomic_workspace(&link, b"new-content");
        result.expect("workspace write must not follow a self-referential symlink");
        let link_meta = fs::symlink_metadata(&link).expect("link metadata");
        assert!(link_meta.file_type().is_file() && !link_meta.file_type().is_symlink());
        assert_eq!(fs::read(&link).expect("read"), b"new-content");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_private_new_file_does_not_gain_group_or_other_access() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("private.json");

        write_atomic(&path, b"{}").expect("private atomic write");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn write_atomic_workspace_rewrites_a_single_link_file() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("workspace.txt");
        fs::write(&path, b"before").expect("write initial content");
        write_atomic_workspace(&path, b"workspace").expect("write_atomic_workspace");
        assert_eq!(fs::read(&path).expect("read"), b"workspace");
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

    // === stray atomic-write temp files ===

    /// Backdate a file's mtime past the sweeper's one-hour threshold, using
    /// std rather than pulling in a dev-dependency just to age a fixture.
    fn age_past_the_threshold(path: &std::path::Path) {
        let two_hours_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 60 * 60);
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open fixture");
        file.set_times(std::fs::FileTimes::new().set_modified(two_hours_ago))
            .expect("age the fixture");
    }

    #[test]
    fn stray_temp_names_match_only_tempfiles_default_shape() {
        // What `tempfile` actually produces.
        assert!(super::is_stray_atomic_write_temp_name(".tmp0dqfST"));
        assert!(super::is_stray_atomic_write_temp_name(".tmpBcX9dY"));
        assert!(super::is_stray_atomic_write_temp_name(".tmpABC123"));

        // User files that must never be swept.
        for safe in [
            ".tmp",
            ".tmpfile",
            ".tmp-backup",
            ".tmp12345",   // five
            ".tmp1234567", // seven
            ".tmpABC12_",  // underscore is not alphanumeric
            "tmpABC123",   // no leading dot
            ".temp123456",
            "config.toml",
        ] {
            assert!(
                !super::is_stray_atomic_write_temp_name(safe),
                "{safe} must not be treated as ours to delete"
            );
        }
    }

    #[test]
    fn sweeping_removes_only_old_strays_and_leaves_everything_else() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let old_stray = dir.path().join(".tmpAAAAAA");
        let fresh_stray = dir.path().join(".tmpBBBBBB");
        let user_file = dir.path().join(".tmp-please-keep");
        let real_file = dir.path().join("config.toml");
        for path in [&old_stray, &fresh_stray, &user_file, &real_file] {
            std::fs::write(path, b"x").expect("write fixture");
        }

        age_past_the_threshold(&old_stray);

        super::sweep_stale_atomic_write_temps(dir.path());

        assert!(
            !old_stray.exists(),
            "an hour-old stray of ours is collected"
        );
        assert!(
            fresh_stray.exists(),
            "a fresh stray may belong to a concurrent write and must be left alone"
        );
        assert!(user_file.exists(), "a user file must never be swept");
        assert!(real_file.exists());
    }

    /// Seal HOME / CODEWHALE_HOME to `tmp` so sweep policy is deterministic
    /// and never inspects the developer's real `~/.codewhale`.
    fn seal_product_home(
        tmp: &std::path::Path,
    ) -> (std::path::PathBuf, Vec<crate::test_support::EnvVarGuard>) {
        use crate::test_support::EnvVarGuard;

        let product = tmp.join("product-home");
        std::fs::create_dir_all(&product).expect("create product home");
        let guards = vec![
            EnvVarGuard::set("HOME", tmp),
            EnvVarGuard::set("USERPROFILE", tmp),
            EnvVarGuard::set("CODEWHALE_HOME", &product),
            EnvVarGuard::remove("CODEWHALE_CONFIG_PATH"),
            EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH"),
            EnvVarGuard::remove("DEEPSEEK_HOME"),
        ];
        (product, guards)
    }

    #[test]
    fn a_private_atomic_write_in_a_product_dir_collects_strays() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let (product, _guards) = seal_product_home(tmp.path());

        assert!(
            super::is_codewhale_owned_state_dir(&product),
            "sealed CODEWHALE_HOME must count as a product dir"
        );

        let stray = product.join(".tmpCCCCCC");
        std::fs::write(&stray, b"stranded by a SIGKILL").expect("write stray");
        age_past_the_threshold(&stray);

        let target = product.join("state.json");
        super::write_atomic(&target, b"{\"ok\":true}").expect("atomic write");

        assert_eq!(std::fs::read(&target).expect("read back"), b"{\"ok\":true}");
        assert!(!stray.exists(), "the write reclaimed the earlier stray");
    }

    #[test]
    fn a_private_atomic_write_to_a_user_chosen_dest_does_not_sweep() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let (_product, _guards) = seal_product_home(tmp.path());
        let user_dir = tmp.path().join("user-chosen");
        std::fs::create_dir_all(&user_dir).expect("create user dest");

        assert!(
            !super::is_codewhale_owned_state_dir(&user_dir),
            "user-chosen dest must not count as a product dir: {}",
            user_dir.display()
        );

        let stray = user_dir.join(".tmpDDDDDD");
        std::fs::write(&stray, b"user or concurrent tempfile").expect("write stray");
        age_past_the_threshold(&stray);

        let target = user_dir.join("session.json");
        super::write_atomic(&target, b"{\"ok\":true}").expect("atomic write");

        assert_eq!(std::fs::read(&target).expect("read back"), b"{\"ok\":true}");
        assert!(
            stray.exists(),
            "/save <path> and other user-chosen dests must not sweep the parent"
        );
    }

    #[test]
    fn atomic_write_product_dir_detection_matches_codewhale_home_not_siblings() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let (product, explicit_guards) = seal_product_home(tmp.path());
        let sessions = product.join("sessions");
        std::fs::create_dir_all(&sessions).expect("create sessions");
        let sibling = tmp.path().join("product-home-extra");
        std::fs::create_dir_all(&sibling).expect("create sibling");

        assert!(super::is_codewhale_owned_state_dir(&product));
        assert!(super::is_codewhale_owned_state_dir(&sessions));
        assert!(!super::is_codewhale_owned_state_dir(&sibling));
        assert!(!super::is_codewhale_owned_state_dir(tmp.path()));
        drop(explicit_guards);

        use crate::test_support::EnvVarGuard;
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        let _no_explicit = EnvVarGuard::remove("CODEWHALE_HOME");
        let _no_config = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _no_legacy_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");
        let _no_legacy_home = EnvVarGuard::remove("DEEPSEEK_HOME");
        let primary_sessions = tmp.path().join(".codewhale").join("sessions");
        let legacy_home = tmp.path().join(".deepseek");
        std::fs::create_dir_all(&primary_sessions).expect("create primary sessions");
        std::fs::create_dir_all(&legacy_home).expect("create legacy home");

        assert!(super::is_codewhale_owned_state_dir(&primary_sessions));
        assert!(super::is_codewhale_owned_state_dir(&legacy_home));
        assert!(!super::is_codewhale_owned_state_dir(
            &tmp.path().join("user-chosen")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_product_subdir_cannot_sweep_an_external_directory() {
        use std::os::unix::fs::symlink;

        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let (product, _guards) = seal_product_home(tmp.path());
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).expect("create external dir");
        let linked = product.join("exports");
        symlink(&external, &linked).expect("link product subdir outside");

        let stray = external.join(".tmpEEEEEE");
        std::fs::write(&stray, b"external tempfile").expect("write external fixture");
        age_past_the_threshold(&stray);

        assert!(
            !super::is_codewhale_owned_state_dir(&linked),
            "physical containment must reject a nested symlink escape"
        );
        super::write_atomic(&linked.join("state.json"), b"{\"ok\":true}")
            .expect("atomic write through link remains non-sweeping");

        assert!(
            stray.exists(),
            "external temp-shaped files must not be swept"
        );
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
    /// macOS / Linux / Windows (where `crate::config::effective_home_dir()` reads
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
    /// because env-mutation redirection of `crate::config::effective_home_dir()` doesn't
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

        let tree = project_tree(root, 1, false);
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

        let tree = project_tree(root, 2, false);
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

        assert_eq!(project_tree(root, 1, false), project_tree(root, 1, false));
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
        assert!(!project_tree(&root, 1, false).contains("Cargo.toml"));
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

    // ===================================================================
    // open_url tests
    // ===================================================================

    #[test]
    fn open_url_builds_platform_command_without_spawning() {
        let command = super::browser_open_command("https://example.com").expect("command");

        #[cfg(target_os = "macos")]
        {
            assert_eq!(command.get_program(), "open");
            assert_eq!(
                command
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
                vec!["https://example.com"]
            );
        }

        #[cfg(any(
            target_os = "netbsd",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        {
            assert_eq!(command.get_program(), "xdg-open");
        }

        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            assert_eq!(command.get_program(), "xdg-open");
            assert_eq!(
                command
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
                vec!["https://example.com"]
            );
        }

        #[cfg(target_os = "windows")]
        {
            assert_eq!(command.get_program(), "cmd");
            assert_eq!(
                command
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
                vec!["/C", "start", "", "https://example.com"]
            );
        }
    }

    #[test]
    fn open_url_rejects_empty_url_gracefully() {
        // An empty URL should fail with a clear error, not panic.
        let result = super::browser_open_command("");
        match result {
            Ok(_) => panic!("empty URL should not build an opener command"),
            Err(e) => {
                let msg = e.to_string();
                assert!(!msg.is_empty(), "error message must not be empty");
                assert!(msg.contains("empty"), "unexpected error message: {msg}");
            }
        }
    }
}
