//! Numbered, confirmation-gated editor for the active `permissions.toml`.

use codewhale_config::{PermissionsFileState, PermissionsSnapshot, ToolAskRule};
use codewhale_execpolicy::PermissionAction;

use crate::commands::CommandResult;
use crate::localization::{MessageId, tr};
use crate::tui::app::{App, AppAction};

pub(super) fn permissions_command(app: &App, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");
    if raw.is_empty() || raw.eq_ignore_ascii_case("list") || raw.eq_ignore_ascii_case("status") {
        return list_permissions(app);
    }

    let parts = raw.split_whitespace().collect::<Vec<_>>();
    if parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case("remove"))
    {
        return remove_permission(app, &parts);
    }
    usage_error(app)
}

fn list_permissions(app: &App) -> CommandResult {
    let snapshot = match load_snapshot(app) {
        Ok(snapshot) => snapshot,
        Err(error) => return operation_error(app, &error),
    };
    CommandResult::message(format_snapshot(app, &snapshot))
}

fn remove_permission(app: &App, parts: &[&str]) -> CommandResult {
    if !matches!(parts.len(), 2 | 4) {
        return usage_error(app);
    }
    let Ok(display_index) = parts[1].parse::<usize>() else {
        return usage_error(app);
    };
    let Some(index) = display_index.checked_sub(1) else {
        return rule_not_found(app, display_index);
    };

    if parts.len() == 2 {
        let snapshot = match load_snapshot(app) {
            Ok(snapshot) => snapshot,
            Err(error) => return operation_error(app, &error),
        };
        let Some(rule) = snapshot.rules().get(index) else {
            return rule_not_found(app, display_index);
        };
        let token = snapshot
            .removal_token(index)
            .expect("snapshots carry one removal token per rule");
        let command = format!("/permissions remove {display_index} --confirm {token}");
        let rule = format_rule(app, display_index, rule);
        let message = tr(app.ui_locale, MessageId::PermissionsRemovePreview)
            .replace("{index}", &display_index.to_string())
            .replace("{rule}", &rule)
            .replace("{command}", &command);
        return CommandResult::message(message);
    }

    if !parts[2].eq_ignore_ascii_case("--confirm") || parts[3].is_empty() {
        return usage_error(app);
    }
    let removed =
        match codewhale_config::remove_permission_rule(app.config_path.clone(), index, parts[3]) {
            Ok(rule) => rule,
            Err(error) => return operation_error(app, &error),
        };
    let message = tr(app.ui_locale, MessageId::PermissionsRemoved)
        .replace("{index}", &display_index.to_string())
        .replace("{action}", action_name(removed.action))
        .replace("{tool}", &escape_field(&removed.tool));
    CommandResult::with_message_and_action(message, AppAction::PermissionRulesChanged)
}

fn load_snapshot(app: &App) -> anyhow::Result<PermissionsSnapshot> {
    codewhale_config::load_permissions_snapshot(app.config_path.clone())
}

fn format_snapshot(app: &App, snapshot: &PermissionsSnapshot) -> String {
    let file_state = match snapshot.file_state() {
        PermissionsFileState::Missing => MessageId::PermissionsFileMissing,
        PermissionsFileState::Empty => MessageId::PermissionsFileEmpty,
        PermissionsFileState::Present => MessageId::PermissionsFilePresent,
    };
    let path = codewhale_config::quote_os_path(snapshot.path());
    let mut output = tr(app.ui_locale, MessageId::PermissionsListHeader)
        .replace("{count}", &snapshot.rules().len().to_string())
        .replace("{file_state}", &tr(app.ui_locale, file_state))
        .replace("{path}", &path);
    if snapshot.rules().is_empty() {
        output.push('\n');
        output.push_str(&tr(app.ui_locale, MessageId::PermissionsNoRules));
    } else {
        for (index, rule) in snapshot.rules().iter().enumerate() {
            output.push_str("\n\n");
            output.push_str(&format_rule(app, index + 1, rule));
        }
    }
    output.push_str("\n\n");
    output.push_str(&format_posture_explainer(app));
    output
}

