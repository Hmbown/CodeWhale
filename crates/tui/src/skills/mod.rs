//! Skill discovery and registry for local SKILL.md files.

pub mod install;
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
pub use system::{install_system_skills, is_bundled_skill_name};

use std::fs;
use std::path::{Path, PathBuf};

use std::collections::{HashMap, HashSet};

use crate::logging;

const MAX_SKILL_DESCRIPTION_CHARS: usize = 280;
const MAX_AVAILABLE_SKILLS_CHARS: usize = 12_000;

// === Defaults ===

#[must_use]
pub fn default_skills_dir() -> PathBuf {
    dirs::home_dir().map_or_else(
        || PathBuf::from("/tmp/codewhale/skills"),
        |p| p.join(".codewhale").join("skills"),
    )
}

/// Global agentskills.io-compatible skills directory (`~/.agents/skills`).
#[must_use]
pub fn agents_global_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".agents").join("skills"))
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
    pub description: String,
    pub body: String,
    /// On-disk path to the `SKILL.md` this was loaded from. The directory
    /// name can differ from the frontmatter `name` for community installs
    /// or manually-placed skills, so callers must use this rather than
    /// reconstructing `<dir>/<name>/SKILL.md`.
    pub path: PathBuf,
}

/// Collection of discovered skills.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: Vec<Skill>,
    warnings: Vec<String>,
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
        let mut registry = Self::default();
        let Ok(canonical_dir) = fs::canonicalize(dir) else {
            return registry;
        };
        if !canonical_dir.is_dir() {
            return registry;
        }

        let mut visited = HashSet::new();
        Self::discover_recursive(dir, 0, &mut registry, &mut visited);
        registry
            .skills
            .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
        registry
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
            match fs::read_to_string(&skill_path) {
                Ok(content) => match Self::parse_skill(&skill_path, &content) {
                    Ok(mut skill) => {
                        if !Self::mark_discovered_dir(&path, visited) {
                            continue;
                        }
                        skill.path = skill_path.clone();
                        registry.skills.push(skill);
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

    fn parse_skill(_path: &Path, content: &str) -> std::result::Result<Skill, String> {
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

            return Ok(Skill {
                name,
                description,
                body: body.trim().to_string(),
                // Filled in by `discover` after parse succeeds; default to an
                // empty path so direct constructors (e.g. tests) compile.
                path: PathBuf::new(),
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
            body: content.trim().to_string(),
            path: PathBuf::new(),
        })
    }

    /// Lookup a skill by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Return all loaded skills.
    pub fn list(&self) -> &[Skill] {
        &self.skills
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

/// Resolve every candidate skills directory for a workspace, in
/// precedence order — most specific first. Used for session-time
/// skill discovery so the model sees skills that originated in
/// other AI-tool conventions installed in the same workspace
/// (#432).
///
/// Precedence (first match wins on name conflicts):
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
/// Only directories that exist on disk are returned — callers don't
/// need to filter further. Returns an empty vec when nothing is
/// installed (the system-prompt skills block is then suppressed).
#[must_use]
#[allow(dead_code)] // Skill directory discovery; used by install/refresh paths (see #3490)
pub fn skills_directories(workspace: &Path) -> Vec<PathBuf> {
    skills_directories_for_mode(workspace, SkillDiscoveryMode::Compatible)
}

#[must_use]
pub fn skills_directories_for_mode(workspace: &Path, mode: SkillDiscoveryMode) -> Vec<PathBuf> {
    let home = dirs::home_dir();
    skills_directories_with_home_and_mode(workspace, home.as_deref(), mode)
}

fn skills_directories_with_home_and_mode(
    workspace: &Path,
    home_dir: Option<&Path>,
    mode: SkillDiscoveryMode,
) -> Vec<PathBuf> {
    let mut candidates = match mode {
        SkillDiscoveryMode::Compatible => vec![
            workspace.join(".agents").join("skills"),
            workspace.join("skills"),
            workspace.join(".opencode").join("skills"),
            workspace.join(".claude").join("skills"),
            workspace.join(".cursor").join("skills"),
            workspace.join(".codewhale").join("skills"),
        ],
        SkillDiscoveryMode::CodeWhaleOnly => codewhale_workspace_skills_dir(workspace)
            .into_iter()
            .collect(),
    };
    if let Some(home) = home_dir {
        match mode {
            SkillDiscoveryMode::Compatible => {
                candidates.push(home.join(".agents").join("skills"));
                candidates.push(home.join(".claude").join("skills"));
                candidates.push(home.join(".codewhale").join("skills"));
                candidates.push(home.join(".deepseek").join("skills"));
            }
            SkillDiscoveryMode::CodeWhaleOnly => {
                candidates.push(home.join(".codewhale").join("skills"));
            }
        }
    } else {
        candidates.push(PathBuf::from("/tmp/codewhale/skills"));
    }
    existing_skill_dirs(candidates)
}

pub(crate) fn codewhale_workspace_skills_dir(workspace: &Path) -> Option<PathBuf> {
    let skills_dir = workspace.join(".codewhale").join("skills");
    let canonical_workspace = fs::canonicalize(workspace).ok()?;
    let canonical_skills = fs::canonicalize(&skills_dir).ok()?;
    (canonical_skills.is_dir() && canonical_skills.starts_with(canonical_workspace))
        .then_some(skills_dir)
}

fn existing_skill_dirs(candidates: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for path in candidates {
        let Ok(canonical_path) = fs::canonicalize(&path) else {
            continue;
        };
        if canonical_path.is_dir() && seen.insert(canonical_path) {
            out.push(path);
        }
    }
    out
}

/// Walk every candidate skills directory for a workspace and merge
/// the discovered skills into a single registry. Name conflicts are
/// resolved with first-match-wins precedence per
/// [`skills_directories`].
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
    let mut merged = SkillRegistry::default();
    for dir in skills_directories_for_mode(workspace, mode) {
        let registry = SkillRegistry::discover(&dir);
        for skill in registry.skills {
            if !merged.skills.iter().any(|s| s.name == skill.name) {
                merged.skills.push(skill);
            }
        }
        for warning in registry.warnings {
            merged.warnings.push(warning);
        }
    }
    merged
}

/// Discover skills from the workspace search set plus the configured install
/// directory. Workspace-local directories keep their normal precedence; a
/// custom configured directory is inserted before global defaults when it is
/// outside that set so explicit configuration cannot be buried by large global
/// libraries.
#[must_use]
#[allow(dead_code)] // Skill discovery helper; used by workspace initialization (see #3490)
pub fn discover_for_workspace_and_dir(workspace: &Path, skills_dir: &Path) -> SkillRegistry {
    discover_for_workspace_and_dir_with_mode(workspace, skills_dir, SkillDiscoveryMode::Compatible)
}

#[must_use]
pub fn discover_for_workspace_and_dir_with_mode(
    workspace: &Path,
    skills_dir: &Path,
    mode: SkillDiscoveryMode,
) -> SkillRegistry {
    let dirs = skill_directories_for_workspace_and_dir(workspace, skills_dir, mode);
    discover_from_directories(dirs)
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
    if !skills_dir.is_dir() || dirs.iter().any(|p| paths_refer_to_same_dir(p, skills_dir)) {
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

fn paths_refer_to_same_dir(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub(crate) fn discover_from_directories(dirs: impl IntoIterator<Item = PathBuf>) -> SkillRegistry {
    let mut merged = SkillRegistry::default();
    for dir in dirs {
        let registry = SkillRegistry::discover(&dir);
        for skill in registry.skills {
            if !merged.skills.iter().any(|s| s.name == skill.name) {
                merged.skills.push(skill);
            }
        }
        for warning in registry.warnings {
            merged.warnings.push(warning);
        }
    }
    merged
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
    let mut dirs = skills_directories_with_home_and_mode(workspace, home_dir, mode);
    insert_configured_skills_dir(&mut dirs, workspace, skills_dir);
    discover_from_directories(dirs)
}

/// Render the system-prompt skills block from every workspace
/// candidate directory plus the global default (#432). Wraps
/// [`discover_in_workspace`] for callers (e.g. `prompts.rs`) that
/// only have the workspace path to hand.
#[must_use]
pub fn render_available_skills_context_for_workspace(workspace: &Path) -> Option<String> {
    let registry = discover_in_workspace(workspace);
    render_skills_block(&registry)
}

#[must_use]
pub fn render_available_skills_context_for_workspace_with_mode(
    workspace: &Path,
    mode: SkillDiscoveryMode,
) -> Option<String> {
    let registry = discover_in_workspace_with_mode(workspace, mode);
    render_skills_block(&registry)
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
    render_skills_block(&registry)
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
    )
}

#[must_use]
pub fn render_available_skills_context_for_workspace_and_dir_with_mode(
    workspace: &Path,
    skills_dir: &Path,
    mode: SkillDiscoveryMode,
) -> Option<String> {
    let registry = discover_for_workspace_and_dir_with_mode(workspace, skills_dir, mode);
    render_skills_block(&registry)
}

fn render_skills_block(registry: &SkillRegistry) -> Option<String> {
    if registry.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("## Skills\n");
    out.push_str(
        "A skill is a set of local instructions stored in a `SKILL.md` file. \
Below is the list of skills available in this session. Each entry includes a \
name, description, and file path so you can open the source for full \
instructions when using a specific skill.\n\n",
    );
    out.push_str("### Available skills\n");

    let mut omitted = 0usize;
    for skill in registry.list() {
        // Use the real on-disk path captured at discovery — the directory
        // name can differ from the frontmatter `name` for community
        // installs, in which case `<dir>/<name>/SKILL.md` would not exist
        // and the model would fail to open it.
        let description = truncate_for_prompt(&skill.description, MAX_SKILL_DESCRIPTION_CHARS);
        let line = if description.is_empty() {
            format!("- {}: (file: {})\n", skill.name, skill.path.display())
        } else {
            format!(
                "- {}: {} (file: {})\n",
                skill.name,
                description,
                skill.path.display()
            )
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
            out.push_str(&truncate_for_prompt(warning, MAX_SKILL_DESCRIPTION_CHARS));
            out.push('\n');
        }
    }

    out.push_str(
        "\n### How to use skills\n\
- Skill bodies live on disk at the listed paths. When a skill is relevant, open only that skill's `SKILL.md` and the specific companion files it references.\n\
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
mod tests {
    use tempfile::TempDir;

    fn create_skill_dir(tmpdir: &TempDir, skill_name: &str, skill_content: &str) {
        let skill_dir = tmpdir.path().join("skills").join(skill_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), skill_content).unwrap();
    }

    #[test]
    fn render_available_skills_context_lists_paths_and_usage() {
        let tmpdir = TempDir::new().unwrap();
        create_skill_dir(
            &tmpdir,
            "test-skill",
            "---\nname: test-skill\ndescription: A test skill\n---\nDo something special",
        );

        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");

        let expected_path = tmpdir
            .path()
            .join("skills")
            .join("test-skill")
            .join("SKILL.md")
            .display()
            .to_string();

        assert!(rendered.contains("## Skills"));
        assert!(rendered.contains("- test-skill: A test skill"));
        assert!(
            rendered.contains(&expected_path),
            "expected path {expected_path:?} not in rendered output"
        );
        assert!(rendered.contains("### How to use skills"));
    }

    #[test]
    fn render_available_skills_context_uses_real_dir_name_not_frontmatter_name() {
        // Regression: when a community-installed or manually-placed skill
        // lives in a directory whose name differs from its frontmatter
        // `name`, the rendered prompt must point to the real on-disk file
        // path, not <skills_dir>/<frontmatter-name>/SKILL.md (which does
        // not exist).
        let tmpdir = TempDir::new().unwrap();
        create_skill_dir(
            &tmpdir,
            "weird-dir-name",
            "---\nname: friendly-name\ndescription: drift case\n---\nbody",
        );

        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");

        let real_path = tmpdir
            .path()
            .join("skills")
            .join("weird-dir-name")
            .join("SKILL.md")
            .display()
            .to_string();
        let stale_path = tmpdir
            .path()
            .join("skills")
            .join("friendly-name")
            .join("SKILL.md")
            .display()
            .to_string();

        assert!(
            rendered.contains(&real_path),
            "expected real on-disk path {real_path:?} in rendered output, got:\n{rendered}"
        );
        assert!(
            !rendered.contains(&stale_path),
            "rendered output must not invent a path under the frontmatter name:\n{rendered}"
        );
    }

    #[test]
    fn render_available_skills_context_returns_none_when_empty() {
        let tmpdir = TempDir::new().unwrap();
        let empty = tmpdir.path().join("skills");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(crate::skills::render_available_skills_context(&empty).is_none());

        let missing = tmpdir.path().join("does-not-exist");
        assert!(crate::skills::render_available_skills_context(&missing).is_none());
    }

    #[test]
    fn render_available_skills_context_truncates_long_descriptions() {
        let tmpdir = TempDir::new().unwrap();
        let long_desc = "x".repeat(2_000);
        let body = format!("---\nname: bigdesc\ndescription: {long_desc}\n---\nbody");
        create_skill_dir(&tmpdir, "bigdesc", &body);

        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");

        let max = super::MAX_SKILL_DESCRIPTION_CHARS;
        assert!(rendered.contains('…'), "expected truncation marker");
        assert!(
            !rendered.contains(&"x".repeat(max + 1)),
            "untruncated long run should not appear"
        );
    }

    #[test]
    fn render_available_skills_context_collapses_internal_whitespace() {
        let tmpdir = TempDir::new().unwrap();
        create_skill_dir(
            &tmpdir,
            "spaced-skill",
            "---\nname: spaced-skill\ndescription: alpha  \t  beta   gamma\n---\nbody",
        );

        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");

        let line = rendered
            .lines()
            .find(|l| l.starts_with("- spaced-skill:"))
            .expect("skill line");
        assert!(line.contains("alpha beta gamma"), "got: {line:?}");
    }

    #[test]
    fn render_available_skills_context_omits_overflowing_skills() {
        let tmpdir = TempDir::new().unwrap();
        let big_desc = "y".repeat(super::MAX_SKILL_DESCRIPTION_CHARS - 20);
        for i in 0..200 {
            let body = format!("---\nname: skill-{i:03}\ndescription: {big_desc}\n---\nbody");
            create_skill_dir(&tmpdir, &format!("skill-{i:03}"), &body);
        }

        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");

        assert!(
            rendered.contains("additional skills omitted from this prompt budget"),
            "expected overflow notice"
        );
        assert!(
            rendered.chars().count() < super::MAX_AVAILABLE_SKILLS_CHARS + 4_000,
            "rendered length should stay near the budget"
        );
    }

    #[test]
    fn render_skills_block_preserves_registry_precedence_under_prompt_budget() {
        let tmpdir = TempDir::new().unwrap();
        let mut registry = super::SkillRegistry::default();
        registry.skills.push(super::Skill {
            name: "workspace-priority".to_string(),
            description: "must survive truncation".to_string(),
            body: "body".to_string(),
            path: tmpdir
                .path()
                .join(".claude")
                .join("skills")
                .join("workspace-priority")
                .join("SKILL.md"),
        });

        let big_desc = "y".repeat(super::MAX_SKILL_DESCRIPTION_CHARS - 20);
        for i in 0..200 {
            registry.skills.push(super::Skill {
                name: format!("aaa-global-{i:03}"),
                description: big_desc.clone(),
                body: "body".to_string(),
                path: tmpdir
                    .path()
                    .join(".deepseek")
                    .join("skills")
                    .join(format!("aaa-global-{i:03}"))
                    .join("SKILL.md"),
            });
        }

        let rendered = super::render_skills_block(&registry).expect("skill context");
        assert!(
            rendered.contains("workspace-priority"),
            "higher-precedence workspace skills must not be reordered behind globals:\n{rendered}"
        );
        assert!(
            rendered.contains("additional skills omitted from this prompt budget"),
            "fixture should exceed prompt budget"
        );
    }

    fn write_skill(dir: &std::path::Path, name: &str, description: &str, body: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
        )
        .unwrap();
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[test]
    fn skills_directories_returns_existing_dirs_in_precedence_order() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path();

        // Create four of the five workspace candidate dirs (skip `.opencode`).
        std::fs::create_dir_all(workspace.join(".agents").join("skills")).unwrap();
        std::fs::create_dir_all(workspace.join("skills")).unwrap();
        std::fs::create_dir_all(workspace.join(".claude").join("skills")).unwrap();
        std::fs::create_dir_all(workspace.join(".cursor").join("skills")).unwrap();

        let dirs = super::skills_directories(workspace);
        // We don't assert on the global default position because it's
        // host-dependent (may not exist on the test machine).
        let mut idx = 0;
        let agents = workspace.join(".agents").join("skills");
        let local = workspace.join("skills");
        let claude = workspace.join(".claude").join("skills");
        let cursor = workspace.join(".cursor").join("skills");

        assert_eq!(dirs.get(idx), Some(&agents), "agents must come first");
        idx += 1;
        assert_eq!(dirs.get(idx), Some(&local), "local must come second");
        idx += 1;
        // .opencode/skills was not created — it must NOT appear.
        assert!(
            !dirs
                .iter()
                .any(|p| p == &workspace.join(".opencode").join("skills")),
            "missing dir must be omitted, got: {dirs:?}"
        );
        assert_eq!(dirs.get(idx), Some(&claude), "claude must come after local");
        idx += 1;
        assert_eq!(
            dirs.get(idx),
            Some(&cursor),
            "cursor must come after claude"
        );
    }

    #[test]
    fn existing_skill_dirs_orders_globals_agents_then_claude_then_deepseek() {
        // Pins the precedence among the three global skill roots (#902).
        // Workspace candidates are tested separately above; here we only
        // exercise the global ordering at the existing_skill_dirs level
        // so the assertion is host-independent.
        let tmpdir = TempDir::new().unwrap();
        let agents_global = tmpdir.path().join(".agents").join("skills");
        let claude_global = tmpdir.path().join(".claude").join("skills");
        let deepseek_global = tmpdir.path().join(".deepseek").join("skills");
        std::fs::create_dir_all(&agents_global).unwrap();
        std::fs::create_dir_all(&claude_global).unwrap();
        std::fs::create_dir_all(&deepseek_global).unwrap();

        let dirs = super::existing_skill_dirs(vec![
            agents_global.clone(),
            claude_global.clone(),
            deepseek_global.clone(),
        ]);

        assert_eq!(dirs, vec![agents_global, claude_global, deepseek_global]);
    }

    #[test]
    fn existing_skill_dirs_keeps_agents_global_before_deepseek_global() {
        let tmpdir = TempDir::new().unwrap();
        let agents_global = tmpdir.path().join(".agents").join("skills");
        let deepseek_global = tmpdir.path().join(".deepseek").join("skills");
        let missing = tmpdir.path().join("missing").join("skills");
        std::fs::create_dir_all(&agents_global).unwrap();
        std::fs::create_dir_all(&deepseek_global).unwrap();

        let dirs = super::existing_skill_dirs(vec![
            missing,
            agents_global.clone(),
            deepseek_global.clone(),
            agents_global.clone(),
        ]);

        assert_eq!(dirs, vec![agents_global, deepseek_global]);
    }

    #[test]
    fn discover_in_workspace_merges_with_first_wins_precedence() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path();

        // Same skill name `shared` in two locations — the higher-precedence
        // dir's version should win.
        write_skill(
            &workspace.join(".agents").join("skills"),
            "shared",
            "agents wins",
            "from agents",
        );
        write_skill(
            &workspace.join(".claude").join("skills"),
            "shared",
            "claude loses",
            "from claude",
        );
        // Unique skill in claude — should still be discovered.
        write_skill(
            &workspace.join(".claude").join("skills"),
            "unique-claude",
            "only here",
            "claude-only",
        );

        let registry = super::discover_in_workspace(workspace);
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"shared"),
            "shared must be present: {names:?}"
        );
        assert!(names.contains(&"unique-claude"));

        let shared = registry.get("shared").expect("shared present");
        assert_eq!(
            shared.description, "agents wins",
            "first-wins precedence should keep .agents/skills version"
        );
        assert!(
            shared.path.starts_with(workspace.join(".agents")),
            "shared.path should be from .agents/skills, got {:?}",
            shared.path
        );
    }

    #[test]
    fn discover_in_workspace_pulls_skills_from_opencode_dir() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path();
        write_skill(
            &workspace.join(".opencode").join("skills"),
            "opencode-only",
            "for interop",
            "body",
        );

        let registry = super::discover_in_workspace(workspace);
        assert!(
            registry.get("opencode-only").is_some(),
            ".opencode/skills must be scanned (#432)"
        );
    }

    #[test]
    fn discover_in_workspace_pulls_skills_from_cursor_dir() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path();
        write_skill(
            &workspace.join(".cursor").join("skills"),
            "cursor-only",
            "for cursor interop",
            "body",
        );

        let registry = super::discover_in_workspace(workspace);
        assert!(
            registry.get("cursor-only").is_some(),
            ".cursor/skills must be scanned"
        );
    }

    #[test]
    fn discover_accepts_plain_markdown_heading_without_frontmatter() {
        let tmpdir = TempDir::new().unwrap();
        let skill_dir = tmpdir.path().join("plain-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "# Plain Skill\n\nUse this skill without YAML frontmatter.\n",
        )
        .unwrap();

        let registry = super::SkillRegistry::discover(tmpdir.path());
        let skill = registry.get("Plain Skill").expect("plain skill parsed");
        assert_eq!(skill.description, "");
        assert!(skill.body.contains("Use this skill"));
    }

    #[test]
    fn discover_warns_for_plain_markdown_without_heading() {
        let tmpdir = TempDir::new().unwrap();
        let skill_dir = tmpdir.path().join("plain-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "Use this skill without a heading or YAML frontmatter.\n",
        )
        .unwrap();

        let registry = super::SkillRegistry::discover(tmpdir.path());
        assert!(registry.is_empty());
        assert!(
            registry
                .warnings()
                .iter()
                .any(|warning| warning.contains("no `# Heading` found")),
            "expected missing-heading warning, got {:?}",
            registry.warnings()
        );
    }

    #[test]
    fn render_available_skills_context_for_workspace_picks_up_cross_tool_dirs() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path();
        write_skill(
            &workspace.join(".claude").join("skills"),
            "from-claude",
            "claude-style skill",
            "body",
        );
        let rendered =
            super::render_available_skills_context_for_workspace(workspace).expect("non-empty");
        assert!(rendered.contains("from-claude"));
    }

    #[test]
    fn codewhale_only_mode_ignores_cross_tool_skill_dirs() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path().join("workspace");
        let home = tmpdir.path().join("home");
        let configured_dir = home.join(".codewhale").join("skills");
        std::fs::create_dir_all(&workspace).unwrap();
        write_skill(
            &workspace.join(".claude").join("skills"),
            "from-claude",
            "claude-style skill",
            "body",
        );
        write_skill(
            &workspace.join(".codewhale").join("skills"),
            "from-codewhale",
            "codewhale skill",
            "body",
        );
        write_skill(
            &home.join(".agents").join("skills"),
            "from-agents",
            "agents skill",
            "body",
        );
        write_skill(
            &configured_dir,
            "configured-codewhale",
            "configured skill",
            "body",
        );

        let registry = super::discover_for_workspace_and_dir_with_home_and_mode(
            &workspace,
            &configured_dir,
            Some(&home),
            super::SkillDiscoveryMode::CodeWhaleOnly,
        );
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"from-codewhale"));
        assert!(names.contains(&"configured-codewhale"));
        assert!(
            !names.contains(&"from-claude") && !names.contains(&"from-agents"),
            "CodeWhale-only mode must not import cross-tool skills: {names:?}"
        );
    }

    #[test]
    fn codewhale_only_mode_still_honors_explicit_configured_dir() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path().join("workspace");
        let home = tmpdir.path().join("home");
        let configured_dir = tmpdir.path().join("my-skills");
        std::fs::create_dir_all(&workspace).unwrap();
        write_skill(
            &configured_dir,
            "configured-skill",
            "explicit configured skill",
            "body",
        );

        let registry = super::discover_for_workspace_and_dir_with_home_and_mode(
            &workspace,
            &configured_dir,
            Some(&home),
            super::SkillDiscoveryMode::CodeWhaleOnly,
        );
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();

        assert_eq!(names, vec!["configured-skill"]);
    }

    #[test]
    fn codewhale_only_mode_rejects_workspace_codewhale_symlink_escape() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path().join("workspace");
        let home = tmpdir.path().join("home");
        let escape_target = tmpdir.path().join("escape-target");
        std::fs::create_dir_all(workspace.join(".codewhale")).unwrap();
        write_skill(&escape_target, "escaped-skill", "escaped skill", "body");

        let link_path = workspace.join(".codewhale").join("skills");
        if let Err(err) = create_dir_symlink(&escape_target, &link_path) {
            eprintln!("skipping symlink escape assertion: {err}");
            return;
        }

        let registry = super::discover_for_workspace_and_dir_with_home_and_mode(
            &workspace,
            &tmpdir.path().join("missing-configured-skills"),
            Some(&home),
            super::SkillDiscoveryMode::CodeWhaleOnly,
        );

        assert!(
            registry.get("escaped-skill").is_none(),
            "CodeWhale-only mode must not follow workspace .codewhale/skills outside the workspace"
        );
    }

    #[test]
    fn discover_for_workspace_and_dir_merges_workspace_and_configured_sources() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path().join("workspace");
        let home = tmpdir.path().join("home");
        let configured_dir = tmpdir.path().join("configured-skills");
        std::fs::create_dir_all(&workspace).unwrap();
        write_skill(
            &workspace.join(".claude").join("skills"),
            "workspace-skill",
            "workspace visible skill",
            "body",
        );
        write_skill(
            &configured_dir,
            "configured-skill",
            "configured visible skill",
            "body",
        );

        let registry = super::discover_for_workspace_and_dir_with_home(
            &workspace,
            &configured_dir,
            Some(&home),
        );
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"workspace-skill"));
        assert!(names.contains(&"configured-skill"));
    }

    #[test]
    fn explicit_configured_skills_dir_precedes_global_defaults() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path().join("workspace");
        let home = tmpdir.path().join("home");
        let configured_dir = tmpdir.path().join("configured-skills");
        std::fs::create_dir_all(&workspace).unwrap();
        write_skill(
            &home.join(".agents").join("skills"),
            "shared-skill",
            "global skill",
            "global body",
        );
        write_skill(
            &configured_dir,
            "shared-skill",
            "configured skill",
            "configured body",
        );

        let registry = super::discover_for_workspace_and_dir_with_home(
            &workspace,
            &configured_dir,
            Some(&home),
        );
        let skill = registry
            .get("shared-skill")
            .expect("shared skill discovered");

        assert_eq!(skill.description, "configured skill");
    }

    /// Regression for the GitHub issue where users organize skills under
    /// vendor / category subdirectories (e.g. cloned skill repos that
    /// bundle several skills together). The old single-level `read_dir`
    /// only ever surfaced `<root>/<skill>/SKILL.md` and silently ignored
    /// `<root>/<vendor>/<skill>/SKILL.md`.
    #[test]
    fn discover_finds_skills_nested_under_vendor_subdirectory() {
        let tmpdir = TempDir::new().unwrap();
        let root = tmpdir.path().join("skills");

        // Two-level nesting: `<root>/<vendor>/<skill>/SKILL.md`. This
        // matches the `clawhub-skills/clawhub/SKILL.md` layout in the
        // bug report.
        write_skill(
            &root.join("clawhub-skills"),
            "clawhub",
            "claw search",
            "body",
        );
        write_skill(
            &root.join("clawhub-skills"),
            "github",
            "github helpers",
            "body",
        );
        // Three-level nesting: `<root>/<org>/<repo>/<skill>/SKILL.md`.
        write_skill(
            &root.join("pasky").join("chrome-cdp-skill"),
            "chrome-cdp",
            "browser automation",
            "body",
        );
        // Mixed-depth: a flat skill alongside the nested layout still
        // works (this is what the bundled `skill-creator` looks like).
        write_skill(&root, "skill-creator", "make skills", "body");

        let registry = super::SkillRegistry::discover(&root);
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"clawhub"), "vendor/skill missed: {names:?}");
        assert!(names.contains(&"github"), "vendor/skill missed: {names:?}");
        assert!(
            names.contains(&"chrome-cdp"),
            "deeply-nested skill missed: {names:?}"
        );
        assert!(
            names.contains(&"skill-creator"),
            "flat top-level skill must still load: {names:?}"
        );
        assert!(
            registry.warnings().is_empty(),
            "well-formed nested layout should not warn: {:?}",
            registry.warnings()
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn discover_follows_symlinked_skill_directories() {
        let tmpdir = TempDir::new().unwrap();
        let source_root = tmpdir.path().join("claude-skills");
        let skills_root = tmpdir.path().join(".deepseek").join("skills");
        write_skill(&source_root, "agent-browser", "browser automation", "body");
        std::fs::create_dir_all(&skills_root).unwrap();
        let link_path = skills_root.join("agent-browser");

        if let Err(err) = create_dir_symlink(&source_root.join("agent-browser"), &link_path) {
            eprintln!("skipping symlink discovery assertion: {err}");
            return;
        }

        let registry = super::SkillRegistry::discover(&skills_root);
        let skill = registry
            .get("agent-browser")
            .expect("symlinked skill directory should be discovered");
        assert_eq!(skill.description, "browser automation");
        assert_eq!(skill.path, link_path.join("SKILL.md"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn discover_dedupes_symlink_cycles_by_canonical_directory() {
        let tmpdir = TempDir::new().unwrap();
        let root = tmpdir.path().join("skills");
        write_skill(&root, "real-skill", "ok", "body");
        let loop_parent = root.join("vendor");
        std::fs::create_dir_all(&loop_parent).unwrap();

        if let Err(err) = create_dir_symlink(&root, &loop_parent.join("loop")) {
            eprintln!("skipping symlink cycle assertion: {err}");
            return;
        }

        let registry = super::SkillRegistry::discover(&root);
        let matches = registry
            .list()
            .iter()
            .filter(|skill| skill.name == "real-skill")
            .count();
        assert_eq!(
            matches, 1,
            "symlink cycle should not rediscover the same canonical skill directory"
        );
    }

    /// Once a directory is identified as a skill (has `SKILL.md`), the
    /// walker must NOT descend into it: any nested `SKILL.md` would be
    /// a fixture / example bundled with the parent skill, not a
    /// separately-installable one. This mirrors the contract that
    /// `tools::skill::collect_companion_files` already documents
    /// ("nested directory — skipped").
    #[test]
    fn discover_does_not_descend_into_a_skill_directory() {
        let tmpdir = TempDir::new().unwrap();
        let root = tmpdir.path().join("skills");

        // Parent skill: <root>/parent/SKILL.md.
        write_skill(&root, "parent", "outer skill", "outer body");
        // Fixture bundled inside the parent's directory:
        // <root>/parent/examples/inner-fixture/SKILL.md. The walker
        // must NOT descend into <root>/parent/ after finding its
        // SKILL.md, so `inner-fixture` must not be loaded.
        write_skill(
            &root.join("parent").join("examples"),
            "inner-fixture",
            "should not load",
            "fixture body",
        );

        let registry = super::SkillRegistry::discover(&root);
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"parent"));
        assert!(
            !names.contains(&"inner-fixture"),
            "nested SKILL.md inside an existing skill must be ignored: {names:?}"
        );
    }

    /// Hidden subdirectories below the root (e.g. `.git`, `.cache`) must
    /// be skipped so a `skills_dir` that lives inside a checked-out repo
    /// doesn't accidentally load random `SKILL.md`-named fixtures from
    /// the VCS metadata. The root itself is exempt — the user explicitly
    /// pointed `skills_dir` at it.
    #[test]
    fn discover_skips_hidden_subdirectories_below_root() {
        let tmpdir = TempDir::new().unwrap();
        let root = tmpdir.path().join("skills");

        write_skill(&root, "real-skill", "ok", "body");
        // A `<root>/.git/<junk>/SKILL.md` lookalike that mustn't load.
        // `.git` is a direct child of the user-provided root (depth 0
        // of the walk), which is exactly the case the old `depth > 0`
        // gate missed.
        write_skill(&root.join(".git"), "vcs-noise", "should not load", "body");

        let registry = super::SkillRegistry::discover(&root);
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"real-skill"));
        assert!(
            !names.contains(&"vcs-noise"),
            "skills under hidden subdirs must be skipped: {names:?}"
        );
    }

    /// The user explicitly chooses the root, so even a hidden path like
    /// `~/.agents/skills` (the layout in the bug report) must work.
    #[test]
    fn discover_honors_a_hidden_root_directory() {
        let tmpdir = TempDir::new().unwrap();
        let root = tmpdir.path().join(".agents").join("skills");

        // Matches the bug report: skills_dir = "~/.agents/skills"
        // with a skill nested at <root>/custom-skills/git-conventions/SKILL.md.
        write_skill(
            &root.join("custom-skills"),
            "git-conventions",
            "conventions",
            "body",
        );

        let registry = super::SkillRegistry::discover(&root);
        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"git-conventions"),
            "hidden root must still be walked: {names:?}"
        );
    }

    /// Mirrors the qa_pty `skills_menu_shows_local_and_global_skills`
    /// scenario without the PTY harness: a workspace-level skill in
    /// `.agents/skills/` and a global skill in `~/.codewhale/skills/`
    /// must both be discoverable.
    #[test]
    fn discover_finds_both_workspace_and_global_skills() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path().join("workspace");
        let home = tmpdir.path().join("home");
        std::fs::create_dir_all(&workspace).unwrap();

        write_skill(
            &workspace.join(".agents").join("skills"),
            "workspace-beta",
            "Workspace beta skill",
            "body",
        );
        write_skill(
            &home.join(".deepseek").join("skills"),
            "global-alpha",
            "Global alpha skill",
            "body",
        );

        let skills_dir = workspace.join(".agents").join("skills");
        let registry =
            super::discover_for_workspace_and_dir_with_home(&workspace, &skills_dir, Some(&home));

        let names: Vec<&str> = registry.list().iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"workspace-beta"),
            "workspace-beta from .agents/skills must be discovered: {names:?}",
        );
        assert!(
            names.contains(&"global-alpha"),
            "global-alpha from ~/.deepseek/skills must be discovered: {names:?}",
        );
    }

    // ── Block scalar parsing (YAML `>` and `|`) ────────────────

    /// `>` (folded block scalar): subsequent indented lines are folded
    /// into a single line joined by spaces.
    #[test]
    fn parse_skill_folded_block_scalar() {
        let tmpdir = TempDir::new().unwrap();
        create_skill_dir(
            &tmpdir,
            "folded-skill",
            "---\nname: folded-skill\ndescription: >\n  line one chinese\n  line two chinese\n---\nbody",
        );
        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");
        assert!(
            rendered.contains("line one chinese line two chinese"),
            "folded block scalar should join lines with space, got:\n{rendered}"
        );
    }

    /// `|` (literal block scalar): subsequent indented lines preserve
    /// newlines.
    #[test]
    fn parse_skill_literal_block_scalar() {
        let tmpdir = TempDir::new().unwrap();
        create_skill_dir(
            &tmpdir,
            "literal-skill",
            "---\nname: literal-skill\ndescription: |\n  line one\n  line two\n---\nbody",
        );
        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");
        // `truncate_for_prompt` collapses whitespace, so the newlines
        // become spaces. The key assertion is that the content is
        // captured (not just `|`).
        assert!(
            rendered.contains("line one line two"),
            "literal block scalar should preserve content, got:\n{rendered}"
        );
    }

    /// `>-` (folded with strip chomping): same as `>` but trailing
    /// whitespace is stripped.
    #[test]
    fn parse_skill_folded_strip_block_scalar() {
        let tmpdir = TempDir::new().unwrap();
        create_skill_dir(
            &tmpdir,
            "strip-skill",
            "---\nname: strip-skill\ndescription: >-\n  alpha\n  beta\n\n---\nbody",
        );
        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");
        assert!(
            rendered.contains("alpha beta"),
            "strip-chomped folded block should join lines, got:\n{rendered}"
        );
    }

    /// Regression: a single-line description (no block scalar) must
    /// still parse correctly after the parser rewrite.
    #[test]
    fn parse_skill_single_line_description_still_works() {
        let tmpdir = TempDir::new().unwrap();
        create_skill_dir(
            &tmpdir,
            "plain-skill",
            "---\nname: plain-skill\ndescription: A simple description\n---\nbody",
        );
        let rendered =
            crate::skills::render_available_skills_context(&tmpdir.path().join("skills"))
                .expect("skill context");
        assert!(
            rendered.contains("- plain-skill: A simple description"),
            "single-line description should still work, got:\n{rendered}"
        );
    }

    /// Direct unit test on the parsed Skill struct (not through rendering)
    /// so we assert the exact description value.
    #[test]
    fn parse_skill_direct_folded_result() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: test\ndescription: >\n  this is a test\n  used to verify parsing\n---\nbody",
        )
        .expect("should parse");
        assert_eq!(skill.name, "test");
        assert_eq!(skill.description, "this is a test used to verify parsing");
    }

    // ── Chomping behaviour ────────────────────────────────────

    /// `>-` (strip): trailing empty lines are stripped. Paragraph
    /// breaks (empty line between text lines) are still folded to a
    /// single space in a block-scalar join (no newline — the simplified
    /// parser treats intra-block empty lines as paragraph breaks that
    /// become a single space in the folded output).
    #[test]
    fn parse_skill_strip_chomp_strips_trailing_empties() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: >-\n  hello\n  world\n\n\n---\nbody",
        )
        .expect("should parse");
        // Trailing empty lines stripped: no whitespace at end, just folded text.
        assert_eq!(skill.description, "hello world");
    }

    /// `>+` (keep): trailing empty lines are preserved. Each trailing
    /// empty line in the block becomes a newline in the description.
    #[test]
    fn parse_skill_keep_chomp_preserves_trailing_empties() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: >+\n  hello\n  world\n\n\n---\nbody",
        )
        .expect("should parse");
        // Two trailing empty lines should become two newlines.
        assert_eq!(skill.description, "hello world\n\n");
    }

    /// `>` (clip): trailing empty lines exceeding one are clipped.
    /// The result should have at most one trailing newline.
    #[test]
    fn parse_skill_clip_chomp_clips_excess_trailing_empties() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: >\n  hello\n  world\n\n\n---\nbody",
        )
        .expect("should parse");
        // clip: 3 trailing empty lines → at most 1 trailing newline.
        assert_eq!(skill.description, "hello world\n");
    }

    /// `>` with no trailing empty lines: clip should not add anything.
    #[test]
    fn parse_skill_clip_chomp_no_trailing_empties() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: >\n  hello\n  world\n---\nbody",
        )
        .expect("should parse");
        assert_eq!(skill.description, "hello world");
    }

    /// `>` with exactly one trailing empty line: clip keeps it.
    #[test]
    fn parse_skill_clip_chomp_one_trailing_empty() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: >\n  hello\n  world\n\n---\nbody",
        )
        .expect("should parse");
        assert_eq!(skill.description, "hello world\n");
    }

    /// `>-` strip vs `>+` keep: same block content, different
    /// trailing newline handling.
    #[test]
    fn parse_skill_strip_vs_keep_trailing() {
        let content = "---\nname: s\ndescription: >{}\n  hello\n  world\n\n\n---\nbody";
        let strip_skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            &content.replace("{}", "-"),
        )
        .expect("strip parse");
        let keep_skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            &content.replace("{}", "+"),
        )
        .expect("keep parse");
        // strip drops trailing empties; keep preserves them.
        assert_eq!(strip_skill.description, "hello world");
        assert_eq!(keep_skill.description, "hello world\n\n");
    }

    /// `|-` literal strip: trailing newlines are stripped.
    #[test]
    fn parse_skill_literal_strip_strips_trailing_newlines() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: |-\n  line one\n  line two\n\n\n---\nbody",
        )
        .expect("should parse");
        // literal: newlines preserved between non-empty lines.
        // strip: trailing empty lines removed.
        assert_eq!(skill.description, "line one\nline two");
    }

    /// `|+` literal keep: trailing newlines are preserved.
    #[test]
    fn parse_skill_literal_keep_preserves_trailing_newlines() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: |+\n  line one\n  line two\n\n\n---\nbody",
        )
        .expect("should parse");
        // literal: newlines preserved between non-empty lines.
        // keep: trailing empty lines are preserved as newlines.
        assert_eq!(skill.description, "line one\nline two\n\n");
    }

    /// Nested relative indentation is preserved in literal (`|`) block
    /// scalars: only the content-level indent (from the first non-empty
    /// line) is stripped, and any deeper indent stays as-is.
    #[test]
    fn parse_skill_literal_preserves_relative_indentation() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: |\n  Usage:\n    $ deepseek --model auto\n    $ deepseek doctor\n---\nbody",
        )
        .expect("should parse");
        assert_eq!(
            skill.description,
            "Usage:\n  $ deepseek --model auto\n  $ deepseek doctor"
        );
    }

    /// Folded (`>`) block scalars also preserve relative indentation
    /// within lines (the extra spaces survive the fold).
    #[test]
    fn parse_skill_folded_preserves_relative_indentation() {
        let skill = super::SkillRegistry::parse_skill(
            std::path::Path::new(""),
            "---\nname: s\ndescription: >\n  See also:\n    the config file\n    the env var\n---\nbody",
        )
        .expect("should parse");
        assert_eq!(
            skill.description,
            "See also:   the config file   the env var"
        );
    }
}
