//! Bridge discovery via the IDE host's MCP-over-WebSocket contract.
//!
//! Discovery order (mirrors opencode's `resolveEditorConnection`):
//! 1. `CLAUDE_CODE_SSE_PORT` or `DEEPSEEK_EDITOR_SSE_PORT` — explicit port
//!    override, always `ws://127.0.0.1:<port>`, no lockfile needed.
//! 2. Scan `~/.claude/ide/*.lock` for the best workspace match against cwd.
//!    "Best" = longest `workspaceFolders` entry that contains cwd, then most
//!    recently modified. If no lockfile's workspace contains cwd, discovery
//!    returns `None` and the TUI stays disconnected.
//!
//! Lockfiles must be JSON. A lockfile is accepted unless it explicitly
//! declares a `transport` value that is not `"ws"` (missing `transport` is
//! fine — opencode treats it the same way).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_BRIDGE_HOST, Error, IDE_BRIDGE_DEEPSEEK_PORT_ENV, IDE_BRIDGE_LOCKFILE_DIR,
    IDE_BRIDGE_PORT_ENV, Result,
};

// ──────────────────────────────────────────────────────────────────────────────
// Public types
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BridgeAuth {
    Inherited,
    Token(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverySource {
    EnvPort,
    Lockfile,
}

#[derive(Debug, Clone)]
pub struct BridgeTarget {
    pub port: u16,
    pub host: String,
    pub auth: BridgeAuth,
    pub source: DiscoverySource,
    pub workspace_folders: Vec<String>,
    pub lockfile: Option<PathBuf>,
    pub ide_name: Option<String>,
}

impl BridgeTarget {
    pub fn ws_url(&self) -> String {
        format!("ws://{}:{}/", self.host, self.port)
    }
}

#[derive(Debug, Clone)]
pub struct LockfileEntry {
    pub port: u16,
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
    pub data: LockfileBody,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockfileBody {
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub workspace_folders: Vec<String>,
    #[serde(default)]
    pub ide_name: Option<String>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub running_in_windows: Option<bool>,
    #[serde(default)]
    pub auth_token: Option<String>,
}

impl LockfileBody {
    /// Accept the lockfile unless it explicitly declares a non-ws transport.
    /// Missing `transport` is fine (opencode compat).
    fn supports_websocket(&self) -> bool {
        match self.transport.as_deref() {
            None => true,
            Some(t) => t.eq_ignore_ascii_case("ws"),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Discover the best IDE bridge target for the current process.
pub fn discover() -> Result<Option<BridgeTarget>> {
    let cwd = std::env::current_dir().ok();
    discover_inner(cwd.as_deref())
}

/// Discover with an explicit directory (used when reconnecting after cd).
pub fn discover_for(directory: &Path) -> Result<Option<BridgeTarget>> {
    discover_inner(Some(directory))
}

fn discover_inner(cwd: Option<&Path>) -> Result<Option<BridgeTarget>> {
    // 1. Env port override (highest priority, no lockfile needed).
    if let Some(port) = port_from_env()? {
        return Ok(Some(BridgeTarget {
            port,
            host: DEFAULT_BRIDGE_HOST.to_string(),
            auth: BridgeAuth::Inherited,
            source: DiscoverySource::EnvPort,
            workspace_folders: Vec::new(),
            lockfile: None,
            ide_name: None,
        }));
    }

    // 2. Lockfile scan.
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let dir = lockfile_dir(&home);
    let entries = scan_lockfile_dir(&dir)?;
    Ok(pick_best(entries, cwd).map(|entry| {
        let auth = match entry.data.auth_token {
            Some(ref token) if !token.is_empty() => BridgeAuth::Token(token.clone()),
            _ => BridgeAuth::Inherited,
        };
        BridgeTarget {
            port: entry.port,
            host: DEFAULT_BRIDGE_HOST.to_string(),
            auth,
            source: DiscoverySource::Lockfile,
            workspace_folders: entry.data.workspace_folders.clone(),
            lockfile: Some(entry.path),
            ide_name: entry.data.ide_name.clone(),
        }
    }))
}

pub fn scan_lockfile_dir(dir: &Path) -> Result<Vec<LockfileEntry>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Discovery(format!("read {}: {e}", dir.display()))),
    };

    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(port) = parse_port_from_filename(&path) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(body) = parse_lockfile_body(&content, &path) else {
            continue;
        };
        if !body.supports_websocket() {
            tracing::debug!(path = %path.display(), "skipping non-WebSocket IDE lockfile");
            continue;
        }
        out.push(LockfileEntry {
            port,
            path,
            modified: meta.modified().ok(),
            data: body,
        });
    }
    Ok(out)
}

// ──────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────────────────────────────────────

fn port_from_env() -> Result<Option<u16>> {
    let raw = std::env::var(IDE_BRIDGE_PORT_ENV)
        .ok()
        .or_else(|| std::env::var(IDE_BRIDGE_DEEPSEEK_PORT_ENV).ok());
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let port: u16 = trimmed
        .parse()
        .map_err(|e| Error::Discovery(format!("port env is not a valid port number: {e}")))?;
    Ok(Some(port))
}

fn lockfile_dir(home: &Path) -> PathBuf {
    let mut path = home.to_path_buf();
    for segment in IDE_BRIDGE_LOCKFILE_DIR.split('/') {
        path.push(segment);
    }
    path
}

fn parse_port_from_filename(path: &Path) -> Option<u16> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".lock")?;
    stem.parse().ok()
}

