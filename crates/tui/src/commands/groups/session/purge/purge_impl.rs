use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub(crate) fn purge(_app: &mut App) -> CommandResult {
    CommandResult::with_message_and_action(
        "Agent context purge triggered...".to_string(),
        AppAction::PurgeContext,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::session::test_support::create_test_app_with_tmpdir;
    use tempfile::TempDir;

    #[test]
    fn purge_triggers_context_purge_action() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = purge(&mut app);

        assert!(result.message.is_some());
        assert!(matches!(result.action, Some(AppAction::PurgeContext)));
    }
}
