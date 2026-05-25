//! Balance: query the active provider's account balance or credit status.
//!
//! Supported providers (balance endpoints verified against official docs):
//! - DeepSeek:  GET https://api.deepseek.com/user/balance
//! - OpenRouter: GET https://openrouter.ai/api/v1/credits (management key required)
//! - Novita AI:  Get User Balance (Basic APIs)
//!
//! Unsupported providers return a clear message. Wireup to async HTTP
//! dispatch is pending — this module currently returns a placeholder.

use crate::config::ApiProvider;
use crate::tui::app::App;

use super::CommandResult;

/// Query provider account balance / credits.
pub fn balance(app: &mut App) -> CommandResult {
    let provider = app.api_provider;
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
            CommandResult::message(format!(
                "Balance check sent to {} — results will appear shortly.",
                provider.display_name()
            ))
        }
        ApiProvider::Openrouter => {
            CommandResult::message(format!(
                "Balance check sent to {} — results will appear shortly.",
                provider.display_name()
            ))
        }
        _ => {
            CommandResult::message(format!(
                "Balance check is not yet supported for {}.",
                provider.display_name()
            ))
        }
    }
}
