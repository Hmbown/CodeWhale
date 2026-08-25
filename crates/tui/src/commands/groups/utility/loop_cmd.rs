//! `/loop [interval] <prompt>` — create an in-session watcher automation.

use codewhale_command_contract::facets::CommandPresentationContext;
use codewhale_command_contract::handler::{CommandCapabilities, CommandContexts, CommandHandler};
use codewhale_command_contract::metadata::{CommandInfo, RegisterCommand};

use crate::automation_manager::{parse_loop_interval, rrule_for_loop_interval};
use crate::commands::CommandResult;
use crate::tui::app::{AppAction, AutomationAction};

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "loop",
    aliases: &["watch", "watcher"],
    usage: "/loop [interval] <prompt>",
    description_key: "cmd_loop_description",
};

pub(in crate::commands) struct LoopCmd;

impl RegisterCommand<CommandResult> for LoopCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn handler() -> CommandHandler<CommandResult> {
        CommandHandler::Contextual {
            capabilities: CommandCapabilities::PRESENTATION,
            handler: loop_contextual,
        }
    }
}

fn loop_contextual(contexts: CommandContexts<'_>, arg: Option<&str>) -> CommandResult {
    let mut parts = contexts.into_parts();
    let Some(presentation) = parts.presentation.as_deref_mut() else {
        return CommandResult::error("Command capability unavailable: presentation");
    };
    loop_command(presentation, arg)
}

fn loop_command(
    presentation: &mut dyn CommandPresentationContext,
    args: Option<&str>,
) -> CommandResult {
    let raw = args.unwrap_or("").trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("list") {
        return CommandResult::action(AppAction::Automation(AutomationAction::List));
    }

    let mut tokens = raw.split_whitespace();
    let first = tokens.next().unwrap_or("");
    match first.to_ascii_lowercase().as_str() {
        "show" | "status" => return single_id(presentation, tokens, AutomationAction::Show),
        "pause" | "stop" => return single_id(presentation, tokens, AutomationAction::Pause),
        "resume" => return single_id(presentation, tokens, AutomationAction::Resume),
        "delete" | "remove" | "rm" => {
            let Some(id) = tokens.next() else {
                return usage_error(presentation);
            };
            if tokens.next().is_some() {
                return usage_error(presentation);
            }
            return CommandResult::action(AppAction::Automation(AutomationAction::Delete {
                id: id.to_string(),
                confirmation: None,
            }));
        }
        "run" | "now" => return single_id(presentation, tokens, AutomationAction::Run),
        _ => {}
    }

    let Ok(interval) = parse_loop_interval(first) else {
        return usage_error(presentation);
    };
    let prompt = raw[first.len()..].trim();
    if prompt.is_empty() {
        return usage_error(presentation);
    }
    let Ok(rrule) = rrule_for_loop_interval(interval) else {
        return usage_error(presentation);
    };
    let name = loop_name(prompt);
    CommandResult::action(AppAction::Automation(AutomationAction::Create {
        name,
        prompt: prompt.to_string(),
        rrule,
        interval_label: first.to_string(),
    }))
}

fn loop_name(prompt: &str) -> String {
    let mut name = String::new();
    for word in prompt.split_whitespace() {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(word);
        if name.chars().count() >= 48 {
            break;
        }
    }
    if name.is_empty() {
        "Loop".to_string()
    } else {
        name
    }
}

fn single_id<'a>(
    presentation: &mut dyn CommandPresentationContext,
    mut tokens: impl Iterator<Item = &'a str>,
    make_action: fn(String) -> AutomationAction,
) -> CommandResult {
    let Some(id) = tokens.next() else {
        return usage_error(presentation);
    };
    if tokens.next().is_some() {
        return usage_error(presentation);
    }
    CommandResult::action(AppAction::Automation(make_action(id.to_string())))
}

fn usage_error(presentation: &mut dyn CommandPresentationContext) -> CommandResult {
    match presentation.translate("loop_usage", &[]) {
        Ok(text) => CommandResult::error(text),
        Err(_) => CommandResult::error(
            "Usage: /loop [interval] <prompt>  (e.g. /loop 45m continue the handoff)",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakePresentation;
    impl CommandPresentationContext for FakePresentation {
        fn translate(&self, key: &str, _r: &[(&str, &str)]) -> Result<String, String> {
            if key == "loop_usage" {
                Ok("Usage: /loop [interval] <prompt>".to_string())
            } else {
                Err("unknown".to_string())
            }
        }
    }

    fn parsed(args: Option<&str>) -> Option<AutomationAction> {
        match loop_command(&mut FakePresentation, args).action {
            Some(AppAction::Automation(action)) => Some(action),
            _ => None,
        }
    }

    #[test]
    fn parses_interval_and_prompt() {
        let Some(AutomationAction::Create {
            prompt,
            rrule,
            interval_label,
            ..
        }) = parsed(Some("45m continue the market-readiness handoff"))
        else {
            panic!("expected create");
        };
        assert_eq!(prompt, "continue the market-readiness handoff");
        assert_eq!(rrule, "FREQ=MINUTELY;INTERVAL=45");
        assert_eq!(interval_label, "45m");
    }

    #[test]
    fn list_and_stop_reuse_automation_actions() {
        assert_eq!(parsed(None), Some(AutomationAction::List));
        assert_eq!(
            parsed(Some("stop abc")),
            Some(AutomationAction::Pause("abc".to_string()))
        );
    }

    #[test]
    fn rejects_missing_prompt() {
        let result = loop_command(&mut FakePresentation, Some("45m"));
        assert!(result.is_error);
        assert!(result.action.is_none());
    }
}
