//! System-skill installer: bundles first-party skills and auto-installs them
//! on first launch.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Bundled catalog generation for the default CodeWhale skill pack (#4691).
///
/// Generation 7 adds the explicit-only `help` router (#4698 parity slice).
/// Generation 8 adds the explicit-only `contributor-onboarding` path
/// requested by @JayBeest (#4227).
/// Generation 9 adds the `handoff` workflow skill (baton-pass for
/// continuous operate-mode operations).
/// Generation 10 adds the bundled `mcp-discovery` skill (Registry-first
/// tool selection).
const BUNDLED_SKILL_VERSION: &str = "10";

// ── system & extension (meta) ───────────────────────────────────────────────
const SKILL_CREATOR_BODY: &str = include_str!("../../assets/skills/skill-creator/SKILL.md");
const DELEGATE_BODY: &str = include_str!("../../assets/skills/delegate/SKILL.md");
const PLUGIN_CREATOR_BODY: &str = include_str!("../../assets/skills/plugin-creator/SKILL.md");
const SKILL_INSTALLER_BODY: &str = include_str!("../../assets/skills/skill-installer/SKILL.md");
const MCP_BUILDER_BODY: &str = include_str!("../../assets/skills/mcp-builder/SKILL.md");
const FLEET_MANAGER_BODY: &str = include_str!("../../assets/skills/fleet-manager/SKILL.md");
const HELP_BODY: &str = include_str!("../../assets/skills/help/SKILL.md");

// ── end-user workflows ──────────────────────────────────────────────────────
const HANDOFF_BODY: &str = include_str!("../../assets/skills/handoff/SKILL.md");
const BEST_OF_N_BODY: &str = include_str!("../../assets/skills/best-of-n/SKILL.md");
const INTERVIEW_BODY: &str = include_str!("../../assets/skills/interview/SKILL.md");
const PLAN_BODY: &str = include_str!("../../assets/skills/plan/SKILL.md");
const IMPLEMENT_BODY: &str = include_str!("../../assets/skills/implement/SKILL.md");
const DEBUG_BODY: &str = include_str!("../../assets/skills/debug/SKILL.md");
const TEST_BODY: &str = include_str!("../../assets/skills/test/SKILL.md");
const REVIEW_BODY: &str = include_str!("../../assets/skills/review/SKILL.md");
const SECURITY_REVIEW_BODY: &str = include_str!("../../assets/skills/security-review/SKILL.md");
const SIMPLIFY_BODY: &str = include_str!("../../assets/skills/simplify/SKILL.md");
const VERIFY_BODY: &str = include_str!("../../assets/skills/verify/SKILL.md");
const RESEARCH_BODY: &str = include_str!("../../assets/skills/research/SKILL.md");
const FRONTEND_DESIGN_BODY: &str = include_str!("../../assets/skills/frontend-design/SKILL.md");
const WEBAPP_TESTING_BODY: &str = include_str!("../../assets/skills/webapp-testing/SKILL.md");
const DOCUMENT_BODY: &str = include_str!("../../assets/skills/document/SKILL.md");
const DATAVIZ_BODY: &str = include_str!("../../assets/skills/dataviz/SKILL.md");
const DOCX_BODY: &str = include_str!("../../assets/skills/docx/SKILL.md");
const PDF_BODY: &str = include_str!("../../assets/skills/pdf/SKILL.md");
const PPTX_BODY: &str = include_str!("../../assets/skills/pptx/SKILL.md");
const XLSX_BODY: &str = include_str!("../../assets/skills/xlsx/SKILL.md");
const DOCUMENTS_ALIAS_BODY: &str = include_str!("../../assets/skills/documents/SKILL.md");
const PRESENTATIONS_ALIAS_BODY: &str = include_str!("../../assets/skills/presentations/SKILL.md");
const SPREADSHEETS_ALIAS_BODY: &str = include_str!("../../assets/skills/spreadsheets/SKILL.md");

