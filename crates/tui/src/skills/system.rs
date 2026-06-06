//! System-skill installer: bundles skills and auto-installs them on first launch.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Bumped whenever any bundled skill's body changes so existing installs
/// upgrade in place. Newly *added* skills install on any version regardless,
/// driven by the recorded known-set in the marker (see [`parse_marker`]).
const BUNDLED_SKILL_VERSION: &str = "2";

const SKILL_CREATOR_BODY: &str = include_str!("../../assets/skills/skill-creator/SKILL.md");
const MEMORY_CONSOLIDATE_BODY: &str =
    include_str!("../../assets/skills/memory-consolidate/SKILL.md");

/// The skills auto-installed on first launch, as `(dir_name, SKILL.md body)`.
const BUNDLED_SKILLS: &[(&str, &str)] = &[
    ("skill-creator", SKILL_CREATOR_BODY),
    ("memory-consolidate", MEMORY_CONSOLIDATE_BODY),
];

const MARKER_NAME: &str = ".system-installed-version";

/// Parse the marker file into `(version, known_skill_names)`.
///
/// Format is line-based: the first line is the bundle version, each subsequent
/// line is the name of a skill that has been installed at some point. Legacy
/// markers (a bare version string with no skill list) predate every skill but
/// `skill-creator`, so they default the known-set to `{skill-creator}` — that
/// keeps "user deleted it, don't recreate" working across the format change.
fn parse_marker(raw: &str) -> (Option<String>, BTreeSet<String>) {
    let mut lines = raw.lines();
    let version = lines.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut known: BTreeSet<String> = lines
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if version.is_some() && known.is_empty() {
        // Legacy single-line marker.
        known.insert("skill-creator".to_string());
    }
    (version, known)
}

fn render_marker(version: &str, known: &BTreeSet<String>) -> String {
    let mut out = String::from(version);
    for name in known {
        out.push('\n');
        out.push_str(name);
    }
    out
}

/// Install bundled system skills into `skills_dir`.
///
/// Per skill, install when any of:
/// - **Fresh install** — no marker at all (first launch).
/// - **Newly bundled** — the skill isn't in the marker's known-set, so the user
///   has never seen it; lay it down even on an existing install.
/// - **Version bump** — the bundle version changed and the skill dir still
///   exists (upgrade in place).
///
/// Skip when the version is current, or when a previously-known skill's dir is
/// gone (the user deleted it on purpose — respect that).
///
/// Idempotent: a second call with nothing changed writes nothing. Errors are
/// filesystem I/O errors; the caller should log but not abort startup.
pub fn install_system_skills(skills_dir: &Path) -> std::io::Result<()> {
    let marker = skills_dir.join(MARKER_NAME);
    let (recorded_version, mut known) = fs::read_to_string(&marker)
        .ok()
        .map(|raw| parse_marker(&raw))
        .unwrap_or((None, BTreeSet::new()));

    let fresh = recorded_version.is_none();
    let version_changed = recorded_version.as_deref() != Some(BUNDLED_SKILL_VERSION);

    let mut wrote_any = false;
    for (name, body) in BUNDLED_SKILLS {
        let target_dir = skills_dir.join(name);
        let target_file = target_dir.join("SKILL.md");
        let dir_exists = target_dir.exists();
        let is_known = known.contains(*name);

        let should_install = if fresh {
            true
        } else if !is_known {
            true
        } else {
            version_changed && dir_exists
        };

        if should_install {
            fs::create_dir_all(&target_dir)?;
            fs::write(&target_file, body)?;
            wrote_any = true;
        }
    }

    // Every bundled skill is "known" after this run (installed or
    // deletion-respected), so future runs keep honoring user deletions.
    let all_names: BTreeSet<String> = BUNDLED_SKILLS.iter().map(|(n, _)| n.to_string()).collect();
    let known_grew = !all_names.is_subset(&known);
    known.extend(all_names);

    if fresh || version_changed || wrote_any || known_grew {
        fs::create_dir_all(skills_dir)?;
        fs::write(&marker, render_marker(BUNDLED_SKILL_VERSION, &known))?;
    }
    Ok(())
}

