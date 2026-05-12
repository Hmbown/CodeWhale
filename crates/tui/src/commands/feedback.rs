use super::CommandResult;
use crate::tui::app::{App, AppAction};

const SECURITY_POLICY_URL: &str = "https://github.com/Hmbown/DeepSeek-TUI/security/policy";

pub fn feedback(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return CommandResult::action(AppAction::OpenFeedbackPicker);
    }
    if matches!(raw, "help" | "--help" | "-h") {
        return CommandResult::message(feedback_help());
    }

    let kind = match parse_feedback_kind(raw) {
        Some(parsed) => parsed,
        None => {
            return CommandResult::error(
                "Unknown feedback type. Use `/feedback` to list feedback options.",
            );
        }
    };

    if matches!(kind, FeedbackKind::Security) {
        return CommandResult::with_message_and_action(
            format!(
                "Review the project's security policy before reporting a vulnerability.\n\n\
                 Trying to open it in your browser. If that fails, open this URL manually:\n\n\
                 {SECURITY_POLICY_URL}\n\n\
                 Do not include sensitive security details in a public issue.",
            ),
            AppAction::OpenExternalUrl {
                url: SECURITY_POLICY_URL.to_string(),
                label: "GitHub security policy".to_string(),
            },
        );
    }

    let url = kind.issue_url();
    let message = format!(
        "Trying to open GitHub {} template in your browser. If that fails, open this URL manually:\n\n{}",
        kind.label().to_ascii_lowercase(),
        url,
    );

    CommandResult::with_message_and_action(
        message,
        AppAction::OpenExternalUrl {
            url,
            label: format!("GitHub {}", kind.label().to_ascii_lowercase()),
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedbackKind {
    Bug,
    Feature,
    Security,
}

impl FeedbackKind {
    fn label(self) -> &'static str {
        match self {
            Self::Bug => "Bug report",
            Self::Feature => "Feature request",
            Self::Security => "Security vulnerability",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Bug => "Report a problem or regression",
            Self::Feature => "Suggest an idea or improvement",
            Self::Security => "Review the security policy",
        }
    }

    fn issue_url_base(self) -> &'static str {
        match self {
            Self::Bug => "https://github.com/Hmbown/DeepSeek-TUI/issues/new?template=bug_report.md",
            Self::Feature => {
                "https://github.com/Hmbown/DeepSeek-TUI/issues/new?template=feature_request.md"
            }
            Self::Security => SECURITY_POLICY_URL,
        }
    }

    fn issue_url(self) -> String {
        self.issue_url_base().to_string()
    }
}

fn feedback_help() -> String {
    let rows = [
        ("1", FeedbackKind::Bug),
        ("2", FeedbackKind::Feature),
        ("3", FeedbackKind::Security),
    ];
    let mut message = String::from("Choose a feedback type:\n\n");
    for (number, kind) in rows {
        message.push_str(&format!(
            "{number}. {}    {}\n",
            kind.label(),
            kind.description()
        ));
    }
    message.push_str("\nUsage:\n");
    for (number, kind) in rows {
        message.push_str(&format!("/feedback {number}    {}\n", kind.label()));
    }
    message.push_str("/feedback bug\n");
    message.push_str("/feedback feature\n");
    message.push_str("/feedback security\n");
    message
}

