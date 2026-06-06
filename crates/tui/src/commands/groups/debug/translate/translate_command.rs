//! Translate command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Translate;
impl Command for Translate {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "translate",
            aliases: &["translation", "transale"],
            usage: "/translate",
            description_id: MessageId::CmdTranslateDescription,
        }
    }

    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::translate_impl::translate(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Translate.info();
        assert_eq!(info.name, "translate");
        assert_eq!(info.usage, "/translate");
        assert!(info.aliases.contains(&"translation"));
    }

    #[test]
    fn execute_toggles_translation() {
        let mut app = crate::commands::groups::test_support::test_app();
        app.translation_enabled = false;
        let result = Translate.execute(&mut app, None);
        assert!(!result.is_error);
        assert!(app.translation_enabled);
        assert!(result.message.is_some());
    }
}
