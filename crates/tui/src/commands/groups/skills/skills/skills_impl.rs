use std::fmt::Write;

use crate::commands::CommandResult;
use crate::commands::groups::skills::support::{
    discover_visible_skills, format_registry_error, installer_settings, needs_approval_message,
    network_denied_message, render_skill_warnings, run_async,
};
use crate::skills::install::{self, RegistryFetchResult, SkillSyncOutcome, SyncResult};
use crate::tui::app::App;

/// List all available skills. Pass `--remote` or `remote` to fetch the
/// curated registry. Pass `sync` to pull the registry index and download all
/// skills to the local cache.
pub(crate) fn list_skills(app: &mut App, arg: Option<&str>) -> CommandResult {
    let mut prefix: Option<String> = None;
    if let Some(arg) = arg {
        let trimmed = arg.trim();
        if trimmed == "--remote" || trimmed == "remote" {
            return list_remote_skills(app);
        }
        if trimmed == "sync" || trimmed == "--sync" {
            return sync_skills(app);
        }
        if !trimmed.is_empty() {
            if trimmed.starts_with('-') || trimmed.split_whitespace().count() > 1 {
                return CommandResult::error("Usage: /skills [--remote|sync|<name-prefix>]");
            }
            prefix = Some(trimmed.to_ascii_lowercase());
        }
    }

    let skills_dir = app.skills_dir.clone();
    let registry = discover_visible_skills(app);
    let warnings = render_skill_warnings(&registry);

    if registry.is_empty() {
        let msg = format!(
            "No skills found.\n\n\
             Skills location: {}\n\n\
             To add skills, create directories with SKILL.md files:\n  \
             {}/my-skill/SKILL.md\n\n\
             Format:\n  \
             ---\n  \
             name: my-skill\n  \
             description: What this skill does\n  \
             allowed-tools: read_file, list_dir\n  \
             ---\n\n  \
             <instructions here>{warnings}",
            skills_dir.display(),
            skills_dir.display()
        );
        return CommandResult::message(msg);
    }

    let filtered: Vec<&crate::skills::Skill> = if let Some(p) = prefix.as_deref() {
        registry
            .list()
            .iter()
            .filter(|s| s.name.to_ascii_lowercase().starts_with(p))
            .collect()
    } else {
        registry.list().iter().collect()
    };

    if filtered.is_empty() {
        let p = prefix.as_deref().unwrap_or("");
        return CommandResult::message(format!(
            "No skills match prefix `{p}` (out of {} available).\n\nRun /skills to see them all.{warnings}",
            registry.len()
        ));
    }

    let mut output = if let Some(p) = prefix.as_deref() {
        format!(
            "Available skills matching `{p}` ({} of {}):\n",
            filtered.len(),
            registry.len()
        )
    } else {
        format!("Available skills ({}):\n", registry.len())
    };
    output.push_str("-----------------------------\n");

    if prefix.is_some() {
        for (idx, skill) in filtered.iter().enumerate() {
            if idx > 0 {
                output.push('\n');
            }
            let _ = writeln!(output, "  /{} - {}", skill.name, skill.description);
        }
    } else {
        let (user_skills, bundled_skills): (
            Vec<&&crate::skills::Skill>,
            Vec<&&crate::skills::Skill>,
        ) = filtered
            .iter()
            .partition(|s| !crate::skills::is_bundled_skill_name(&s.name));

        if !user_skills.is_empty() {
            let _ = writeln!(output, "Your skills ({}):", user_skills.len());
            for skill in &user_skills {
                let _ = writeln!(output, "  /{} - {}", skill.name, skill.description);
            }
            if !bundled_skills.is_empty() {
                output.push('\n');
            }
        }

        if !bundled_skills.is_empty() {
            let _ = writeln!(output, "Built-in skills ({}):", bundled_skills.len());
            if user_skills.is_empty() {
                for skill in &bundled_skills {
                    let _ = writeln!(output, "  /{} - {}", skill.name, skill.description);
                }
            } else {
                let names: Vec<String> = bundled_skills
                    .iter()
                    .map(|s| format!("/{}", s.name))
                    .collect();
                output.push_str("  ");
                output.push_str(&names.join(", "));
                output.push('\n');
                output.push_str("  (run /skills <name> for details on a built-in)\n");
            }
        }
    }

    let _ = write!(
        output,
        "\nUse /skill <name> to run a skill\nSkills location: {}{}",
        skills_dir.display(),
        warnings
    );

    CommandResult::message(output)
}

