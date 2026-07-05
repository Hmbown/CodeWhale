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
    // In the full onboarding flow Esc steps back one screen (per
    // KEYBINDINGS.md), so the quit chord is `2/N` and an extra Esc hint is
    // shown. When the trust screen is a standalone gate there is no previous
    // screen: Esc quits, matching the historical `2/N/Esc` hint.
    let standalone_gate = app.onboarding_workspace_trust_gate;
    let deny_chord = if standalone_gate { "2/N/Esc" } else { "2/N" };
    let mut footer = vec![
        Span::styled(
            app.tr(MessageId::OnboardTrustFooterPrefix).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        Span::styled(
            "Enter/1/Y",
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.tr(MessageId::OnboardTrustFooterMiddle).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        Span::styled(
            deny_chord,
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.tr(MessageId::OnboardTrustFooterSuffix).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
    ];
    if !standalone_gate {
        footer.push(Span::styled(
            app.tr(MessageId::OnboardTrustFooterEscBack).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ));
    }
    lines.push(Line::from(footer));
    lines
}
