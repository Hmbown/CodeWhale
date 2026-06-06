use crate::commands::CommandResult;
use crate::commands::groups::skills::support::{
    discover_visible_skills, installer_settings, needs_approval_message, network_denied_message,
    path_or_default, render_skill_warnings, run_async,
};
use crate::skills::install::{self, InstallOutcome, InstallSource, UpdateResult};
use crate::tui::app::App;
use crate::tui::history::HistoryCell;

pub(crate) fn run_skill(app: &mut App, args: Option<&str>) -> CommandResult {
    let raw = match args {
        Some(n) => n.trim(),
        None => {
            return CommandResult::error(
                "Usage: /skill <name>\n\nSubcommands:\n  /skill install <github:owner/repo|https://...|<registry-name>>\n  /skill update <name>\n  /skill uninstall <name>\n  /skill trust <name>",
            );
        }
    };

    let mut iter = raw.splitn(2, char::is_whitespace);
    let head = iter.next().unwrap_or("").trim();
    let rest = iter.next().unwrap_or("").trim();
    match head {
        "install" => return install_skill(app, rest),
        "update" => return update_skill(app, rest),
        "uninstall" => return uninstall_skill(app, rest),
        "trust" => return trust_skill(app, rest),
        _ => {}
    }

    activate_skill(app, raw)
}

/// Try to run a skill by exact slash-command name.
///
/// This is used by the command dispatcher after static command lookup misses.
pub(crate) fn run_skill_by_name(
    app: &mut App,
    name: &str,
    _arg: Option<&str>,
) -> Option<CommandResult> {
    let registry = discover_visible_skills(app);
    if registry.get(name).is_some() {
        Some(activate_skill(app, name))
    } else {
        None
    }
}

fn activate_skill(app: &mut App, name: &str) -> CommandResult {
    let name = if name == "new" { "skill-creator" } else { name };
    let registry = discover_visible_skills(app);

    if let Some(skill) = registry.get(name) {
        let instruction = format!(
            "You are now using a skill. Follow these instructions:\n\n# Skill: {}\n\n{}\n\n---\n\nNow respond to the user's request following the above skill instructions.",
            skill.name, skill.body
        );

        app.add_message(HistoryCell::System {
            content: format!("Activated skill: {}\n\n{}", skill.name, skill.description),
        });
        app.active_skill = Some(instruction);

        CommandResult::message(format!(
            "Skill '{}' activated.\n\nDescription: {}\n\nType your request and the skill instructions will be applied.",
            skill.name, skill.description
        ))
    } else {
        let available: Vec<String> = registry.list().iter().map(|s| s.name.clone()).collect();
        let warnings = render_skill_warnings(&registry);

        if available.is_empty() {
            CommandResult::error(format!(
                "Skill '{name}' not found. No skills installed.\n\nUse /skills to see how to add skills.{warnings}"
            ))
        } else {
            CommandResult::error(format!(
                "Skill '{}' not found.\n\nAvailable skills: {}{}",
                name,
                available.join(", "),
                warnings
            ))
        }
    }
}

fn install_skill(app: &mut App, spec: &str) -> CommandResult {
    if spec.is_empty() {
        return CommandResult::error(
            "Usage: /skill install <github:owner/repo|https://...|<registry-name>>",
        );
    }
    let source = match InstallSource::parse(spec) {
        Ok(s) => s,
        Err(err) => return CommandResult::error(format!("Invalid install source: {err}")),
    };
    let skills_dir = app.skills_dir.clone();
    let (network, max_size, registry_url) = installer_settings(app);

    let outcome = run_async(async move {
        install::install_with_registry(
            source,
            &skills_dir,
            max_size,
            &network,
            false,
            &registry_url,
        )
        .await
    });

    match outcome {
        Ok(InstallOutcome::Installed(installed)) => {
            app.refresh_skill_cache();
            let path_str = path_or_default(&installed.path);
            CommandResult::message(format!(
                "Installed skill '{}' from {}.\nLocation: {}\n\nRun /skills to see it in the list.",
                installed.name, spec, path_str
            ))
        }
        Ok(InstallOutcome::NeedsApproval(host)) => {
            CommandResult::error(needs_approval_message(&host))
        }
        Ok(InstallOutcome::NetworkDenied(host)) => {
            CommandResult::error(network_denied_message(&host))
        }
        Err(err) => CommandResult::error(format!("Install failed: {err:#}")),
    }
}

