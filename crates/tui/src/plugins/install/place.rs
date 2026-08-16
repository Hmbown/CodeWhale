//! Place a staged bundle at its final path, and the containment guards.
//!
//! [`finalize_install`] is the only writer of the destination directory: it
//! renames the staged tree into place atomically, restores the backup on any
//! failure, and writes `.installed-from` **last** so a partial install never
//! leaves a stale marker. [`ensure_target_within_plugins_dir`] mirrors
//! discovery's fail-closed rule that a bundle must resolve to a direct child
//! of the plugins root.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::skills::install::{self as skill_install, validate_skill_name_segment};

use super::stage::StagedPlugin;
use super::{INSTALLED_FROM_MARKER, InstalledPlugin, PluginInstallError, PluginInstallOutcome};

pub(super) fn finalize_install(
    staged: StagedPlugin,
    spec: &str,
    url: Option<&str>,
    source_checksum: &str,
    user_plugins_dir: &Path,
    update: bool,
) -> Result<PluginInstallOutcome> {
    let final_path = user_plugins_dir.join(&staged.name);
    let mut backup_path: Option<PathBuf> = None;
    if final_path.exists() {
        if !update {
            let has_marker = final_path.join(INSTALLED_FROM_MARKER).exists();
            let _ = fs::remove_dir_all(&staged.staged_path);
            if has_marker {
                return Err(PluginInstallError::AlreadyInstalled(staged.name).into());
            }
            return Err(PluginInstallError::NotInstalledHere(staged.name).into());
        }
        if !final_path.join(INSTALLED_FROM_MARKER).exists() {
            let _ = fs::remove_dir_all(&staged.staged_path);
            return Err(PluginInstallError::NotInstalledHere(staged.name).into());
        }
        let backup = user_plugins_dir.join(format!("{}.bak", staged.name));
        if backup.exists() {
            fs::remove_dir_all(&backup).ok();
        }
        fs::rename(&final_path, &backup).with_context(|| {
            format!(
                "failed to backup existing plugin at {}",
                final_path.display()
            )
        })?;
        if let Err(error) = fs::rename(&staged.staged_path, &final_path) {
            fs::rename(&backup, &final_path).ok();
            return Err(error).context("failed to install staged plugin");
        }
        backup_path = Some(backup);
    } else if let Err(error) = fs::rename(&staged.staged_path, &final_path) {
        let _ = fs::remove_dir_all(&staged.staged_path);
        return Err(error).context("failed to install staged plugin");
    }

    // Discovery fail-closed rule: the installed bundle must canonicalize to a
    // direct child of the user plugins root.
    if let Err(error) = ensure_target_within_plugins_dir(&final_path, user_plugins_dir) {
        let _ = fs::remove_dir_all(&final_path);
        if let Some(backup) = backup_path.take() {
            let _ = fs::rename(&backup, &final_path);
        }
        return Err(error);
    }

    // Write the marker last so a partial install never leaves a stale
    // `.installed-from` on disk.
    if let Err(error) = skill_install::write_installed_from_v2(
        &final_path,
        spec,
        url,
        source_checksum,
        &staged.content_hash,
        &staged.name,
    ) {
        let _ = fs::remove_dir_all(&final_path);
        if let Some(backup) = backup_path.take() {
            let _ = fs::rename(&backup, &final_path);
        }
        return Err(error);
    }
    let installed_content_hash =
        match crate::plugins::agent_plugin::resolve_manifest_path(&final_path)
            .ok_or_else(|| anyhow::anyhow!("installed plugin has no supported manifest"))
            .and_then(|manifest_path| {
                crate::plugins::manifest::PluginManifest::validate_from_path(&manifest_path)
                    .map(|validated| validated.content_hash)
                    .map_err(anyhow::Error::msg)
            }) {
            Ok(hash) => hash,
            Err(error) => {
                let _ = fs::remove_dir_all(&final_path);
                if let Some(backup) = backup_path.take() {
                    let _ = fs::rename(&backup, &final_path);
                }
                return Err(error).context("installed plugin failed post-copy validation");
            }
        };
    if let Some(backup) = backup_path {
        fs::remove_dir_all(&backup).ok();
    }

    Ok(PluginInstallOutcome::Installed(InstalledPlugin {
        name: staged.name,
        path: final_path,
        content_hash: staged.content_hash,
        installed_content_hash,
        source_checksum: source_checksum.to_string(),
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Path guards
// ─────────────────────────────────────────────────────────────────────────────

pub(super) fn plugin_target_path(name: &str, user_plugins_dir: &Path) -> Result<PathBuf> {
    let name = validate_skill_name_segment(name)
        .map_err(|error| anyhow::anyhow!("plugin name is not a safe directory name: {error:#}"))?;
    Ok(user_plugins_dir.join(name))
}

/// The resolved bundle must be a direct child of the resolved plugins root,
/// matching discovery's fail-closed containment rule.
pub(super) fn ensure_target_within_plugins_dir(
    target: &Path,
    user_plugins_dir: &Path,
) -> Result<()> {
    let root = fs::canonicalize(user_plugins_dir).with_context(|| {
        format!(
            "failed to resolve plugins directory {}",
            user_plugins_dir.display()
        )
    })?;
    let target = fs::canonicalize(target)
        .with_context(|| format!("failed to resolve {}", target.display()))?;
    if target.parent() != Some(root.as_path()) {
        bail!(
            "plugin path {} escapes plugins directory {}",
            target.display(),
            root.display()
        );
    }
    Ok(())
}
