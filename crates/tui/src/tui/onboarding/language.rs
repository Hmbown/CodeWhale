//! Language picker for first-run onboarding (#566).
//!
//! Surfaces every locale the TUI ships translations for, plus an `auto`
//! option that defers to `LC_ALL` / `LANG`. Selection persists via
//! `Settings::save` immediately so the rest of onboarding (and every
//! subsequent session) reads the chosen tag.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::App;

/// Locale options shown in the picker. Order matches the keyboard hotkeys.
/// Each entry is `(hotkey, settings_tag, native_name, english_label)`.
/// `settings_tag` is what `Settings::set("locale", …)` accepts and what
/// `localization::Locale` resolves on next read.
pub const LANGUAGE_OPTIONS: &[(char, &str, &str, &str)] = &[
    ('1', "auto", "Auto-detect", "(LC_ALL / LANG)"),
    ('2', "en", "English", ""),
    ('3', "ja", "日本語", "(Japanese)"),
    ('4', "zh-Hans", "简体中文", "(Simplified Chinese)"),
    ('5', "zh-Hant", "繁體中文", "(Traditional Chinese)"),
    ('6', "pt-BR", "Português (Brasil)", "(Brazilian Portuguese)"),
    (
        '7',
        "es-419",
        "Español (Latinoamérica)",
        "(Latin American Spanish)",
    ),
    ('8', "vi", "Tiếng Việt", "(Vietnamese)"),
];

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let current_owned = app.current_locale_tag();
    let current = current_owned.as_str();

    let mut out: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardLanguageTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardLanguageBlurb).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
    ];

    for (hotkey, tag, native, english) in LANGUAGE_OPTIONS {
        let is_current = current == *tag;
        let bullet = if is_current { "●" } else { "○" };
        let bullet_color = if is_current {
            palette::WHALE_ACCENT_PRIMARY
        } else {
            palette::TEXT_MUTED
        };
        let mut spans: Vec<Span<'static>> = vec![
            Span::styled(format!("  {bullet}  "), Style::default().fg(bullet_color)),
            Span::styled(
                format!("[{hotkey}] "),
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                native.to_string(),
                Style::default().fg(palette::TEXT_PRIMARY),
            ),
        ];
        if !english.is_empty() {
            spans.push(Span::styled(
                format!(" {english}"),
                Style::default().fg(palette::TEXT_MUTED),
            ));
        }
        out.push(Line::from(spans));
    }

    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardLanguageFooter).to_string(),
        Style::default().fg(palette::TEXT_MUTED),
    )));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::Locale;

    /// Every locale we ship translations for must be offered in the picker,
    /// otherwise the footer advertises hotkeys that select nothing and users
    /// can never reach a supported UI language (#3929).
    #[test]
    fn picker_offers_every_shipped_locale() {
        let offered: Vec<&str> = LANGUAGE_OPTIONS.iter().map(|(_, tag, _, _)| *tag).collect();
        assert!(
            offered.contains(&"auto"),
            "picker must keep the auto-detect entry"
        );
        for locale in Locale::shipped() {
            let tag = locale.tag();
            assert!(
                offered.contains(&tag),
                "shipped locale {tag} is not offered in the language picker"
            );
        }
    }

    /// Hotkeys must be the contiguous digits `1..=N` so the footer's "1-N"
    /// range stays truthful and `KeyCode::Char` lookups resolve.
    #[test]
    fn picker_hotkeys_are_contiguous_digits() {
        for (idx, (hotkey, tag, _, _)) in LANGUAGE_OPTIONS.iter().enumerate() {
            let expected = char::from_digit((idx + 1) as u32, 10).expect("digit");
            assert_eq!(
                *hotkey, expected,
                "option {tag} should use hotkey {expected}, not {hotkey}"
            );
        }
    }
}
