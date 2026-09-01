use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use super::agent_plugin::resolve_manifest_path;
use super::manifest::{PluginManifest, ValidatedManifest};
use super::path_identity::metadata_is_link_or_reparse;
use super::registry::PluginRegistry;
use super::types::{
    LoadedPlugin, PluginDiagnostic, PluginId, PluginOrigin, PluginScope, PluginSkillSnapshot,
    PluginTrustStatus,
};

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub workspace: PathBuf,
    pub user_plugins_dir: PathBuf,
    pub workspace_plugins_dir: PathBuf,
    pub builtin_plugin_dirs: Vec<PathBuf>,
    pub state_path: PathBuf,
}

/// Cheap on-disk catalog fingerprint: plugin-bundle directories only.
///
/// Used to nudge `/plugin reload` when a bundle appears, disappears, or is
/// rewritten without consulting process environment or auto-applying trust.
/// `state.json` writes are ignored so enable/trust does not look like a
/// catalog change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginCatalogStamp {
    entries: BTreeMap<PathBuf, Option<SystemTime>>,
}

impl PluginCatalogStamp {
    #[must_use]
    pub fn capture(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut entries = BTreeMap::new();
        for root in roots {
            let Ok(read) = fs::read_dir(&root) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                let Ok(metadata) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
                    continue;
                }
                entries.insert(path, metadata.modified().ok());
            }
        }
        Self { entries }
    }
}

#[must_use]
pub fn default_user_plugins_dir() -> PathBuf {
    codewhale_config::codewhale_home()
        .map(|path| path.join("plugins"))
        .unwrap_or_else(|error| {
            // Never fall back to a shared, predictable temporary directory:
            // that would turn a home-resolution failure into ambient plugin
            // discovery. A fresh nonexistent sentinel keeps startup read-only
            // and fail-closed on every supported platform.
            tracing::warn!(
                target: "plugins",
                %error,
                "Codewhale home could not be resolved; user plugin discovery is disabled"
            );
            std::env::temp_dir()
                .join(format!(
                    ".codewhale-home-unavailable-{}",
                    uuid::Uuid::new_v4().simple()
                ))
                .join("plugins")
        })
}

#[must_use]
pub fn default_workspace_plugins_dir(workspace: &Path) -> PathBuf {
    workspace.join(".codewhale").join("plugins")
}

#[cfg(test)]
#[must_use]
pub fn discover_with_config(config: &DiscoveryConfig) -> PluginRegistry {
    let context = super::context::PluginDiscoveryContext::from_config_and_environment(
        config,
        super::context::HostEnvironment::capture(),
    );
    discover_with_context(config, context)
}

#[must_use]
pub(crate) fn discover_with_context(
    config: &DiscoveryConfig,
    context: std::sync::Arc<super::context::PluginDiscoveryContext>,
) -> PluginRegistry {
    let mut diagnostics = Vec::new();
    let mut candidates = Vec::new();

    for root in &config.builtin_plugin_dirs {
        scan_root(
            root,
            PluginScope::Builtin,
            PluginOrigin::Builtin,
            &mut candidates,
            &mut diagnostics,
        );
    }
    scan_root(
        &config.user_plugins_dir,
        PluginScope::User,
        PluginOrigin::CodeWhaleHome,
        &mut candidates,
        &mut diagnostics,
    );
    scan_root(
        &config.workspace_plugins_dir,
        PluginScope::Workspace,
        PluginOrigin::Workspace,
        &mut candidates,
        &mut diagnostics,
    );

    candidates.sort_by(|left, right| {
        left.scope
            .cmp(&right.scope)
            .then_with(|| left.name().cmp(right.name()))
            .then_with(|| left.canonical_root.cmp(&right.canonical_root))
    });

    let mut seen_roots = HashSet::new();
    let mut seen_names = BTreeSet::new();
    let mut plugins = Vec::new();
    for plugin in candidates {
        if !seen_roots.insert(plugin.canonical_root.clone()) {
            diagnostics.push(PluginDiagnostic::warning(
                "duplicate-root",
                format!(
                    "Ignoring duplicate plugin discovery at {}",
                    plugin.canonical_root.display()
                ),
                Some(plugin.canonical_root.clone()),
            ));
            continue;
        }
        if !seen_names.insert(plugin.name().to_string()) {
            diagnostics.push(PluginDiagnostic::warning(
                "name-conflict",
                format!(
                    "Plugin `{}` at {} is shadowed by the higher-precedence bundle with the same name",
                    plugin.name(),
                    plugin.canonical_root.display()
                ),
                Some(plugin.canonical_root.clone()),
            ));
            continue;
        }
        plugins.push(plugin);
    }

    PluginRegistry::from_discovery(
        plugins,
        diagnostics,
        config.state_path.clone(),
        config.workspace.clone(),
        Some(context),
    )
}

