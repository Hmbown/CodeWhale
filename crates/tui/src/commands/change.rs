//! `/change` command — show the latest changelog entry, translated to the
//! user's locale when it is not English.
//!
//! Usage: `/change`
//!
//! Uses the DeepSeek-TUI changelog embedded at compile time, extracts the
//! most recent version section, and displays it. When the UI locale is not
//! English and the current session can reach a model, the command also fires a
//! `SendMessage` action that asks the model to translate the changelog into
//! the user's language.

use crate::localization::{Locale, MessageId, tr};
use crate::tui::app::{App, AppAction};

use super::CommandResult;

/// Maximum length of the changelog excerpt we'll show inline (characters).
/// If the changelog section exceeds this, we truncate and show a notice.
/// 4096 chars is large enough for most version entries.
const MAX_INLINE_CHANGELOG_CHARS: usize = 4096;
const DEEPSEEK_TUI_CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");

/// Execute the `/change` command.
pub fn change(app: &mut App) -> CommandResult {
    let latest_section = match extract_latest_changelog_section(DEEPSEEK_TUI_CHANGELOG) {
        Some(s) => s,
        None => {
            return CommandResult::error(
                "Could not find a version section in the bundled DeepSeek-TUI changelog. \
                 Expected a line starting with `## [`. "
                    .to_string(),
            );
        }
    };

    let locale = app.ui_locale;
    let header = tr(locale, MessageId::CmdChangeHeader);

    let section_text = inline_changelog_section(&latest_section);

    // If the user's locale is English, just display.
    // Otherwise, also ask the model to translate.
    if locale == Locale::En {
        CommandResult::message(format!(
            "{header}\n─────────────────────────────\n{section_text}"
        ))
    } else if app.offline_mode || app.onboarding_needs_api_key {
        let fallback = tr(locale, MessageId::CmdChangeTranslationUnavailable);
        CommandResult::message(format!(
            "{header}\n\
─────────────────────────────\n\
{fallback}\n\n\
{section_text}"
        ))
    } else {
        let queued = tr(locale, MessageId::CmdChangeTranslationQueued);
        let display_text = format!(
            "{header}\n\
─────────────────────────────\n\
{queued}\n\n\
{section_text}"
        );
        let lang_name = match locale {
            Locale::ZhHans => "Simplified Chinese (中文)",
            Locale::Ja => "Japanese (日本語)",
            Locale::PtBr => "Brazilian Portuguese (Português)",
            // Fallback — should never reach here since we check En above.
            Locale::En => "English",
        };

        let translation_prompt = format!(
            "Translate the following changelog into {lang_name}. \
             Keep all markdown formatting, version numbers, dates, \
             contributor names, and code references intact. \
             Output ONLY the translated changelog, no preamble or commentary.\n\n\
             {latest_section}"
        );

        CommandResult::with_message_and_action(
            display_text,
            AppAction::SendMessage(translation_prompt),
        )
    }
}

fn inline_changelog_section(section: &str) -> String {
    if section.len() <= MAX_INLINE_CHANGELOG_CHARS {
        return section.to_string();
    }

    let truncated: String = section.chars().take(MAX_INLINE_CHANGELOG_CHARS).collect();
    format!(
        "{truncated}\n\
\n\
[... {} characters omitted from the bundled DeepSeek-TUI changelog]",
        section.len() - MAX_INLINE_CHANGELOG_CHARS
    )
}

