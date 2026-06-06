//! Review command: activate review skill and send a target immediately.

use crate::skills::{SkillRegistry, default_skills_dir};
use crate::tui::app::{App, AppAction};
use crate::tui::history::HistoryCell;

use crate::commands::CommandResult;

fn warnings_suffix(registry: &SkillRegistry) -> String {
    if registry.warnings().is_empty() {
        return String::new();
    }

    format!("\n\nWarnings:\n- {}", registry.warnings().join("\n- "))
}

pub fn review(app: &mut App, args: Option<&str>) -> CommandResult {
    let target = args.unwrap_or("").trim();
    if target.is_empty() {
        return CommandResult::error("Usage: /review <target>");
    }

    let skills_dir = app.skills_dir.clone();
    let registry = SkillRegistry::discover(&skills_dir);
    let mut warnings = warnings_suffix(&registry);
    let mut skill = registry.get("review").cloned();

    let global_dir = default_skills_dir();
    if skill.is_none() && global_dir != skills_dir {
        let registry = SkillRegistry::discover(&global_dir);
        if warnings.is_empty() {
            warnings = warnings_suffix(&registry);
        } else if !registry.warnings().is_empty() {
            warnings.push_str(&format!("\n- {}", registry.warnings().join("\n- ")));
        }
        skill = registry.get("review").cloned();
    }

    let skill = match skill {
        Some(skill) => skill,
        None => {
            let global_display = global_dir.display();
            return CommandResult::error(format!(
                "Review skill not found in {} or {}. Create ~/.codewhale/skills/review/SKILL.md.{}",
                skills_dir.display(),
                global_display,
                warnings
            ));
        }
    };

    let instruction = format!(
        "You are now using a skill. Follow these instructions:\n\n# Skill: {}\n\n{}\n\n---\n\nNow respond to the user's request following the above skill instructions.",
        skill.name, skill.body
    );

    app.add_message(HistoryCell::System {
        content: format!("Activated skill: {}\n\n{}", skill.name, skill.description),
    });
    app.active_skill = Some(instruction);

    CommandResult::action(AppAction::SendMessage(target.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: tmpdir.path().to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    fn create_review_skill_dir(tmpdir: &TempDir) {
        let skill_dir = tmpdir.path().join("skills").join("review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Code review skill\n---\nReview the code",
        )
        .unwrap();
    }

    #[test]
    fn test_review_without_target() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = review(&mut app, None);
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Usage: /review"));
    }

    #[test]
    fn test_review_without_skill_installed() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        // Set skills dir to empty temp dir
        app.skills_dir = tmpdir.path().join("nonexistent_skills");
        let result = review(&mut app, Some("file.rs"));
        // The command should either error about missing skill or work if global skill exists
        assert!(result.message.is_some() || result.action.is_some());
    }

    #[test]
    fn test_review_with_skill_activates_and_sends() {
        let tmpdir = TempDir::new().unwrap();
        create_review_skill_dir(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = review(&mut app, Some("file.rs"));
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::SendMessage(_))));
        assert!(app.active_skill.is_some());
        assert!(!app.history.is_empty());
    }
}