fn scan_root(
    root: &Path,
    scope: PluginScope,
    origin: PluginOrigin,
    plugins: &mut Vec<LoadedPlugin>,
    diagnostics: &mut Vec<PluginDiagnostic>,
) {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return;
    };
    if metadata_is_link_or_reparse(&metadata) {
        diagnostics.push(PluginDiagnostic::error(
            "root-symlink",
            format!(
                "Plugin discovery root may not be a symbolic link or reparse point: {}",
                root.display()
            ),
            Some(root.to_path_buf()),
        ));
        return;
    }
    if !metadata.is_dir() {
        diagnostics.push(PluginDiagnostic::error(
            "root-not-directory",
            format!(
                "Plugin discovery root is not a directory: {}",
                root.display()
            ),
            Some(root.to_path_buf()),
        ));
        return;
    }
    let canonical_discovery_root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            diagnostics.push(PluginDiagnostic::error(
                "root-canonicalize-failed",
                format!(
                    "Failed to canonicalize plugin root {}: {error}",
                    root.display()
                ),
                Some(root.to_path_buf()),
            ));
            return;
        }
    };

    let mut entries = match fs::read_dir(root) {
        Ok(entries) => match entries.collect::<Result<Vec<_>, _>>() {
            Ok(entries) => entries,
            Err(error) => {
                diagnostics.push(PluginDiagnostic::error(
                    "root-read-failed",
                    format!("Failed to read plugin root {}: {error}", root.display()),
                    Some(root.to_path_buf()),
                ));
                return;
            }
        },
        Err(error) => {
            diagnostics.push(PluginDiagnostic::error(
                "root-read-failed",
                format!("Failed to read plugin root {}: {error}", root.display()),
                Some(root.to_path_buf()),
            ));
            return;
        }
    };
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let plugin_root = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&plugin_root) else {
            continue;
        };
        if metadata_is_link_or_reparse(&metadata) {
            diagnostics.push(PluginDiagnostic::error(
                "bundle-symlink",
                format!(
                    "Plugin bundle directory may not be a symbolic link or reparse point: {}",
                    plugin_root.display()
                ),
                Some(plugin_root),
            ));
            continue;
        }
        if !metadata.is_dir() {
            continue;
        }
        // Agent Plugins v1.0.0 `plugin.json` is preferred; a legacy
        // `plugin.toml` in the same bundle stays readable.
        let Some(manifest_path) = resolve_manifest_path(&plugin_root) else {
            continue;
        };
        match load_plugin(&manifest_path, &canonical_discovery_root, scope, origin) {
            Ok(plugin) => plugins.push(plugin),
            Err(error) => diagnostics.push(PluginDiagnostic::error(
                "manifest-invalid",
                error,
                Some(manifest_path),
            )),
        }
    }
}

