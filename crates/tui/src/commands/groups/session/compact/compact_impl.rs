use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub(crate) fn compact(_app: &mut App) -> CommandResult {
    CommandResult::with_message_and_action(
        "Context compaction triggered...".to_string(),
        AppAction::CompactContext,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::session::test_support::create_test_app_with_tmpdir;
    use tempfile::TempDir;

    #[test]
    fn test_compact_toggles_state() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = compact(&mut app);

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("compaction") || msg.contains("Compact"));
        assert!(matches!(result.action, Some(AppAction::CompactContext)));
    }
}