/// Extract the latest version section from CHANGELOG.md content.
///
/// Looks for the first `## [version] - date` heading and returns all lines
/// from that heading up to the next `## [` heading (or end of file).
/// Leading and trailing whitespace is trimmed.
fn extract_latest_changelog_section(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut start_idx: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("## [") {
            start_idx = Some(i);
            break;
        }
    }

    let start = start_idx?;

    // Find the next `## [` heading (or end)
    let mut end = lines.len();
    for i in (start + 1)..lines.len() {
        if lines[i].trim().starts_with("## [") {
            end = i;
            break;
        }
    }

    let section = lines[start..end].join("\n");
    let section = section.trim().to_string();

    if section.is_empty() {
        return None;
    }

    Some(section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::tui::app::{App, TuiOptions};
    fn make_app(tmpdir: &tempfile::TempDir, locale: Locale, has_api_key: bool) -> App {
        let mut config = Config::default();
        if has_api_key {
            config.api_key = Some("test-key".to_string());
        }
        let mut app = App::new(
            TuiOptions {
                model: "deepseek-v4-pro".to_string(),
                workspace: tmpdir.path().to_path_buf(),
                config_path: None,
                config_profile: None,
                allow_shell: false,
                use_alt_screen: true,
                use_mouse_capture: false,
                use_bracketed_paste: true,
                max_subagents: 1,
                skills_dir: tmpdir.path().join("skills"),
                memory_path: tmpdir.path().join("memory.md"),
                notes_path: tmpdir.path().join("notes.txt"),
                mcp_config_path: tmpdir.path().join("mcp.json"),
                use_memory: false,
                start_in_agent_mode: false,
                skip_onboarding: true,
                yolo: false,
                resume_session_id: None,
                initial_input: None,
            },
            &config,
        );
        app.ui_locale = locale;
        app
    }

    #[test]
    fn extract_latest_section_finds_first_version() {
        let content = "\n\
## [0.8.26] - 2026-05-09\n\
\n\
A security + polish release.\n\
\n\
### Fixed\n\
\n\
- Fixed something\n\
\n\
## [0.8.25] - 2026-05-09\n\
\n\
A stabilization release.\n";
        let section = extract_latest_changelog_section(content).expect("should find a section");
        assert!(section.contains("0.8.26"));
        assert!(section.contains("Fixed something"));
        assert!(!section.contains("0.8.25"));
    }

    #[test]
    fn extract_latest_section_handles_0_8_29_style_fixture() {
        let content = "\n\
# Changelog\n\
\n\
## [0.8.29] - 2026-05-11\n\
\n\
Release candidate polish.\n\
\n\
### Added\n\
- New note-management command.\n\
\n\
## [0.8.28] - 2026-05-10\n\
\n\
Previous release.\n";
        let section = extract_latest_changelog_section(content).expect("should find a section");
        assert!(section.contains("0.8.29"));
        assert!(section.contains("2026-05-11"));
        assert!(section.contains("New note-management command"));
        assert!(!section.contains("0.8.28"));
    }

    #[test]
    fn extract_latest_section_returns_none_for_empty_content() {
        assert!(extract_latest_changelog_section("").is_none());
    }

    #[test]
    fn extract_latest_section_returns_none_for_no_version_headers() {
        let content = "# Just a heading\n\nSome text\n";
        assert!(extract_latest_changelog_section(content).is_none());
    }

    #[test]
    fn extract_latest_section_handles_single_version() {
        let content = "\n## [0.8.26] - 2026-05-09\n\nOnly one version.\n";
        let section = extract_latest_changelog_section(content).expect("should find a section");
        assert!(section.contains("0.8.26"));
        assert!(section.contains("Only one version"));
    }

    #[test]
    fn extract_latest_section_handles_subheadings() {
        let content = "\n\
## [0.8.26] - 2026-05-09\n\
\n\
### Added\n\
- New feature A\n\
\n\
### Fixed\n\
- Fixed bug B\n\
\n\
## [0.8.25] - 2026-05-09\n\
";
        let section = extract_latest_changelog_section(content).expect("should find a section");
        assert!(section.contains("New feature A"));
        assert!(section.contains("Fixed bug B"));
        assert!(!section.contains("0.8.25"));
    }

    #[test]
    fn change_uses_bundled_release_notes_without_workspace_changelog() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::En, false);
        let result = change(&mut app);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        let expected = extract_latest_changelog_section(DEEPSEEK_TUI_CHANGELOG)
            .expect("bundled changelog should have a release section");
        assert!(msg.contains(expected.lines().next().unwrap()));
    }

    #[test]
    fn change_ignores_workspace_changelog() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("CHANGELOG.md"),
            "\n## [9.9.9] - 2099-01-01\n\nWorkspace changelog.\n",
        )
        .unwrap();
        let mut app = make_app(&tmp, Locale::En, false);
        let result = change(&mut app);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        assert!(!msg.contains("9.9.9"));
        assert!(!msg.contains("Workspace changelog"));
    }

    #[test]
    fn change_in_english_returns_message_without_action() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::En, true);
        let result = change(&mut app);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        let expected = extract_latest_changelog_section(DEEPSEEK_TUI_CHANGELOG)
            .expect("bundled changelog should have a release section");
        assert!(msg.contains(expected.lines().next().unwrap()));
        assert!(
            result.action.is_none(),
            "English locale should not send translation"
        );
    }

    #[test]
    fn change_in_non_english_also_sends_translation_action() {
        for (locale, _label) in [
            (Locale::ZhHans, "zh-Hans"),
            (Locale::Ja, "ja"),
            (Locale::PtBr, "pt-BR"),
        ] {
            let tmp = tempfile::TempDir::new().unwrap();
            let mut app = make_app(&tmp, locale, true);
            let result = change(&mut app);
            assert!(!result.is_error, "Failed for locale {locale:?}");
            let msg = result.message.expect("should have a message");
            assert!(msg.contains(tr(locale, MessageId::CmdChangeTranslationQueued)));
            assert!(
                matches!(result.action, Some(AppAction::SendMessage(_))),
                "Non-English locale should send translation, got {:?}",
                result.action
            );
            if let Some(AppAction::SendMessage(prompt)) = &result.action {
                let expected = extract_latest_changelog_section(DEEPSEEK_TUI_CHANGELOG)
                    .expect("bundled changelog should have a release section");
                assert!(prompt.contains(expected.lines().next().unwrap()));
            }
        }
    }

    #[test]
    fn change_in_non_english_without_api_key_uses_explicit_fallback() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::ZhHans, false);
        let result = change(&mut app);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        assert!(msg.contains(tr(
            Locale::ZhHans,
            MessageId::CmdChangeTranslationUnavailable
        )));
        assert!(
            result.action.is_none(),
            "missing API key should not send translation"
        );
    }

    #[test]
    fn change_in_non_english_offline_uses_explicit_fallback() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::Ja, true);
        app.offline_mode = true;
        let result = change(&mut app);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        assert!(msg.contains(tr(Locale::Ja, MessageId::CmdChangeTranslationUnavailable)));
        assert!(
            result.action.is_none(),
            "offline mode should not send translation"
        );
    }

    #[test]
    fn extract_latest_ignores_lines_before_first_version() {
        let content = "\n\
# Changelog\n\
\n\
Some intro text.\n\
\n\
## [0.8.26] - 2026-05-09\n\
\n\
Content\n\
";
        let section = extract_latest_changelog_section(content).expect("should find a section");
        assert!(section.contains("0.8.26"));
        assert!(!section.contains("Changelog"));
        assert!(!section.contains("intro text"));
    }
}
