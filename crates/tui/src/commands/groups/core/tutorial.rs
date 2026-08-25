//! `/tutorial` (`/tour`) — opt-in onboarding pager (#5556).
//!
//! Never shown automatically. The first page maps concepts for operators
//! arriving from Claude Code, Cursor, or Codex; later pages cover keys,
//! the composer, models, Fleet, and workflows.

use crate::commands::CommandResult;
use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::{Locale, MessageId, tr};
use crate::tui::app::App;
use crate::tui::pager::{PagerPage, PagerView};

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "tutorial",
    aliases: &["tour"],
    usage: "/tutorial",
    description_id: MessageId::CmdTutorialDescription,
};

pub(in crate::commands) struct TutorialCmd;

impl RegisterCommand for TutorialCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        open_tutorial_pager(app);
        CommandResult::ok()
    }
}

const PAGES: &[(MessageId, MessageId)] = &[
    (
        MessageId::TutorialComingFromTitle,
        MessageId::TutorialComingFromBody,
    ),
    (MessageId::TutorialKeysTitle, MessageId::TutorialKeysBody),
    (
        MessageId::TutorialComposerTitle,
        MessageId::TutorialComposerBody,
    ),
    (MessageId::TutorialModelTitle, MessageId::TutorialModelBody),
    (MessageId::TutorialFleetTitle, MessageId::TutorialFleetBody),
    (
        MessageId::TutorialWorkflowsTitle,
        MessageId::TutorialWorkflowsBody,
    ),
];

fn tutorial_pages(locale: Locale, width: u16) -> Vec<PagerPage> {
    PAGES
        .iter()
        .map(|(title_id, body_id)| {
            let title = tr(locale, *title_id).into_owned();
            let body = tr(locale, *body_id);
            PagerPage::from_text(title, body.as_ref(), width)
        })
        .collect()
}

fn open_tutorial_pager(app: &mut App) {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80)
        .saturating_sub(2);
    app.view_stack.push(PagerView::from_pages(
        tutorial_pages(app.ui_locale, width),
        0,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{command_infos, execute};
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use crate::tui::views::ModalKind;
    use std::path::PathBuf;

    fn test_app() -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn tutorial_is_registered_with_tour_alias() {
        let info = command_infos()
            .into_iter()
            .find(|cmd| cmd.name == "tutorial")
            .expect("tutorial command");
        assert_eq!(info.aliases, &["tour"]);
        assert_eq!(info.usage, "/tutorial");
        assert!(info.description_for(Locale::En).contains("composer"));
    }

    #[test]
    fn tutorial_is_not_shown_on_a_fresh_session() {
        let app = test_app();
        assert!(app.view_stack.is_empty(), "tutorial must stay opt-in");
    }

    #[test]
    fn tutorial_opens_a_multipage_pager_starting_on_the_arrivals_page() {
        let pages = tutorial_pages(Locale::En, 72);
        assert_eq!(pages.len(), 6);
        let first_title = tr(Locale::En, MessageId::TutorialComingFromTitle);
        assert!(first_title.contains("Claude"));
        assert!(first_title.contains("Cursor"));
        assert!(first_title.contains("Codex"));
        let first_body = tr(Locale::En, MessageId::TutorialComingFromBody);
        assert!(first_body.contains("Claude"));
        assert!(first_body.contains("Cursor"));
        assert!(first_body.contains("Codex"));

        let mut app = test_app();
        let result = execute("/tutorial", &mut app);
        assert!(!result.is_error, "{result:?}");
        assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Pager));

        let mut via_alias = test_app();
        let alias = execute("/tour", &mut via_alias);
        assert!(!alias.is_error, "{alias:?}");
        assert_eq!(via_alias.view_stack.top_kind(), Some(ModalKind::Pager));
    }
}
