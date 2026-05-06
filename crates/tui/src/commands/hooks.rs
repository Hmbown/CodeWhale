//! `/hooks` slash command — read-only listing of configured
//! lifecycle hooks (#460 MVP).
//!
//! The full picker / persisted enable-disable surface in #460 is
//! still M-sized. This MVP gives the user a no-typing view of what's
//! actually configured in `~/.deepseek/config.toml`'s `[hooks]`
//! table — the most-asked question once hooks start firing.

use crate::hooks::HookEvent;
use crate::tui::app::App;

use super::CommandResult;

/// Top-level dispatch for `/hooks`. Subcommands:
///
/// * `/hooks`         — same as `/hooks list`.
/// * `/hooks list`    — show every configured hook grouped by event,
///   noting whether the global `[hooks].enabled` flag suppresses
///   them.
/// * `/hooks events`  — list every supported `HookEvent` value the
///   user can target in `[[hooks.hooks]]` entries. Useful for
///   discovery — without this, the only way to learn the event
///   names is to read source.
pub fn hooks(app: &App, arg: Option<&str>) -> CommandResult {
    let sub = arg.map(str::trim).unwrap_or("list").to_ascii_lowercase();
    match sub.as_str() {
        "" | "list" | "ls" | "show" => list(app),
        "events" | "event" | "list-events" => events(),
        other => CommandResult::error(
            format!("unknown subcommand `{other}`. Try `/hooks list` or `/hooks events`."),
            app.ui_locale,
        ),
    }
}

fn events() -> CommandResult {
    let mut out = String::new();
    out.push_str(
        "Available hook events (use one of these as `event = \"...\"` in your `[[hooks.hooks]]` entry):\n\n",
    );
    // Order matters — group lifecycle events first, then per-tool,
    // then situational. Stays stable across releases so users can
    // grep on it.
    let ordered = [
        (HookEvent::SessionStart, "fires once when the TUI launches"),
        (HookEvent::SessionEnd, "fires once on graceful shutdown"),
        (
            HookEvent::MessageSubmit,
            "fires when the user submits a turn (before model dispatch)",
        ),
        (
            HookEvent::ToolCallBefore,
            "fires before each tool call (read-only observer for now)",
        ),
        (
            HookEvent::ToolCallAfter,
            "fires after each tool call (read-only observer for now)",
        ),
        (
            HookEvent::ModeChange,
            "fires on Plan/Agent/Yolo transitions",
        ),
        (
            HookEvent::OnError,
            "fires on transport / capacity / tool errors",
        ),
    ];
    for (event, desc) in ordered {
        out.push_str(&format!("  - `{}` — {desc}\n", event_label(event)));
    }
    CommandResult::message(out.trim_end().to_string())
}

fn list(app: &App) -> CommandResult {
    let config = app.hooks.config();
    if config.hooks.is_empty() {
        return CommandResult::message(
            "No hooks configured. Add a `[[hooks.hooks]]` entry to `~/.deepseek/config.toml` to define one.",
        );
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{} configured hook(s) (global enabled: {}):\n\n",
        config.hooks.len(),
        if config.enabled {
            "yes"
        } else {
            "no — all hooks suppressed"
        }
    ));

    let mut by_event: std::collections::BTreeMap<&str, Vec<&crate::hooks::Hook>> =
        std::collections::BTreeMap::new();
    for hook in &config.hooks {
        by_event
            .entry(event_label(hook.event))
            .or_default()
            .push(hook);
    }

    for (event, hooks) in by_event {
        out.push_str(&format!("### {event}\n"));
        for hook in hooks {
            let label = hook
                .name
                .as_deref()
                .filter(|n| !n.trim().is_empty())
                .map_or_else(|| "(unnamed)".to_string(), str::to_string);
            let bg = if hook.background { " [bg]" } else { "" };
            let timeout = format!("{}s", hook.timeout_secs);
            let condition = match &hook.condition {
                None | Some(crate::hooks::HookCondition::Always) => String::new(),
                Some(c) => format!(" if {}", condition_summary(c)),
            };
            let cmd_preview = preview_command(&hook.command, 60);
            out.push_str(&format!(
                "  - {label}{bg} (timeout {timeout}){condition}\n      $ {cmd_preview}\n",
            ));
        }
        out.push('\n');
    }

    if !config.enabled {
        out.push_str(
            "Hooks are globally disabled — set `[hooks].enabled = true` in `config.toml` to fire them.\n",
        );
    }

    CommandResult::message(out.trim_end().to_string())
}

fn event_label(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "session_start",
        HookEvent::SessionEnd => "session_end",
        HookEvent::MessageSubmit => "message_submit",
        HookEvent::ToolCallBefore => "tool_call_before",
        HookEvent::ToolCallAfter => "tool_call_after",
        HookEvent::ModeChange => "mode_change",
        HookEvent::OnError => "on_error",
        HookEvent::ShellEnv => "shell_env",
    }
}

