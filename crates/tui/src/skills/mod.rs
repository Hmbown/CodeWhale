//! Skill discovery and registry for local SKILL.md files.

pub mod audit;
/// Provider-free contract tests for the bundled starter pack (#4698).
#[cfg(test)]
mod catalog_matrix;
pub mod install;
pub mod mutation;
mod package_digest;
pub mod roots;
mod system;
// Re-exports kept for documentation parity and downstream consumers; the
// binary itself imports directly from `skills::install`. `#[allow(...)]`
// silences the dead-code warning that fires because no `bin` source path
// references these names through `skills::*`.
#[allow(unused_imports)]
pub use install::{
    DEFAULT_MAX_SIZE_BYTES, DEFAULT_REGISTRY_URL, INSTALLED_FROM_MARKER, InstallOutcome,
    InstallSource, InstalledSkill, RegistryDocument, RegistryEntry, RegistryFetchResult,
    SkillSyncOutcome, SyncResult, UpdateResult, default_cache_skills_dir,
};
#[allow(unused_imports)]
pub use roots::{
    CompatibleHarness, SkillRootAccess, SkillRootCatalog, SkillRootDescriptor, SkillRootId,
    SkillRootKind, SkillScope, classify_configured_skills_dir, safe_display_path,
};
#[allow(unused_imports)]
pub use system::is_exact_bundled_skill;
pub use system::{
    BundledSkillTier, bundled_skill_tier, install_system_skills, is_bundled_skill_name,
};

use std::fs;
use std::path::{Path, PathBuf};

use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock};

use crate::logging;

const MAX_SKILL_DESCRIPTION_CHARS: usize = 280;
const MAX_AVAILABLE_SKILLS_CHARS: usize = 12_000;
const MAX_SKILL_NAME_CHARS: usize = 64;

/// Test-only observations of the synchronous skill-discovery walk.
///
/// Definitions are intentionally tied to concrete filesystem operations:
/// - `root_discovery_calls`: entries into [`SkillRegistry::discover`], including
///   roots that are missing or are not directories.
/// - `directories_visited`: unique directories accepted by cycle detection and
///   then submitted to `read_dir` by the recursive walker.
/// - `skill_md_read_attempts`: calls to `read_to_string(<child>/SKILL.md)`,
///   including expected not-found results for organizational directories.
///
/// These counters do not cache or otherwise change discovery behavior. They are
/// thread-local so unrelated parallel tests cannot contaminate a measurement.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SkillDiscoveryMetrics {
    pub(crate) root_discovery_calls: usize,
    pub(crate) directories_visited: usize,
    pub(crate) skill_md_read_attempts: usize,
}

#[cfg(test)]
impl SkillDiscoveryMetrics {
    #[must_use]
    pub(crate) fn delta_since(self, earlier: Self) -> Self {
        Self {
            root_discovery_calls: self
                .root_discovery_calls
                .saturating_sub(earlier.root_discovery_calls),
            directories_visited: self
                .directories_visited
                .saturating_sub(earlier.directories_visited),
            skill_md_read_attempts: self
                .skill_md_read_attempts
                .saturating_sub(earlier.skill_md_read_attempts),
        }
    }
}

#[cfg(test)]
thread_local! {
    static SKILL_DISCOVERY_METRICS: std::cell::Cell<SkillDiscoveryMetrics> =
        const { std::cell::Cell::new(SkillDiscoveryMetrics {
            root_discovery_calls: 0,
            directories_visited: 0,
            skill_md_read_attempts: 0,
        }) };
}

#[cfg(test)]
pub(crate) fn reset_discovery_metrics() {
    SKILL_DISCOVERY_METRICS.set(SkillDiscoveryMetrics::default());
}

#[cfg(test)]
#[must_use]
pub(crate) fn discovery_metrics_snapshot() -> SkillDiscoveryMetrics {
    SKILL_DISCOVERY_METRICS.get()
}

#[cfg(test)]
fn record_root_discovery_call() {
    SKILL_DISCOVERY_METRICS.with(|cell| {
        let mut metrics = cell.get();
        metrics.root_discovery_calls += 1;
        cell.set(metrics);
    });
}

#[cfg(test)]
fn record_directory_visit() {
    SKILL_DISCOVERY_METRICS.with(|cell| {
        let mut metrics = cell.get();
        metrics.directories_visited += 1;
        cell.set(metrics);
    });
}

#[cfg(test)]
fn record_skill_md_read_attempt() {
    SKILL_DISCOVERY_METRICS.with(|cell| {
        let mut metrics = cell.get();
        metrics.skill_md_read_attempts += 1;
        cell.set(metrics);
    });
}

// === Defaults ===

#[must_use]
pub fn default_skills_dir() -> PathBuf {
    crate::config::effective_home_dir().map_or_else(
        || PathBuf::from("/tmp/codewhale/skills"),
        |p| p.join(".codewhale").join("skills"),
    )
}

/// Global agentskills.io-compatible skills directory (`~/.agents/skills`).
#[must_use]
pub fn agents_global_skills_dir() -> Option<PathBuf> {
    crate::config::effective_home_dir().map(|p| p.join(".agents").join("skills"))
}

// === Types ===

/// Session-time skill discovery scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillDiscoveryMode {
    /// Preserve the existing broad compatibility scan across CodeWhale,
    /// agentskills.io, Claude, OpenCode, Cursor, and legacy DeepSeek roots.
    Compatible,
    /// Scan only CodeWhale-owned roots. Callers that also pass an explicit
    /// `skills_dir` still get that directory because it is user configuration.
    CodeWhaleOnly,
}

impl SkillDiscoveryMode {
    #[must_use]
    pub fn from_codewhale_only(value: bool) -> Self {
        if value {
            Self::CodeWhaleOnly
        } else {
            Self::Compatible
        }
    }
}