fn parse_lockfile_body(content: &str, path: &Path) -> Option<LockfileBody> {
    match serde_json::from_str(content) {
        Ok(body) => Some(body),
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "skipping malformed IDE lockfile");
            None
        }
    }
}

/// Pick the best lockfile: longest workspace-folder match against cwd, then
/// most recently modified. If no lockfile's workspace contains cwd, return
/// `None` (never fall back to an unrelated lockfile).
fn pick_best(entries: Vec<LockfileEntry>, cwd: Option<&Path>) -> Option<LockfileEntry> {
    let Some(cwd) = cwd else {
        // Without a cwd we cannot tell which lockfile belongs to the current
        // project. Return None rather than guessing.
        return None;
    };

    let cwd_normalized = normalize_path(cwd);

    let mut scored: Vec<(LockfileEntry, usize)> = entries
        .into_iter()
        .filter_map(|entry| {
            let best = entry
                .data
                .workspace_folders
                .iter()
                .map(|folder| {
                    path_contains_length(&normalize_path(Path::new(folder)), &cwd_normalized)
                })
                .max()
                .unwrap_or(0);
            if best > 0 { Some((entry, best)) } else { None }
        })
        .collect();

    if scored.is_empty() {
        return None;
    }

    // Sort: longest match first, then newest mtime.
    scored.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            let ta = a.0.modified.unwrap_or(SystemTime::UNIX_EPOCH);
            let tb = b.0.modified.unwrap_or(SystemTime::UNIX_EPOCH);
            tb.cmp(&ta)
        })
    });

    Some(scored.remove(0).0)
}

fn normalize_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

