//! Balance: query the active provider's account balance or credit status.
//!
//! Provider-specific network dispatch is still pending. Until that lands, keep
//! this command explicit about being a scaffold so users do not mistake it for
//! a live balance lookup.

use crate::config::ApiProvider;
use crate::tui::app::App;

use crate::commands::CommandResult;

/// Query provider account balance / credits.
pub fn balance(app: &mut App) -> CommandResult {
    let provider = app.api_provider;
    match provider {
        ApiProvider::Deepseek
        | ApiProvider::DeepseekCN
        | ApiProvider::Openrouter
        | ApiProvider::Novita => CommandResult::message(format!(
            "Balance check for {} is planned, but provider balance network dispatch is not wired in this build yet.",
            provider.display_name()
        )),
        _ => CommandResult::message(format!(
            "Balance check is not supported for {} yet. Check the provider dashboard for account balance details.",
            provider.display_name()
        )),
    }
}
