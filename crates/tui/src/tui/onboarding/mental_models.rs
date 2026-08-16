//! First-run primer for Codewhale's two independent control axes.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::AppModeUi;
use crate::tui::app::{App, AppMode};

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let plan_mode = AppMode::Plan
        .display_name_localized(app.ui_locale)
        .to_string();
    let act_mode = AppMode::Agent
        .display_name_localized(app.ui_locale)
        .to_string();
    let operate_mode = AppMode::Operate
        .display_name_localized(app.ui_locale)
        .to_string();
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
            Span::styled(plan_mode, Style::default().fg(palette::WHALE_ACTION)),
            Span::raw(format!(
                " ({}) · ",
                app.tr(MessageId::OnboardMentalPlanHint)
            )),
            Span::styled(act_mode, Style::default().fg(palette::WHALE_ACTION)),
            Span::raw(format!(" ({}) · ", app.tr(MessageId::OnboardMentalActHint))),
            Span::styled(operate_mode, Style::default().fg(palette::WHALE_ACTION)),
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
            ..crate::test_support::test_tui_options(PathBuf::from("."))
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
        assert!(body.contains("Work"));
        assert!(body.contains("Operate"));
        assert!(body.contains("Ask"));
        assert!(body.contains("Auto-Review"));
        assert!(body.contains("Full Access"));
        assert!(body.contains("Tab"));
        assert!(body.contains("Shift+Tab"));
        assert!(body.contains("constitution"));
    }

    #[test]
    fn primer_localizes_mode_names_consistently() {
        let options = TuiOptions {
            model: "test-model".to_string(),
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::Ko;
        let body = lines(&app)
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");

        for mode in [AppMode::Plan, AppMode::Agent, AppMode::Operate] {
            let localized = mode.display_name_localized(app.ui_locale);
            assert!(body.contains(localized.as_ref()));
        }
    }
}