fn parse_feedback_kind(input: &str) -> Option<FeedbackKind> {
    Some(match input.to_ascii_lowercase().as_str() {
        "1" | "bug" | "bug-report" | "bug_report" => FeedbackKind::Bug,
        "2" | "feature" | "feature-request" | "feature_request" | "enhancement" => {
            FeedbackKind::Feature
        }
        "3" | "security" | "vulnerability" | "private" => FeedbackKind::Security,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn test_app() -> (App, TempDir) {
        let tmpdir = TempDir::new().expect("tempdir");
        let workspace = tmpdir.path().to_path_buf();
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: workspace.clone(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: workspace.join("skills"),
            memory_path: workspace.join("memory.md"),
            notes_path: workspace.join("notes.txt"),
            mcp_config_path: workspace.join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.current_session_id = Some("session-123".to_string());
        (app, tmpdir)
    }

    fn external_url(result: &CommandResult) -> &str {
        match result.action.as_ref() {
            Some(AppAction::OpenExternalUrl { url, .. }) => url,
            other => panic!("expected external URL action, got {other:?}"),
        }
    }

    #[test]
    fn feedback_without_args_opens_feedback_picker() {
        let (mut app, _tmpdir) = test_app();
        let result = feedback(&mut app, None);
        assert_eq!(result.action, Some(AppAction::OpenFeedbackPicker));
        assert!(result.message.is_none());
        assert!(!result.is_error);
    }

    #[test]
    fn feedback_help_lists_feedback_types() {
        let (mut app, _tmpdir) = test_app();
        let result = feedback(&mut app, Some("--help"));
        let message = result.message.expect("feedback help");
        assert!(message.contains("1. Bug report"));
        assert!(message.contains("2. Feature request"));
        assert!(message.contains("3. Security vulnerability"));
        assert!(!message.contains("Blank issue"));
        assert!(message.contains("/feedback bug"));
        assert!(!message.contains("<description>"));
    }

    #[test]
    fn feedback_bug_opens_bug_template_url_without_prefilled_body() {
        let (mut app, _tmpdir) = test_app();
        let result = feedback(&mut app, Some("bug"));
        assert!(!result.is_error);
        let message = result
            .message
            .as_deref()
            .expect("feedback command returns guidance");
        let url = external_url(&result);

        assert!(message.contains("Trying to open GitHub bug report template"));
        assert!(message.contains("open this URL manually"));
        assert!(message.contains(url));
        assert!(url.contains("template=bug_report.md"));
        assert!(!url.contains("title="));
        assert!(!url.contains("body="));
    }

    #[test]
    fn feedback_feature_generates_feature_template_url() {
        let (mut app, _tmpdir) = test_app();
        let result = feedback(&mut app, Some("2"));
        let message = result
            .message
            .as_deref()
            .expect("feedback command returns guidance");
        let url = external_url(&result);
        assert!(message.contains("Trying to open GitHub feature request template"));
        assert!(message.contains("open this URL manually"));
        assert!(message.contains(url));
        assert!(url.contains("template=feature_request.md"));
        assert!(!url.contains("title="));
        assert!(!url.contains("body="));
    }

    #[test]
    fn feedback_template_urls_do_not_prefill_titles() {
        let (mut app, _tmpdir) = test_app();
        let bug = feedback(&mut app, Some("bug"));
        let feature = feedback(&mut app, Some("feature"));

        assert!(!external_url(&bug).contains("title="));
        assert!(!external_url(&feature).contains("title="));
    }

    #[test]
    fn feedback_urls_use_template_only() {
        let bug = FeedbackKind::Bug.issue_url();
        let feature = FeedbackKind::Feature.issue_url();

        assert_eq!(
            bug,
            "https://github.com/Hmbown/DeepSeek-TUI/issues/new?template=bug_report.md"
        );
        assert_eq!(
            feature,
            "https://github.com/Hmbown/DeepSeek-TUI/issues/new?template=feature_request.md"
        );
    }

    #[test]
    fn feedback_security_uses_security_policy() {
        let (mut app, _tmpdir) = test_app();
        let result = feedback(&mut app, Some("security"));
        let message = result
            .message
            .as_deref()
            .expect("security feedback message");
        assert_eq!(external_url(&result), SECURITY_POLICY_URL);
        assert!(message.contains(SECURITY_POLICY_URL));
        assert!(message.contains("Do not include sensitive security details"));
        assert!(!message.contains("/issues/new"));
    }

    #[test]
    fn feedback_unknown_type_returns_error() {
        let (mut app, _tmpdir) = test_app();
        let result = feedback(&mut app, Some("other thing"));
        assert!(result.is_error);
        let message = result.message.expect("error message");
        assert!(message.contains("Unknown feedback type"));
    }
}