fn load_plugin(
    manifest_path: &Path,
    canonical_discovery_root: &Path,
    scope: PluginScope,
    origin: PluginOrigin,
) -> Result<LoadedPlugin, String> {
    let validated = PluginManifest::validate_from_path(manifest_path)?;
    if validated.canonical_root.parent() != Some(canonical_discovery_root) {
        return Err(format!(
            "plugin bundle resolved outside its Codewhale-owned discovery root: {}",
            validated.canonical_root.display()
        ));
    }
    let id = plugin_id(
        scope,
        &validated.manifest.plugin.name,
        &validated.canonical_root,
    );
    let mut diagnostics = validated
        .warnings
        .iter()
        .map(|warning| {
            PluginDiagnostic::warning(
                "manifest-legacy",
                warning.clone(),
                Some(manifest_path.to_path_buf()),
            )
        })
        .collect::<Vec<_>>();

    let (skill_snapshots, skill_diagnostics) = parse_skill_snapshots(&validated)?;
    diagnostics.extend(skill_diagnostics);

    let unsupported = validated.inventory.unsupported_labels();
    if !unsupported.is_empty() {
        diagnostics.push(PluginDiagnostic::warning(
            "component-inactive",
            format!(
                "compatibility {}; inactive components remain inventoried but are not activated: {}",
                validated.inventory.compatibility().as_str(),
                unsupported.join(", ")
            ),
            Some(manifest_path.to_path_buf()),
        ));
    }

    // Skill parsing happens after hashing. Revalidate once so a concurrent
    // bundle edit cannot pair a reviewed hash with different in-memory Skill
    // instructions or MCP configuration. Active Skill bodies are replaced by
    // snapshots parsed from the Codewhale-owned staged tree in `apply_state`.
    let refreshed = PluginManifest::validate_from_path(manifest_path)?;
    if refreshed.content_hash != validated.content_hash
        || refreshed.capability_hash != validated.capability_hash
    {
        return Err(format!(
            "plugin `{}` changed during discovery; reload and review the stable bundle",
            validated.manifest.plugin.name
        ));
    }
    let validated = refreshed;

    Ok(LoadedPlugin {
        id,
        manifest: validated.manifest,
        base_path: validated.canonical_root.clone(),
        canonical_root: validated.canonical_root,
        staged_root: None,
        scope,
        origin,
        enabled: false,
        trust_status: PluginTrustStatus::NeverReviewed,
        applicable: validated.applicable,
        inventory: validated.inventory,
        components: validated.components,
        content_hash: validated.content_hash,
        capability_hash: validated.capability_hash,
        state_generation: 0,
        skill_snapshots,
        diagnostics,
    })
}

fn parse_skill_snapshots(
    validated: &ValidatedManifest,
) -> Result<(Vec<PluginSkillSnapshot>, Vec<PluginDiagnostic>), String> {
    let mut diagnostics = Vec::new();
    let mut skill_snapshots = Vec::new();
    for skills_dir in &validated.components.skills {
        let registry = crate::skills::SkillRegistry::discover(skills_dir);
        for warning in registry.warnings() {
            diagnostics.push(PluginDiagnostic::warning(
                "skill-invalid",
                warning.clone(),
                Some(skills_dir.clone()),
            ));
        }
        for skill in registry.list() {
            let relative = skill
                .path
                .strip_prefix(&validated.canonical_root)
                .map_err(|_| {
                    format!(
                        "plugin `{}` Skill path escaped the reviewed bundle",
                        validated.manifest.plugin.name
                    )
                })?;
            let expected_hash = validated.file_hashes.get(relative).ok_or_else(|| {
                format!(
                    "plugin `{}` Skill was not present in the reviewed byte inventory",
                    validated.manifest.plugin.name
                )
            })?;
            let bytes = read_skill_bytes(&skill.path)?;
            let actual_hash = hash_skill_bytes(&bytes);
            if &actual_hash != expected_hash {
                return Err(format!(
                    "plugin `{}` Skill changed between review hashing and parsing",
                    validated.manifest.plugin.name
                ));
            }
            let content = std::str::from_utf8(&bytes)
                .map_err(|_| "plugin Skill must be valid UTF-8".to_string())?;
            let (skill, parse_warnings) =
                crate::skills::SkillRegistry::parse_verified_content(&skill.path, content)?;
            for warning in parse_warnings {
                diagnostics.push(PluginDiagnostic::warning(
                    "skill-invalid",
                    warning,
                    Some(skill.path.clone()),
                ));
            }
            skill_snapshots.push(PluginSkillSnapshot {
                name: skill.name.clone(),
                description: skill.description.clone(),
                localized_descriptions: skill.localized_descriptions.clone(),
                invocation: skill.invocation,
                aliases: skill.aliases.clone(),
                body: skill.body.clone(),
                path: skill.path.clone(),
                source_hash: actual_hash,
            });
        }
    }
    skill_snapshots.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut seen_skills = BTreeSet::new();
    for skill in &skill_snapshots {
        if !seen_skills.insert(skill.name.clone()) {
            return Err(format!(
                "plugin `{}` declares duplicate skill name `{}`",
                validated.manifest.plugin.name, skill.name
            ));
        }
    }

    Ok((skill_snapshots, diagnostics))
}