/// Remove all bundled system skills and the version marker.
///
/// Intended for tests and `deepseek setup --clean`.  Ignores missing files.
#[allow(dead_code)]
pub fn uninstall_system_skills(skills_dir: &Path) -> std::io::Result<()> {
    for (name, _) in BUNDLED_SKILLS {
        let target_dir = skills_dir.join(name);
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }
    }
    let marker = skills_dir.join(MARKER_NAME);
    if marker.exists() {
        fs::remove_file(&marker)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn skill_file(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join("skill-creator").join("SKILL.md")
    }

    fn marker_file(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join(".system-installed-version")
    }

    /// The version recorded in the marker (its first line).
    fn marker_version(tmp: &TempDir) -> String {
        let raw = fs::read_to_string(marker_file(tmp)).unwrap();
        raw.lines().next().unwrap_or_default().trim().to_string()
    }

    // ── fresh install ─────────────────────────────────────────────────────────

    #[test]
    fn fresh_install_creates_skill_and_marker() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        assert!(skill_file(&tmp).exists(), "SKILL.md should be created");
        assert!(marker_file(&tmp).exists(), "marker should be created");
        assert_eq!(marker_version(&tmp), BUNDLED_SKILL_VERSION);
    }

    // ── idempotence ───────────────────────────────────────────────────────────

    #[test]
    fn calling_twice_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        // Overwrite SKILL.md with sentinel to detect an undesired second write.
        fs::write(skill_file(&tmp), "sentinel").unwrap();

        install_system_skills(tmp.path()).unwrap();

        let contents = fs::read_to_string(skill_file(&tmp)).unwrap();
        assert_eq!(
            contents, "sentinel",
            "second install should not overwrite SKILL.md when version is current"
        );
    }

    // ── user deleted the directory ────────────────────────────────────────────

    #[test]
    fn user_deleted_dir_is_not_recreated() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        // Simulate user deliberately removing the skill directory.
        fs::remove_dir_all(tmp.path().join("skill-creator")).unwrap();

        // Re-launch must NOT recreate the directory.
        install_system_skills(tmp.path()).unwrap();

        assert!(
            !skill_file(&tmp).exists(),
            "skill-creator must not be recreated after user deleted it"
        );
    }

    // ── version bump re-installs ──────────────────────────────────────────────

    #[test]
    fn outdated_marker_triggers_reinstall() {
        let tmp = TempDir::new().unwrap();

        // Simulate a previous install at a lower version.
        let skill_dir = tmp.path().join("skill-creator");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "old content").unwrap();
        fs::write(marker_file(&tmp), "0").unwrap(); // older than BUNDLED_SKILL_VERSION

        install_system_skills(tmp.path()).unwrap();

        let contents = fs::read_to_string(skill_file(&tmp)).unwrap();
        assert_ne!(
            contents, "old content",
            "outdated skill should be overwritten on version bump"
        );
        assert_eq!(
            contents, SKILL_CREATOR_BODY,
            "re-installed file must match the bundled body"
        );

        assert_eq!(
            marker_version(&tmp),
            BUNDLED_SKILL_VERSION,
            "marker should be updated"
        );
    }

    // ── uninstall ─────────────────────────────────────────────────────────────

    #[test]
    fn uninstall_removes_skill_and_marker() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();
        uninstall_system_skills(tmp.path()).unwrap();

        assert!(!skill_file(&tmp).exists(), "SKILL.md should be removed");
        assert!(!marker_file(&tmp).exists(), "marker should be removed");
    }

    #[test]
    fn uninstall_on_clean_dir_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        // Must not panic or error.
        uninstall_system_skills(tmp.path()).unwrap();
    }

    // ── multi-skill bundle ────────────────────────────────────────────────────

    #[test]
    fn fresh_install_lays_down_every_bundled_skill() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        for (name, _) in BUNDLED_SKILLS {
            assert!(
                tmp.path().join(name).join("SKILL.md").exists(),
                "bundled skill `{name}` should be installed on fresh launch"
            );
        }
        // Marker records the version plus every bundled skill name.
        let (_, known) = parse_marker(&fs::read_to_string(marker_file(&tmp)).unwrap());
        for (name, _) in BUNDLED_SKILLS {
            assert!(known.contains(*name), "`{name}` should be recorded as known");
        }
    }

    #[test]
    fn newly_bundled_skill_installs_on_upgrade_from_legacy_marker() {
        let tmp = TempDir::new().unwrap();

        // Simulate an old install that only knew skill-creator: legacy
        // single-line marker + just the skill-creator dir present.
        let sc_dir = tmp.path().join("skill-creator");
        fs::create_dir_all(&sc_dir).unwrap();
        fs::write(sc_dir.join("SKILL.md"), "old skill-creator").unwrap();
        fs::write(marker_file(&tmp), "1").unwrap();

        install_system_skills(tmp.path()).unwrap();

        // The skill the user never saw is laid down...
        assert!(
            tmp.path().join("memory-consolidate").join("SKILL.md").exists(),
            "a newly-bundled skill must install on upgrade"
        );
        // ...and the version-bumped existing skill is refreshed.
        assert_eq!(
            fs::read_to_string(sc_dir.join("SKILL.md")).unwrap(),
            SKILL_CREATOR_BODY,
            "version bump should refresh the existing skill body"
        );
    }

    #[test]
    fn deleted_bundled_skill_is_not_recreated_at_current_version() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        // User deletes one bundled skill; marker still records it as known.
        fs::remove_dir_all(tmp.path().join("memory-consolidate")).unwrap();

        install_system_skills(tmp.path()).unwrap();

        assert!(
            !tmp.path().join("memory-consolidate").join("SKILL.md").exists(),
            "a deleted bundled skill must not be recreated at the current version"
        );
    }
}
