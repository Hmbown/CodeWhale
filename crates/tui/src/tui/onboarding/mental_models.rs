//! First-run primer for Codewhale's two independent control axes.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::App;

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let current_mode = app.mode.display_name_localized(app.ui_locale).to_string();
    let current_permission = app.approval_mode.permission_chip_label().to_string();

    vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardMentalTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                app.tr(MessageId::OnboardMentalModesLabel).to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" — "),
            Span::styled("Plan", Style::default().fg(palette::WHALE_ACTION)),
            Span::raw(format!(
                " ({}) · ",
                app.tr(MessageId::OnboardMentalPlanHint)
            )),
            Span::styled("Act", Style::default().fg(palette::WHALE_ACTION)),
            Span::raw(format!(" ({}) · ", app.tr(MessageId::OnboardMentalActHint))),
            Span::styled("Operate", Style::default().fg(palette::WHALE_ACTION)),
            Span::raw(format!(
                " ({})",
                app.tr(MessageId::OnboardMentalOperateHint)
            )),
        ]),
        Line::from(vec![
            Span::styled(
                app.tr(MessageId::OnboardMentalPermissionLabel).to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" — "),
            Span::styled("Ask", Style::default().fg(palette::WHALE_ACTION)),
            Span::raw(" · "),
            Span::styled("Auto-Review", Style::default().fg(palette::WHALE_ACTION)),
            Span::raw(" · "),
            Span::styled("Full Access", Style::default().fg(palette::WHALE_ACTION)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw(format!(
                "{}  ",
                app.tr(MessageId::OnboardMentalCurrentLabel)
            )),
            Span::styled(
                current_mode,
                Style::default()
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" · "),
            Span::styled(
                current_permission,
                Style::default()
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardMentalConstitution).to_string(),
            Style::default().fg(palette::WHALE_HUMAN),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Tab",
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {}  ", app.tr(MessageId::OnboardMentalCycleMode))),
            Span::styled(
                "Shift+Tab",
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {}",
                app.tr(MessageId::OnboardMentalCyclePermission)
            )),
        ]),
        Line::from(vec![
            Span::styled(
                "Enter",
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {}  ", app.tr(MessageId::OnboardMentalContinue))),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {}", app.tr(MessageId::OnboardMentalBack))),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    #[test]
    fn primer_names_both_control_axes_and_current_values() {
        let options = TuiOptions {
            model: "test-model".to_string(),
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
        app.ui_locale = crate::localization::Locale::En;
        let body = lines(&app)
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(body.contains("How Codewhale works"));
        assert!(body.contains("Plan"));
        assert!(body.contains("Act"));
        assert!(body.contains("Operate"));
        assert!(body.contains("Ask"));
        assert!(body.contains("Auto-Review"));
        assert!(body.contains("Full Access"));
        assert!(body.contains("Tab"));
        assert!(body.contains("Shift+Tab"));
        assert!(body.contains("constitution"));
    }
}
