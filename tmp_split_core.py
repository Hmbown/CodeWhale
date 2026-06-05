import os

core_dir = r'C:\myWork\AboimPintoConsulting\CodeWhale-worktrees\feat\command-strategy\crates\tui\src\commands\groups\core'

def write_cmd(fname, sname, aliases, usage, msgid, back_mod, back_fn, has_args):
    args_param = 'args: Option<&str>' if has_args else '_args: Option<&str>'
    args_pass = 'args' if has_args else '_args'

    if back_fn == 'exit':
        exec_body = f'        crate::commands::back::{back_mod}::{back_fn}()'
    elif back_fn in ('clear', 'models', 'deepseek_links', 'home_dashboard', 'subagents'):
        exec_body = f'        crate::commands::back::{back_mod}::{back_fn}(app)'
    else:
        exec_body = f'        crate::commands::back::{back_mod}::{back_fn}(app, {args_pass})'

    content = f'''//! {sname} command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct {sname};
impl Command for {sname} {{
    fn info(&self) -> &'static CommandInfo {{
        &CommandInfo {{
            name: "{fname}",
            aliases: &{aliases},
            usage: "{usage}",
            description_id: MessageId::{msgid},
        }}
    }}
    fn execute(&self, app: &mut App, {args_param}) -> CommandResult {{
{exec_body}
    }}
}}
'''
    path = os.path.join(core_dir, fname + '.rs')
    with open(path, 'w', encoding='utf-8') as f:
        f.write(content)
    print(f'Created {fname}.rs')

# Simple commands
write_cmd('help', 'Help', '["?", "bangzhu", "\u5e2e\u52a9"]', '/help [command]', 'CmdHelpDescription', 'core', 'help', True)
write_cmd('clear', 'Clear', '["qingping"]', '/clear', 'CmdClearDescription', 'core', 'clear', False)
write_cmd('exit', 'Exit', '["quit", "q", "tuichu"]', '/exit', 'CmdExitDescription', 'core', 'exit', False)
write_cmd('model', 'Model', '["moxing"]', '/model [name]', 'CmdModelDescription', 'core', 'model', True)
write_cmd('models', 'Models', '["moxingliebiao"]', '/models', 'CmdModelsDescription', 'core', 'models', False)
write_cmd('provider', 'Provider', '[]', '/provider [name] [model]', 'CmdProviderDescription', 'provider', 'provider', True)
write_cmd('links', 'Links', '["dashboard", "api", "lianjie"]', '/links', 'CmdLinksDescription', 'core', 'deepseek_links', False)
write_cmd('feedback', 'Feedback', '[]', '/feedback [bug|feature|security]', 'CmdFeedbackDescription', 'feedback', 'feedback', True)
write_cmd('home', 'Home', '["stats", "overview", "zhuye", "shouye"]', '/home', 'CmdHomeDescription', 'core', 'home_dashboard', False)
write_cmd('workspace', 'Workspace', '["cwd"]', '/workspace [path]', 'CmdWorkspaceDescription', 'core', 'workspace_switch', True)
write_cmd('subagents', 'Subagents', '["agents", "zhinengti"]', '/subagents', 'CmdSubagentsDescription', 'core', 'subagents', False)
write_cmd('profile', 'Profile', '["dangan"]', '/profile <name>', 'CmdHelpDescription', 'core', 'profile_switch', True)

# Agent command (special - has inline logic)
agent_content = '''//! Agent command.

use crate::tui::app::{App, AppAction};
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

use super::parse_depth_prefixed_arg;

pub struct Agent;
impl Command for Agent {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "agent",
            aliases: &["daili"],
            usage: "/agent [N] <task>",
            description_id: MessageId::CmdAgentDescription,
        }
    }
    fn execute(&self, _app: &mut App, arg: Option<&str>) -> CommandResult {
        let (max_depth, task) = match parse_depth_prefixed_arg(arg, 1) {
            Ok(parsed) => parsed,
            Err(message) => return CommandResult::error(message),
        };
        let task = match task {
            Some(task) if !task.trim().is_empty() => task.trim().to_string(),
            _ => {
                return CommandResult::error(
                    "Usage: /agent [N] <task>\\n\\n\
                     Opens a persistent sub-agent session with recursive agent depth N (0-3, default 1).",
                );
            }
        };
        let message = format!(
            "Open a persistent sub-agent session for this task. Call `agent_open` with name `slash_agent`, `prompt: {task:?}`, and `max_depth: {max_depth}`. Use `agent_eval` to wait for the next terminal/current projection and `handle_read` on the returned transcript_handle if you need more detail. Verify any claimed side effects before reporting success."
        );
        CommandResult::with_message_and_action(
            format!("Opening persistent sub-agent at depth {max_depth}..."),
            AppAction::SendMessage(message),
        )
    }
}
'''
with open(os.path.join(core_dir, 'agent.rs'), 'w', encoding='utf-8') as f:
    f.write(agent_content)
print('Created agent.rs')

# Relay command (special - calls build_relay_instruction)
relay_content = '''//! Relay command.

use crate::tui::app::{App, AppAction};
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

use super::build_relay_instruction;

pub struct Relay;
impl Command for Relay {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "relay",
            aliases: &["batonpass", "\\u{63E5}\\u{529B}"],
            usage: "/relay [focus]",
            description_id: MessageId::CmdRelayDescription,
        }
    }
    fn execute(&self, app: &mut App, arg: Option<&str>) -> CommandResult {
        let focus = arg.map(str::trim).filter(|value| !value.is_empty());
        let message = build_relay_instruction(app, focus);
        CommandResult::with_message_and_action(
            "Preparing session relay at .deepseek/handoff.md...",
            AppAction::SendMessage(message),
        )
    }
}
'''
with open(os.path.join(core_dir, 'relay.rs'), 'w', encoding='utf-8') as f:
    f.write(relay_content)
print('Created relay.rs')
print('Done - all files created')
