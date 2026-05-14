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

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};

use crate::logging;

const MAX_SKILL_DESCRIPTION_CHARS: usize = 280;
const MAX_AVAILABLE_SKILLS_CHARS: usize = 12_000;

// === Defaults ===

#[allow(dead_code)]
#[must_use]
pub fn default_skills_dir() -> PathBuf {
    dirs::home_dir().map_or_else(
        || PathBuf::from("/tmp/deepseek/skills"),
        |p| p.join(".deepseek").join("skills"),
    )
}

/// Global agentskills.io-compatible skills directory (`~/.agents/skills`).
#[must_use]
pub fn agents_global_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".agents").join("skills"))
}

/// Global Claude-compatible skills directory (`~/.claude/skills`). The
/// SKILL.md frontmatter convention is shared across the broader Claude
/// ecosystem, so picking up the global path lets users inherit skills
/// they already installed for other Claude-compatible tools without
/// re-authoring them in DeepSeek's native layout (#902).
#[allow(dead_code)]
#[must_use]
pub fn claude_global_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".claude").join("skills"))
}

// === Types ===

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
            for raw in frontmatter.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once(':') {
                    let value = value.trim();
                    let unquoted = if (value.starts_with('"')
                        && value.ends_with('"')
                        && value.len() >= 2)
                        || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
                    {
                        &value[1..value.len() - 1]
                    } else {
                        value
                    };
                    metadata.insert(key.trim().to_ascii_lowercase(), unquoted.to_string());
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

/// Render a compact model-visible skills block.
///
/// The full `SKILL.md` body is intentionally not included here. This mirrors
/// Resolve the active skills directory given a workspace, mirroring the
/// hierarchy `App::new` walks: `<workspace>/.agents/skills` →
/// `<workspace>/skills` → [`agents_global_skills_dir`] (`~/.agents/skills`,
/// when present) → [`default_skills_dir`] (`~/.deepseek/skills`).
/// Returns the first directory that exists, or the global default
/// (which itself falls back to `/tmp/deepseek/skills` if the user
/// has no home directory).
///
/// Kept for callers that want a single canonical directory (e.g.
/// "where do I install a new skill?"). For session-time discovery
/// that should pick up cross-tool skill folders too, use
/// [`skills_directories`] / [`discover_in_workspace`] (#432).
#[must_use]
#[allow(dead_code)] // Intentionally kept for the "single canonical install dir" surface; live callers use discover_in_workspace.
pub fn resolve_skills_dir(workspace: &Path) -> PathBuf {
    let agents = workspace.join(".agents").join("skills");
    if agents.exists() {
        return agents;
    }
    let local = workspace.join("skills");
    if local.exists() {
        return local;
    }
    if let Some(global_agents) = agents_global_skills_dir()
        && global_agents.exists()
    {
        return global_agents;
    }
    default_skills_dir()
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
/// 6. [`agents_global_skills_dir`] — agentskills.io global.
/// 7. [`claude_global_skills_dir`] — Claude-ecosystem global (#902).
/// 8. [`default_skills_dir`] — DeepSeek global, user-installed.
///
/// Only directories that exist on disk are returned — callers don't
/// need to filter further. Returns an empty vec when nothing is
/// installed (the system-prompt skills block is then suppressed).
#[must_use]
pub fn skills_directories(workspace: &Path) -> Vec<PathBuf> {
    let home = dirs::home_dir();
    skills_directories_with_home(workspace, home.as_deref())
}

fn skills_directories_with_home(workspace: &Path, home_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = vec![
        workspace.join(".agents").join("skills"),
        workspace.join("skills"),
        workspace.join(".opencode").join("skills"),
        workspace.join(".claude").join("skills"),
        workspace.join(".cursor").join("skills"),
    ];
    if let Some(home) = home_dir {
        candidates.push(home.join(".agents").join("skills"));
        candidates.push(home.join(".claude").join("skills"));
        candidates.push(home.join(".deepseek").join("skills"));
    } else {
        candidates.push(PathBuf::from("/tmp/deepseek/skills"));
    }
    existing_skill_dirs(candidates)
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
    let mut merged = SkillRegistry::default();
    for dir in skills_directories(workspace) {
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
/// directory. Workspace/global directories keep their normal precedence; a
/// custom configured directory is appended when it is outside that set.
#[must_use]
pub fn discover_for_workspace_and_dir(workspace: &Path, skills_dir: &Path) -> SkillRegistry {
    let dirs = skills_directories(workspace);
    discover_for_workspace_dirs_and_dir(dirs, skills_dir)
}

fn discover_for_workspace_dirs_and_dir(mut dirs: Vec<PathBuf>, skills_dir: &Path) -> SkillRegistry {
    if skills_dir.is_dir() && !dirs.iter().any(|p| p == skills_dir) {
        dirs.push(skills_dir.to_path_buf());
    }

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
fn discover_for_workspace_and_dir_with_home(
    workspace: &Path,
    skills_dir: &Path,
    home_dir: Option<&Path>,
) -> SkillRegistry {
    let dirs = skills_directories_with_home(workspace, home_dir);
    discover_for_workspace_dirs_and_dir(dirs, skills_dir)
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

/// Codex's progressive-disclosure contract: the model sees skill names,
/// descriptions, and paths up front, then opens the specific `SKILL.md` only
/// when a skill is relevant.
///
/// Single-directory variant — use
/// [`render_available_skills_context_for_workspace`] when scanning
/// a workspace for cross-tool skill folders (#432).
#[must_use]
pub fn render_available_skills_context(skills_dir: &Path) -> Option<String> {
    let registry = SkillRegistry::discover(skills_dir);
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

// === CLI Helpers ===

#[allow(dead_code)] // CLI utility for future use
pub fn list(skills_dir: &Path) -> Result<()> {
    if !skills_dir.exists() {
        println!("No skills directory found at {}", skills_dir.display());
        return Ok(());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(skills_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            entries.push(entry.file_name().to_string_lossy().to_string());
        }
    }

    if entries.is_empty() {
        println!("No skills found in {}", skills_dir.display());
        return Ok(());
    }

    entries.sort();
    for entry in entries {
        println!("{entry}");
    }
    Ok(())
}

#[allow(dead_code)] // CLI utility for future use
pub fn show(skills_dir: &Path, name: &str) -> Result<()> {
    let path = skills_dir.join(name).join("SKILL.md");
    let contents =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    println!("{contents}");
    Ok(())
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
    fn claude_global_skills_dir_returns_home_relative_path() {
        // Smoke test for the #902 helper. We don't assert the exact path
        // because dirs::home_dir() is host-dependent; we just pin the
        // suffix shape so a future refactor can't silently rename it.
        let path = super::claude_global_skills_dir().expect("home dir resolves on test host");
        assert!(path.ends_with(".claude/skills") || path.ends_with(r".claude\skills"));
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
    /// `.agents/skills/` and a global skill in `~/.deepseek/skills/`
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
}
