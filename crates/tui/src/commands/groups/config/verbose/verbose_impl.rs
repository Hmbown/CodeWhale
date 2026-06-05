use crate::commands::CommandResult;
use crate::tui::app::App;

pub fn verbose(app: &mut App, arg: Option<&str>) -> CommandResult {
    let next = match arg.map(str::trim).filter(|s| !s.is_empty()) {
        None => !app.verbose_transcript,
        Some(raw) => match raw.to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "yes" => true,
            "off" | "false" | "0" | "no" => false,
            "toggle" => !app.verbose_transcript,
            _ => {
                return CommandResult::error(
                    "Usage: /verbose [on|off]. Compact thinking remains available when verbose is off.",
                );
            }
        },
    };

    app.verbose_transcript = next;
    app.mark_history_updated();
    CommandResult::message(if next {
        "Verbose transcript on: live thinking renders in full."
    } else {
        "Verbose transcript off: live thinking stays compact."
    })
}