// ── power / explicit-only ───────────────────────────────────────────────────
const BATCH_BODY: &str = include_str!("../../assets/skills/batch/SKILL.md");
const DEPENDENCY_UPDATE_BODY: &str = include_str!("../../assets/skills/dependency-update/SKILL.md");
const RELEASE_BODY: &str = include_str!("../../assets/skills/release/SKILL.md");
const CONTRIBUTOR_ONBOARDING_BODY: &str =
    include_str!("../../assets/skills/contributor-onboarding/SKILL.md");

// Optional integration (not auto-installed for every user): Feishu body kept for
// digest/migration helpers only.
const FEISHU_BODY: &str = include_str!("../../assets/skills/feishu/SKILL.md");
const MCP_DISCOVERY_BODY: &str = include_str!("../../assets/skills/mcp-discovery/SKILL.md");

// Legacy v4 body retained solely for digest-based safe retirement (#4691).
const V4_BEST_PRACTICES_BODY: &str = include_str!("../../assets/skills/v4-best-practices/SKILL.md");

struct BundledSkill {
    name: &'static str,
    body: &'static str,
    introduced_in: u32,
}

/// Skills auto-installed for every user on fresh install / upgrade.
const BUNDLED_SKILLS: &[BundledSkill] = &[
    // System & extension
    BundledSkill {
        name: "skill-creator",
        body: SKILL_CREATOR_BODY,
        introduced_in: 1,
    },
    BundledSkill {
        name: "delegate",
        body: DELEGATE_BODY,
        introduced_in: 2,
    },
    BundledSkill {
        name: "plugin-creator",
        body: PLUGIN_CREATOR_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "skill-installer",
        body: SKILL_INSTALLER_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "mcp-builder",
        body: MCP_BUILDER_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "fleet-manager",
        body: FLEET_MANAGER_BODY,
        introduced_in: 4,
    },
    BundledSkill {
        name: "help",
        body: HELP_BODY,
        introduced_in: 7,
    },
    // End-user workflows
    BundledSkill {
        name: "handoff",
        body: HANDOFF_BODY,
        introduced_in: 9,
    },
    BundledSkill {
        name: "best-of-n",
        body: BEST_OF_N_BODY,
        introduced_in: 6,
    },
    BundledSkill {
        name: "interview",
        body: INTERVIEW_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "plan",
        body: PLAN_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "implement",
        body: IMPLEMENT_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "debug",
        body: DEBUG_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "test",
        body: TEST_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "review",
        body: REVIEW_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "security-review",
        body: SECURITY_REVIEW_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "simplify",
        body: SIMPLIFY_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "verify",
        body: VERIFY_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "research",
        body: RESEARCH_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "frontend-design",
        body: FRONTEND_DESIGN_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "webapp-testing",
        body: WEBAPP_TESTING_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "document",
        body: DOCUMENT_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "dataviz",
        body: DATAVIZ_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "docx",
        body: DOCX_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "pdf",
        body: PDF_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "pptx",
        body: PPTX_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "xlsx",
        body: XLSX_BODY,
        introduced_in: 5,
    },
    // Compatibility aliases for pre-v5 artifact names
    BundledSkill {
        name: "documents",
        body: DOCUMENTS_ALIAS_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "presentations",
        body: PRESENTATIONS_ALIAS_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "spreadsheets",
        body: SPREADSHEETS_ALIAS_BODY,
        introduced_in: 3,
    },
    // Power / explicit-only
    BundledSkill {
        name: "batch",
        body: BATCH_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "dependency-update",
        body: DEPENDENCY_UPDATE_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "release",
        body: RELEASE_BODY,
        introduced_in: 5,
    },
    BundledSkill {
        name: "contributor-onboarding",
        body: CONTRIBUTOR_ONBOARDING_BODY,
        introduced_in: 8,
    },
    BundledSkill {
        name: "mcp-discovery",
        body: MCP_DISCOVERY_BODY,
        introduced_in: 10,
    },
];