fn list_remote_skills(app: &mut App) -> CommandResult {
    let (network, _max_size, registry_url) = installer_settings(app);
    let registry = run_async(async move { install::fetch_registry(&network, &registry_url).await });
    match registry {
        Ok(RegistryFetchResult::Loaded(doc)) => {
            if doc.skills.is_empty() {
                return CommandResult::message("Registry is empty.");
            }
            let mut out = format!("Available remote skills ({}):\n", doc.skills.len());
            out.push_str("-----------------------------\n");
            for (name, entry) in &doc.skills {
                let _ = writeln!(
                    out,
                    "  {name} - {} (source: {})",
                    entry.description.clone().unwrap_or_default(),
                    entry.source
                );
            }
            let _ = write!(out, "\nInstall with: /skill install <name>");
            CommandResult::message(out)
        }
        Ok(RegistryFetchResult::NeedsApproval(host)) => {
            CommandResult::error(needs_approval_message(&host))
        }
        Ok(RegistryFetchResult::Denied(host)) => {
            CommandResult::error(network_denied_message(&host))
        }
        Err(err) => CommandResult::error(format_registry_error("Failed to fetch registry", &err)),
    }
}

fn sync_skills(app: &mut App) -> CommandResult {
    let (network, max_size, registry_url) = installer_settings(app);
    let cache_dir = install::default_cache_skills_dir();

    let result = run_async(async move {
        install::sync_registry(&network, &registry_url, &cache_dir, max_size).await
    });

    match result {
        Ok(SyncResult::RegistryDenied(host)) => CommandResult::error(network_denied_message(&host)),
        Ok(SyncResult::RegistryNeedsApproval(host)) => {
            CommandResult::error(needs_approval_message(&host))
        }
        Ok(SyncResult::Done { outcomes }) => {
            let total = outcomes.len();
            let mut downloaded = 0usize;
            let mut fresh = 0usize;
            let mut failed = 0usize;
            let mut out = String::from("Registry sync complete.\n\n");

            for outcome in &outcomes {
                match outcome {
                    SkillSyncOutcome::Downloaded { name, path } => {
                        downloaded += 1;
                        let _ = writeln!(out, "  [+] {name} - downloaded to {}", path.display());
                    }
                    SkillSyncOutcome::Fresh { name } => {
                        fresh += 1;
                        let _ = writeln!(out, "  [=] {name} - already up to date");
                    }
                    SkillSyncOutcome::Failed { name, reason } => {
                        failed += 1;
                        let _ = writeln!(out, "  [!] {name} - failed: {reason}");
                    }
                    SkillSyncOutcome::Denied { name, host } => {
                        failed += 1;
                        let _ = writeln!(out, "  [!] {name} - network denied ({host})");
                    }
                    SkillSyncOutcome::NeedsApproval { name, host } => {
                        failed += 1;
                        let _ = writeln!(
                            out,
                            "  [?] {name} - needs approval for {host} (run `/network allow {host}` then retry)"
                        );
                    }
                }
            }

            let _ = write!(
                out,
                "\n{total} skill(s) processed: {downloaded} downloaded, {fresh} up-to-date, {failed} failed."
            );

            CommandResult::message(out)
        }
        Err(err) => CommandResult::error(format_registry_error("Sync failed", &err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::skills::test_support::{
        IsolatedHome, create_skill_dir, create_test_app_with_tmpdir,
    };
    use tempfile::TempDir;

    #[cfg_attr(
        target_os = "windows",
        ignore = "dirs crate uses Win32 API, cannot override"
    )]
    #[test]
    fn test_list_skills_empty_directory() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = list_skills(&mut app, None);

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("No skills found"));
        assert!(msg.contains("Skills location:"));
    }

    #[test]
    fn test_list_skills_with_skills() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "test-skill",
            "---\nname: test-skill\ndescription: A test skill\n---\nDo something",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = list_skills(&mut app, None);

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Available skills"));
        assert!(msg.contains("/test-skill"));
    }

    #[cfg_attr(
        target_os = "windows",
        ignore = "dirs crate uses Win32 API, cannot override"
    )]
    #[test]
    fn test_list_skills_filters_by_name_prefix() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: First\n---\nbody",
        );
        create_skill_dir(
            &tmpdir,
            "alphabet-helper",
            "---\nname: alphabet-helper\ndescription: Helper\n---\nbody",
        );
        create_skill_dir(
            &tmpdir,
            "beta-skill",
            "---\nname: beta-skill\ndescription: Second\n---\nbody",
        );

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, Some("alph"));
        let msg = result.message.expect("filter result has message");

        assert!(msg.contains("/alpha-skill"));
        assert!(msg.contains("/alphabet-helper"));
        assert!(
            !msg.contains("/beta-skill"),
            "beta-skill must be filtered out"
        );
        assert!(
            msg.contains("matching `alph`") && msg.contains("2 of 3"),
            "header should show count + total, got: {msg}"
        );
    }

    #[test]
    fn test_list_skills_filter_is_case_insensitive() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: First\n---\nbody",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = list_skills(&mut app, Some("ALPH"));

        let msg = result.message.expect("case-insensitive filter has message");
        assert!(msg.contains("/alpha-skill"));
    }

    #[test]
    fn test_list_skills_filter_with_zero_matches_says_so() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: First\n---\nbody",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = list_skills(&mut app, Some("nonexistent"));

        let msg = result.message.expect("zero-match filter still has message");
        assert!(msg.contains("No skills match prefix `nonexistent`"));
        assert!(msg.contains("Run /skills"));
    }

    #[test]
    fn test_list_skills_rejects_flag_like_prefix() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = list_skills(&mut app, Some("--bogus"));

        assert!(
            result.is_error,
            "expected usage error for --bogus, got: {result:?}"
        );
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("name-prefix")),
            "expected --bogus error message to mention name-prefix, got: {result:?}"
        );
    }

    #[test]
    fn test_list_skills_renders_user_skills_under_your_skills_section() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: First skill\n---\nDo alpha work",
        );
        create_skill_dir(
            &tmpdir,
            "beta-skill",
            "---\nname: beta-skill\ndescription: Second skill\n---\nDo beta work",
        );

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, None);
        let msg = result.message.unwrap();

        let section = msg
            .find("Your skills")
            .expect("user skills section header missing");
        let alpha = msg.find("/alpha-skill").expect("alpha skill should render");
        let beta = msg.find("/beta-skill").expect("beta skill should render");
        assert!(
            alpha > section,
            "alpha-skill should follow the header: {msg}"
        );
        assert!(beta > section, "beta-skill should follow the header: {msg}");
        assert!(msg.contains("/alpha-skill - First skill"), "got: {msg}");
        assert!(msg.contains("/beta-skill - Second skill"), "got: {msg}");
    }

    #[test]
    fn test_list_skills_merges_workspace_and_configured_dirs() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let workspace_skill_dir = tmpdir
            .path()
            .join(".agents")
            .join("skills")
            .join("workspace-skill");
        std::fs::create_dir_all(&workspace_skill_dir).unwrap();
        std::fs::write(
            workspace_skill_dir.join("SKILL.md"),
            "---\nname: workspace-skill\ndescription: Workspace skill\n---\nDo workspace work",
        )
        .unwrap();
        create_skill_dir(
            &tmpdir,
            "configured-skill",
            "---\nname: configured-skill\ndescription: Configured skill\n---\nDo configured work",
        );

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, None);
        let msg = result.message.unwrap();

        assert!(msg.contains("/workspace-skill"), "got: {msg}");
        assert!(msg.contains("/configured-skill"), "got: {msg}");
    }
}