fn update_skill(app: &mut App, name: &str) -> CommandResult {
    if name.is_empty() {
        return CommandResult::error("Usage: /skill update <name>");
    }
    let skills_dir = app.skills_dir.clone();
    let (network, max_size, registry_url) = installer_settings(app);
    let owned_name = name.to_string();
    let outcome = run_async(async move {
        install::update_with_registry(&owned_name, &skills_dir, max_size, &network, &registry_url)
            .await
    });

    match outcome {
        Ok(UpdateResult::NoChange) => {
            CommandResult::message(format!("Skill '{name}': no upstream change."))
        }
        Ok(UpdateResult::Updated(installed)) => CommandResult::message(format!(
            "Skill '{}' updated. Location: {}",
            installed.name,
            path_or_default(&installed.path)
        )),
        Ok(UpdateResult::NeedsApproval(host)) => {
            CommandResult::error(needs_approval_message(&host))
        }
        Ok(UpdateResult::NetworkDenied(host)) => {
            CommandResult::error(network_denied_message(&host))
        }
        Err(err) => CommandResult::error(format!("Update failed: {err:#}")),
    }
}

fn uninstall_skill(app: &mut App, name: &str) -> CommandResult {
    if name.is_empty() {
        return CommandResult::error("Usage: /skill uninstall <name>");
    }
    match install::uninstall(name, &app.skills_dir) {
        Ok(()) => {
            app.refresh_skill_cache();
            CommandResult::message(format!("Removed skill '{name}'."))
        }
        Err(err) => CommandResult::error(format!("Uninstall failed: {err:#}")),
    }
}

fn trust_skill(app: &mut App, name: &str) -> CommandResult {
    if name.is_empty() {
        return CommandResult::error("Usage: /skill trust <name>");
    }
    match install::trust(name, &app.skills_dir) {
        Ok(()) => CommandResult::message(format!(
            "Marked skill '{name}' as trusted. Tools that consult the .trusted marker may now invoke its scripts/."
        )),
        Err(err) => CommandResult::error(format!("Trust failed: {err:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::skills::test_support::{
        IsolatedHome, create_skill_dir, create_test_app_with_tmpdir,
    };
    use tempfile::TempDir;

    #[test]
    fn test_skill_subcommand_dispatch_install_usage() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = run_skill(&mut app, Some("install"));

        let msg = result.message.unwrap();
        assert!(msg.contains("/skill install"), "got: {msg}");
    }

    #[test]
    fn test_skill_subcommand_dispatch_uninstall_missing() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = run_skill(&mut app, Some("uninstall absent-skill"));

        let msg = result.message.unwrap();
        assert!(msg.contains("not installed"), "got: {msg}");
    }

    #[test]
    fn test_run_skill_without_name() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = run_skill(&mut app, None);

        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Usage: /skill"));
    }

    #[test]
    fn test_run_skill_not_found() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = run_skill(&mut app, Some("nonexistent"));

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn test_run_skill_activates() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "test-skill",
            "---\nname: test-skill\ndescription: A test skill\n---\nDo something special",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = run_skill(&mut app, Some("test-skill"));

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Skill 'test-skill' activated"));
        assert!(msg.contains("A test skill"));
        assert!(app.active_skill.is_some());
        assert!(!app.history.is_empty());
    }

    #[test]
    fn run_skill_by_name_activates_existing_skill() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "direct-skill",
            "---\nname: direct-skill\ndescription: Direct skill\n---\nDo direct work",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = run_skill_by_name(&mut app, "direct-skill", None);

        assert!(result.is_some());
        assert!(app.active_skill.is_some());
        assert!(!app.history.is_empty());
    }
}