/// Product-facing grouping for the bundled catalog.
///
/// User and compatible skills remain outside these two buckets. The grouping
/// is deliberately attached to the shipped catalog instead of inferred from
/// arbitrary community metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BundledSkillTier {
    CoreAgentic,
    FormatTooling,
}

impl BundledSkillTier {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::CoreAgentic => "core",
            Self::FormatTooling => "tools",
        }
    }
}

/// Return the curated tier for a bundled skill name.
#[must_use]
pub fn bundled_skill_tier(name: &str) -> Option<BundledSkillTier> {
    if !is_bundled_skill_name(name) {
        return None;
    }
    let tier = match name {
        "skill-creator" | "plugin-creator" | "skill-installer" | "mcp-builder" | "help"
        | "frontend-design" | "webapp-testing" | "document" | "dataviz" | "docx" | "pdf"
        | "pptx" | "xlsx" | "documents" | "presentations" | "spreadsheets" => {
            BundledSkillTier::FormatTooling
        }
        _ => BundledSkillTier::CoreAgentic,
    };
    Some(tier)
}

/// Canonical names of every skill in the shipped starter pack, in bundle order.
///
/// Exposed so the catalog fixture matrix (#4698) can assert a *bijection*
/// between the checked-in fixture and the real bundle: a skill added or removed
/// without updating the fixture fails the build rather than silently changing
/// what every user gets installed.
#[must_use]
#[cfg(test)]
pub fn bundled_skill_names() -> Vec<&'static str> {
    BUNDLED_SKILLS.iter().map(|skill| skill.name).collect()
}

/// The shipped generation marker written to `.system-installed-version`.
#[must_use]
#[cfg(test)]
pub fn bundled_skill_generation() -> &'static str {
    BUNDLED_SKILL_VERSION
}

/// Legacy v4-best-practices body digest helper (not in BUNDLED_SKILLS).
fn v4_best_practices_body() -> &'static str {
    V4_BEST_PRACTICES_BODY
}

fn feishu_body() -> &'static str {
    FEISHU_BODY
}

/// Whether a skill name matches one of the bundled first-party skills.
///
/// Used by `/skills` to distinguish user-created skills (which should be
/// surfaced prominently) from the always-installed bundle (which can be
/// rendered compactly when many skills are present).
///
/// Prefer [`is_exact_bundled_skill`] when classifying audit rows — name-only
/// matches can collide with user overrides of the same command name.
#[must_use]
pub fn is_bundled_skill_name(name: &str) -> bool {
    BUNDLED_SKILLS.iter().any(|s| s.name == name)
}

/// True when `name` is a bundled skill **and** `skill_md_content` exactly
/// matches the shipped asset body (byte-for-byte).
///
/// Used by the skill audit inventory so a user-edited copy of a bundled name
/// is not misclassified as built-in.
#[must_use]
pub fn is_exact_bundled_skill(name: &str, skill_md_content: &str) -> bool {
    BUNDLED_SKILLS
        .iter()
        .any(|s| s.name == name && s.body == skill_md_content)
}

/// Attempt to install a single bundled skill into `skills_dir`.
///
/// Returns `true` if installation occurred (fresh install or version bump).
fn install_one(
    skills_dir: &Path,
    skill: &BundledSkill,
    installed_version: Option<&str>,
) -> std::io::Result<bool> {
    let target_dir = skills_dir.join(skill.name);
    let target_file = target_dir.join("SKILL.md");
    let dir_exists = target_dir.exists();
    let installed_number = installed_version.and_then(|value| value.parse::<u32>().ok());

    let should_install = match (installed_version, installed_number, dir_exists) {
        // Fresh install: neither marker nor directory.
        (None, _, false) => true,
        // Newly bundled skill: add it for older system-skill installs.
        (Some(_), Some(version), _) if version < skill.introduced_in => true,
        // Version bump for an existing skill: refresh only if the user has not
        // intentionally deleted that skill directory.
        (Some(version), _, true) if version != BUNDLED_SKILL_VERSION => true,
        // Every other case: current install, user-deleted dir, or pre-existing
        // user-owned skill without our marker.
        _ => false,
    };

    if should_install {
        // Never overwrite a user-modified copy that no longer matches a known
        // shipped body (#4691 non-destructive upgrade table).
        if target_file.exists() {
            let existing = fs::read_to_string(&target_file).unwrap_or_default();
            if !existing.is_empty() && existing != skill.body {
                // Preserve user/compatible-root content; skip replace-by-name.
                return Ok(false);
            }
        }
        fs::create_dir_all(&target_dir)?;
        fs::write(&target_file, skill.body)?;
    }
    Ok(should_install)
}

