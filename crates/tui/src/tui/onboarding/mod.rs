//! Onboarding flow rendering and helpers.

pub mod api_key;
pub mod language;
pub mod trust_directory;
pub mod welcome;

use std::path::{Path, PathBuf};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
};

use crate::config::ApiProvider;
use crate::palette;
use crate::tui::app::{App, OnboardingState};

const ONBOARDED_MARKER_FILE: &str = ".onboarded";

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().style(Style::default().bg(palette::WHALE_BG));
    f.render_widget(block, area);

    const TOP_MARGIN: u16 = 2;
    let content_width = 76.min(area.width.saturating_sub(4));
    let content_height = 20.min(area.height.saturating_sub(TOP_MARGIN + 2));
    let content_area = Rect {
        x: (area.width.saturating_sub(content_width)) / 2,
        y: TOP_MARGIN,
        width: content_width,
        height: content_height,
    };

    let lines = match app.onboarding {
        OnboardingState::Welcome => welcome::lines(app),
        OnboardingState::Language => language::lines(app),
        OnboardingState::Provider => provider_lines(app),
        OnboardingState::ApiKey => api_key::lines(app),
        OnboardingState::TrustDirectory => trust_directory::lines(app),
        OnboardingState::Tips => tips_lines(app),
        OnboardingState::None => Vec::new(),
    };

    if !lines.is_empty() {
        let mut panel = Block::default()
            .title(Line::from(Span::styled(
                " Codewhale ",
                Style::default()
                    .fg(palette::WHALE_HUMAN)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_PANEL))
            .padding(Padding::new(2, 2, 1, 1));
        if !app.onboarding_workspace_trust_gate {
            let (step, total) = onboarding_step(app);
            panel = panel.title_bottom(Line::from(Span::styled(
                format!(" Step {step}/{total} "),
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        let inner = panel.inner(content_area);
        f.render_widget(panel, content_area);
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        f.render_widget(paragraph, inner);
    }
}

fn onboarding_step(app: &App) -> (usize, usize) {
    let needs_trust = !app.trust_mode && needs_trust(&app.workspace);
    // Welcome + Language + Tips are always shown.
    let mut total = 3;
    if app.onboarding_needs_api_key {
        total += 2;
    }
    if needs_trust {
        total += 1;
    }

    let step = match app.onboarding {
        OnboardingState::Welcome => 1,
        OnboardingState::Language => 2,
        OnboardingState::Provider => 3,
        OnboardingState::ApiKey => 4,
        OnboardingState::TrustDirectory => {
            if app.onboarding_needs_api_key {
                5
            } else {
                3
            }
        }
        OnboardingState::Tips => total,
        OnboardingState::None => total,
    };

    (step, total)
}

pub fn tips_lines(app: &App) -> Vec<ratatui::text::Line<'static>> {
    use crate::localization::MessageId;
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span};

    vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardTipsTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::raw(app.tr(MessageId::OnboardTipsLine1).to_string())),
        Line::from(Span::raw(app.tr(MessageId::OnboardTipsLine2).to_string())),
        Line::from(Span::raw(app.tr(MessageId::OnboardTipsLine3).to_string())),
        Line::from(Span::raw(app.tr(MessageId::OnboardTipsLine4).to_string())),
        Line::from(vec![
            Span::raw(app.tr(MessageId::OnboardTipsDoctorPrefix).to_string()),
            Span::styled(
                "codewhale doctor",
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.tr(MessageId::OnboardTipsDoctorSuffix).to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                app.tr(MessageId::OnboardTipsFooterEnter).to_string(),
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.tr(MessageId::OnboardTipsFooterAction).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]),
    ]
}

pub fn default_marker_path() -> Option<PathBuf> {
    crate::config::effective_home_dir().map(|home| marker_path_with_home(&home))
}

fn marker_path_with_home(home: &Path) -> PathBuf {
    let primary = home.join(".codewhale").join(ONBOARDED_MARKER_FILE);
    if primary.exists() {
        return primary;
    }
    let legacy = home.join(".deepseek").join(ONBOARDED_MARKER_FILE);
    if legacy.exists() {
        return legacy;
    }
    primary
}

pub fn is_onboarded() -> bool {
    default_marker_path().is_some_and(|path| path.exists())
}

pub fn mark_onboarded() -> std::io::Result<PathBuf> {
    let home = crate::config::effective_home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })?;
    mark_onboarded_at_home(&home)
}

fn mark_onboarded_at_home(home: &Path) -> std::io::Result<PathBuf> {
    let path = marker_path_with_home(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, "")?;
    Ok(path)
}

pub fn needs_trust(workspace: &Path) -> bool {
    if crate::config::is_workspace_trusted(workspace) {
        return false;
    }

    let markers = [
        workspace.join(".deepseek").join("trusted"),
        workspace.join(".deepseek").join("trust.json"),
    ];
    !markers.iter().any(|path| path.exists())
}

