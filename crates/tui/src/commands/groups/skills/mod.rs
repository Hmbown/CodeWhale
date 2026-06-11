//! Skills command area: listing and running skills, review, and restore.

mod restore;
mod review;
#[allow(clippy::module_inception)]
mod skills;

pub(in crate::commands) use self::skills::run_skill_by_name;

use crate::commands::CommandResult;
use crate::tui::app::App;

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "skills" | "jinengliebiao" => skills::list_skills(app, arg),
        "skill" | "jineng" => skills::run_skill(app, arg),
        "review" | "shencha" => review::review(app, arg),
        "restore" => restore::restore(app, arg),
        _ => return None,
    };
    Some(result)
}
