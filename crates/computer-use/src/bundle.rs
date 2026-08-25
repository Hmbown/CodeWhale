//! The plugin bundle, embedded so `codewhale computer-use setup` can install
//! it without a network fetch. `crates/computer-use/bundle/` is the source
//! of truth; the TUI plugin tests validate that directory as a real bundle.

use std::path::{Path, PathBuf};

pub const BUNDLE_NAME: &str = "computer-use";

/// `(relative path, contents)` for every file in the bundle.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.json", include_str!("../bundle/plugin.json")),
    ("mcp.json", include_str!("../bundle/mcp.json")),
    ("README.md", include_str!("../bundle/README.md")),
    (
        "skills/computer-use/SKILL.md",
        include_str!("../bundle/skills/computer-use/SKILL.md"),
    ),
    (
        "agents/computer-operator.toml",
        include_str!("../bundle/agents/computer-operator.toml"),
    ),
    (
        "commands/computer.md",
        include_str!("../bundle/commands/computer.md"),
    ),
];

/// Bundle version as declared in `plugin.json`.
pub fn version() -> String {
    serde_json::from_str::<serde_json::Value>(FILES[0].1)
        .ok()
        .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "0.0.0".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Nothing existed; all files written.
    Installed,
    /// Every file already matched.
    UpToDate,
    /// Files differed and were replaced (`force` or a marker we own).
    Updated,
}

/// The user plugins root Codewhale scans: `$CODEWHALE_HOME/plugins` or
/// `~/.codewhale/plugins`.
pub fn user_plugins_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CODEWHALE_HOME")
        && !home.trim().is_empty()
    {
        return Some(PathBuf::from(home.trim()).join("plugins"));
    }
    crate::config::home_dir().map(|h| h.join(".codewhale").join("plugins"))
}

/// Compare the on-disk bundle with the embedded files.
pub fn differs(dir: &Path) -> bool {
    FILES.iter().any(|(rel, contents)| {
        std::fs::read_to_string(dir.join(rel))
            .map(|on_disk| on_disk != *contents)
            .unwrap_or(true)
    })
}

/// Write the bundle to `dir/`, plus `marker` (the provenance file the
/// Codewhale installer expects) when given. Refuses to touch a directory
/// that exists without that marker unless `force` is set.
pub fn write(
    dir: &Path,
    marker: Option<(&str, &str)>,
    force: bool,
) -> Result<WriteOutcome, String> {
    let exists = dir.exists();
    if exists && !differs(dir) && marker.is_none_or(|(name, _)| dir.join(name).is_file()) {
        return Ok(WriteOutcome::UpToDate);
    }
    if exists
        && !force
        && let Some((name, _)) = marker
        && !dir.join(name).is_file()
    {
        return Err(format!(
            "{} exists but was not installed by Codewhale (no {name} marker); remove it or rerun with --force",
            dir.display()
        ));
    }
    for (rel, contents) in FILES {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, contents)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    }
    if let Some((name, contents)) = marker {
        std::fs::write(dir.join(name), contents)
            .map_err(|e| format!("failed to write {name}: {e}"))?;
    }
    Ok(if exists {
        WriteOutcome::Updated
    } else {
        WriteOutcome::Installed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_bundle_matches_source_tree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundle");
        for (rel, contents) in FILES {
            let on_disk = std::fs::read_to_string(root.join(rel)).unwrap();
            assert_eq!(&on_disk, contents, "{rel} drifted from the embedded copy");
        }
        assert!(version().starts_with(|c: char| c.is_ascii_digit()));
    }

    #[test]
    fn write_install_update_and_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("computer-use");
        let marker = Some((".installed-from", "{\"source\":\"test\"}"));
        assert_eq!(write(&dir, marker, false).unwrap(), WriteOutcome::Installed);
        assert!(dir.join(".installed-from").is_file());
        assert_eq!(write(&dir, marker, false).unwrap(), WriteOutcome::UpToDate);
        std::fs::write(dir.join("mcp.json"), "{}").unwrap();
        assert_eq!(write(&dir, marker, false).unwrap(), WriteOutcome::Updated);
        // A hand-placed bundle without the marker is left alone.
        std::fs::remove_file(dir.join(".installed-from")).unwrap();
        std::fs::write(dir.join("mcp.json"), "{}").unwrap();
        assert!(write(&dir, marker, false).is_err());
        assert_eq!(write(&dir, marker, true).unwrap(), WriteOutcome::Updated);
    }
}