pub fn mark_trusted(workspace: &Path) -> anyhow::Result<PathBuf> {
    crate::config::save_workspace_trust(workspace)
}

// ── API key validation and state-machine transitions ─────────────────

/// Result of inspecting an API-key string entered during onboarding.
///
/// `Accept` always lets the user proceed; the optional `warning` is shown
/// as a non-blocking status message (short keys, unusual formats, etc.).
/// `Reject` blocks the keystroke flow until the user fixes the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyValidation {
    Accept { warning: Option<String> },
    Reject(String),
}

/// Whether onboarding may activate `provider` without saving an API key.
///
/// Keep this aligned with the runtime's self-hosted provider contract. A local
/// server can still opt into bearer authentication with
/// `auth_mode = "api_key"`, in which case onboarding must require a key too.
#[must_use]
pub fn onboarding_provider_allows_empty_api_key(
    config: &crate::config::Config,
    provider: ApiProvider,
) -> bool {
    provider.is_self_hosted()
        && !crate::config::auth_mode_requires_api_key(
            config.auth_mode_for_provider(provider).as_deref(),
        )
}

/// Validate an API key entered during onboarding. Empty input is accepted only
/// for a truly keyless self-hosted route. Other whitespace-only or
/// whitespace-containing keys are rejected; short or hyphen-less keys are
/// accepted with a warning so unusual provider key formats still work.
#[must_use]
pub fn validate_api_key_for_onboarding(
    config: &crate::config::Config,
    provider: ApiProvider,
    api_key: &str,
) -> ApiKeyValidation {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        if onboarding_provider_allows_empty_api_key(config, provider) {
            return ApiKeyValidation::Accept { warning: None };
        }
        return ApiKeyValidation::Reject("API key cannot be empty.".to_string());
    }
    if trimmed.contains(char::is_whitespace) {
        return ApiKeyValidation::Reject(
            "API key appears malformed (contains whitespace).".to_string(),
        );
    }
    if trimmed.len() < 16 {
        return ApiKeyValidation::Accept {
            warning: Some(
                "API key looks short. Double-check it, but unusual formats are allowed."
                    .to_string(),
            ),
        };
    }
    if !trimmed.contains('-') {
        return ApiKeyValidation::Accept {
            warning: Some(
                "API key format looks unusual. Check that the full key was copied.".to_string(),
            ),
        };
    }
    ApiKeyValidation::Accept { warning: None }
}

/// Welcome → Language transition. Clears the status message bar.
pub fn advance_onboarding_from_welcome(app: &mut App) {
    app.status_message = None;
    app.onboarding = OnboardingState::Language;
}

/// Language → next step. Routes to Provider/ApiKey when the session lacks a
/// key, to TrustDirectory when the workspace is untrusted, otherwise to Tips.
pub fn advance_onboarding_after_language(app: &mut App) {
    app.status_message = None;
    if app.onboarding_needs_api_key {
        app.onboarding = OnboardingState::Provider;
    } else if !app.trust_mode && needs_trust(&app.workspace) {
        app.onboarding = OnboardingState::TrustDirectory;
    } else {
        app.onboarding = OnboardingState::Tips;
    }
}

pub fn advance_onboarding_after_api_key(app: &mut App) {
    app.status_message = None;
    if !app.trust_mode && needs_trust(&app.workspace) {
        app.onboarding = OnboardingState::TrustDirectory;
    } else {
        app.onboarding = OnboardingState::Tips;
    }
}

fn provider_lines(app: &App) -> Vec<ratatui::text::Line<'static>> {
    use crate::localization::MessageId;
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span};

    vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardProviderTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardProviderBlurb).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardProviderFooter).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
    ]
}

