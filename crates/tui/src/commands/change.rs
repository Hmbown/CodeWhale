//! `/change` command — show a changelog entry, translated to the user's
//! locale when it is not English.
//!
//! Usage: `/change [version]`
//!
//! Uses the DeepSeek-TUI changelog embedded at compile time. With no argument,
//! extracts the most recent section. With a version argument like `0.8.32`,
//! extracts that specific version's section. When the UI locale is not
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
const DEEPSEEK_TUI_CHANGELOG: &str = include_str!("../../CHANGELOG.md");

/// Execute the `/change` command.
///
/// If `version` is `None`, shows the latest non-empty version section.
/// If `version` is `Some(v)`, shows the section for that version.
pub fn change(app: &mut App, version: Option<&str>) -> CommandResult {
    let section = if let Some(ver) = version {
        let ver = ver.trim();
        if ver.is_empty() {
            extract_latest_changelog_section(DEEPSEEK_TUI_CHANGELOG)
        } else {
            extract_changelog_section_by_version(DEEPSEEK_TUI_CHANGELOG, ver)
        }
    } else {
        extract_latest_changelog_section(DEEPSEEK_TUI_CHANGELOG)
    };

    let latest_section = match section {
        Some(s) => s,
        None => {
            let msg = if let Some(ver) = version {
                let ver = ver.trim();
                if ver.is_empty() {
                    "Could not find a version section in the bundled DeepSeek-TUI changelog. \
                     Expected a line starting with `## [`."
                        .to_string()
                } else {
                    format!(
                        "Could not find version \"{ver}\" in the bundled DeepSeek-TUI changelog."
                    )
                }
            } else {
                "Could not find a version section in the bundled DeepSeek-TUI changelog. \
                 Expected a line starting with `## [`."
                    .to_string()
            };
            return CommandResult::error(msg);
        }
    };

    let locale = app.ui_locale;
    let header = tr(locale, MessageId::CmdChangeHeader);

    let prev_hint = if let Some(prev_ver) = previous_version_hint(DEEPSEEK_TUI_CHANGELOG, version) {
        let template = tr(locale, MessageId::CmdChangePreviousVersion);
        format!("\n\n{}", template.replace("{version}", &prev_ver))
    } else {
        String::new()
    };

    let section_text = inline_changelog_section(&latest_section);

    // If the user's locale is English, just display.
    // Otherwise, also ask the model to translate.
    if locale == Locale::En {
        CommandResult::message(format!(
            "{header}\n─────────────────────────────\n{section_text}{prev_hint}"
        ))
    } else if app.offline_mode || app.onboarding_needs_api_key {
        let fallback = tr(locale, MessageId::CmdChangeTranslationUnavailable);
        CommandResult::message(format!(
            "{header}\n\
─────────────────────────────\n\
{fallback}\n\n\
{section_text}{prev_hint}"
        ))
    } else {
        let queued = tr(locale, MessageId::CmdChangeTranslationQueued);
        let display_text = format!(
            "{header}\n\
─────────────────────────────\n\
{queued}\n\n\
{section_text}{prev_hint}"
        );
        let translation_source = format!("{latest_section}{prev_hint}");
        let lang_name = match locale {
            Locale::ZhHans => "Simplified Chinese (中文)",
            Locale::ZhHant => "Traditional Chinese (繁體中文)",
            Locale::Ja => "Japanese (日本語)",
            Locale::PtBr => "Brazilian Portuguese (Português)",
            Locale::Es419 => "Latin American Spanish (Español latinoamericano)",
            // Fallback — should never reach here since we check En above.
            Locale::En => "English",
        };

        let translation_prompt = format!(
            "Translate the following changelog into {lang_name}. \
             Keep all markdown formatting, version numbers, dates, \
             contributor names, and code references intact. \
             Output ONLY the translated changelog, no preamble or commentary.\n\n\
             {translation_source}"
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
///
/// Skips empty sections (e.g. `## [Unreleased]` with no content) to find
/// the first section that actually has content.
fn extract_latest_changelog_section(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();

    // Find the first `## [` heading index
    let first_idx = {
        let mut idx = None;
        for (i, line) in lines.iter().enumerate() {
            if line.trim().starts_with("## [") {
                idx = Some(i);
                break;
            }
        }
        idx?
    };

    // Starting from `first_idx`, walk through headings until we find a
    // section with non-empty content.
    let mut pos = first_idx;
    loop {
        let end = lines
            .iter()
            .enumerate()
            .skip(pos + 1)
            .find(|(_, line)| line.trim().starts_with("## ["))
            .map_or(lines.len(), |(i, _)| i);

        if section_has_body_content(&lines[pos + 1..end]) {
            return Some(lines[pos..end].join("\n").trim().to_string());
        }

        // Empty section — try the next heading (if any)
        if end >= lines.len() {
            return None;
        }
        pos = end;
    }
}

/// Extract a specific version section from CHANGELOG.md content.
///
/// Looks for `## [<version>]` or `## [<version> - date]` and returns all
/// lines from that heading up to the next `## [` heading (or end of file).
fn extract_changelog_section_by_version(content: &str, version: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut start_idx: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("## [") {
            // Check if this heading matches the requested version.
            // Format: `## [0.8.32] - 2026-05-12` or `## [0.8.32]`
            let bracket_end = trimmed.find(']')?;
            let heading_ver = &trimmed[4..bracket_end]; // skip "## ["
            if heading_ver == version {
                start_idx = Some(i);
                break;
            }
        }
    }

    let start = start_idx?;

    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim().starts_with("## ["))
        .map_or(lines.len(), |(i, _)| i);

    if !section_has_body_content(&lines[start + 1..end]) {
        return None;
    }

    Some(lines[start..end].join("\n").trim().to_string())
}

/// Extract the version number of the section immediately preceding the latest
/// non-empty section in the changelog.
///
/// Walks past empty sections (e.g. `## [Unreleased]`) the same way
/// [`extract_latest_changelog_section`] does, then returns the version from
/// the next `## [version]` heading after the first contentful section.
fn extract_previous_version_number(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let first_idx = lines.iter().position(|l| l.trim().starts_with("## ["))?;

    let mut pos = first_idx;
    loop {
        let end = lines
            .iter()
            .enumerate()
            .skip(pos + 1)
            .find(|(_, l)| l.trim().starts_with("## ["))
            .map_or(lines.len(), |(i, _)| i);

        if section_has_body_content(&lines[pos + 1..end]) {
            // Found the latest contentful section heading at `pos`.
            return next_contentful_version_after(&lines, end);
        }

        if end >= lines.len() {
            return None;
        }
        pos = end;
    }
}

fn section_has_body_content(lines: &[&str]) -> bool {
    lines.iter().any(|line| !line.trim().is_empty())
}

fn previous_version_hint(content: &str, version: Option<&str>) -> Option<String> {
    match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(version) => extract_previous_version_number_after_version(content, version),
        None => extract_previous_version_number(content),
    }
}