fn read_skill_bytes(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read as _;

    let file = super::manifest::open_bundle_file(path)
        .map_err(|e| format!("failed to open plugin Skill without following links: {e}"))?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed to read plugin Skill: {e}"))?;
    if bytes.len() > 1024 * 1024 {
        return Err("plugin Skill exceeds the one-megabyte parse limit".to_string());
    }
    Ok(bytes)
}

fn hash_skill_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codewhale-plugin-file-bytes-v1\0");
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn load_staged_skill_snapshots(
    staged_root: &Path,
    expected_content_hash: &str,
    expected_capability_hash: &str,
) -> Result<Vec<PluginSkillSnapshot>, String> {
    let staged_manifest = resolve_manifest_path(staged_root)
        .ok_or_else(|| "staged plugin has no plugin.json or plugin.toml".to_string())?;
    let validated = PluginManifest::validate_from_path(&staged_manifest)?;
    if validated.canonical_root != staged_root
        || validated.content_hash != expected_content_hash
        || validated.capability_hash != expected_capability_hash
    {
        return Err("staged plugin Skill snapshot no longer matches reviewed content".to_string());
    }
    let (snapshots, diagnostics) = parse_skill_snapshots(&validated)?;
    if let Some(diagnostic) = diagnostics.first() {
        return Err(format!(
            "staged plugin Skill snapshot is invalid: {}",
            diagnostic.message
        ));
    }
    // Parsing is explicitly bound to the first validation's file inventory;
    // a final whole-bundle pass also rejects additions/removals/config drift
    // that occurred during directory traversal.
    let refreshed = PluginManifest::validate_from_path(&staged_manifest)?;
    if refreshed.content_hash != validated.content_hash
        || refreshed.capability_hash != validated.capability_hash
        || refreshed.file_hashes != validated.file_hashes
    {
        return Err("staged plugin changed while its Skill snapshots were parsed".to_string());
    }
    Ok(snapshots)
}