/// Parsed representation of a SKILL.md definition.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    /// Default (language-neutral, usually English) description.
    pub description: String,
    /// Optional locale-specific descriptions, keyed by lowercased locale tag
    /// (e.g. `zh`, `zh-hant`, `ja`). Populated from `description_<tag>:`
    /// frontmatter keys so a skill author can ship a shorter, native-language
    /// description for non-English sessions (saves prompt tokens; see #3354).
    pub localized_descriptions: HashMap<String, String>,
    /// Whether the skill may be selected from the model's catalogue or only
    /// loaded after an explicit user request. Missing metadata preserves the
    /// historical model-and-user behavior.
    pub invocation: SkillInvocation,
    /// Alternate names accepted by `load_skill`; aliases never become extra
    /// prompt entries, so they do not inflate the catalogue or create a
    /// second instruction surface.
    pub aliases: Vec<String>,
    pub body: String,
    /// On-disk path to the `SKILL.md` this was loaded from. The directory
    /// name can differ from the frontmatter `name` for community installs
    /// or manually-placed skills, so callers must use this rather than
    /// reconstructing `<dir>/<name>/SKILL.md`.
    pub path: PathBuf,
    pub source: SkillSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillInvocation {
    ModelAndUser,
    ExplicitOnly,
}

impl SkillInvocation {
    fn from_frontmatter(value: Option<&str>) -> Self {
        match value.map(str::trim).map(|value| value.to_ascii_lowercase()) {
            Some(value) if value == "explicit-only" || value == "explicit_only" => {
                Self::ExplicitOnly
            }
            _ => Self::ModelAndUser,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Native,
    Plugin {
        plugin_id: String,
        plugin_name: String,
        authority: Box<crate::plugins::types::PluginAuthority>,
    },
}

impl Skill {
    /// Pick the best description for a session `locale_tag`, falling back to the
    /// default `description` when no localized variant matches.
    ///
    /// Order: exact (lowercased) tag match, then the primary language subtag
    /// (so `en-us` → `en`, `pt-br` → `pt`, `zh-cn` → `zh`), then default.
    ///
    /// Chinese is the one place where the primary-subtag fallback would be
    /// *wrong*: Traditional and Simplified are written differently, so a
    /// Traditional tag (`zh-hant`, or the Traditional regions `zh-tw` / `zh-hk`
    /// / `zh-mo`) must NOT borrow a Simplified `description_zh`. Those match only
    /// an exact `description_zh-hant`-style key, else the default. Simplified
    /// tags (`zh`, `zh-hans`, `zh-cn`, …) still fold to `description_zh`.
    #[must_use]
    pub fn description_for_locale(&self, locale_tag: &str) -> &str {
        if self.localized_descriptions.is_empty() {
            return &self.description;
        }
        let normalized = locale_tag.trim().to_ascii_lowercase();
        if let Some(desc) = self.localized_descriptions.get(&normalized) {
            return desc;
        }
        if let Some((primary, _)) = normalized.split_once('-') {
            // Don't let a Traditional-Chinese session fall back to a Simplified
            // (`zh`) description — different written form, not just a region.
            let traditional_chinese = primary == "zh"
                && (normalized.contains("hant")
                    || normalized.ends_with("-tw")
                    || normalized.ends_with("-hk")
                    || normalized.ends_with("-mo"));
            if !traditional_chinese && let Some(desc) = self.localized_descriptions.get(primary) {
                return desc;
            }
        }
        &self.description
    }
}

/// Collection of discovered skills.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: Vec<Skill>,
    warnings: Vec<String>,
}

/// Cheap metadata stamp used to validate one watched discovery path.
///
/// Some filesystems expose modification times at a coarse resolution. Keeping
/// the file length alongside the timestamp lets an immediate content rewrite
/// invalidate the cache even when the timestamp is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WatchedPathStamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

/// One cached discovery's watched filesystem entries: a path and the metadata
/// stamp observed during the validating walk. `None` means the path was
/// unreadable at walk time; any later readability or metadata change
/// invalidates the entry.
pub(crate) type WatchedPaths = Vec<(PathBuf, Option<WatchedPathStamp>)>;

pub(crate) fn watched_path_stamp(path: &Path) -> Option<WatchedPathStamp> {
    fs::metadata(path).ok().map(|metadata| WatchedPathStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

impl SkillRegistry {
    /// Maximum directory-traversal depth when discovering skills.
    ///
    /// Defends against pathological configurations (e.g. a user pointing
    /// `skills_dir` at `~`) without artificially limiting realistic
    /// vendored layouts like `<root>/<org>/<repo>/<skill>/SKILL.md`.
    const MAX_DISCOVERY_DEPTH: usize = 8;

    /// Discover skills from the given directory.
    ///
    /// The search walks `dir` recursively: any directory that contains a
    /// `SKILL.md` is loaded as a single skill, and the walk does **not**
    /// descend further into that directory (companion files live next to
    /// `SKILL.md`, and `tools::skill::collect_companion_files` already
    /// treats nested subdirs as out-of-scope). This lets users organize
    /// skills by vendor / category — e.g.
    /// `<root>/<vendor>/<skill>/SKILL.md` — instead of being forced into
    /// a flat `<root>/<skill>/SKILL.md` layout.
    ///
    /// Hidden subdirectories (names starting with `.`) below the root
    /// are skipped to avoid descending into VCS / cache trees like
    /// `.git/`. The provided `dir` itself is always honored, even if
    /// hidden — that's what the user explicitly configured.
    /// Symlinked directories are followed when they resolve to directories,
    /// with canonical path tracking plus [`Self::MAX_DISCOVERY_DEPTH`] keeping
    /// the walk finite when a skills layout contains cycles.
    #[must_use]
    pub fn discover(dir: &Path) -> Self {
        Self::discover_watched(dir).0
    }

    /// Discover skills like [`Self::discover`], also returning the watched
    /// filesystem set (every visited directory and every parsed `SKILL.md`)
    /// with its metadata stamp. The discovery cache validates hits by
    /// re-stat()ing only this set instead of re-walking every root.
    pub(crate) fn discover_watched(dir: &Path) -> (Self, WatchedPaths) {
        #[cfg(test)]
        record_root_discovery_call();
        let mut registry = Self::default();
        let mut watched = WatchedPaths::default();
        let Ok(canonical_dir) = fs::canonicalize(dir) else {
            return (registry, watched);
        };
        if !canonical_dir.is_dir() {
            return (registry, watched);
        }

        let mut visited = HashSet::new();
        Self::discover_recursive(dir, 0, &mut registry, &mut visited);
        registry
            .skills
            .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
        watched.extend(visited.iter().map(|p| (p.clone(), watched_path_stamp(p))));
        watched.extend(
            registry
                .skills
                .iter()
                .map(|skill| (skill.path.clone(), watched_path_stamp(&skill.path))),
        );
        (registry, watched)
    }

    fn discover_recursive(
        dir: &Path,
        depth: usize,
        registry: &mut Self,
        visited: &mut HashSet<PathBuf>,
    ) {
        if depth > Self::MAX_DISCOVERY_DEPTH {
            return;
        }
        if !Self::mark_discovered_dir(dir, visited) {
            return;
        }

        #[cfg(test)]
        record_directory_visit();
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(err) => {
                // Only surface a warning for the user-provided root
                // (depth == 0). Nested permission errors are usually
                // noise (e.g. a stray `.Trash` inside someone's
                // `~/.agents/skills`).
                if depth == 0 {
                    registry.push_warning(format!(
                        "Failed to read skills directory {}: {err}",
                        dir.display()
                    ));
                }
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            // Skip hidden subdirectories. Common offenders are `.git`,
            // `.cache`, `.Trash`. The provided root itself is exempt:
            // the user explicitly pointed `skills_dir` at it and we
            // never filter it (it's passed directly to this function,
            // not iterated). This check applies to *children* of the
            // current directory at every depth — including depth 0,
            // because a `.git/` right next to the skills we want is
            // exactly the kind of noise we must not descend into.
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }

            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }

            let skill_path = path.join("SKILL.md");
            #[cfg(test)]
            record_skill_md_read_attempt();
            match fs::read_to_string(&skill_path) {
                Ok(content) => match Self::parse_skill(&skill_path, &content) {
                    Ok(mut skill) => {
                        if !Self::mark_discovered_dir(&path, visited) {
                            continue;
                        }
                        skill.path = skill_path.clone();
                        registry.normalize_skill_name(&mut skill, &skill_path);
                        // Two sibling directories under the same root can
                        // normalize to the same command name (e.g. `My Skill/`
                        // and `my_skill/` both slugify to `my-skill`). Keep the
                        // first (matching the cross-root merge in
                        // `discover_from_directories_with_plugins`) and warn instead of
                        // silently pushing an unreachable duplicate (#3919).
                        let shadowed_by = registry
                            .skills
                            .iter()
                            .find(|s| s.name == skill.name)
                            .map(|s| s.path.clone());
                        if let Some(existing_path) = shadowed_by {
                            registry.push_warning(format!(
                                "Skill `{}` at {} is shadowed by {}.",
                                skill.name,
                                skill.path.display(),
                                existing_path.display()
                            ));
                        } else {
                            registry.skills.push(skill);
                        }
                        // This directory IS a skill. Don't descend further:
                        // any nested `SKILL.md` would be a fixture or
                        // example bundled with the parent skill, not a
                        // separately-installable skill.
                        continue;
                    }
                    Err(reason) => {
                        if !Self::mark_discovered_dir(&path, visited) {
                            continue;
                        }
                        registry.push_warning(format!(
                            "Failed to parse {}: {reason}",
                            skill_path.display()
                        ));
                        // Still treat this directory as "claimed" — a
                        // malformed SKILL.md shouldn't cause us to
                        // double-load nested fixtures as skills.
                        continue;
                    }
                },
                Err(err) if skill_path.exists() => {
                    if !Self::mark_discovered_dir(&path, visited) {
                        continue;
                    }
                    registry
                        .push_warning(format!("Failed to read {}: {err}", skill_path.display()));
                    continue;
                }
                Err(_) => {
                    // No SKILL.md here — recurse to look for nested
                    // skill directories (e.g. `<vendor>/<skill>/SKILL.md`).
                }
            }

            Self::discover_recursive(&path, depth + 1, registry, visited);
        }
    }

    fn mark_discovered_dir(dir: &Path, visited: &mut HashSet<PathBuf>) -> bool {
        let key = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        visited.insert(key)
    }

    fn push_warning(&mut self, warning: String) {
        logging::warn(&warning);
        self.warnings.push(warning);
    }

    fn normalize_skill_name(&mut self, skill: &mut Skill, skill_path: &Path) {
        let normalized = normalize_skill_name_for_lookup(&skill.name);
        if normalized != skill.name || !is_valid_skill_name(&skill.name) {
            let original = skill.name.clone();
            skill.name = normalized;
            self.push_warning(format!(
                "Skill name `{original}` in {} is not a safe command name; using `{}` instead.",
                skill_path.display(),
                skill.name
            ));
        }
    }

    pub(crate) fn parse_skill(_path: &Path, content: &str) -> std::result::Result<Skill, String> {
        let trimmed = content.trim_start();

        // Try to parse frontmatter block first. If absent, fall back to
        // extracting the first `# Heading` as the skill name so that plain
        // Markdown files (no `---` fence) are accepted instead of rejected.
        if trimmed.starts_with("---") {
            let start = content
                .find("---")
                .ok_or_else(|| "missing frontmatter opening delimiter".to_string())?;
            let rest = &content[start + 3..];
            let end = rest
                .find("---")
                .ok_or_else(|| "missing frontmatter closing delimiter".to_string())?;
            let frontmatter = &rest[..end];
            let body = &rest[end + 3..];

            let mut metadata = HashMap::new();
            let lines: Vec<&str> = frontmatter.lines().collect();
            let mut i = 0;
            while i < lines.len() {
                let raw = lines[i];
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') {
                    i += 1;
                    continue;
                }
                if let Some((key, value)) = line.split_once(':') {
                    let value = value.trim();
                    // Check for YAML block scalar indicators: > (folded), | (literal),
                    // optionally with chomping: >-, >+, |-, |+
                    let is_block_scalar = matches!(value, ">" | "|" | ">-" | ">+" | "|-" | "|+");
                    if is_block_scalar {
                        let is_folded = value.starts_with('>');
                        let chomp = if value.ends_with('-') {
                            "strip"
                        } else if value.ends_with('+') {
                            "keep"
                        } else {
                            "clip"
                        };
                        // Determine the base indentation from the key line
                        let base_indent = raw.len() - raw.trim_start().len();
                        let mut block_lines: Vec<&str> = Vec::new();
                        let mut content_indent: Option<usize> = None;
                        i += 1;
                        while i < lines.len() {
                            let raw_line = lines[i];
                            if raw_line.trim().is_empty() {
                                // Empty lines are part of the block
                                block_lines.push("");
                                i += 1;
                                continue;
                            }
                            let line_indent = raw_line.len() - raw_line.trim_start().len();
                            if line_indent > base_indent {
                                // Track content indent from the first non-empty
                                // line so we strip only that one level of
                                // leading whitespace, preserving any deeper
                                // relative indentation (YAML §8.1.2).
                                if content_indent.is_none() {
                                    content_indent = Some(line_indent);
                                }
                                block_lines.push(raw_line);
                                i += 1;
                            } else {
                                break;
                            }
                        }
                        let content_indent = content_indent.unwrap_or(base_indent);
                        // Strip only the content indent from each non-empty
                        // line so nested indentation survives.
                        let block_lines: Vec<&str> = block_lines
                            .iter()
                            .map(|raw| {
                                if raw.is_empty() {
                                    ""
                                } else {
                                    let indent = raw.len() - raw.trim_start().len();
                                    let strip = std::cmp::min(indent, content_indent);
                                    &raw[strip..]
                                }
                            })
                            .collect();
                        // Apply chomping to trailing empty lines before folding.
                        // Chomping operates on the raw block_lines (before join), so
                        // strip / keep / clip behave per the YAML spec.
                        let block_lines = if matches!(chomp, "strip") {
                            // strip: remove all trailing empty lines
                            let mut lines = block_lines;
                            while lines.last().is_some_and(|s| s.is_empty()) {
                                lines.pop();
                            }
                            lines
                        } else if matches!(chomp, "keep") {
                            // keep: no modification
                            block_lines
                        } else {
                            // clip: keep at most one trailing empty line
                            let mut lines = block_lines;
                            while lines.len() >= 2
                                && lines[lines.len() - 1].is_empty()
                                && lines[lines.len() - 2].is_empty()
                            {
                                lines.pop();
                            }
                            lines
                        };
                        let description = if is_folded {
                            // Folded: join non-empty lines with spaces; empty
                            // lines become paragraph breaks.
                            let mut result = String::new();
                            let mut pending_space = false;
                            for line in &block_lines {
                                if line.is_empty() {
                                    result.push('\n');
                                    pending_space = false;
                                } else {
                                    if pending_space {
                                        result.push(' ');
                                    }
                                    result.push_str(line);
                                    pending_space = true;
                                }
                            }
                            result
                        } else {
                            // Literal: join with newlines.
                            block_lines.join("\n")
                        };
                        metadata.insert(key.trim().to_ascii_lowercase(), description);
                    } else {
                        let unquoted = match value {
                            v if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
                                || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2) =>
                            {
                                &v[1..v.len() - 1]
                            }
                            _ => value,
                        };
                        metadata.insert(key.trim().to_ascii_lowercase(), unquoted.to_string());
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }

            let name = metadata
                .get("name")
                .filter(|name| !name.is_empty())
                .cloned()
                .ok_or_else(|| "missing required frontmatter field: name".to_string())?;

            let description = metadata.get("description").cloned().unwrap_or_default();

            let invocation =
                SkillInvocation::from_frontmatter(metadata.get("invocation").map(String::as_str));
            let aliases = metadata
                .get("aliases-for")
                .into_iter()
                .flat_map(|value| value.split([',', ' ', '\t']))
                .map(str::trim)
                .filter(|alias| !alias.is_empty())
                .map(normalize_skill_name_for_lookup)
                .filter(|alias| is_valid_skill_name(alias))
                .collect();

            // Collect `description_<tag>:` frontmatter keys (already lowercased
            // above) into locale-specific descriptions, e.g. `description_zh`.
            let localized_descriptions = metadata
                .iter()
                .filter_map(|(key, value)| {
                    key.strip_prefix("description_")
                        .filter(|tag| !tag.is_empty())
                        .map(|tag| (tag.to_string(), value.clone()))
                })
                .collect();

            return Ok(Skill {
                name,
                description,
                localized_descriptions,
                invocation,
                aliases,
                body: body.trim().to_string(),
                // Filled in by `discover` after parse succeeds; default to an
                // empty path so direct constructors (e.g. tests) compile.
                path: PathBuf::new(),
                source: SkillSource::Native,
            });
        }

        // Graceful degradation: no frontmatter fence found.
        // Extract the first `# Heading` as the skill name.
        let heading_re = regex::Regex::new(r"(?m)^#\s+(.+)$").expect("static regex is valid");
        let name = heading_re
            .captures(content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "no frontmatter and no `# Heading` found to use as skill name".to_string()
            })?;

        Ok(Skill {
            name,
            description: String::new(),
            localized_descriptions: HashMap::new(),
            invocation: SkillInvocation::ModelAndUser,
            aliases: Vec::new(),
            body: content.trim().to_string(),
            path: PathBuf::new(),
            source: SkillSource::Native,
        })
    }