/// Returns the length of `parent` if `child` is equal to or nested inside
/// `parent`, otherwise 0. This is the "longest match" metric.
fn path_contains_length(parent: &str, child: &str) -> usize {
    if parent.is_empty() {
        return 0;
    }
    if child == parent {
        return parent.len();
    }
    if let Some(rest) = child.strip_prefix(parent)
        && rest.starts_with('/')
    {
        return parent.len();
    }
    0
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    fn temp_dir(suffix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "deepseek-ide-bridge-{}-{}-{}",
            suffix,
            std::process::id(),
            uuid::Uuid::new_v4(),
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn lockfile_without_transport_is_accepted() {
        let home = temp_dir("no-transport");
        let dir = lockfile_dir(&home);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("5000.lock"),
            r#"{"workspaceFolders":["/projects/app"],"authToken":"tok"}"#,
        )
        .unwrap();

        let entries = scan_lockfile_dir(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].port, 5000);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn explicit_non_ws_transport_is_rejected() {
        let home = temp_dir("sse-transport");
        let dir = lockfile_dir(&home);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("5000.lock"),
            r#"{"transport":"sse","workspaceFolders":["/projects/app"]}"#,
        )
        .unwrap();

        let entries = scan_lockfile_dir(&dir).unwrap();
        assert!(entries.is_empty());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn longest_workspace_match_wins() {
        let home = temp_dir("longest-match");
        let dir = lockfile_dir(&home);
        fs::create_dir_all(&dir).unwrap();

        // Port 1000: workspace is /projects
        fs::write(
            dir.join("1000.lock"),
            r#"{"transport":"ws","workspaceFolders":["/projects"]}"#,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        // Port 2000: workspace is /projects/app (longer match)
        fs::write(
            dir.join("2000.lock"),
            r#"{"transport":"ws","workspaceFolders":["/projects/app"]}"#,
        )
        .unwrap();

        let entries = scan_lockfile_dir(&dir).unwrap();
        let chosen = pick_best(entries, Some(Path::new("/projects/app/src"))).unwrap();
        assert_eq!(chosen.port, 2000);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn mtime_breaks_tie_when_match_length_equal() {
        let home = temp_dir("mtime-tie");
        let dir = lockfile_dir(&home);
        fs::create_dir_all(&dir).unwrap();

        fs::write(
            dir.join("1000.lock"),
            r#"{"transport":"ws","workspaceFolders":["/projects/app"]}"#,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        fs::write(
            dir.join("2000.lock"),
            r#"{"transport":"ws","workspaceFolders":["/projects/app"]}"#,
        )
        .unwrap();

        let entries = scan_lockfile_dir(&dir).unwrap();
        let chosen = pick_best(entries, Some(Path::new("/projects/app"))).unwrap();
        // 2000 is newer
        assert_eq!(chosen.port, 2000);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn unrelated_lockfile_returns_none() {
        let home = temp_dir("unrelated");
        let dir = lockfile_dir(&home);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("3000.lock"),
            r#"{"transport":"ws","workspaceFolders":["F:/obsidian-changqiu"]}"#,
        )
        .unwrap();

        let entries = scan_lockfile_dir(&dir).unwrap();
        let chosen = pick_best(entries, Some(Path::new("E:\\GitHub\\DeepSeek-TUI")));
        assert!(chosen.is_none());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn path_boundary_respected() {
        let home = temp_dir("boundary");
        let dir = lockfile_dir(&home);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("1000.lock"),
            r#"{"transport":"ws","workspaceFolders":["E:\\projects\\app"]}"#,
        )
        .unwrap();

        let entries = scan_lockfile_dir(&dir).unwrap();
        // "E:\projects\application" shares prefix but is a different dir
        let chosen = pick_best(entries, Some(Path::new("E:\\projects\\application")));
        assert!(chosen.is_none());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn no_cwd_returns_none() {
        let home = temp_dir("no-cwd");
        let dir = lockfile_dir(&home);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("1000.lock"), r#"{"transport":"ws"}"#).unwrap();

        let entries = scan_lockfile_dir(&dir).unwrap();
        assert!(pick_best(entries, None).is_none());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn malformed_lockfiles_are_skipped() {
        let home = temp_dir("malformed");
        let dir = lockfile_dir(&home);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("1234.lock"), "not json").unwrap();
        fs::write(
            dir.join("2345.lock"),
            r#"{"authToken":"ok","ideName":"ok","transport":"ws","workspaceFolders":["/x"]}"#,
        )
        .unwrap();

        let entries = scan_lockfile_dir(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].port, 2345);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn ws_url_format() {
        let target = BridgeTarget {
            port: 5000,
            host: DEFAULT_BRIDGE_HOST.to_string(),
            auth: BridgeAuth::Inherited,
            source: DiscoverySource::Lockfile,
            workspace_folders: Vec::new(),
            lockfile: None,
            ide_name: None,
        };
        assert_eq!(target.ws_url(), "ws://127.0.0.1:5000/");
    }
}