/// What the active permission posture decides on its own and what it never
/// decides, so a person can predict Auto-Review without reading the policy
/// engine. Rules above are the durable allow/ask/deny surface; the posture is
/// the session-only layer that decides everything the rules did not.
fn format_posture_explainer(app: &App) -> String {
    let posture = app.approval_mode;
    let mut text = tr(app.ui_locale, MessageId::PermissionsPostureHeader)
        .replace("{posture}", posture.permission_chip_label());
    text.push('\n');
    text.push_str(&tr(
        app.ui_locale,
        match posture {
            crate::tui::approval::ApprovalMode::Suggest => MessageId::PermissionsPostureAsk,
            crate::tui::approval::ApprovalMode::Auto => MessageId::PermissionsPostureAuto,
            crate::tui::approval::ApprovalMode::Bypass => MessageId::PermissionsPostureBypass,
            crate::tui::approval::ApprovalMode::Never => MessageId::PermissionsPostureNever,
        },
    ));
    text.push('\n');
    let audit_path = crate::audit::audit_log_path()
        .map(|path| codewhale_config::quote_os_path(&path))
        .unwrap_or_else(|| "$CODEWHALE_HOME/audit.log".to_string());
    text.push_str(
        &tr(app.ui_locale, MessageId::PermissionsReceiptsNote).replace("{audit_path}", &audit_path),
    );
    text
}

fn format_rule(app: &App, display_index: usize, rule: &ToolAskRule) -> String {
    let scope = rule.workspace.as_deref().map_or_else(
        || tr(app.ui_locale, MessageId::PermissionsScopeGlobal).into_owned(),
        |workspace| {
            tr(app.ui_locale, MessageId::PermissionsScopeRepo)
                .replace("{workspace}", &escape_field(workspace))
        },
    );
    let applicability = if rule_applies_in_workspace(rule, &app.workspace) {
        tr(app.ui_locale, MessageId::PermissionsAppliesHere)
    } else {
        tr(app.ui_locale, MessageId::PermissionsInactiveHere)
    };
    tr(app.ui_locale, MessageId::PermissionsRuleEntry)
        .replace("{index}", &display_index.to_string())
        .replace("{action}", action_name(rule.action))
        .replace("{tool}", &escape_field(&rule.tool))
        .replace("{matcher}", &format_matcher(app, rule))
        .replace("{scope}", &scope)
        .replace("{applicability}", &applicability)
}

fn format_matcher(app: &App, rule: &ToolAskRule) -> String {
    let mut matchers = Vec::new();
    if let Some(command) = rule.command.as_deref() {
        let message_id = if rule.command_exact {
            MessageId::PermissionsMatchExactCommand
        } else {
            MessageId::PermissionsMatchCommandPrefix
        };
        matchers.push(tr(app.ui_locale, message_id).replace("{command}", &escape_field(command)));
    }
    if let Some(path) = rule.path.as_deref() {
        matchers.push(
            tr(app.ui_locale, MessageId::PermissionsMatchExactPath)
                .replace("{path}", &escape_field(path)),
        );
    }
    if matchers.is_empty() {
        tr(app.ui_locale, MessageId::PermissionsMatchAnyInvocation).into_owned()
    } else {
        matchers.join(" + ")
    }
}

fn rule_applies_in_workspace(rule: &ToolAskRule, workspace: &std::path::Path) -> bool {
    let Some(rule_workspace) = rule.workspace.as_deref() else {
        return true;
    };
    let workspace = workspace.to_string_lossy();
    let Some(rule_workspace) = codewhale_execpolicy::normalize_workspace_scope(rule_workspace)
    else {
        return false;
    };
    let Some(workspace) = codewhale_execpolicy::normalize_workspace_scope(&workspace) else {
        return false;
    };
    rule_workspace == workspace
}

fn action_name(action: PermissionAction) -> &'static str {
    match action {
        PermissionAction::Allow => "allow",
        PermissionAction::Ask => "ask",
        PermissionAction::Deny => "deny",
    }
}

fn escape_field(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() || is_bidi_format_control(character) => {
                escaped.extend(character.escape_unicode());
            }
            character => escaped.push(character),
        }
    }
    escaped
}

