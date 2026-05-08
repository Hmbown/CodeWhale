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
pub use system::install_system_skills;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use std::collections::HashMap;

use crate::logging;

const MAX_SKILL_DESCRIPTION_CHARS: usize = 512;
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

/// Returns `~/.agents/skills` for agentskills.io compatibility (#741).
#[must_use]
pub fn agents_skills_dir() -> PathBuf {
    dirs::home_dir().map_or_else(
        || PathBuf::from("/tmp/agents/skills"),
        |p| p.join(".agents").join("skills"),
    )
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
    /// Discover skills from the given directory.
    #[must_use]
    pub fn discover(dir: &Path) -> Self {
        let mut registry = Self::default();
        if !dir.exists() {
            return registry;
        }

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type()
                    && ft.is_dir()
                {
                    let skill_path = entry.path().join("SKILL.md");
                    match fs::read_to_string(&skill_path) {
                        Ok(content) => match Self::parse_skill(&skill_path, &content) {
                            Ok(mut skill) => {
                                skill.path = skill_path.clone();
                                registry.skills.push(skill);
                            }
                            Err(reason) => registry.push_warning(format!(
                                "Failed to parse {}: {reason}",
                                skill_path.display()
                            )),
                        },
                        Err(err) if skill_path.exists() => {
                            registry.push_warning(format!(
                                "Failed to read {}: {err}",
                                skill_path.display()
                            ));
                        }
                        Err(_) => {}
                    }
                }
            }
        } else {
            registry.push_warning(format!("Failed to read skills directory {}", dir.display()));
        }
        registry
    }

    fn push_warning(&mut self, warning: String) {
        logging::warn(&warning);
        self.warnings.push(warning);
    }

    fn parse_skill(_path: &Path, content: &str) -> std::result::Result<Skill, String> {
        let trimmed = content.trim_start();
        let (frontmatter, body) = if trimmed.starts_with("---") {
            let start = content
                .find("---")
                .ok_or_else(|| "missing frontmatter opening delimiter".to_string())?;
            let rest = &content[start + 3..];
            let end = rest
                .find("---")
                .ok_or_else(|| "missing frontmatter closing delimiter".to_string())?;
            (&rest[..end], &rest[end + 3..])
        } else {
            return Err("missing frontmatter opening delimiter '---'".to_string());
        };

        let mut metadata = HashMap::new();
        for raw in frontmatter.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                metadata.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }

        let name = metadata
            .get("name")
            .filter(|name| !name.is_empty())
            .cloned()
            .ok_or_else(|| "missing required frontmatter field: name".to_string())?;

        let description = metadata.get("description").cloned().unwrap_or_default();

        let body = body.trim().to_string();

        Ok(Skill {
            name,
            description,
            body,
            // Filled in by `discover` after parse succeeds; default to an
            // empty path so direct constructors (e.g. tests) compile.
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
/// `<workspace>/skills` → `~/.agents/skills` → [`default_skills_dir`] (`~/.deepseek/skills`).
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
    let home_agents = agents_skills_dir();
    if home_agents.exists() {
        return home_agents;
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
/// 5. `~/.agents/skills` — agentskills.io / open AI agents convention (#741).
/// 6. [`default_skills_dir`] — global, user-installed.
///
/// Only directories that exist on disk are returned — callers don't
/// need to filter further. Returns an empty vec when nothing is
/// installed (the system-prompt skills block is then suppressed).
#[must_use]
pub fn skills_directories(workspace: &Path) -> Vec<PathBuf> {
    let candidates = [
        workspace.join(".agents").join("skills"),
        workspace.join("skills"),
        workspace.join(".opencode").join("skills"),
        workspace.join(".claude").join("skills"),
        agents_skills_dir(),
        default_skills_dir(),
    ];
    let mut out = Vec::new();
    for path in candidates {
        if path.is_dir() && !out.iter().any(|p: &PathBuf| p == &path) {
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

    let mut skills = registry.list().to_vec();
    skills.sort_by(|a, b| a.name.cmp(&b.name));

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
    for skill in skills {
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
- Discovery: The list above is the skills available in this session. Skill bodies live on disk at the listed paths.\n\
- Trigger rules: If the user names a skill (with `$SkillName`, `/skill <name>`, or plain text) OR the task clearly matches a skill description above, use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.\n\
- Missing/blocked: If a named skill is missing or its `SKILL.md` cannot be read, say so briefly and continue with the best fallback.\n\
- Progressive disclosure: After deciding to use a skill, read only that skill's `SKILL.md`. When it references relative paths such as `scripts/foo.py`, resolve them relative to the skill directory.\n\
- Context hygiene: Load only the specific referenced files needed for the task. Avoid bulk-loading unrelated skill resources.\n\
- Safety: Do not execute scripts from a community skill unless the user explicitly asks or the skill has been trusted for script use.\n",
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

    fn write_skill(dir: &std::path::Path, name: &str, description: &str, body: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
        )
        .unwrap();
    }

    #[test]
    fn skills_directories_returns_existing_dirs_in_precedence_order() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path();

        // Create three of the four candidate dirs (skip `.opencode`).
        std::fs::create_dir_all(workspace.join(".agents").join("skills")).unwrap();
        std::fs::create_dir_all(workspace.join("skills")).unwrap();
        std::fs::create_dir_all(workspace.join(".claude").join("skills")).unwrap();

        let dirs = super::skills_directories(workspace);
        // We don't assert on the global default position because it's
        // host-dependent (may not exist on the test machine).
        let mut idx = 0;
        let agents = workspace.join(".agents").join("skills");
        let local = workspace.join("skills");
        let claude = workspace.join(".claude").join("skills");

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
}