fn plugin_id(scope: PluginScope, name: &str, canonical_root: &Path) -> PluginId {
    let mut hasher = Sha256::new();
    // v2 intentionally invalidates receipts produced by the former lossy
    // Unicode path identity.
    hasher.update(b"codewhale-plugin-id-v2\0");
    hasher.update(scope.as_str().as_bytes());
    hasher.update(b"\0");
    super::path_identity::hash_os_path(&mut hasher, b"canonical-plugin-root", canonical_root);
    let digest = hasher.finalize();
    let suffix = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    PluginId(format!("{}/{suffix}/{name}", scope.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(root: &Path, dir: &str, name: &str) -> PathBuf {
        let plugin = root.join(dir);
        fs::create_dir_all(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            format!("schema_version = 1\n[plugin]\nname = {name:?}\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        plugin
    }

    fn config(tmp: &Path) -> DiscoveryConfig {
        DiscoveryConfig {
            workspace: tmp.join("project"),
            user_plugins_dir: tmp.join("user"),
            workspace_plugins_dir: tmp.join("workspace"),
            builtin_plugin_dirs: vec![tmp.join("builtin")],
            state_path: tmp.join("state.json"),
        }
    }

    #[test]
    fn user_and_workspace_bundles_are_disabled_and_untrusted_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(tmp.path());
        write_plugin(&cfg.user_plugins_dir, "a", "user-plugin");
        write_plugin(&cfg.workspace_plugins_dir, "b", "workspace-plugin");

        let registry = discover_with_config(&cfg);
        assert_eq!(registry.len(), 2);
        assert!(registry.list().iter().all(|plugin| !plugin.enabled));
        assert!(registry.list().iter().all(|plugin| !plugin.trusted()));
        assert!(!cfg.state_path.exists(), "discovery must be read-only");
    }

    #[test]
    fn precedence_is_builtin_then_user_then_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(tmp.path());
        write_plugin(&cfg.builtin_plugin_dirs[0], "z", "same");
        write_plugin(&cfg.user_plugins_dir, "a", "same");
        write_plugin(&cfg.workspace_plugins_dir, "b", "same");

        let registry = discover_with_config(&cfg);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get("same").unwrap().scope, PluginScope::Builtin);
        assert_eq!(
            registry
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == "name-conflict")
                .count(),
            2
        );
    }

    #[test]
    fn discovery_is_sorted_and_plugin_ids_are_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(tmp.path());
        write_plugin(&cfg.user_plugins_dir, "z", "zulu");
        write_plugin(&cfg.user_plugins_dir, "a", "alpha");

        let first = discover_with_config(&cfg);
        let second = discover_with_config(&cfg);
        let names = first
            .list()
            .iter()
            .map(|plugin| plugin.name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["alpha", "zulu"]);
        assert_eq!(
            first.get("alpha").unwrap().id,
            second.get("alpha").unwrap().id
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_ids_distinguish_lossy_colliding_native_roots() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let tmp = tempfile::tempdir().unwrap();
        let left_name = OsString::from_vec(vec![b'p', 0xff]);
        let right_name = OsString::from_vec(vec![b'p', 0xfe]);
        assert_eq!(left_name.to_string_lossy(), right_name.to_string_lossy());
        let left = tmp.path().join(left_name);
        let right = tmp.path().join(right_name);
        assert_ne!(
            plugin_id(PluginScope::User, "same", &left),
            plugin_id(PluginScope::User, "same", &right)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_discovery_root_fails_closed() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_plugin(outside.path(), "a", "outside");
        let cfg = config(tmp.path());
        symlink(outside.path(), &cfg.workspace_plugins_dir).unwrap();

        let registry = discover_with_config(&cfg);
        assert!(registry.is_empty());
        assert!(
            registry
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "root-symlink")
        );
    }

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("invoke Windows junction creation");
        assert!(
            output.status.success(),
            "failed to create junction: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn junction_discovery_root_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_plugin(outside.path(), "a", "outside");
        let cfg = config(tmp.path());
        create_junction(&cfg.workspace_plugins_dir, outside.path());

        let registry = discover_with_config(&cfg);
        assert!(registry.is_empty());
        assert!(
            registry
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "root-symlink")
        );
    }

    #[cfg(windows)]
    #[test]
    fn junction_bundle_entry_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = write_plugin(outside.path(), "actual", "outside");
        let cfg = config(tmp.path());
        fs::create_dir_all(&cfg.workspace_plugins_dir).unwrap();
        create_junction(&cfg.workspace_plugins_dir.join("linked"), &target);

        let registry = discover_with_config(&cfg);
        assert!(registry.is_empty());
        assert!(
            registry
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "bundle-symlink")
        );
    }

    #[test]
    fn discovery_ignores_ambient_compatibility_roots() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let workspace = tmp.path().join("workspace");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        write_plugin(
            &workspace.join(".claude/plugins"),
            "ambient",
            "ambient-plugin",
        );
        write_plugin(
            &workspace.join(".cursor/plugins"),
            "ambient",
            "cursor-plugin",
        );

        let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
        let registry = discovery.registry_for_workspace(&workspace);
        assert!(registry.is_empty());
        assert!(!home.join("plugins/state.json").exists());
    }
}