fn is_bidi_format_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn usage_error(app: &App) -> CommandResult {
    CommandResult::error(tr(app.ui_locale, MessageId::PermissionsUsage))
}

fn rule_not_found(app: &App, display_index: usize) -> CommandResult {
    CommandResult::error(
        tr(app.ui_locale, MessageId::PermissionsRuleNotFound)
            .replace("{index}", &display_index.to_string()),
    )
}

fn operation_error(app: &App, error: &anyhow::Error) -> CommandResult {
    CommandResult::error(
        tr(app.ui_locale, MessageId::PermissionsOperationFailed)
            .replace("{error}", &format!("{error:#}")),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::localization::Locale;
    use crate::tui::app::TuiOptions;

    use super::*;

    fn test_app(config_path: std::path::PathBuf, workspace: std::path::PathBuf) -> App {
        let config = crate::config::Config::default();
        let mut app = App::new(
            TuiOptions {
                workspace,
                ..crate::test_support::test_tui_options(std::path::PathBuf::from("."))
            },
            &config,
        );
        app.config_path = Some(config_path);
        app.ui_locale = Locale::En;
        app
    }

    #[test]
    fn list_shows_source_scope_matcher_and_workspace_applicability() {
        let dir = tempfile::tempdir().expect("tempdir");
        let other = tempfile::tempdir().expect("other tempdir");
        let config_path = dir.path().join("config.toml");
        let permissions_path = dir.path().join("permissions.toml");
        fs::write(
            &permissions_path,
            format!(
                r#"
[[rules]]
tool = "exec_shell"
command = "cargo test"
command_exact = true
workspace = {workspace:?}
action = "allow"

[[rules]]
tool = "edit_file"
path = "src/lib.rs"
workspace = {other:?}
"#,
                workspace = dir.path().to_string_lossy(),
                other = other.path().to_string_lossy(),
            ),
        )
        .expect("write permissions");
        let displayed_permissions_path =
            codewhale_config::resolve_permissions_path(Some(config_path.clone()))
                .expect("resolve permissions path");
        let app = test_app(config_path, dir.path().to_path_buf());

        let result = permissions_command(&app, Some("list"));
        let message = result.message.expect("list message");

        assert!(!result.is_error);
        assert!(message.contains(&codewhale_config::quote_os_path(
            &displayed_permissions_path
        )));
        assert!(message.contains("#1 | allow | exec_shell"));
        assert!(message.contains("exact command `cargo test`"));
        assert!(message.contains("active in this workspace"));
        assert!(message.contains("#2 | ask | edit_file"));
        assert!(message.contains("exact normalized path `src/lib.rs`"));
        assert!(message.contains("not active in this workspace"));
    }

    #[test]
    fn list_preserves_missing_empty_and_malformed_diagnostics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let permissions_path = dir.path().join("permissions.toml");
        let displayed_permissions_path =
            codewhale_config::resolve_permissions_path(Some(config_path.clone()))
                .expect("resolve permissions path");
        let app = test_app(config_path, dir.path().to_path_buf());

        let missing = permissions_command(&app, None);
        let missing_message = missing.message.expect("missing message");
        assert!(!missing.is_error);
        assert!(missing_message.contains("File status: missing"));
        assert!(missing_message.contains("Rule count: 0"));

        fs::write(&permissions_path, "").expect("write empty permissions");
        let empty = permissions_command(&app, Some("status"));
        let empty_message = empty.message.expect("empty message");
        assert!(!empty.is_error);
        assert!(empty_message.contains("File status: empty"));

        fs::write(
            &permissions_path,
            "[[rules]]\ntool = \"do-not-echo-this\"\ncommand = ",
        )
        .expect("write malformed permissions");
        let malformed = permissions_command(&app, Some("list"));
        let malformed_message = malformed.message.expect("malformed message");
        assert!(malformed.is_error);
        assert!(malformed_message.contains("Permission rule operation failed"));
        assert!(malformed_message.contains(&codewhale_config::quote_os_path(
            &displayed_permissions_path
        )));
        assert!(malformed_message.contains("file contents were omitted"));
        assert!(!malformed_message.contains("do-not-echo-this"));
    }

    #[test]
    fn remove_requires_preview_token_then_emits_live_reload_action() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let permissions_path = dir.path().join("permissions.toml");
        let original = "[[rules]]\ntool = \"exec_shell\"\ncommand = \"cargo test\"\n";
        fs::write(&permissions_path, original).expect("write permissions");
        let app = test_app(config_path, dir.path().to_path_buf());

        let preview = permissions_command(&app, Some("remove 1"));
        let preview_message = preview.message.expect("preview message");
        assert!(!preview.is_error);
        assert_eq!(
            fs::read_to_string(&permissions_path).expect("read previewed permissions"),
            original
        );
        let confirm_command = preview_message
            .split('`')
            .find(|part| part.starts_with("/permissions remove 1 --confirm "))
            .expect("confirmation command");
        let confirm_arg = confirm_command
            .strip_prefix("/permissions ")
            .expect("command prefix");

        let confirmed = permissions_command(&app, Some(confirm_arg));

        assert!(!confirmed.is_error);
        assert_eq!(confirmed.action, Some(AppAction::PermissionRulesChanged));
        let persisted = fs::read_to_string(&permissions_path).expect("read edited permissions");
        let parsed: codewhale_config::PermissionsToml =
            toml::from_str(&persisted).expect("parse edited permissions");
        assert!(parsed.rules.is_empty());
    }

    #[test]
    fn legacy_config_ask_rules_entry_uses_the_permissions_editor_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        fs::write(
            dir.path().join("permissions.toml"),
            "[[rules]]\ntool = \"exec_shell\"\ncommand = \"cargo test\"\n",
        )
        .expect("write permissions");
        let mut app = test_app(config_path, dir.path().to_path_buf());

        let result = super::super::config::config_command(&mut app, Some("ask-rules list"));
        let message = result.message.expect("compatibility list message");

        assert!(!result.is_error);
        assert!(message.contains("Permission rules"));
        assert!(message.contains("#1 | ask | exec_shell"));
    }

    #[test]
    fn permissions_command_is_registered_with_compatibility_aliases() {
        let info = crate::commands::get_command_info("permissions").expect("permissions command");

        assert_eq!(info.name, "permissions");
        assert!(info.aliases.contains(&"permission-rules"));
        assert!(info.usage.contains("remove <rule-number>"));
    }

    #[test]
    fn invalid_workspace_scopes_never_appear_active() {
        let mut rule = ToolAskRule::exec_shell("cargo test");
        rule.workspace = Some("../not-an-absolute-scope".to_string());

        assert!(!rule_applies_in_workspace(
            &rule,
            std::path::Path::new("also-relative")
        ));
    }

    #[test]
    fn displayed_rule_fields_escape_terminal_and_bidi_controls() {
        assert_eq!(
            escape_field("cargo\u{1b}\n\u{202e}test"),
            "cargo\\u{1b}\\n\\u{202e}test"
        );
    }

    #[test]
    fn permission_messages_keep_placeholder_parity_across_complete_locales() {
        let ids = [
            MessageId::PermissionsListHeader,
            MessageId::PermissionsRuleEntry,
            MessageId::PermissionsMatchExactCommand,
            MessageId::PermissionsMatchCommandPrefix,
            MessageId::PermissionsMatchExactPath,
            MessageId::PermissionsScopeRepo,
            MessageId::PermissionsRemovePreview,
            MessageId::PermissionsRemoved,
            MessageId::PermissionsRuleNotFound,
            MessageId::PermissionsOperationFailed,
        ];
        for id in ids {
            let english = placeholders(&tr(Locale::En, id));
            for locale in Locale::shipped_complete() {
                assert_eq!(
                    placeholders(&tr(*locale, id)),
                    english,
                    "{} {id:?} placeholder drift",
                    locale.tag()
                );
            }
        }
    }

    fn placeholders(message: &str) -> std::collections::BTreeSet<String> {
        message
            .split('{')
            .skip(1)
            .filter_map(|suffix| suffix.split_once('}').map(|(name, _)| name.to_string()))
            .collect()
    }
}