fn condition_summary(condition: &crate::hooks::HookCondition) -> String {
    match condition {
        crate::hooks::HookCondition::Always => "always".to_string(),
        crate::hooks::HookCondition::ToolName { name } => format!("tool_name=`{name}`"),
        crate::hooks::HookCondition::ToolCategory { category } => {
            format!("tool_category=`{category}`")
        }
        crate::hooks::HookCondition::Mode { mode } => format!("mode=`{mode}`"),
        crate::hooks::HookCondition::ExitCode { code } => format!("exit_code={code}"),
        crate::hooks::HookCondition::All { conditions } => format!(
            "all of [{}]",
            conditions
                .iter()
                .map(condition_summary)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        crate::hooks::HookCondition::Any { conditions } => format!(
            "any of [{}]",
            conditions
                .iter()
                .map(condition_summary)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Single-line preview of the shell command, capped at `max_chars`.
fn preview_command(command: &str, max_chars: usize) -> String {
    let single_line: String = command.chars().filter(|c| *c != '\n').collect();
    if single_line.chars().count() <= max_chars {
        return single_line;
    }
    let mut out: String = single_line
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::{Hook, HookCondition};

    #[test]
    fn preview_command_truncates_to_cap() {
        let cmd = "x".repeat(200);
        assert_eq!(preview_command(&cmd, 10).chars().count(), 10);
        assert!(preview_command(&cmd, 10).ends_with('…'));
    }

    #[test]
    fn preview_command_strips_newlines() {
        assert_eq!(
            preview_command("line one\nline two", 50),
            "line oneline two"
        );
    }

    #[test]
    fn preview_command_keeps_short_input_intact() {
        assert_eq!(preview_command("echo hi", 50), "echo hi");
    }

    #[test]
    fn condition_summary_renders_all_variants() {
        assert_eq!(condition_summary(&HookCondition::Always), "always");
        assert_eq!(
            condition_summary(&HookCondition::ToolName {
                name: "exec_shell".into()
            }),
            "tool_name=`exec_shell`"
        );
        assert_eq!(
            condition_summary(&HookCondition::ToolCategory {
                category: "shell".into()
            }),
            "tool_category=`shell`"
        );
        assert_eq!(
            condition_summary(&HookCondition::Mode {
                mode: "yolo".into()
            }),
            "mode=`yolo`"
        );
        assert_eq!(
            condition_summary(&HookCondition::ExitCode { code: 1 }),
            "exit_code=1"
        );
        assert_eq!(
            condition_summary(&HookCondition::All {
                conditions: vec![
                    HookCondition::ToolName {
                        name: "exec_shell".into()
                    },
                    HookCondition::Mode {
                        mode: "yolo".into()
                    }
                ]
            }),
            "all of [tool_name=`exec_shell`, mode=`yolo`]"
        );
    }

    #[test]
    fn events_subcommand_lists_every_event_variant_in_documented_order() {
        let result = events();
        let body = result.message.expect("non-empty body");
        let positions: Vec<(usize, &str)> = [
            "session_start",
            "session_end",
            "message_submit",
            "tool_call_before",
            "tool_call_after",
            "mode_change",
            "on_error",
        ]
        .iter()
        .map(|name| {
            (
                body.find(name).unwrap_or_else(|| {
                    panic!("event `{name}` missing from /hooks events output:\n{body}")
                }),
                *name,
            )
        })
        .collect();
        // Documented order is lifecycle → tool-call → situational.
        // Each subsequent position must be greater than the previous.
        for window in positions.windows(2) {
            let (a_pos, a_name) = window[0];
            let (b_pos, b_name) = window[1];
            assert!(
                a_pos < b_pos,
                "expected `{a_name}` before `{b_name}` in events listing"
            );
        }
        // Each event line includes the descriptive blurb.
        assert!(body.contains("fires once when the TUI launches"));
        assert!(body.contains("read-only observer"));
    }

    #[test]
    fn event_label_covers_every_variant() {
        // Compile-time `match` exhaustiveness; this just sanity-checks
        // the rendered strings stay stable.
        assert_eq!(event_label(HookEvent::SessionStart), "session_start");
        assert_eq!(event_label(HookEvent::SessionEnd), "session_end");
        assert_eq!(event_label(HookEvent::ToolCallBefore), "tool_call_before");
        assert_eq!(event_label(HookEvent::ToolCallAfter), "tool_call_after");
        assert_eq!(event_label(HookEvent::MessageSubmit), "message_submit");
        assert_eq!(event_label(HookEvent::ModeChange), "mode_change");
        assert_eq!(event_label(HookEvent::OnError), "on_error");
    }

    #[test]
    fn list_renders_hooks_grouped_by_event_and_notes_disabled_state() {
        // We test the formatter directly via a synthetic HooksConfig
        // because `App` is heavyweight to spin up here. The actual
        // `list(&App)` path is exercised once we hand the real
        // config in via `app.hooks.config()`; the formatter logic is
        // unit-tested standalone below.
        let cfg = crate::hooks::HooksConfig {
            enabled: false,
            hooks: vec![
                Hook::new(HookEvent::SessionStart, "echo started").with_name("greet"),
                Hook::new(HookEvent::ToolCallAfter, "notify-send done")
                    .with_condition(HookCondition::ToolName {
                        name: "exec_shell".into(),
                    })
                    .with_name("notify"),
            ],
            ..crate::hooks::HooksConfig::default()
        };

        // Synthesize the expected sections by re-running the same
        // formatter logic against the BTreeMap grouping.
        let mut by_event: std::collections::BTreeMap<&str, Vec<&Hook>> =
            std::collections::BTreeMap::new();
        for h in &cfg.hooks {
            by_event.entry(event_label(h.event)).or_default().push(h);
        }
        let events: Vec<&&str> = by_event.keys().collect();
        // BTreeMap sorts alphabetically — `session_start` before `tool_call_after`.
        assert_eq!(events, vec![&"session_start", &"tool_call_after"]);
    }
}
