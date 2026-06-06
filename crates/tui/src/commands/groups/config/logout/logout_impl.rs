use crate::commands::CommandResult;
use crate::config::clear_active_provider_api_key;
use crate::tui::app::App;
use crate::tui::app::OnboardingState;

pub fn logout(app: &mut App) -> CommandResult {
    let provider_name = app.api_provider.as_str();
    match clear_active_provider_api_key(provider_name) {
        Ok(()) => {
            app.onboarding = OnboardingState::ApiKey;
            app.onboarding_needs_api_key = true;
            app.api_key_input.clear();
            app.api_key_cursor = 0;
            CommandResult::message(format!(
                "Cleared API key for {provider_name}. \
                 Use `codewhale auth clear --provider <id>` to clear a different provider."
            ))
        }
        Err(e) => CommandResult::error(format!("Failed to clear API key for {provider_name}: {e}")),
    }
}