    /// Parse one already-read Skill body while preserving the same name
    /// normalization contract as filesystem discovery. Plugin discovery uses
    /// this after checking the exact byte digest against its reviewed bundle
    /// inventory, so parsing never has to reopen the mutable pathname.
    pub(crate) fn parse_verified_content(
        path: &Path,
        content: &str,
    ) -> std::result::Result<(Skill, Vec<String>), String> {
        let mut registry = Self::default();
        let mut skill = Self::parse_skill(path, content)?;
        skill.path = path.to_path_buf();
        registry.normalize_skill_name(&mut skill, path);
        Ok((skill, registry.warnings))
    }

    /// Lookup a skill by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        let normalized = normalize_skill_name_for_lookup(name);
        self.skills
            .iter()
            .find(|s| s.name == normalized)
            .or_else(|| {
                self.skills
                    .iter()
                    .find(|s| s.aliases.iter().any(|alias| alias == &normalized))
            })
    }

    /// Return all loaded skills.
    pub fn list(&self) -> &[Skill] {
        &self.skills
    }

    /// Apply the shared exact-name activation state after filesystem/plugin
    /// discovery. A qualified plugin Skill can be hidden independently, but
    /// this never changes the plugin bundle's trust or MCP lifecycle.
    #[must_use]
    pub(crate) fn into_enabled(self) -> Self {
        self.into_enabled_with_state(crate::skill_state::SkillStateStore::load_default())
    }

    #[must_use]
    fn into_enabled_with_state(
        mut self,
        state: anyhow::Result<crate::skill_state::SkillStateStore>,
    ) -> Self {
        match state {
            Ok(state) => self.skills.retain(|skill| state.is_enabled(&skill.name)),
            Err(error) => {
                let hidden_plugin_skills = self
                    .skills
                    .iter()
                    .filter(|skill| matches!(skill.source, SkillSource::Plugin { .. }))
                    .count();
                self.skills
                    .retain(|skill| matches!(skill.source, SkillSource::Native));
                self.push_warning(format!(
                    "Failed to read Skill activation state; native Skills remain available for recovery, but {hidden_plugin_skills} reviewed plugin Skill(s) were hidden fail-closed: {error}"
                ));
            }
        }
        self
    }

    /// Parse or I/O warnings encountered while discovering skills.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Check whether any skills were loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Return the number of loaded skills.
    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }
}

