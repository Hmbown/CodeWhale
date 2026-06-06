//! Review command: activate review skill and send a target immediately.

use crate::skills::{SkillRegistry, default_skills_dir};
use crate::tui::app::{App, AppAction};
use crate::tui::history::HistoryCell;

use super::CommandResult;

fn warnings_suffix(registry: &SkillRegistry) -> String {
    if registry.warnings().is_empty() {
        return String::new();
    }

    format!("\n\nWarnings:\n- {}", registry.warnings().join("\n- "))
}

/// Review effort tier (Claude Code's `/code-review <level>` analog). Controls
/// how broadly the review hunts and whether it surfaces uncertain findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEffort {
    Low,
    Medium,
    High,
    Max,
}

impl ReviewEffort {
    fn from_token(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            "max" | "ultra" => Some(Self::Max),
            _ => None,
        }
    }

    /// Directive appended to the review instruction, shaping breadth and the
    /// confidence bar for what gets reported.
    fn directive(self) -> &'static str {
        match self {
            Self::Low => "Review effort: low — report only the few highest-confidence, clearly correct findings. Skip style nits and speculative issues.",
            Self::Medium => "Review effort: medium — report high-confidence correctness bugs and clear cleanups. Keep findings focused; avoid speculation.",
            Self::High => "Review effort: high — broaden coverage across the diff and adjacent code paths; you may include lower-confidence findings, clearly flagged as uncertain.",
            Self::Max => "Review effort: max — be exhaustive. Trace edge cases, error paths, and concurrency; surface even low-confidence findings, each labelled with your confidence.",
        }
    }
}

/// Split `/review` args into an optional leading effort token and the target.
/// Accepts `--effort high <target>`, `--effort=high <target>`, or a bare
/// leading `high <target>`. Defaults to Medium when no level is given.
fn parse_effort_and_target(args: &str) -> (ReviewEffort, &str) {
    let args = args.trim();
    // `--effort <level> <target>` or `--effort=<level> <target>`
    if let Some(rest) = args.strip_prefix("--effort") {
        let rest = rest.trim_start_matches('=').trim_start();
        let (level_tok, target) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if let Some(effort) = ReviewEffort::from_token(level_tok) {
            return (effort, target.trim());
        }
    }
    // Bare leading level: `high <target>`
    if let Some((first, rest)) = args.split_once(char::is_whitespace)
        && let Some(effort) = ReviewEffort::from_token(first)
    {
        return (effort, rest.trim());
    }
    (ReviewEffort::Medium, args)
}

pub fn review(app: &mut App, args: Option<&str>) -> CommandResult {
    let (effort, target) = parse_effort_and_target(args.unwrap_or(""));
    if target.is_empty() {
        return CommandResult::error(
            "Usage: /review [--effort low|medium|high|max] <target>",
        );
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
                "Review skill not found in {} or {}. Create ~/.deepseek/skills/review/SKILL.md.{}",
                skills_dir.display(),
                global_display,
                warnings
            ));
        }
    };

    let instruction = format!(
        "You are now using a skill. Follow these instructions:\n\n# Skill: {}\n\n{}\n\n{}\n\n---\n\nNow respond to the user's request following the above skill instructions.",
        skill.name,
        skill.body,
        effort.directive()
    );

    app.add_message(HistoryCell::System {
        content: format!(
            "Activated skill: {} ({:?} effort)\n\n{}",
            skill.name, effort, skill.description
        ),
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

    #[test]
    fn parse_effort_defaults_to_medium_and_keeps_target() {
        let (effort, target) = parse_effort_and_target("src/lib.rs");
        assert_eq!(effort, ReviewEffort::Medium);
        assert_eq!(target, "src/lib.rs");
    }

    #[test]
    fn parse_effort_reads_bare_leading_level() {
        let (effort, target) = parse_effort_and_target("high src/lib.rs");
        assert_eq!(effort, ReviewEffort::High);
        assert_eq!(target, "src/lib.rs");
    }

    #[test]
    fn parse_effort_reads_flag_forms_and_aliases() {
        assert_eq!(
            parse_effort_and_target("--effort max the diff"),
            (ReviewEffort::Max, "the diff")
        );
        assert_eq!(
            parse_effort_and_target("--effort=low file.rs"),
            (ReviewEffort::Low, "file.rs")
        );
        // "ultra" aliases to Max.
        assert_eq!(
            parse_effort_and_target("ultra file.rs"),
            (ReviewEffort::Max, "file.rs")
        );
    }

    #[test]
    fn parse_effort_treats_unknown_first_token_as_part_of_target() {
        let (effort, target) = parse_effort_and_target("MyClass.method");
        assert_eq!(effort, ReviewEffort::Medium);
        assert_eq!(target, "MyClass.method");
    }

    #[test]
    fn review_injects_effort_directive_into_active_skill() {
        let tmpdir = TempDir::new().unwrap();
        create_review_skill_dir(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = review(&mut app, Some("--effort high file.rs"));
        assert!(matches!(result.action, Some(AppAction::SendMessage(_))));
        let active = app.active_skill.expect("active skill set");
        assert!(active.contains("Review effort: high"));
    }
}
