//! Balance: query the active provider's account balance or credit status.

use crate::config::ApiProvider;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

/// Query provider account balance / credits.
pub fn balance(app: &mut App) -> CommandResult {
    let provider = app.api_provider;
    match provider {
        ApiProvider::Deepseek
        | ApiProvider::DeepseekCN
        | ApiProvider::Openrouter
        | ApiProvider::Novita => CommandResult::action(AppAction::FetchBalance),
        _ => CommandResult::message(format!(
            "Balance check is not supported for {} yet. Check the provider dashboard for account balance details.",
            provider.display_name()
        )),
    }
}
