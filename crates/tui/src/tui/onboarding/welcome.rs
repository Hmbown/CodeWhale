//! Welcome screen content for onboarding.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::App;

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let steps = welcome_step_labels(app).join(" -> ");
    let version = app
        .tr(MessageId::OnboardWelcomeVersion)
        .replace("{version}", env!("CARGO_PKG_VERSION"));
    let next_steps = app
        .tr(MessageId::OnboardWelcomeSteps)
        .replace("{steps}", &steps);

    vec![
        Line::from(Span::styled(
            "codewhale",
            Style::default()
                .fg(palette::WHALE_ACCENT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            version,
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardWelcomeLead).to_string(),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardWelcomeSetupBlurb).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            next_steps,
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardWelcomeDefaults).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardWelcomeEnter).to_string(),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardWelcomeExit).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
    ]
}

fn welcome_step_labels(app: &App) -> Vec<String> {
    let mut steps = vec![app.tr(MessageId::OnboardWelcomeStepLanguage).to_string()];
    if app.onboarding_needs_api_key {
        steps.push(app.tr(MessageId::OnboardWelcomeStepApiKey).to_string());
    }
    if !app.trust_mode && super::needs_trust(&app.workspace) {
        steps.push(app.tr(MessageId::OnboardWelcomeStepTrust).to_string());
    }
    steps.push(app.tr(MessageId::OnboardWelcomeStepTips).to_string());
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn test_app_with_locale(locale: Locale) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = locale;
        app
    }

    fn body(app: &App) -> String {
        lines(app)
            .into_iter()
            .flat_map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn welcome_copy_centers_constitution_first_setup() {
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding_needs_api_key = false;
        app.trust_mode = true;
        let body = body(&app);

        // The dual meaning of "code" opens the arc: software and law.
        assert!(body.contains("Code means two things"));
        assert!(body.contains("the law this agent works under"));
        assert!(body.contains("only these screens will appear"));
        assert!(body.contains("Next: choose language -> setup tips."));
        assert!(body.contains("/constitution"));
        assert!(!body.contains("add an API key"));
        assert!(!body.contains("land in the chat"));
    }

    #[test]
    fn welcome_steps_include_optional_api_key_and_trust_screens() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut app = test_app_with_locale(Locale::En);
        app.workspace = tmp.path().to_path_buf();
        app.onboarding_needs_api_key = true;
        app.trust_mode = false;

        let body = body(&app);

        assert!(body.contains(
            "Next: choose language -> connect API key -> trust workspace -> setup tips."
        ));
    }

    #[test]
    fn welcome_copy_uses_locale_registry() {
        let mut app = test_app_with_locale(Locale::ZhHans);
        app.onboarding_needs_api_key = false;
        app.trust_mode = true;

        let body = body(&app);

        assert!(body.contains("代码在这里有两层含义"));
        assert!(body.contains("接下来：选择语言 -> 设置提示。"));
        assert!(!body.contains("Press Enter"));
    }
}