fn is_valid_skill_name(name: &str) -> bool {
    let char_count = name.chars().count();
    char_count > 0
        && char_count <= MAX_SKILL_NAME_CHARS
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

pub(crate) fn normalize_skill_name_for_lookup(name: &str) -> String {
    if let Some((plugin, skill)) = name.trim().split_once(':')
        && !plugin.is_empty()
        && !skill.is_empty()
        && !skill.contains(':')
    {
        return format!(
            "{}:{}",
            normalize_skill_name_segment(plugin),
            normalize_skill_name_segment(skill)
        );
    }
    normalize_skill_name_segment(name)
}

fn normalize_skill_name_segment(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;

    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() && out.len() < MAX_SKILL_NAME_CHARS {
                out.push('-');
            }
            pending_dash = false;
            if out.len() < MAX_SKILL_NAME_CHARS {
                out.push(ch.to_ascii_lowercase());
            }
        } else {
            pending_dash = true;
        }

        if out.len() >= MAX_SKILL_NAME_CHARS {
            break;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "skill".to_string()
    } else {
        out
    }
}

/// Resolve every candidate skills directory for a workspace, in
/// precedence order — most specific first. Used for session-time
/// skill discovery so the model sees skills that originated in
/// other AI-tool conventions installed in the same workspace
/// (#432).
///
/// Precedence is defined once in [`roots::SkillRootCatalog`] (first
/// match wins on name conflicts):
///
/// 1. `<workspace>/.agents/skills` — deepseek-native convention.
/// 2. `<workspace>/skills` — flat, project-local.
/// 3. `<workspace>/.opencode/skills` — OpenCode interop.
/// 4. `<workspace>/.claude/skills` — Claude Code interop.
/// 5. `<workspace>/.cursor/skills` — Cursor interop.
/// 6. `<workspace>/.codewhale/skills` — CodeWhale workspace skills.
/// 7. [`agents_global_skills_dir`] — agentskills.io global.
/// 8. `~/.claude/skills` — Claude-ecosystem global (#902).
/// 9. `~/.codewhale/skills` — CodeWhale global, primary install target.
/// 10. `~/.deepseek/skills` — legacy DeepSeek global fallback.
///
/// Compatible audit may also observe `.codex/skills`, but that root is
/// never activated for runtime discovery in this catalog.
///
/// Only directories that exist on disk are returned — callers don't
/// need to filter further. Returns an empty vec when nothing is
/// installed (the system-prompt skills block is then suppressed).
#[must_use]
pub fn skills_directories_for_mode(workspace: &Path, mode: SkillDiscoveryMode) -> Vec<PathBuf> {
    let home = crate::config::effective_home_dir();
    skills_directories_with_home_and_mode(workspace, home.as_deref(), mode)
}

