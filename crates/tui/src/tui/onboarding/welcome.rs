use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::{Locale, MessageId, tr};
use crate::palette;

pub fn lines(locale: Locale) -> Vec<Line<'static>> {
    let version = tr(locale, MessageId::OnboardWelcomeVersion)
        .replace("{version}", env!("CARGO_PKG_VERSION"));
    vec![
        Line::from(Span::styled(
            "codewhale",
            Style::default()
                .fg(palette::DEEPSEEK_BLUE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            version,
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeDesc),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeDesc2),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeDesc3),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeEnter),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(Span::styled(
            tr(locale, MessageId::OnboardWelcomeExit),
            Style::default().fg(palette::TEXT_MUTED),
        )),
    ]
}
