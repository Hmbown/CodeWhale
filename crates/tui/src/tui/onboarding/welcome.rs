//! Welcome screen content for onboarding.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::{Locale, MessageId, tr};
use crate::palette;

pub fn lines(locale: Locale) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeTitle),
            Style::default()
                .fg(palette::DEEPSEEK_BLUE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("Version {}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeSubtitle),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeDesc),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            "The main composer is multi-line, so you can write full prompts instead of squeezing everything into one line.",
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomePressEnter),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeCtrlCExit),
            Style::default().fg(palette::TEXT_MUTED),
        )),
    ]
}
