use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub(crate) fn profile_switch(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let profile_name = match arg {
        Some(name) if !name.trim().is_empty() => name.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /profile <name>\n\nSwitch to a named config profile. Profiles are defined in ~/.codewhale/config.toml under [profiles] sections.",
            );
        }
    };
    CommandResult::with_message_and_action(
        format!("Switching to profile '{profile_name}'..."),
        AppAction::SwitchProfile {
            profile: profile_name,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::core::test_support::create_test_app;

    #[test]
    fn profile_without_arg_returns_usage() {
        let mut app = create_test_app();
        let result = profile_switch(&mut app, None);
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage: /profile"));
    }

    #[test]
    fn profile_with_name_returns_switch_action() {
        let mut app = create_test_app();
        let result = profile_switch(&mut app, Some("work"));
        assert!(matches!(
            result.action,
            Some(AppAction::SwitchProfile { profile }) if profile == "work"
        ));
    }
}