/// Install bundled system skills into `skills_dir`.
///
/// Behaviour:
/// - Fresh install (no marker, no dir): installs every bundled skill, then
///   writes the version marker.
/// - Version bump (marker present with older version): re-installs any existing
///   bundled skill and installs newly introduced bundled skills.
/// - User deleted a skill dir while marker still present at same version: leaves
///   it gone.
/// - Idempotent: calling twice with no changes is a no-op.
///
/// Errors are I/O errors from the filesystem; the caller should log them but not
/// abort startup.
pub fn install_system_skills(skills_dir: &Path) -> std::io::Result<()> {
    let marker = skills_dir.join(".system-installed-version");

    // A marker can be left behind as an invalid file (or even as a directory
    // after an interrupted/manual install). Treat it as an untrusted marker,
    // but still repair it after reconciling the bundled skills. This keeps
    // user-edited skill bodies intact while allowing missing skills to be
    // restored and future upgrades to be versioned again.
    let (installed_version, repair_marker) = match fs::read_to_string(&marker) {
        Ok(contents) => match contents.trim().parse::<u32>() {
            Ok(_) => (Some(contents.trim().to_string()), false),
            Err(_) => (None, true),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, false),
        Err(_) => (None, true),
    };

    let mut changed = false;
    for skill in BUNDLED_SKILLS {
        changed |= install_one(skills_dir, skill, installed_version.as_deref())?;
    }

    // Safe retirement: remove only an unchanged CodeWhale-owned v4-best-practices.
    changed |= retire_unchanged_v4_best_practices(skills_dir)?;

    // Feishu is optional: do not install for every user. If an older bundle
    // installed an exact shipped copy, leave it; never delete by name alone.
    let _ = feishu_body();

    if changed || repair_marker {
        fs::create_dir_all(skills_dir)?;
        if marker.exists() && !marker.is_file() {
            if marker.is_dir() {
                fs::remove_dir_all(&marker)?;
            } else {
                fs::remove_file(&marker)?;
            }
        }
        write_marker_atomically(&marker, BUNDLED_SKILL_VERSION)?;
    }
    Ok(())
}

/// Delete `v4-best-practices` only when the installed SKILL.md exactly matches
/// the last shipped bundled body (byte-for-byte). Modified or user-owned copies
/// are preserved.
fn retire_unchanged_v4_best_practices(skills_dir: &Path) -> std::io::Result<bool> {
    let dir = skills_dir.join("v4-best-practices");
    let file = dir.join("SKILL.md");
    if !file.exists() {
        return Ok(false);
    }
    let existing = fs::read_to_string(&file)?;
    if existing != v4_best_practices_body() {
        return Ok(false);
    }
    fs::remove_dir_all(&dir)?;
    Ok(true)
}

fn write_marker_atomically(marker: &Path, version: &str) -> std::io::Result<()> {
    let parent = marker
        .parent()
        .expect("skill version marker should have a parent directory");
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(version.as_bytes())?;
    temporary.as_file().sync_all()?;
    // `rename` atomically replaces a file on Unix. Windows refuses to replace
    // an existing destination, so remove only this reserved marker first.
    #[cfg(windows)]
    if marker.exists() {
        fs::remove_file(marker)?;
    }
    fs::rename(temporary.path(), marker)
}

#[cfg(test)]
mod tests;
