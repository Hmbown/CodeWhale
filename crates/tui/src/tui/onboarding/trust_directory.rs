//! Workspace trust prompt for onboarding.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::App;

/// Wrap a path-bearing line at `/` boundaries so a deep workspace never
/// hard-splits mid-component under ratatui's whitespace-only `Wrap`.
/// Continuation lines are indented to read as one location.
fn wrap_on_path_separators(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chunk = String::new();
    let flush = |current: &mut String, chunk: &mut String, out: &mut Vec<String>| {
        if chunk.is_empty() {
            return;
        }
        let candidate_len = current.chars().count() + chunk.chars().count();
        if candidate_len > width && !current.is_empty() {
            out.push(std::mem::take(current));
            current.push_str("  ");
        }
        current.push_str(chunk);
        chunk.clear();
    };
    for ch in text.chars() {
        chunk.push(ch);
        if ch == '/' {
            flush(&mut current, &mut chunk, &mut out);
        }
    }
    flush(&mut current, &mut chunk, &mut out);
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        vec![String::new()]
    } else {
        out
    }
}

pub fn lines(app: &App, content_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardTrustTitle).to_string(),
        Style::default()
            .fg(palette::WHALE_INFO)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardTrustQuestion).to_string(),
        Style::default().fg(palette::TEXT_PRIMARY),
    )));
    let location = format!(
        "{}{}",
        app.tr(MessageId::OnboardTrustLocationPrefix),
        crate::utils::display_path(&app.workspace)
    );
    for segment in wrap_on_path_separators(&location, content_width) {
        lines.push(Line::from(Span::styled(
            segment,
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }
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
            "2/U",
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.tr(MessageId::OnboardTrustFooterUntrustedMiddle)
                .to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        Span::styled(
            "3/N/Esc",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    #[test]
    fn prompt_names_the_workspace_boundary_and_effects() {
        let options = TuiOptions {
            model: "test-model".to_string(),
            ..crate::test_support::test_tui_options(PathBuf::from("workspace-fixture"))
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::En;
        let body = lines(&app, 70)
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(body.contains("Know this workspace"));
        assert!(body.contains("instructions and files"));
        assert!(body.contains("prompt injection"));
        assert!(body.contains("tools and hooks"));
        assert!(body.contains("1/Y"));
        assert!(body.contains("2/U"));
        assert!(body.contains("continue without trusting"));
        assert!(body.contains("3/N/Esc"));
        assert!(body.contains("quit Codewhale"));
    }
}
