//! First-party plugin bundles that ship inside the binary.
//!
//! [`super::discovery::DiscoveryConfig::builtin_plugin_dirs`] and
//! [`super::types::PluginScope::Builtin`] have existed since plugin discovery
//! landed, with no producer: every construction site passed an empty list, so
//! the in-repo `crates/tui/plugins/computer-use` bundle reached nobody who had
//! not cloned the repository. This module is that producer. It is not a second install
//! path — installed bundles still arrive through
//! [`super::install`], and discovery, trust, and enablement are unchanged.
//!
//! The bundle is embedded with `include_str!` (the same way locale packs and
//! the mobile client are embedded) and written under
//! `$CODEWHALE_HOME/builtin-plugins` on first run, so one binary carries it to
//! every distribution channel — npm, tarball, `cargo install`, brew — without
//! any of them learning about plugin files.
//!
//! Two properties this must not lose:
//!
//! * **Materializing is not enabling.** A freshly written builtin bundle is
//!   `NeverReviewed` and disabled like any other, because
//!   [`super::registry`] enables only what the user's `state.json` says.
//!   Computer use can drive the desktop; it waits to be reviewed.
//! * **A partial tree is never discoverable.** The bundle is staged in a
//!   sibling directory and swapped into place, and the stamp that marks it
//!   current is written last.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::path_identity::metadata_is_link_or_reparse;

/// Directory under the Codewhale home that this module owns entirely.
/// Deliberately *not* inside `plugins/`: that root is scanned as
/// [`super::types::PluginScope::User`], and a bundle found twice is a
/// duplicate-root diagnostic rather than a plugin.
const BUILTIN_DIR_NAME: &str = "builtin-plugins";

/// File recording the digest of the bundle currently on disk.
const STAMP_NAME: &str = ".stamp";

const COMPUTER_USE: &str = "computer-use";

macro_rules! bundle_file {
    ($relative:literal) => {
        (
            $relative,
            include_str!(concat!("../../plugins/computer-use/", $relative)),
        )
    };
}

/// The runtime tree of `crates/tui/plugins/computer-use`, relative path → contents.
///
/// Development-only files (`tests/`, `scripts/smoke.mjs`, `package.json`,
/// `README.md`) are deliberately absent: nothing at runtime reads them, and
/// the `.mjs` extension already makes every module ESM without a
/// `"type": "module"` declaration.
const COMPUTER_USE_FILES: &[(&str, &str)] = &[
    bundle_file!("plugin.json"),
    bundle_file!("mcp.json"),
    bundle_file!("commands/computer.md"),
    bundle_file!("skills/computer-use/SKILL.md"),
    bundle_file!("skills/recording/SKILL.md"),
    bundle_file!("agent.mjs"),
    bundle_file!("mcp/server.mjs"),
    bundle_file!("src/exec.mjs"),
    bundle_file!("src/registry.mjs"),
    bundle_file!("src/remote-runtime.mjs"),
    bundle_file!("src/tools.mjs"),
    bundle_file!("src/transport.mjs"),
    bundle_file!("src/backends/darwin.mjs"),
    bundle_file!("src/backends/harmonyos.mjs"),
    bundle_file!("src/backends/linux.mjs"),
    bundle_file!("src/backends/win32.mjs"),
];

