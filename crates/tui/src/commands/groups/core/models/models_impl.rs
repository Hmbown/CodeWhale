use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub(crate) fn models(_app: &mut App) -> CommandResult {
    CommandResult::action(AppAction::FetchModels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::core::test_support::create_test_app;

    #[test]
    fn test_models_triggers_fetch_action() {
        let mut app = create_test_app();
        let result = models(&mut app);
        assert!(result.message.is_none());
        assert!(matches!(result.action, Some(AppAction::FetchModels)));
    }
}
