//! Balance: query the active provider's remaining prepaid credit.

use crate::config::provider_has_balance_api;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

/// Query provider account balance / credits.
pub fn balance(app: &mut App) -> CommandResult {
    let provider = app.api_provider;
    if !provider_has_balance_api(provider) {
        return CommandResult::message(format!(
            "Balance check is not supported for {} yet. Check the provider dashboard for account balance details.",
            provider.display_name()
        ));
    }
    CommandResult::action(AppAction::FetchBalance)
}