/// Digest of one bundle's entire contents, including its file names, so a
/// renamed or removed file is as much a change as an edited one.
fn digest(files: &[(&str, &str)]) -> String {
    let mut hasher = Sha256::new();
    for (relative, contents) in files {
        hasher.update((relative.len() as u64).to_le_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((contents.len() as u64).to_le_bytes());
        hasher.update(contents.as_bytes());
    }
    super::manifest::hex_digest(hasher.finalize())
}

/// Discovery roots holding the built-in bundles, writing them out if what is
/// on disk is not already exactly this build's copy. An empty list is the
/// honest answer when the bundle cannot be written: discovery then finds no
/// built-in plugin, rather than a broken one.
///
/// Deliberately not memoized. The result is derived from `$CODEWHALE_HOME`,
/// and caching a home-derived path process-wide would pin whichever caller ran
/// first — which is wrong the moment the home differs between callers, as it
/// does across tests in one process. The steady-state cost is reading one
/// stamp file.
#[must_use]
pub fn materialized_dirs() -> Vec<PathBuf> {
    match materialize() {
        Ok(Some(root)) => vec![root],
        Ok(None) => Vec::new(),
        Err(error) => {
            tracing::warn!(
                target: "plugins",
                %error,
                "built-in plugin bundles could not be written; they will not be discovered"
            );
            Vec::new()
        }
    }
}

/// `Ok(None)` when there is no Codewhale home to write into yet.
///
/// Startup runs this for *every* command, `doctor` and `setup status`
/// included, and those are contractually read-only: they must not bring a
/// home directory into existence as a side effect of inventorying plugins
/// (`crates/tui/tests/integration/diagnostic_read_only.rs`). Materializing
/// into an existing home only keeps that promise, and costs nothing in
/// practice — the home exists from the moment Codewhale is configured or run.
fn materialize() -> io::Result<Option<PathBuf>> {
    let home = codewhale_config::codewhale_home().map_err(io::Error::other)?;
    if !home.is_dir() {
        return Ok(None);
    }
    let root = home.join(BUILTIN_DIR_NAME);
    reject_symlink(&root)?;
    fs::create_dir_all(&root)?;
    write_bundle(&root, COMPUTER_USE, COMPUTER_USE_FILES)?;
    Ok(Some(root))
}

/// Write one bundle into `root/<name>` when what is there is not already
/// exactly this bundle. Staged then swapped, so `root/<name>` is either the
/// previous bundle or this one — never half of either.
fn write_bundle(root: &Path, name: &str, files: &[(&str, &str)]) -> io::Result<()> {
    let destination = root.join(name);
    let stamp_path = destination.join(STAMP_NAME);
    let want = digest(files);
    if fs::read_to_string(&stamp_path).is_ok_and(|found| found.trim() == want) {
        return Ok(());
    }
    reject_symlink(&destination)?;

    let staging = root.join(format!(".staging-{name}"));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    for (relative, contents) in files {
        let path = staging.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)?;
    }
    // Last, so an interrupted write leaves a bundle that fails the stamp check
    // and is rewritten on the next run.
    fs::write(staging.join(STAMP_NAME), &want)?;

    if destination.exists() {
        fs::remove_dir_all(&destination)?;
    }
    fs::rename(&staging, &destination)?;
    Ok(())
}

/// Refuse to write through a symbolic link or reparse point, the same rule
/// [`super::discovery`] applies when it scans a plugin root.
fn reject_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata_is_link_or_reparse(&metadata) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "built-in plugin path may not be a symbolic link or reparse point: {}",
                path.display()
            ),
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::plugins::types::{PluginScope, PluginTrustStatus};

    #[test]
    fn computer_use_is_discovered_but_never_auto_enabled() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&home).unwrap();
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);

        let registry = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv()
            .registry_for_workspace(&workspace);
        let plugin = registry
            .get(COMPUTER_USE)
            .expect("the built-in computer-use bundle must be discovered");

        assert_eq!(plugin.scope, PluginScope::Builtin);
        // Computer use can drive the desktop. Shipping it is not consenting to
        // it: the user reviews and enables it like any other bundle.
        assert!(!plugin.enabled);
        assert_eq!(plugin.trust_status, PluginTrustStatus::NeverReviewed);
        assert!(!home.join("plugins/state.json").exists(), "read-only");
    }

    #[test]
    fn a_stale_or_tampered_bundle_is_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write_bundle(&root, COMPUTER_USE, COMPUTER_USE_FILES).unwrap();

        let server = root.join(COMPUTER_USE).join("mcp/server.mjs");
        let original = fs::read_to_string(&server).unwrap();
        fs::write(&server, "throw new Error('tampered')").unwrap();
        // An edit alone is not noticed — the stamp is the contract, and
        // rewriting on every start would fight a user debugging in place.
        write_bundle(&root, COMPUTER_USE, COMPUTER_USE_FILES).unwrap();
        assert_eq!(
            fs::read_to_string(&server).unwrap(),
            "throw new Error('tampered')"
        );

        // A stamp that does not match this build's bundle does force a rewrite,
        // which is what an upgraded binary sees.
        fs::write(root.join(COMPUTER_USE).join(STAMP_NAME), "stale").unwrap();
        write_bundle(&root, COMPUTER_USE, COMPUTER_USE_FILES).unwrap();
        assert_eq!(fs::read_to_string(&server).unwrap(), original);
        assert_eq!(
            fs::read_to_string(root.join(COMPUTER_USE).join(STAMP_NAME)).unwrap(),
            digest(COMPUTER_USE_FILES)
        );
    }

    #[test]
    fn a_home_that_does_not_exist_yet_is_never_created() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("absent-home");
        let _guard = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);

        // Read-only diagnostics run this on every startup; conjuring the home
        // here would break their contract.
        assert!(materialized_dirs().is_empty());
        assert!(!home.exists());
    }
}