fn skills_directories_with_home_and_mode(
    workspace: &Path,
    home_dir: Option<&Path>,
    mode: SkillDiscoveryMode,
) -> Vec<PathBuf> {
    roots::skills_directories_with_home_and_mode(workspace, home_dir, mode)
}

pub(crate) use roots::codewhale_workspace_skills_dir;
#[cfg(test)]
pub(crate) use roots::existing_skill_dirs;

/// Walk every candidate skills directory for a workspace and merge
/// the discovered skills into a single registry. Name conflicts are
/// resolved with first-match-wins precedence per
/// [`skills_directories_for_mode`].
///
/// Warnings from each scanned directory accumulate so the model
/// (and the user via `/skill list`) can see why a skill didn't
/// load.
#[must_use]
pub fn discover_in_workspace(workspace: &Path) -> SkillRegistry {
    discover_in_workspace_with_mode(workspace, SkillDiscoveryMode::Compatible)
}

#[must_use]
pub fn discover_in_workspace_with_mode(
    workspace: &Path,
    mode: SkillDiscoveryMode,
) -> SkillRegistry {
    discover_in_workspace_with_mode_and_plugins(workspace, mode, None)
}

#[must_use]
pub fn discover_in_workspace_with_mode_and_plugins(
    workspace: &Path,
    mode: SkillDiscoveryMode,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> SkillRegistry {
    discover_from_directories_with_plugins(skills_directories_for_mode(workspace, mode), plugins)
}

/// Discover skills from the workspace search set plus the configured install
/// directory. Workspace-local directories keep their normal precedence; a
/// custom configured directory is inserted before global defaults when it is
/// outside that set so explicit configuration cannot be buried by large global
/// libraries.
#[must_use]
pub fn discover_for_workspace_and_dir_with_mode_and_plugins(
    workspace: &Path,
    skills_dir: &Path,
    mode: SkillDiscoveryMode,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> SkillRegistry {
    let dirs = skill_directories_for_workspace_and_dir(workspace, skills_dir, mode);
    discover_from_directories_with_plugins(dirs, plugins)
}

#[must_use]
pub fn skill_directories_for_workspace_and_dir(
    workspace: &Path,
    skills_dir: &Path,
    mode: SkillDiscoveryMode,
) -> Vec<PathBuf> {
    let mut dirs = skills_directories_for_mode(workspace, mode);
    insert_configured_skills_dir(&mut dirs, workspace, skills_dir);
    dirs
}

fn insert_configured_skills_dir(dirs: &mut Vec<PathBuf>, workspace: &Path, skills_dir: &Path) {
    if !skills_dir.is_dir()
        || dirs
            .iter()
            .any(|p| roots::paths_refer_to_same_dir(p, skills_dir))
    {
        return;
    }

    let workspace_root = fs::canonicalize(workspace).ok();
    let insert_at = workspace_root
        .as_ref()
        .and_then(|root| {
            dirs.iter()
                .position(|dir| fs::canonicalize(dir).map_or(true, |dir| !dir.starts_with(root)))
        })
        .unwrap_or(dirs.len());
    dirs.insert(insert_at, skills_dir.to_path_buf());
}

pub(crate) fn discover_from_directories_with_plugins(
    dirs: impl IntoIterator<Item = PathBuf>,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> SkillRegistry {
    let dirs: Vec<PathBuf> = dirs.into_iter().collect();
    // The watched-validated cache covers the disk-walk merge. Plugin skills
    // merge from the in-memory plugin registry per call, so plugin state
    // changes apply immediately and the cache needs no plugin identity.
    let merged = cached_merged_discovery(dirs);
    merge_plugin_skills(merged, plugins)
}

fn merge_plugin_skills(
    mut merged: SkillRegistry,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> SkillRegistry {
    if let Some(plugins) = plugins {
        merge_active_plugin_skills(&mut merged, plugins);
    }
    merged
}

/// Merge every directory's registry with first-match-wins precedence,
/// collecting each directory's watched filesystem set for cache validation.
fn merge_watched_directories(dirs: Vec<PathBuf>) -> (SkillRegistry, WatchedPaths) {
    let mut merged = SkillRegistry::default();
    let mut watched = WatchedPaths::default();
    for dir in dirs {
        watched.push((dir.clone(), watched_path_stamp(&dir)));
        let (registry, dir_watched) = SkillRegistry::discover_watched(&dir);
        watched.extend(dir_watched);
        for skill in registry.skills {
            if let Some(existing) = merged.skills.iter().find(|s| s.name == skill.name) {
                merged.push_warning(format!(
                    "Skill `{}` at {} is shadowed by {}.",
                    skill.name,
                    skill.path.display(),
                    existing.path.display()
                ));
            } else {
                merged.skills.push(skill);
            }
        }
        for warning in registry.warnings {
            merged.warnings.push(warning);
        }
    }
    (merged, watched)
}

/// One cached merged discovery: the resolved registry plus the watched
/// filesystem entries a hit must re-stat before reuse.
struct DiscoveryCacheEntry {
    watched: WatchedPaths,
    registry: SkillRegistry,
}

/// Bound the cache so distinct workspaces/modes cannot grow it without
/// limit; a full cache is simply cleared on the next miss.
const MAX_DISCOVERY_CACHE_ENTRIES: usize = 8;

fn discovery_cache() -> &'static RwLock<HashMap<Vec<PathBuf>, DiscoveryCacheEntry>> {
    static CACHE: OnceLock<RwLock<HashMap<Vec<PathBuf>, DiscoveryCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Drop every cached merged discovery. Called after any skill
/// install/uninstall/update so the next build re-walks from disk.
pub fn clear_skill_discovery_cache() {
    discovery_cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// Merged discovery for one resolved directory set, cached by that set.
/// A hit re-stats only the watched entries (each visited directory and
/// parsed `SKILL.md`); any metadata or readability change re-walks fully.
fn cached_merged_discovery(dirs: Vec<PathBuf>) -> SkillRegistry {
    {
        let read = discovery_cache()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = read.get(&dirs)
            && entry
                .watched
                .iter()
                .all(|(path, stamp)| watched_path_stamp(path) == *stamp)
        {
            return entry.registry.clone();
        }
    }
    let (merged, watched) = merge_watched_directories(dirs.clone());
    let mut write = discovery_cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if write.len() >= MAX_DISCOVERY_CACHE_ENTRIES {
        write.clear();
    }
    write.insert(
        dirs,
        DiscoveryCacheEntry {
            watched,
            registry: merged.clone(),
        },
    );
    merged
}

fn merge_active_plugin_skills(
    registry: &mut SkillRegistry,
    plugins: &crate::plugins::PluginRegistry,
) {
    let Some(state_path) = plugins.state_path().map(Path::to_path_buf) else {
        return;
    };
    let plugins = plugins
        .list()
        .into_iter()
        .filter_map(|plugin| {
            plugin
                .authority(state_path.clone(), plugins.workspace().to_path_buf())
                .map(|authority| (plugin.clone(), authority))
        })
        .collect::<Vec<_>>();
    merge_plugin_skills_from_plugins(registry, plugins);
}

fn merge_plugin_skills_from_plugins(
    registry: &mut SkillRegistry,
    plugins: impl IntoIterator<
        Item = (
            crate::plugins::types::LoadedPlugin,
            crate::plugins::types::PluginAuthority,
        ),
    >,
) {
    for (plugin, authority) in plugins {
        // Keep the adapter independently fail-closed for headless callers.
        if !plugin.active()
            || crate::plugins::registry::verify_plugin_authority(&authority).is_err()
        {
            continue;
        }
        let plugin_id = plugin.id.to_string();
        let plugin_name = plugin.name().to_string();
        for snapshot in plugin.skill_snapshots {
            let qualified_name = format!("{plugin_name}:{}", snapshot.name);
            if let Some(existing) = registry
                .skills
                .iter()
                .find(|skill| skill.name == qualified_name)
            {
                registry.push_warning(format!(
                    "Plugin skill `{qualified_name}` at {} is shadowed by {}.",
                    snapshot.path.display(),
                    existing.path.display()
                ));
                continue;
            }
            registry.skills.push(Skill {
                name: qualified_name,
                description: snapshot.description,
                localized_descriptions: snapshot.localized_descriptions,
                invocation: snapshot.invocation,
                aliases: snapshot.aliases,
                body: snapshot.body,
                path: snapshot.path,
                source: SkillSource::Plugin {
                    plugin_id: plugin_id.clone(),
                    plugin_name: plugin_name.clone(),
                    authority: Box::new(authority.clone()),
                },
            });
        }
    }
}

#[cfg(test)]
pub(crate) fn discover_for_workspace_and_dir_with_home(
    workspace: &Path,
    skills_dir: &Path,
    home_dir: Option<&Path>,
) -> SkillRegistry {
    discover_for_workspace_and_dir_with_home_and_mode(
        workspace,
        skills_dir,
        home_dir,
        SkillDiscoveryMode::Compatible,
    )
}

#[cfg(test)]
pub(crate) fn discover_for_workspace_and_dir_with_home_and_mode(
    workspace: &Path,
    skills_dir: &Path,
    home_dir: Option<&Path>,
    mode: SkillDiscoveryMode,
) -> SkillRegistry {
    discover_for_workspace_and_dir_with_home_and_mode_and_plugins(
        workspace, skills_dir, home_dir, mode, None,
    )
}

#[cfg(test)]
pub(crate) fn discover_for_workspace_and_dir_with_home_and_mode_and_plugins(
    workspace: &Path,
    skills_dir: &Path,
    home_dir: Option<&Path>,
    mode: SkillDiscoveryMode,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> SkillRegistry {
    let mut dirs = skills_directories_with_home_and_mode(workspace, home_dir, mode);
    insert_configured_skills_dir(&mut dirs, workspace, skills_dir);
    discover_from_directories_with_plugins(dirs, plugins)
}

/// Render the system-prompt skills block from every workspace
/// candidate directory plus the global default (#432). Wraps
/// [`discover_in_workspace`] for callers (e.g. `prompts.rs`) that
/// only have the workspace path to hand.
#[must_use]
pub fn render_available_skills_context_for_workspace(workspace: &Path) -> Option<String> {
    let registry = discover_in_workspace(workspace);
    render_skills_block(&registry, "en", workspace)
}

#[must_use]
pub fn render_available_skills_context_for_workspace_with_mode_and_plugins(
    workspace: &Path,
    mode: SkillDiscoveryMode,
    locale: &str,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> Option<String> {
    let registry = discover_in_workspace_with_mode_and_plugins(workspace, mode, plugins);
    render_skills_block(&registry, locale, workspace)
}

/// Codex's progressive-disclosure contract: the model sees skill names,
/// descriptions, and paths up front, then opens the specific `SKILL.md` only
/// when a skill is relevant.
///
/// Single-directory variant — use
/// [`render_available_skills_context_for_workspace`] when scanning
/// a workspace for cross-tool skill folders (#432).
#[cfg(test)]
#[must_use]
fn render_available_skills_context(skills_dir: &Path) -> Option<String> {
    let registry = SkillRegistry::discover(skills_dir);
    render_skills_block(&registry, "en", skills_dir)
}

/// Union variant: merge skills discovered in the `workspace` (cross-tool skill
/// folders) and an explicitly-configured `skills_dir`.
#[must_use]
pub fn render_available_skills_context_for_workspace_and_dir(
    workspace: &Path,
    skills_dir: &Path,
) -> Option<String> {
    render_available_skills_context_for_workspace_and_dir_with_mode(
        workspace,
        skills_dir,
        SkillDiscoveryMode::Compatible,
        "en",
    )
}

#[must_use]
pub fn render_available_skills_context_for_workspace_and_dir_with_mode(
    workspace: &Path,
    skills_dir: &Path,
    mode: SkillDiscoveryMode,
    locale: &str,
) -> Option<String> {
    let registry =
        discover_for_workspace_and_dir_with_mode_and_plugins(workspace, skills_dir, mode, None)
            .into_enabled();
    render_skills_block(&registry, locale, workspace)
}

#[must_use]
pub fn render_available_skills_context_for_workspace_and_dir_with_mode_and_plugins(
    workspace: &Path,
    skills_dir: &Path,
    mode: SkillDiscoveryMode,
    locale: &str,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> Option<String> {
    let registry =
        discover_for_workspace_and_dir_with_mode_and_plugins(workspace, skills_dir, mode, plugins)
            .into_enabled();
    render_skills_block(&registry, locale, workspace)
}

/// Replace absolute path prefixes in free-form text (skill load warnings)
/// with privacy-safe stand-ins before the text enters the system-prompt
/// prefix (#4632). Workspace paths become `.`, home-dir paths become `~`.
fn sanitize_prompt_path_text(text: &str, workspace: &Path) -> String {
    let mut out = text.to_string();
    if let Some(ws) = workspace.to_str()
        && !ws.is_empty()
    {
        out = out.replace(ws, ".");
    }
    if let Some(home) = crate::config::effective_home_dir()
        && let Some(home_str) = home.to_str()
        && !home_str.is_empty()
    {
        out = out.replace(home_str, "~");
    }
    // Environment variables are process-global, and concurrent embedders or
    // tests may temporarily redirect HOME after discovery recorded a warning.
    // Scrub conventional home roots by shape as a final privacy boundary.
    for marker in ["/Users/", "/home/"] {
        while let Some(start) = out.find(marker) {
            let user_start = start + marker.len();
            let user_len = out[user_start..]
                .find(|ch: char| ch == '/' || ch.is_whitespace())
                .unwrap_or(out.len() - user_start);
            out.replace_range(start..user_start + user_len, "~");
        }
    }
    out
}

/// Render a skill path without leaking private absolute paths into the
/// system-prompt prefix (#4632): workspace skills become workspace-relative,
/// home-dir skills become `~/…`, and anything else is reduced to its trailing
/// components so the prefix stays free of user-identifying absolute paths.
fn privacy_safe_skill_path(path: &Path, workspace: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(workspace) {
        return rel.display().to_string();
    }
    if let Some(home) = crate::config::effective_home_dir()
        && let Ok(rel) = path.strip_prefix(&home)
    {
        return format!("~/{}", rel.display());
    }
    match (path.parent().and_then(Path::file_name), path.file_name()) {
        (Some(dir), Some(file)) => {
            format!("…/{}/{}", dir.to_string_lossy(), file.to_string_lossy())
        }
        _ => path
            .file_name()
            .map(|file| file.to_string_lossy().into_owned())
            .unwrap_or_else(|| "SKILL.md".to_string()),
    }
}

fn render_skills_block(registry: &SkillRegistry, locale: &str, workspace: &Path) -> Option<String> {
    if registry.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("## Skills\n");
    out.push_str(
        "A skill is a set of local instructions stored in a `SKILL.md` file. \
Below is the list of skills available in this session. Each entry includes a \
name, description, and source locator. Native skills expose a file path; \
reviewed plugin snapshots must be opened with `load_skill`.\n\n",
    );
    out.push_str("### Available skills\n");

    let mut omitted = 0usize;
    for skill in registry.list() {
        if skill.invocation == SkillInvocation::ExplicitOnly {
            // Explicit-only skills remain loadable by their canonical name or
            // alias, but must not be presented as model-selectable catalogue
            // entries. This keeps opt-in power skills from becoming ambient
            // instructions or consuming prompt budget.
            continue;
        }
        // Native skills expose the real on-disk path captured at discovery.
        // Plugin skills expose only their reviewed snapshot identity so the
        // model cannot bypass the content-bound trust receipt via a mutable
        // source path.
        // Use the real on-disk path captured at discovery — the directory
        // name can differ from the frontmatter `name` for community
        // installs, in which case `<dir>/<name>/SKILL.md` would not exist
        // and the model would fail to open it. Rendered privacy-safe
        // (workspace-relative or ~/…) so the prompt prefix never embeds
        // absolute user paths (#4632).
        let display_path = privacy_safe_skill_path(&skill.path, workspace);
        let description = truncate_for_prompt(
            skill.description_for_locale(locale),
            MAX_SKILL_DESCRIPTION_CHARS,
        );
        let source = match &skill.source {
            SkillSource::Native => format!("file: {display_path}"),
            SkillSource::Plugin {
                plugin_id,
                plugin_name,
                ..
            } => format!("reviewed plugin snapshot: {plugin_name} ({plugin_id}); use load_skill"),
        };
        let line = if description.is_empty() {
            format!("- {}: ({source})\n", skill.name)
        } else {
            format!("- {}: {} ({source})\n", skill.name, description)
        };

        if out.chars().count() + line.chars().count() > MAX_AVAILABLE_SKILLS_CHARS {
            omitted += 1;
        } else {
            out.push_str(&line);
        }
    }

    if omitted > 0 {
        out.push_str(&format!(
            "- ... {omitted} additional skills omitted from this prompt budget.\n"
        ));
    }

    if !registry.warnings().is_empty() {
        out.push_str("\n### Skill load warnings\n");
        for warning in registry.warnings().iter().take(8) {
            out.push_str("- ");
            out.push_str(&truncate_for_prompt(
                &sanitize_prompt_path_text(warning, workspace),
                MAX_SKILL_DESCRIPTION_CHARS,
            ));
            out.push('\n');
        }
    }

    out.push_str(
        "\n### How to use skills\n\
- Use `load_skill` to open any skill body by name. This is required for reviewed plugin snapshots and is the preferred path for native skills, including global skills outside the workspace. Direct file reads retain the normal workspace/trust boundary.\n\
- Trigger rules: use a skill when the user names it (`$SkillName`, `/skill <name>`, or plain text) or the task clearly matches its description. Do not carry skills across turns unless re-mentioned.\n\
- Missing/blocked: if a named skill is missing or cannot be read, say so briefly and continue with the best fallback.\n\
- Safety: do not execute scripts from a community skill unless the user explicitly asks or the skill has been trusted for script use.\n",
    );

    Some(out)
}

fn truncate_for_prompt(value: &str, max_chars: usize) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= max_chars {
        return single_line;
    }

    let mut truncated = single_line
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests;
