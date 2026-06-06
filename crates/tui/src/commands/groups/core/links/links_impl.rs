use crate::commands::CommandResult;
use crate::localization::{MessageId, tr};
use crate::tui::app::App;

pub(crate) fn deepseek_links(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    CommandResult::message(format!(
        "{}\n\
-----------------------------\n\
{} https://platform.deepseek.com\n\
{}      https://platform.deepseek.com/docs\n\n\
{}",
        tr(locale, MessageId::LinksTitle),
        tr(locale, MessageId::LinksDashboard),
        tr(locale, MessageId::LinksDocs),
        tr(locale, MessageId::LinksTip),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::core::test_support::create_test_app;

    #[test]
    fn test_deepseek_links() {
        let mut app = create_test_app();
        let result = deepseek_links(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("DeepSeek Links"));
        assert!(msg.contains("https://platform.deepseek.com"));
        assert!(result.action.is_none());
    }
}
