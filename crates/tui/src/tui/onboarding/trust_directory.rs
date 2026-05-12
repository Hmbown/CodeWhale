//! Workspace trust prompt for onboarding.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::App;

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardTrustTitle).to_string(),
        Style::default()
            .fg(palette::DEEPSEEK_SKY)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardTrustQuestion).to_string(),
        Style::default().fg(palette::TEXT_PRIMARY),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "{}{}",
            app.tr(MessageId::OnboardTrustLocationPrefix),
            crate::utils::display_path(&app.workspace)
        ),
        Style::default().fg(palette::TEXT_MUTED),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardTrustRiskHint).to_string(),
        Style::default().fg(palette::TEXT_MUTED),
    )));
    lines.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardTrustEffectHint).to_string(),
        Style::default().fg(palette::TEXT_MUTED),
    )));
    if let Some(message) = app.status_message.as_deref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(palette::STATUS_WARNING),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            app.tr(MessageId::OnboardTrustFooterPrefix).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        Span::styled(
            "1/Y",
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.tr(MessageId::OnboardTrustFooterMiddle).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        Span::styled(
            "2/N",
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.tr(MessageId::OnboardTrustFooterSuffix).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
    ]));
    lines
}