fn extract_previous_version_number_after_version(content: &str, version: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let current_start = lines.iter().position(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("## [")
            .and_then(|rest| rest.split_once(']'))
            .is_some_and(|(heading_ver, _)| heading_ver == version)
    })?;

    let current_end = lines
        .iter()
        .enumerate()
        .skip(current_start + 1)
        .find(|(_, line)| line.trim().starts_with("## ["))
        .map_or(lines.len(), |(i, _)| i);

    next_contentful_version_after(&lines, current_end)
}

fn next_contentful_version_after(lines: &[&str], mut pos: usize) -> Option<String> {
    while pos < lines.len() {
        let heading = lines[pos].trim();
        if !heading.starts_with("## [") {
            pos += 1;
            continue;
        }

        let end = lines
            .iter()
            .enumerate()
            .skip(pos + 1)
            .find(|(_, line)| line.trim().starts_with("## ["))
            .map_or(lines.len(), |(i, _)| i);

        if section_has_body_content(&lines[pos + 1..end]) {
            let bracket_end = heading.find(']')?;
            return Some(heading[4..bracket_end].to_string());
        }

        pos = end;
    }

    None
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
        let result = change(&mut app, None);
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
        let result = change(&mut app, None);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        assert!(!msg.contains("9.9.9"));
        assert!(!msg.contains("Workspace changelog"));
    }

    #[test]
    fn change_in_english_returns_message_without_action() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::En, true);
        let result = change(&mut app, None);
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
            let result = change(&mut app, None);
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
                let prev_ver = extract_previous_version_number(DEEPSEEK_TUI_CHANGELOG)
                    .expect("bundled changelog should have a previous release");
                assert!(
                    prompt.contains(&prev_ver),
                    "translation prompt should include previous-version hint: {prompt}"
                );
            }
        }
    }

    #[test]
    fn change_in_non_english_without_api_key_uses_explicit_fallback() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::ZhHans, false);
        let result = change(&mut app, None);
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
        let result = change(&mut app, None);
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

    #[test]
    fn extract_latest_skips_empty_unreleased_section() {
        let content = "\n\
## [Unreleased]\n\
\n\
## [0.8.32] - 2026-05-12\n\
\n\
A release with content.\n\
\n\
### Fixed\n\
- Something fixed\n\
\n\
## [0.8.31] - 2026-05-11\n\
\n\
Previous release.\n";
        let section = extract_latest_changelog_section(content).expect("should skip Unreleased");
        assert!(section.contains("0.8.32"));
        assert!(section.contains("Something fixed"));
        assert!(!section.contains("Unreleased"));
        assert!(!section.contains("0.8.31"));
    }

    #[test]
    fn extract_latest_skips_entirely_empty_unreleased() {
        // `## [Unreleased]` followed immediately by the next version heading.
        let content = "\n\
## [Unreleased]\n\
## [0.8.32] - 2026-05-12\n\
\n\
Content here.\n";
        let section = extract_latest_changelog_section(content).expect("should find 0.8.32");
        assert!(section.contains("0.8.32"));
        assert!(!section.contains("Unreleased"));
    }

    #[test]
    fn extract_latest_returns_none_when_all_sections_empty() {
        let content = "\n\
## [Unreleased]\n\
## [Future]\n";
        assert!(extract_latest_changelog_section(content).is_none());
    }

    #[test]
    fn extract_latest_skips_multiple_empty_sections() {
        let content = "\n\
## [Unreleased]\n\
\n\
## [Next]\n\
\n\
## [0.8.32] - 2026-05-12\n\
\n\
Real content.\n";
        let section = extract_latest_changelog_section(content).expect("should find 0.8.32");
        assert!(section.contains("0.8.32"));
        assert!(section.contains("Real content"));
    }

    #[test]
    fn extract_by_version_finds_exact_version() {
        let content = "\n\
## [0.8.32] - 2026-05-12\n\
\n\
Release content.\n\
\n\
## [0.8.31] - 2026-05-11\n\
\n\
Earlier release.\n";
        let section =
            extract_changelog_section_by_version(content, "0.8.31").expect("should find 0.8.31");
        assert!(section.contains("0.8.31"));
        assert!(section.contains("Earlier release"));
        assert!(!section.contains("0.8.32"));
    }

    #[test]
    fn extract_by_version_returns_none_for_missing_version() {
        let content = "\n\
## [0.8.32] - 2026-05-12\n\
\n\
Content.\n";
        assert!(extract_changelog_section_by_version(content, "9.9.9").is_none());
    }

    #[test]
    fn extract_by_version_finds_version_without_date() {
        let content = "\n\
## [Unreleased]\n\
\n\
Nothing.\n";
        let section = extract_changelog_section_by_version(content, "Unreleased")
            .expect("should find Unreleased");
        assert!(section.contains("Unreleased"));
        assert!(section.contains("Nothing"));
    }

    #[test]
    fn extract_by_version_respects_empty_sections() {
        // `## [0.8.32]` is empty, should return None for it
        let content = "\n\
## [0.8.32] - 2026-05-12\n\
## [0.8.31] - 2026-05-11\n\
\n\
Content.\n";
        assert!(extract_changelog_section_by_version(content, "0.8.32").is_none());
    }

    #[test]
    fn change_with_version_arg_shows_older_release() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::En, false);
        let result = change(&mut app, Some("0.8.1"));
        // 0.8.1 is a very old release; if it exists, the result should not be an error.
        // If that exact version doesn't exist in the bundled changelog, we still
        // expect a proper error message referencing the version.
        if result.is_error {
            let msg = result.message.as_deref().unwrap_or("");
            assert!(msg.contains("0.8.1"), "error should mention version: {msg}");
        } else {
            let msg = result.message.expect("should have a message");
            assert!(msg.contains("0.8.1"));
        }
    }

    #[test]
    fn change_with_empty_version_arg_acts_as_default() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::En, false);
        let result_default = change(&mut app, None);
        assert!(!result_default.is_error);

        let mut app2 = make_app(&tmp, Locale::En, false);
        let result_empty = change(&mut app2, Some(""));
        assert!(!result_empty.is_error);

        // Both should have the same message content
        let msg_default = result_default.message.as_deref().unwrap_or("");
        let msg_empty = result_empty.message.as_deref().unwrap_or("");
        assert_eq!(msg_default, msg_empty);
    }

    #[test]
    fn change_with_nonexistent_version_returns_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::En, false);
        let result = change(&mut app, Some("99.99.99"));
        assert!(result.is_error);
        let msg = result.message.as_deref().unwrap_or("");
        assert!(
            msg.contains("99.99.99"),
            "error should mention version: {msg}"
        );
    }

    #[test]
    fn extract_by_version_ignores_substring_matches() {
        let content =
            "\n## [0.8.1] - 2026-01-01\n\nContent A.\n\n## [0.8.10] - 2026-01-10\n\nContent B.\n";
        let section =
            extract_changelog_section_by_version(content, "0.8.1").expect("should find 0.8.1");
        assert!(section.contains("Content A"));
        assert!(!section.contains("Content B"));
    }

    // --- extract_previous_version_number tests ---

    #[test]
    fn prev_version_finds_second_heading() {
        let content = "\n\
## [0.8.32] - 2026-05-12\n\
\n\
Release content.\n\
\n\
## [0.8.31] - 2026-05-11\n\
\n\
Earlier release.\n";
        let prev = extract_previous_version_number(content).expect("should find 0.8.31");
        assert_eq!(prev, "0.8.31");
    }

    #[test]
    fn prev_version_skips_empty_unreleased_section() {
        let content = "\n\
## [Unreleased]\n\
\n\
## [0.8.32] - 2026-05-12\n\
\n\
Actual release.\n\
\n\
## [0.8.31] - 2026-05-11\n\
\n\
Older release.\n";
        let prev = extract_previous_version_number(content)
            .expect("should skip Unreleased and find 0.8.31");
        assert_eq!(prev, "0.8.31");
    }

    #[test]
    fn prev_version_returns_none_for_single_version() {
        let content = "\n## [0.8.32] - 2026-05-12\n\nOnly one version.\n";
        assert!(extract_previous_version_number(content).is_none());
    }

    #[test]
    fn prev_version_returns_none_for_empty_content() {
        assert!(extract_previous_version_number("").is_none());
    }

    #[test]
    fn prev_version_returns_none_for_no_version_headers() {
        let content = "# Just a heading\n\nNo versions here.\n";
        assert!(extract_previous_version_number(content).is_none());
    }

    #[test]
    fn prev_version_handles_adjacent_headings() {
        let content = "\n\
## [0.8.32] - 2026-05-12\n\
\n\
Content.\n\
## [0.8.31] - 2026-05-11\n\
\n\
Older content.\n";
        let prev = extract_previous_version_number(content)
            .expect("should find 0.8.31 even with no blank line after section");
        assert_eq!(prev, "0.8.31");
    }

    #[test]
    fn prev_version_skips_multiple_empty_sections() {
        let content = "\n\
## [Unreleased]\n\
\n\
## [Future]\n\
\n\
## [0.8.32] - 2026-05-12\n\
\n\
Real release.\n\
\n\
## [0.8.31] - 2026-05-11\n\
\n\
Older release.\n";
        let prev = extract_previous_version_number(content)
            .expect("should skip Unreleased and Future, find 0.8.31");
        assert_eq!(prev, "0.8.31");
    }

    #[test]
    fn prev_version_after_explicit_version_finds_next_older_release() {
        let content = "\n\
## [0.8.32] - 2026-05-12\n\
\n\
Current release.\n\
\n\
## [0.8.31] - 2026-05-11\n\
\n\
Requested release.\n\
\n\
## [0.8.30] - 2026-05-10\n\
\n\
Older release.\n";
        let prev = extract_previous_version_number_after_version(content, "0.8.31")
            .expect("should find 0.8.30");
        assert_eq!(prev, "0.8.30");
    }

    #[test]
    fn prev_version_after_explicit_version_skips_empty_sections() {
        let content = "\n\
## [0.8.32] - 2026-05-12\n\
\n\
Current release.\n\
\n\
## [0.8.31] - 2026-05-11\n\
\n\
Requested release.\n\
\n\
## [Future]\n\
\n\
## [0.8.30] - 2026-05-10\n\
\n\
Older release.\n";
        let prev = extract_previous_version_number_after_version(content, "0.8.31")
            .expect("should skip Future and find 0.8.30");
        assert_eq!(prev, "0.8.30");
    }

    // --- change() output hint tests ---

    #[test]
    fn change_without_args_includes_previous_version_hint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::En, false);
        let result = change(&mut app, None);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        // The previous version hint should be part of the output.
        // We can't assert an exact version number since the changelog changes,
        // but the hint message key should appear.
        assert!(
            msg.contains("Previous version:") || msg.contains("run `/change"),
            "expected previous-version hint in output, got: {msg}"
        );
    }

    #[test]
    fn change_with_explicit_version_includes_previous_hint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::En, false);
        let result = change(&mut app, Some("0.8.32"));
        assert!(!result.is_error);
        let msg = result.message.as_deref().unwrap_or("");
        assert!(
            msg.contains("Previous version:") && msg.contains("0.8.31"),
            "explicit version should show previous-version hint: {msg}"
        );
    }

    #[test]
    fn change_hint_uses_localized_template() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::ZhHans, true);
        let result = change(&mut app, None);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        // zh-Hans template: "上一个版本:"
        assert!(
            msg.contains("上一个版本"),
            "zh-Hans output should contain localized hint: {msg}"
        );
    }

    #[test]
    fn change_hint_in_japanese() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::Ja, true);
        let result = change(&mut app, None);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        assert!(
            msg.contains("前のバージョン"),
            "ja output should contain localized hint: {msg}"
        );
    }

    #[test]
    fn change_hint_in_portuguese() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = make_app(&tmp, Locale::PtBr, true);
        let result = change(&mut app, None);
        assert!(!result.is_error);
        let msg = result.message.expect("should have a message");
        assert!(
            msg.contains("Versão anterior"),
            "pt-BR output should contain localized hint: {msg}"
        );
    }
}
