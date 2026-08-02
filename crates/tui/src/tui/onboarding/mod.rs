//! Onboarding flow rendering and helpers.

pub mod language;
pub mod mental_models;
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
        OnboardingState::Appearance => appearance_lines(app),
        OnboardingState::Provider => provider_lines(app),
        OnboardingState::TrustDirectory => {
            // Inner text width: panel borders (2) plus horizontal padding (4).
            trust_directory::lines(app, usize::from(content_width.saturating_sub(6)))
        }
        OnboardingState::MentalModels => mental_models::lines(app),
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

/// Position and length of the first-run spine.
///
/// Welcome, Language, Appearance (#3937), Mental Models, and Tips are always
/// shown; provider setup and the trust screen are conditional.
fn onboarding_step(app: &App) -> (usize, usize) {
    let mut total = 5;
    if app.onboarding_had_provider_step {
        total += 1;
    }
    if app.onboarding_had_trust_step {
        total += 1;
    }

    let step = match app.onboarding {
        OnboardingState::Welcome => 1,
        OnboardingState::Language => 2,
        OnboardingState::Appearance => 3,
        OnboardingState::Provider => 4,
        OnboardingState::TrustDirectory => {
            if app.onboarding_had_provider_step {
                5
            } else {
                4
            }
        }
        OnboardingState::MentalModels => total - 1,
        OnboardingState::Tips => total,
        OnboardingState::None => total,
    };

    (step, total)
}

/// The card rendered behind the theme picker on the appearance step (#3937).
///
/// The picker itself owns the list and the live preview; this card carries the
/// promise (nothing is saved until Enter) in one short line, because that is
/// the part a first-run user cannot discover from the list alone.
fn appearance_lines(app: &App) -> Vec<ratatui::text::Line<'static>> {
    use crate::localization::MessageId;
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span};

    vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardAppearanceTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardAppearanceBlurb).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardAppearanceFooter).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
    ]
}

pub fn tips_lines(app: &App) -> Vec<ratatui::text::Line<'static>> {
    use crate::localization::MessageId;
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span};

    let mut lines = vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardTipsTitle).to_string(),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    // The offline choice is a durable posture, not a passing toast: the final
    // screen states it plainly alongside the one command that recovers.
    if app.onboarding_explore_offline {
        lines.push(Line::from(Span::styled(
            app.tr(MessageId::OnboardOfflineTipsLine).to_string(),
            Style::default().fg(palette::STATUS_WARNING),
        )));
        lines.push(Line::from(""));
    }
    lines.extend([
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
    ]);
    lines
}

pub fn default_marker_path() -> Option<PathBuf> {
    let primary_home = codewhale_config::codewhale_home().ok()?;
    let legacy_home = if codewhale_config::codewhale_home_is_explicit() {
        None
    } else {
        codewhale_config::legacy_deepseek_home().ok()
    };
    Some(marker_path_with_roots(
        &primary_home,
        legacy_home.as_deref(),
    ))
}

#[cfg(test)]
fn marker_path_with_home(home: &Path) -> PathBuf {
    marker_path_with_roots(
        &home.join(".codewhale"),
        Some(home.join(".deepseek").as_path()),
    )
}

fn marker_path_with_roots(primary_home: &Path, legacy_home: Option<&Path>) -> PathBuf {
    let primary = primary_home.join(ONBOARDED_MARKER_FILE);
    if primary.exists() {
        return primary;
    }
    if let Some(legacy_home) = legacy_home {
        let legacy = legacy_home.join(ONBOARDED_MARKER_FILE);
        if legacy.exists() {
            return legacy;
        }
    }
    primary
}

pub fn is_onboarded() -> bool {
    default_marker_path().is_some_and(|path| path.exists())
}

pub fn mark_onboarded() -> std::io::Result<PathBuf> {
    let path = default_marker_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Codewhale home directory not found",
        )
    })?;
    mark_onboarded_at_path(path)
}

#[cfg(test)]
fn mark_onboarded_at_home(home: &Path) -> std::io::Result<PathBuf> {
    let path = marker_path_with_home(home);
    mark_onboarded_at_path(path)
}

