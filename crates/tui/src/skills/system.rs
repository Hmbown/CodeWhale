//! System-skill installer: bundles first-party skills and auto-installs them
//! on first launch.

use std::fs;
use std::path::Path;

const BUNDLED_SKILL_VERSION: &str = "3";
const SKILL_CREATOR_BODY: &str = include_str!("../../assets/skills/skill-creator/SKILL.md");
const DELEGATE_BODY: &str = include_str!("../../assets/skills/delegate/SKILL.md");
const V4_BEST_PRACTICES_BODY: &str = include_str!("../../assets/skills/v4-best-practices/SKILL.md");
const PLUGIN_CREATOR_BODY: &str = include_str!("../../assets/skills/plugin-creator/SKILL.md");
const SKILL_INSTALLER_BODY: &str = include_str!("../../assets/skills/skill-installer/SKILL.md");
const MCP_BUILDER_BODY: &str = include_str!("../../assets/skills/mcp-builder/SKILL.md");
const DOCUMENTS_BODY: &str = include_str!("../../assets/skills/documents/SKILL.md");
const PRESENTATIONS_BODY: &str = include_str!("../../assets/skills/presentations/SKILL.md");
const SPREADSHEETS_BODY: &str = include_str!("../../assets/skills/spreadsheets/SKILL.md");
const PDF_BODY: &str = include_str!("../../assets/skills/pdf/SKILL.md");
const FEISHU_BODY: &str = include_str!("../../assets/skills/feishu/SKILL.md");

struct BundledSkill {
    name: &'static str,
    body: &'static str,
    introduced_in: u32,
}

const BUNDLED_SKILLS: &[BundledSkill] = &[
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
        name: "v4-best-practices",
        body: V4_BEST_PRACTICES_BODY,
        introduced_in: 3,
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
        name: "documents",
        body: DOCUMENTS_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "presentations",
        body: PRESENTATIONS_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "spreadsheets",
        body: SPREADSHEETS_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "pdf",
        body: PDF_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "feishu",
        body: FEISHU_BODY,
        introduced_in: 3,
    },
];

/// Whether a skill name matches one of the bundled first-party skills.
///
/// Used by `/skills` to distinguish user-created skills (which should be
/// surfaced prominently) from the always-installed bundle (which can be
/// rendered compactly when many skills are present).
#[must_use]
pub fn is_bundled_skill_name(name: &str) -> bool {
    BUNDLED_SKILLS.iter().any(|s| s.name == name)
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

    let installed_version = fs::read_to_string(&marker)
        .ok()
        .map(|s| s.trim().to_string());

    let mut changed = false;
    for skill in BUNDLED_SKILLS {
        changed |= install_one(skills_dir, skill, installed_version.as_deref())?;
    }

    if changed {
        fs::create_dir_all(skills_dir)?;
        fs::write(&marker, BUNDLED_SKILL_VERSION)?;
    }
    Ok(())
}