/// Re-validate the current `api_key_input` and project the result onto
/// `app.status_message`. `show_empty_error` reports the "cannot be empty"
/// message even when the input has not been touched yet (used right
/// before submission); otherwise an empty input clears the status bar.
pub fn sync_api_key_validation_status(
    app: &mut App,
    config: &crate::config::Config,
    show_empty_error: bool,
) {
    if app.api_key_input.trim().is_empty() && !show_empty_error {
        app.status_message = None;
        return;
    }

    match validate_api_key_for_onboarding(config, app.onboarding_provider, &app.api_key_input) {
        ApiKeyValidation::Accept { warning } => {
            app.status_message = warning;
        }
        ApiKeyValidation::Reject(message) => {
            app.status_message = Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProviderConfig, ProvidersConfig};
    use crate::localization::Locale;
    use crate::tui::app::{App, TuiOptions};
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

    fn flattened(lines: Vec<ratatui::text::Line<'static>>) -> String {
        lines
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
    fn tips_copy_points_to_setup_and_constitution() {
        let app = test_app_with_locale(Locale::En);
        let body = flattened(tips_lines(&app));

        assert!(body.contains("/setup"));
        assert!(body.contains("/constitution"));
        assert!(body.contains("/provider"));
        assert!(body.contains("/model"));
        assert!(body.contains("codewhale doctor"));
        assert!(body.contains("open setup if it needs attention"));
        assert!(!body.contains("open the workspace"));
    }

    #[test]
    fn trust_footer_advertises_only_explicit_trust_keys() {
        let app = test_app_with_locale(Locale::En);
        let lines = trust_directory::lines(&app);
        let footer = lines
            .last()
            .expect("trust footer")
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(footer, "Press 1/Y to trust and continue, 2/N/Esc to quit");
    }

    #[test]
    fn fresh_install_marker_path_uses_codewhale_not_legacy() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let expected = tmp.path().join(".codewhale").join(ONBOARDED_MARKER_FILE);
        assert_eq!(marker_path_with_home(tmp.path()), expected);

        let written = mark_onboarded_at_home(tmp.path()).expect("mark onboarded");
        assert_eq!(written, expected);
        assert!(expected.exists());
        assert!(
            !tmp.path().join(".deepseek").exists(),
            "fresh onboarding must not recreate the legacy .deepseek dir"
        );
    }

    #[test]
    fn existing_legacy_marker_is_preserved() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join(".deepseek").join(ONBOARDED_MARKER_FILE);
        std::fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("mkdir legacy");
        std::fs::write(&legacy, "").expect("seed legacy marker");

        assert_eq!(marker_path_with_home(tmp.path()), legacy);
        assert_eq!(
            mark_onboarded_at_home(tmp.path()).expect("mark onboarded"),
            legacy
        );
    }

    #[test]
    fn codewhale_marker_wins_over_legacy_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let primary = tmp.path().join(".codewhale").join(ONBOARDED_MARKER_FILE);
        let legacy = tmp.path().join(".deepseek").join(ONBOARDED_MARKER_FILE);
        for marker in [&primary, &legacy] {
            std::fs::create_dir_all(marker.parent().expect("marker parent")).expect("mkdir");
            std::fs::write(marker, "").expect("seed marker");
        }

        assert_eq!(marker_path_with_home(tmp.path()), primary);
    }

    #[test]
    fn validate_rejects_empty_or_whitespace() {
        let config = Config::default();
        assert!(matches!(
            validate_api_key_for_onboarding(&config, ApiProvider::Deepseek, ""),
            ApiKeyValidation::Reject(_)
        ));
        assert!(matches!(
            validate_api_key_for_onboarding(&config, ApiProvider::Deepseek, "   "),
            ApiKeyValidation::Reject(_)
        ));
        assert!(matches!(
            validate_api_key_for_onboarding(&config, ApiProvider::Deepseek, "sk live abc"),
            ApiKeyValidation::Reject(_)
        ));
    }

    #[test]
    fn validate_accepts_empty_for_every_keyless_self_hosted_provider() {
        let config = Config::default();
        for provider in [ApiProvider::Ollama, ApiProvider::Sglang, ApiProvider::Vllm] {
            assert_eq!(
                validate_api_key_for_onboarding(&config, provider, ""),
                ApiKeyValidation::Accept { warning: None },
                "{} should keep its keyless runtime contract",
                provider.as_str()
            );
        }
    }

    #[test]
    fn explicit_local_api_key_auth_keeps_empty_input_blocking() {
        let config = Config {
            providers: Some(ProvidersConfig {
                ollama: ProviderConfig {
                    auth_mode: Some("api_key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        assert!(matches!(
            validate_api_key_for_onboarding(&config, ApiProvider::Ollama, ""),
            ApiKeyValidation::Reject(_)
        ));
    }

    #[test]
    fn validate_warns_on_short_or_no_hyphen_keys_but_accepts() {
        let config = Config::default();
        match validate_api_key_for_onboarding(&config, ApiProvider::Deepseek, "abc123") {
            ApiKeyValidation::Accept { warning: Some(_) } => {}
            _ => panic!("expected accept-with-warning"),
        }
        match validate_api_key_for_onboarding(&config, ApiProvider::Deepseek, "abcdefghijklmnop") {
            ApiKeyValidation::Accept { warning: Some(_) } => {}
            _ => panic!("expected accept-with-warning"),
        }
    }

    #[test]
    fn validate_accepts_well_formed_key() {
        let config = Config::default();
        assert_eq!(
            validate_api_key_for_onboarding(&config, ApiProvider::Deepseek, "sk-1234567890abcdef",),
            ApiKeyValidation::Accept { warning: None }
        );
    }
}