fn mark_onboarded_at_path(path: PathBuf) -> std::io::Result<PathBuf> {
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

/// Welcome → Language transition. Clears the status message bar.
pub fn advance_onboarding_from_welcome(app: &mut App) {
    app.status_message = None;
    app.onboarding = OnboardingState::Language;
}

/// Language → appearance (#3937).
pub fn advance_onboarding_after_language(app: &mut App) {
    app.status_message = None;
    app.onboarding = OnboardingState::Appearance;
}

/// Appearance → next step. Exactly the routing the language step used to
/// perform: the appearance step is inserted into the spine, it replaces
/// nothing. Routes to Provider setup when the session lacks a key, to
/// TrustDirectory when the workspace is untrusted, otherwise to the
/// mental-model primer.
pub fn advance_onboarding_after_appearance(app: &mut App) {
    app.status_message = None;
    if app.onboarding_needs_api_key {
        app.onboarding = OnboardingState::Provider;
    } else if !app.trust_mode && needs_trust(&app.workspace) {
        app.onboarding = OnboardingState::TrustDirectory;
    } else {
        app.onboarding = OnboardingState::MentalModels;
    }
}

/// Take the explicit "explore offline" exit advertised by Provider setup
/// (#3927).
///
/// The contract this encodes, in full:
///
/// * **No provider is selected and no route is activated.** This function must
///   never reach `switch_provider`, never persist `provider`, and never write a
///   credential. Callers pass only `&mut App`, which makes that structural.
/// * **No draft secret is owned by `App`.** The caller closes the canonical
///   picker before entering this transition, dropping its private draft.
/// * **`onboarding_needs_api_key` stays true**, because nothing was supplied.
///   The launch surface, `/setup`, and doctor keep telling the truth.
/// * **The remaining onboarding steps still run** — trust, then the mental
///   model primer and tips — so browsing offline is a complete first run and
///   not an early exit.
/// * Queue semantics are inherited from `offline_mode`, untouched here.
pub fn choose_offline_explore(app: &mut App) {
    app.api_key_env_only = false;
    app.onboarding_needs_api_key = true;
    app.onboarding_explore_offline = true;
    app.offline_mode = true;
    // `advance_*` clears the status bar, so the label is applied after it.
    advance_onboarding_after_provider(app);
    app.status_message = Some(
        app.tr(crate::localization::MessageId::OnboardOfflineNotice)
            .into_owned(),
    );
    app.needs_redraw = true;
}

/// Clear the offline-explore label once a real route is activated (#3927).
///
/// This is the *only* thing that retires the label: it is not time-based and
/// not cleared by dismissing a screen.
pub fn clear_offline_explore_on_route_activation(app: &mut App) {
    app.onboarding_explore_offline = false;
}

pub fn advance_onboarding_after_provider(app: &mut App) {
    app.status_message = None;
    if !app.trust_mode && needs_trust(&app.workspace) {
        app.onboarding = OnboardingState::TrustDirectory;
    } else if app.onboarding_missing_key_recovery {
        app.onboarding = OnboardingState::Tips;
    } else {
        app.onboarding = OnboardingState::MentalModels;
    }
}

pub fn back_from_mental_models(app: &mut App) {
    app.status_message = None;
    app.onboarding = if app.onboarding_had_trust_step {
        OnboardingState::TrustDirectory
    } else if app.onboarding_had_provider_step {
        OnboardingState::Provider
    } else {
        OnboardingState::Appearance
    };
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
            app.tr(MessageId::OnboardOfflineOption).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
        Line::from(Span::styled(
            app.tr(MessageId::OnboardProviderFooter).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn test_app_with_locale(locale: Locale) -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
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
        let lines = trust_directory::lines(&app, 70);
        let footer = lines
            .last()
            .expect("trust footer")
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(
            footer,
            "Press 1/Y to trust and continue, 2/U to continue without trusting, 3/N/Esc to quit Codewhale"
        );
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
    fn explicit_codewhale_home_marker_survives_restart_resolution() {
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ambient_home = tmp.path().join("ambient profile");
        let isolated_home = tmp.path().join("isolated Codewhale state");
        let ambient_legacy = ambient_home.join(".deepseek").join(ONBOARDED_MARKER_FILE);
        std::fs::create_dir_all(ambient_legacy.parent().expect("legacy parent"))
            .expect("mkdir legacy");
        std::fs::write(&ambient_legacy, "").expect("seed ambient legacy marker");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &ambient_home);
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", &ambient_home);
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &isolated_home);

        let expected = isolated_home.join(ONBOARDED_MARKER_FILE);
        assert_eq!(default_marker_path().as_deref(), Some(expected.as_path()));
        assert!(!is_onboarded());

        let written = mark_onboarded().expect("mark onboarded");

        assert_eq!(written, expected);
        assert!(is_onboarded());
        assert_eq!(default_marker_path().as_deref(), Some(expected.as_path()));
        assert!(ambient_legacy.exists(), "legacy marker remains untouched");
        assert!(
            !ambient_home.join(".codewhale").exists(),
            "an explicit state root must not write into the ambient profile"
        );
    }

    // ── #3937: the "Make it yours" appearance step ───────────────────────

    #[test]
    fn appearance_sits_between_language_and_the_routing_the_language_step_used_to_do() {
        // Untrusted workspace, key already present: language used to route
        // straight to trust. It now routes through appearance and lands in
        // exactly the same place.
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding = OnboardingState::Language;
        app.onboarding_needs_api_key = false;
        app.trust_mode = false;
        app.workspace = tempfile::tempdir().expect("tempdir").path().to_path_buf();

        advance_onboarding_after_language(&mut app);
        assert_eq!(app.onboarding, OnboardingState::Appearance);
        advance_onboarding_after_appearance(&mut app);
        assert_eq!(app.onboarding, OnboardingState::TrustDirectory);

        // And the credential path is unchanged too.
        let mut keyless = test_app_with_locale(Locale::En);
        keyless.onboarding = OnboardingState::Language;
        keyless.onboarding_needs_api_key = true;
        advance_onboarding_after_language(&mut keyless);
        assert_eq!(keyless.onboarding, OnboardingState::Appearance);
        advance_onboarding_after_appearance(&mut keyless);
        assert_eq!(keyless.onboarding, OnboardingState::Provider);
    }

    #[test]
    fn the_step_counter_grows_with_the_spine_instead_of_overflowing() {
        // The shortest first run: no key step, no trust step.
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding_had_provider_step = false;
        app.onboarding_had_trust_step = false;
        app.onboarding = OnboardingState::Appearance;
        let (step, total) = onboarding_step(&app);
        assert_eq!((step, total), (3, 5));

        // The longest: provider setup plus trust.
        app.onboarding_had_provider_step = true;
        app.onboarding_had_trust_step = true;
        for (state, expected) in [
            (OnboardingState::Welcome, 1),
            (OnboardingState::Language, 2),
            (OnboardingState::Appearance, 3),
            (OnboardingState::Provider, 4),
            (OnboardingState::TrustDirectory, 5),
            (OnboardingState::MentalModels, 6),
            (OnboardingState::Tips, 7),
        ] {
            app.onboarding = state;
            let (step, total) = onboarding_step(&app);
            assert_eq!(step, expected, "{state:?}");
            assert_eq!(total, 7, "{state:?}");
            assert!(step <= total, "{state:?} overflowed the counter");
        }
    }

    #[test]
    fn back_from_the_primer_returns_to_appearance_when_there_was_no_provider_step() {
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding = OnboardingState::MentalModels;
        app.onboarding_had_provider_step = false;
        app.onboarding_had_trust_step = false;

        back_from_mental_models(&mut app);

        assert_eq!(app.onboarding, OnboardingState::Appearance);
    }

    #[test]
    fn appearance_card_states_the_promise_the_theme_list_cannot() {
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding = OnboardingState::Appearance;
        let body = flattened(appearance_lines(&app));

        assert!(body.contains("Make It Yours"));
        // The card exists to say what the picker's list cannot: nothing is
        // saved until Enter, and Esc puts back what was there.
        assert!(body.contains("Enter"));
        assert!(body.contains("Esc"));
    }

    #[test]
    fn appearance_copy_is_translated_in_every_complete_pack() {
        use crate::localization::{MessageId, tr};

        for locale in Locale::shipped_complete() {
            for id in [
                MessageId::OnboardAppearanceTitle,
                MessageId::OnboardAppearanceBlurb,
                MessageId::OnboardAppearanceFooter,
                MessageId::OnboardWelcomeStepAppearance,
            ] {
                let text = tr(*locale, id);
                assert!(!text.is_empty(), "{locale:?} {id:?} is empty");
                assert!(!text.contains('{'), "{locale:?} {id:?}: {text}");
                if *locale != Locale::En {
                    assert_ne!(
                        text,
                        tr(Locale::En, id),
                        "{locale:?} {id:?} silently fell back to English"
                    );
                }
            }
        }
    }

    // ── #3927: the explicit offline ("explore") choice ───────────────────

    #[test]
    fn explore_offline_selects_no_provider_and_keeps_the_key_still_missing() {
        let mut app = test_app_with_locale(Locale::En);
        let provider_before = app.api_provider;
        let model_before = app.model.clone();
        app.onboarding = OnboardingState::Provider;
        app.onboarding_needs_api_key = true;
        app.trust_mode = true;

        choose_offline_explore(&mut app);

        // No provider selected, no route activated.
        assert_eq!(app.api_provider, provider_before);
        assert_eq!(app.model, model_before);
        assert!(!app.api_key_env_only);
        // The install still honestly reports that no credential exists.
        assert!(app.onboarding_needs_api_key);
        assert!(app.onboarding_explore_offline);
        assert!(app.offline_mode);
    }

    #[test]
    fn explore_offline_label_contains_only_recovery_guidance() {
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding = OnboardingState::Provider;
        app.trust_mode = true;

        choose_offline_explore(&mut app);

        let label = app.status_message.clone().expect("offline label");
        assert!(
            label.contains("/provider"),
            "label must name recovery: {label}"
        );

        app.onboarding = OnboardingState::Tips;
        let tips = flattened(tips_lines(&app));
        assert!(tips.contains("/provider"));
    }

    #[test]
    fn explore_offline_still_traverses_trust_then_the_rest_of_onboarding() {
        let mut app = test_app_with_locale(Locale::En);
        app.onboarding = OnboardingState::Provider;
        app.trust_mode = false;
        app.workspace = tempfile::tempdir().expect("tempdir").path().to_path_buf();

        choose_offline_explore(&mut app);
        assert_eq!(app.onboarding, OnboardingState::TrustDirectory);

        // A trusted workspace skips only the trust screen, never the primer.
        let mut trusted = test_app_with_locale(Locale::En);
        trusted.onboarding = OnboardingState::Provider;
        trusted.trust_mode = true;
        choose_offline_explore(&mut trusted);
        assert_eq!(trusted.onboarding, OnboardingState::MentalModels);
    }

    #[test]
    fn offline_label_only_clears_when_a_route_is_activated() {
        let mut app = test_app_with_locale(Locale::En);
        app.trust_mode = true;
        choose_offline_explore(&mut app);
        assert!(app.onboarding_explore_offline);

        // Walking the rest of onboarding does not clear it.
        app.onboarding = OnboardingState::Tips;
        assert!(app.onboarding_explore_offline);
        back_from_mental_models(&mut app);
        assert!(app.onboarding_explore_offline);

        clear_offline_explore_on_route_activation(&mut app);
        assert!(!app.onboarding_explore_offline);
    }

    #[test]
    fn provider_screen_advertises_the_offline_choice() {
        let app = test_app_with_locale(Locale::En);
        let provider = flattened(provider_lines(&app));
        assert!(
            provider.contains("Ctrl+O"),
            "offline exit must be advertised: {provider}"
        );
        assert!(provider.contains("offline"), "{provider}");
    }

    #[test]
    fn offline_choice_copy_is_translated_in_every_complete_pack() {
        use crate::localization::{MessageId, tr};

        for locale in Locale::shipped_complete() {
            for id in [
                MessageId::OnboardOfflineOption,
                MessageId::OnboardOfflineNotice,
                MessageId::OnboardOfflineTipsLine,
            ] {
                let text = tr(*locale, id);
                assert!(!text.is_empty(), "{locale:?} {id:?} is empty");
                if *locale != Locale::En {
                    assert_ne!(
                        text,
                        tr(Locale::En, id),
                        "{locale:?} {id:?} silently fell back to English"
                    );
                }
            }
            // Commands and key names are composed in code, never translated.
            assert!(tr(*locale, MessageId::OnboardOfflineNotice).contains("/provider"));
            assert!(tr(*locale, MessageId::OnboardOfflineOption).contains("Ctrl+O"));
        }
    }
}