/// Remove all system skills and the version marker.
///
/// Intended for tests and `deepseek setup --clean`.  Ignores missing files.
#[allow(dead_code)]
pub fn uninstall_system_skills(skills_dir: &Path) -> std::io::Result<()> {
    let marker = skills_dir.join(".system-installed-version");

    for skill in BUNDLED_SKILLS {
        let dir = skills_dir.join(skill.name);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
    }
    if marker.exists() {
        fs::remove_file(&marker)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn skill_file(tmp: &TempDir, name: &str) -> std::path::PathBuf {
        tmp.path().join(name).join("SKILL.md")
    }

    fn skill_dir(tmp: &TempDir, name: &str) -> std::path::PathBuf {
        tmp.path().join(name)
    }

    fn marker_file(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join(".system-installed-version")
    }

    // ── fresh install ─────────────────────────────────────────────────────────

    #[test]
    fn fresh_install_creates_bundled_skills_and_marker() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            assert!(
                skill_file(&tmp, skill.name).exists(),
                "{} SKILL.md should be created",
                skill.name
            );
        }
        assert!(marker_file(&tmp).exists(), "marker should be created");

        let ver = fs::read_to_string(marker_file(&tmp)).unwrap();
        assert_eq!(ver.trim(), BUNDLED_SKILL_VERSION);
    }

    #[test]
    fn fresh_install_skills_parse_for_discovery() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        let registry = crate::skills::SkillRegistry::discover(tmp.path());
        assert!(
            registry.warnings().is_empty(),
            "bundled skills should parse cleanly: {:?}",
            registry.warnings()
        );

        for skill in BUNDLED_SKILLS {
            let parsed = registry
                .get(skill.name)
                .unwrap_or_else(|| panic!("{} should be discoverable", skill.name));
            assert!(
                !parsed.description.is_empty(),
                "{} should include model-visible description",
                skill.name
            );
        }
    }

    // ── idempotence ───────────────────────────────────────────────────────────

    #[test]
    fn calling_twice_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            fs::write(
                skill_file(&tmp, skill.name),
                format!("{}-sentinel", skill.name),
            )
            .unwrap();
        }

        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            let body = fs::read_to_string(skill_file(&tmp, skill.name)).unwrap();
            assert_eq!(
                body,
                format!("{}-sentinel", skill.name),
                "second install should not overwrite {}",
                skill.name
            );
        }
    }

    // ── user deleted a directory ──────────────────────────────────────────────

    #[test]
    fn user_deleted_dir_is_not_recreated() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        // Simulate user deliberately removing one skill directory.
        fs::remove_dir_all(skill_dir(&tmp, "delegate")).unwrap();

        // Re-launch must NOT recreate the deleted directory.
        install_system_skills(tmp.path()).unwrap();

        assert!(
            !skill_file(&tmp, "delegate").exists(),
            "delegate must not be recreated after user deleted it"
        );
        assert!(
            skill_file(&tmp, "skill-creator").exists(),
            "skill-creator should still be present (not deleted by user)"
        );
    }

    #[test]
    fn user_deleted_all_dirs_are_not_recreated() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            fs::remove_dir_all(skill_dir(&tmp, skill.name)).unwrap();
        }

        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            assert!(
                !skill_file(&tmp, skill.name).exists(),
                "{} must not be recreated after user deletion",
                skill.name
            );
        }
    }

    // ── version bump re-installs ──────────────────────────────────────────────

    #[test]
    fn outdated_marker_triggers_reinstall_of_existing_skills() {
        let tmp = TempDir::new().unwrap();

        // Simulate a previous install at a lower version with all skills present.
        for skill in BUNDLED_SKILLS {
            fs::create_dir_all(skill_dir(&tmp, skill.name)).unwrap();
            fs::write(skill_file(&tmp, skill.name), format!("old-{}", skill.name)).unwrap();
        }
        fs::write(marker_file(&tmp), "0").unwrap(); // older than BUNDLED_SKILL_VERSION

        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            let body = fs::read_to_string(skill_file(&tmp, skill.name)).unwrap();
            assert_ne!(
                body,
                format!("old-{}", skill.name),
                "outdated {} should be overwritten",
                skill.name
            );
            assert_eq!(body, skill.body);
        }

        let ver = fs::read_to_string(marker_file(&tmp)).unwrap();
        assert_eq!(ver.trim(), BUNDLED_SKILL_VERSION);
    }

    // ── partial previous install ─────────────────────────────────────────────

    #[test]
    fn version_bump_adds_skills_introduced_after_marker() {
        let tmp = TempDir::new().unwrap();

        // Simulate state from v2: v1/v2 skills exist, v3 skills do not.
        for skill in BUNDLED_SKILLS
            .iter()
            .filter(|skill| skill.introduced_in <= 2)
        {
            fs::create_dir_all(skill_dir(&tmp, skill.name)).unwrap();
            fs::write(skill_file(&tmp, skill.name), format!("old-{}", skill.name)).unwrap();
        }
        fs::write(marker_file(&tmp), "2").unwrap();

        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            assert_eq!(
                fs::read_to_string(skill_file(&tmp, skill.name)).unwrap(),
                skill.body,
                "{} should be installed or refreshed",
                skill.name
            );
        }
    }

    #[test]
    fn version_bump_respects_deleted_existing_skill_while_adding_new_skill() {
        let tmp = TempDir::new().unwrap();

        // Simulate v2 where older bundled skills had been deliberately removed
        // before v3 introduced more system skills.
        fs::write(marker_file(&tmp), "2").unwrap();

        install_system_skills(tmp.path()).unwrap();

        assert!(
            !skill_file(&tmp, "skill-creator").exists(),
            "version bump should not recreate deleted skill-creator"
        );
        assert!(
            !skill_file(&tmp, "delegate").exists(),
            "version bump should not recreate deleted delegate"
        );
        for skill in BUNDLED_SKILLS
            .iter()
            .filter(|skill| skill.introduced_in > 2)
        {
            assert!(
                skill_file(&tmp, skill.name).exists(),
                "version bump should install newly introduced {}",
                skill.name
            );
        }
        let ver = fs::read_to_string(marker_file(&tmp)).unwrap();
        assert_eq!(ver.trim(), BUNDLED_SKILL_VERSION);
    }

    // ── uninstall ─────────────────────────────────────────────────────────────

    #[test]
    fn uninstall_removes_bundled_skills_and_marker() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();
        uninstall_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            assert!(
                !skill_file(&tmp, skill.name).exists(),
                "{} should be removed",
                skill.name
            );
        }
        assert!(!marker_file(&tmp).exists(), "marker should be removed");
    }

    #[test]
    fn uninstall_on_clean_dir_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        // Must not panic or error.
        uninstall_system_skills(tmp.path()).unwrap();
    }
}
