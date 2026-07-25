//! API key entry screen for onboarding.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::MessageId;
use crate::palette;
use crate::tui::app::App;

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let provider = app.onboarding_provider;
    let mut lines = vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardApiKeyTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "{} ({})",
                app.tr(MessageId::OnboardApiKeyStep1),
                provider.display_name()
            ),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
    ];
    let credential_help = provider.credential_help();
    if app.onboarding_uses_kimi_code_plan() {
        lines.push(Line::from(Span::styled(
            app.tr(MessageId::KimiCodePlanApiKeyHint).replace(
                "{console}",
                crate::config::KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL,
            ),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(Span::styled(
            app.tr(MessageId::KimiCodePlanRouteHint)
                .replace("{route}", crate::config::DEFAULT_KIMI_CODE_BASE_URL),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(Span::styled(
            app.tr(MessageId::KimiCodePlanNoImportHint),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    } else if let Some(url) = credential_help.credential_url {
        lines.push(Line::from(Span::styled(
            url.to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    } else if credential_help.acquisition
        == codewhale_config::provider::CredentialAcquisition::LocalOptional
    {
        lines.push(Line::from(Span::styled(
            app.tr(MessageId::OnboardApiKeyLocalHint).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            credential_help.guidance.to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }
    let saved_hint = app
        .tr(MessageId::OnboardApiKeySavedHint)
        .replace("{path}", &effective_config_path_display(app));
    lines.extend([
        Line::from(Span::styled(
            app.tr(MessageId::OnboardApiKeyStep2).to_string(),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
        Line::from(""),
        Line::from(Span::styled(
            saved_hint,
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardApiKeyFormatHint).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
    ]);

    let masked = mask_key(&app.api_key_input);
    let placeholder = app.tr(MessageId::OnboardApiKeyPlaceholder).to_string();
    let display = if masked.is_empty() {
        placeholder
    } else {
        masked
    };
    lines.push(Line::from(vec![
        Span::styled(
            app.tr(MessageId::OnboardApiKeyLabel).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        Span::styled(
            display,
            Style::default()
                .fg(palette::TEXT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    if let Some(message) = app.status_message.as_deref() {
        lines.push(Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(palette::STATUS_WARNING),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        app.tr(MessageId::OnboardApiKeyFooter).to_string(),
        Style::default().fg(palette::TEXT_MUTED),
    )));

    lines
}

fn mask_key(input: &str) -> String {
    let trimmed = input.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return String::new();
    }
    if len <= 4 {
        return "*".repeat(len);
    }
    let visible: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{}{}", "*".repeat(len - 4), visible)
}

/// Display path for the effective config.toml (#3986).
///
/// Prefers the App's session `config_path` override, then the same resolution
/// used by persistence (`CODEWHALE_CONFIG_PATH` / `CODEWHALE_HOME` / default).
/// Collapses `$HOME` to `~` when the path is under the process home.
fn effective_config_path_display(app: &App) -> String {
    let path = app
        .config_path
        .clone()
        .or_else(|| crate::config_persistence::config_toml_path(None).ok())
        .unwrap_or_else(|| std::path::PathBuf::from("~/.codewhale/config.toml"));
    collapse_home_prefix(&path)
}

fn collapse_home_prefix(path: &std::path::Path) -> String {
    if let Some(home) = crate::config::effective_home_dir()
        && let Ok(rel) = path.strip_prefix(&home)
    {
        if rel.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", rel.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::localization::Locale;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn test_app_with_locale(locale: Locale) -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = locale;
        app.onboarding_provider = ApiProvider::Zai;
        app
    }

    #[test]
    fn api_key_saved_hint_uses_effective_config_path() {
        // Isolated installs set CODEWHALE_CONFIG_PATH; the UI must not hardcode
        // ~/.codewhale/config.toml (#3986).
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = tmp.path().join("isolated-config.toml");
        let _cfg = crate::test_support::EnvVarGuard::set(
            "CODEWHALE_CONFIG_PATH",
            config.to_string_lossy().as_ref(),
        );
        let mut app = test_app_with_locale(Locale::En);
        app.config_path = Some(config.clone());
        let body: String = lines(&app)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains(config.to_string_lossy().as_ref())
                || body.contains("isolated-config.toml"),
            "saved hint should show effective path, body was:\n{body}"
        );
        assert!(
            !body.contains("~/.codewhale/config.toml"),
            "must not hardcode default home path when isolated: {body}"
        );
    }

    #[test]
    fn api_key_screen_renders_in_selected_locale() {
        // The most-visible regression of the missing onboarding-localization:
        // after the user picks 简体中文 at step 2, step 3 used to remain
        // English. Pin that the rendered lines actually contain the
        // translated strings for each locale we ship.
        let zh = test_app_with_locale(Locale::ZhHans);
        let body: String = lines(&zh)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("连接你的 API 密钥"),
            "title is provider-neutral and localized for zh-Hans"
        );
        assert!(
            body.contains("z.ai/model-api"),
            "expected default provider credential URL, got: {body}"
        );
        assert!(
            body.contains("密钥"),
            "expected zh-Hans 'key' label, got: {body}"
        );
        assert!(
            body.contains("按 Enter 继续"),
            "expected zh-Hans footer, got: {body}"
        );

        let ja = test_app_with_locale(Locale::Ja);
        let body: String = lines(&ja)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("キー"),
            "expected ja 'key' label, got: {body}"
        );

        let en = test_app_with_locale(Locale::En);
        let body: String = lines(&en)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("Press Enter to continue"),
            "expected en footer, got: {body}"
        );
    }

    #[test]
    fn local_provider_copy_makes_the_key_optional() {
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding_provider = ApiProvider::Ollama;
        let body = lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(body.contains("Local runtimes usually need no pasted key"));
        assert!(body.contains("If this provider requires a key"));
        assert!(body.contains("paste key here if required"));
        assert!(body.contains("Press Enter to continue"));
    }

    #[test]
    fn kimi_onboarding_points_to_the_api_key_console_and_paste_path() {
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding_provider = ApiProvider::Moonshot;
        let body = lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(body.contains("https://platform.kimi.ai/console/api-keys"));
        assert!(body.contains("paste it below"));
        assert!(body.contains("paste key here if required"));
        assert!(!body.contains("OAuth"));
        assert!(!body.contains("device login"));
    }

    #[test]
    fn kimi_code_plan_onboarding_uses_membership_key_guidance() {
        let mut app = test_app_with_locale(Locale::En);
        app.api_provider = ApiProvider::Moonshot;
        app.onboarding_provider = ApiProvider::Moonshot;
        app.active_route_base_url = crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string();
        app.model = crate::config::KIMI_CODE_K3_MODEL.to_string();
        let body = lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(body.contains("https://www.kimi.com/code/console"));
        assert!(body.contains("api.kimi.com/coding/v1"));
        assert!(body.contains("does not import Kimi CLI credentials"));
        assert!(!body.contains("https://platform.kimi.ai/console/api-keys"));
        assert!(!body.contains("OAuth"));
    }
}
