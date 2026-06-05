import os

base = r'crates/tui/src/commands/groups'

groups = {
    'session': [
        ('rename', '["gaiming", "chongmingming"]', '/rename <title>', 'CmdRenameDescription', 'rename::rename'),
        ('save', '[]', '/save [path]', 'CmdSaveDescription', 'session::save'),
        ('fork', '["branch"]', '/fork', 'CmdForkDescription', 'session::fork'),
        ('new', '[]', '/new', 'CmdNewDescription', 'session::new_session'),
        ('sessions', '["resume"]', '/sessions', 'CmdSessionsDescription', 'session::sessions'),
        ('load', '["jiazai"]', '/load <file>', 'CmdLoadDescription', 'session::load'),
        ('compact', '["yasuo"]', '/compact', 'CmdCompactDescription', 'session::compact'),
        ('purge', '["qingchu"]', '/purge', 'CmdPurgeDescription', 'session::purge'),
        ('export', '["daochu"]', '/export [path]', 'CmdExportDescription', 'session::export'),
    ],
    'config': [
        ('config', '[]', '/config [key] [value]', 'CmdConfigDescription', 'config::config_command'),
        ('settings', '[]', '/settings', 'CmdSettingsDescription', 'config::show_settings'),
        ('status', '[]', '/status', 'CmdStatusDescription', 'status::status'),
        ('statusline', '[]', '/statusline', 'CmdStatuslineDescription', 'config::status_line'),
        ('mode', '[]', '/mode [plan|yolo|agent]', 'CmdModeDescription', 'config::mode'),
        ('theme', '[]', '/theme [name]', 'CmdThemeDescription', 'config::theme'),
        ('verbose', '[]', '/verbose [on|off]', 'CmdVerboseDescription', 'config::verbose'),
        ('trust', '["xinren"]', '/trust [path]', 'CmdTrustDescription', 'config::trust'),
        ('logout', '[]', '/logout', 'CmdLogoutDescription', 'config::logout'),
    ],
    'debug': [
        ('translate', '["translation", "transale"]', '/translate', 'CmdTranslateDescription', 'core::translate'),
        ('tokens', '[]', '/tokens', 'CmdTokensDescription', 'debug::tokens'),
        ('cost', '[]', '/cost', 'CmdCostDescription', 'debug::cost'),
        ('balance', '[]', '/balance', 'CmdBalanceDescription', 'balance::balance'),
        ('cache', '[]', '/cache [count|inspect|stats|zones|warmup]', 'CmdCacheDescription', 'debug::cache'),
        ('system', '["xitong"]', '/system', 'CmdSystemDescription', 'debug::system_prompt'),
        ('context', '["ctx"]', '/context', 'CmdContextDescription', 'debug::context'),
        ('edit', '[]', '/edit', 'CmdEditDescription', 'debug::edit'),
        ('diff', '[]', '/diff', 'CmdDiffDescription', 'debug::diff'),
        ('undo', '[]', '/undo', 'CmdUndoDescription', 'debug::patch_undo'),
        ('retry', '["chongshi"]', '/retry', 'CmdRetryDescription', 'debug::retry'),
    ],
    'project': [
        ('change', '[]', '/change <description>', 'CmdChangeDescription', 'change::change'),
        ('init', '[]', '/init', 'CmdInitDescription', 'init::init'),
        ('lsp', '[]', '/lsp <command>', 'CmdLspDescription', 'config::lsp_command'),
        ('share', '[]', '/share [path]', 'CmdShareDescription', 'share::share(xxx)'),
        ('goal', '["hunt", "mubiao", "\\u{72e9}\\u{730e}"]', '/goal [start|show|close <reason>]', 'CmdGoalDescription', 'goal::hunt'),
    ],
    'skills': [
        ('skills', '["jinengliebiao"]', '/skills [--remote|sync|<prefix>]', 'CmdSkillsDescription', 'skills::list_skills'),
        ('skill', '["jineng"]', '/skill <name|install|update|uninstall|trust>', 'CmdSkillDescription', 'skills::run_skill'),
        ('review', '["shencha"]', '/review <target>', 'CmdReviewDescription', 'review::review'),
        ('restore', '[]', '/restore [N]', 'CmdRestoreDescription', 'restore::restore'),
    ],
    'memory': [
        ('note', '[]', '/note <text>', 'CmdNoteDescription', 'note::note'),
        ('memory', '[]', '/memory [show|path|clear|edit|help]', 'CmdMemoryDescription', 'memory::memory'),
        ('attach', '["image", "media", "fujian"]', '/attach <path|url> [description]', 'CmdAttachDescription', 'attachment::attach'),
    ],
    'utility': [
        ('queue', '["queued"]', '/queue [list|edit <n>|drop <n>|clear]', 'CmdQueueDescription', 'queue::queue'),
        ('stash', '["park"]', '/stash [list|pop|clear]', 'CmdStashDescription', 'stash::stash'),
        ('hooks', '["hook", "gouzi"]', '/hooks [list|events]', 'CmdHooksDescription', 'hooks::hooks'),
        ('anchor', '["maodian"]', '/anchor <text>', 'CmdAnchorDescription', 'anchor::anchor'),
        ('network', '[]', '/network [allow|deny] <host>', 'CmdNetworkDescription', 'network::network'),
        ('mcp', '[]', '/mcp [list|restart|stop|start|add|remove]', 'CmdMcpDescription', 'mcp::mcp'),
        ('rlm', '["recursive", "digui"]', '/rlm [N] <file_or_text>', 'CmdRlmDescription', 'rlm_inline'),
        ('task', '["tasks"]', '/task [list|read|revert|cancel]', 'CmdTaskDescription', 'task::task'),
        ('jobs', '["job", "zuoye"]', '/jobs', 'CmdJobsDescription', 'jobs::jobs'),
        ('slop', '["canzha"]', '/slop [query|export]', 'CmdSlopDescription', 'config::slop'),
    ],
}

def to_struct(fname):
    overrides = {'lsp':'Lsp','mcp':'Mcp','undo':'Undo','share':'Share','goal':'Goal',
                 'init':'Init','new':'New','edit':'Edit','diff':'Diff','slop':'Slop',
                 'rlm':'Rlm','job':'Jobs','task':'Task'}
    return overrides.get(fname, fname.capitalize())

for group_name in sorted(groups.keys()):
    dir_path = os.path.join(base, group_name)
    os.makedirs(dir_path, exist_ok=True)
    commands = groups[group_name]
    struct_name = group_name.capitalize() + 'Commands'

    # Generate mod.rs
    mods = []
    for fname, _, _, _, _ in commands:
        mods.append(f'mod {fname};')

    cmd_enum = '\n'.join(f'            Box::new({to_struct(fname)}),' for fname, _, _, _, _ in commands)

    mod_rs = f'''//! {group_name.capitalize()} commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

{chr(10).join(mods)}

use crate::commands::traits::{{Command, CommandGroup}};

pub struct {struct_name};
impl CommandGroup for {struct_name} {{
    fn commands(&self) -> Vec<Box<dyn Command>> {{
        vec![
{cmd_enum}
        ]
    }}
}}
'''

    # Add helpers for utility group
    if group_name == 'utility':
        mod_rs += '''

// ── Helpers ────────────────────────────────────────────────────────────────

fn parse_depth_prefixed_arg(
    arg: Option<&str>,
    default_depth: u32,
) -> Result<(u32, Option<&str>), String> {
    let Some(raw) = arg.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok((default_depth, None));
    };
    let mut parts = raw.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    if first.chars().all(|ch| ch.is_ascii_digit()) {
        let depth: u32 = first
            .parse()
            .map_err(|_| "Depth must be an integer from 0 to 3".to_string())?;
        if depth > 3 {
            return Err("Depth must be between 0 and 3".to_string());
        }
        Ok((depth, parts.next().map(str::trim)))
    } else {
        Ok((default_depth, Some(raw)))
    }
}

fn resolves_to_existing_file(app: &App, input: &str) -> bool {
    let path = std::path::Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        app.workspace.join(path)
    };
    candidate.is_file()
}
'''

    with open(os.path.join(dir_path, 'mod.rs'), 'w', encoding='utf-8') as f:
        f.write(mod_rs)

    # Generate individual command files
    for fname, aliases, usage, msgid, back_path in commands:
        sname = to_struct(fname)

        # Handle special commands
        if fname == 'undo':
            exec_body = '''        let result = crate::commands::back::debug::patch_undo(app);
        if result.message.as_deref().is_none_or(|m| {
            m.starts_with("No snapshots found")
                || m.starts_with("No tool or pre-turn")
                || m.starts_with("Snapshot repo")
        }) {
            crate::commands::back::debug::undo_conversation(app)
        } else {
            result
        }'''
        elif fname == 'share':
            exec_body = '        crate::commands::share::share(app, args)'
        elif fname == 'rlm':
            exec_body = '''        rlm(app, args)'''
        elif '(' in back_path:
            exec_body = f'        crate::commands::back::{back_path}(app, args)'
        else:
            exec_body = f'        crate::commands::back::{back_path}(app, args)'

        content = f'''//! {sname} command.

use crate::commands::traits::{{Command, CommandInfo}};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

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
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {{
{exec_body}
    }}
}}
'''
        with open(os.path.join(dir_path, fname + '.rs'), 'w', encoding='utf-8') as f:
            f.write(content)

    print(f'{group_name}: {len(commands)} commands ok')

# Generate special rlm.rs with inline logic
rlm_dir = os.path.join(base, 'utility')
rlm_content = '''//! RLM command.

use crate::tui::app::{App, AppAction};
use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

use super::{parse_depth_prefixed_arg, resolves_to_existing_file};

pub struct Rlm;
impl Command for Rlm {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "rlm",
            aliases: &["recursive", "digui"],
            usage: "/rlm [N] <file_or_text>",
            description_id: MessageId::CmdRlmDescription,
        }
    }
    fn execute(&self, app: &mut App, arg: Option<&str>) -> CommandResult {
        let (max_depth, target) = match parse_depth_prefixed_arg(arg, 1) {
            Ok(parsed) => parsed,
            Err(message) => return CommandResult::error(message),
        };
        let target = match target {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            _ => {
                return CommandResult::error(
                    "Usage: /rlm [N] <file_or_text>\\n\\n\
                     Opens a persistent RLM context with sub_rlm depth N (0-3, default 1).".to_string(),
                );
            }
        };
        let source_arg = if resolves_to_existing_file(app, &target) {
            format!("file_path: \\"{target}\\"")
        } else {
            format!("content: {target:?}")
        };
        let message = format!(
            "Open and use a persistent RLM session. Call `rlm_open` with name `slash_rlm` and {source_arg}. Call `rlm_configure` with `sub_rlm_max_depth: {max_depth}`."
        );
        CommandResult::with_message_and_action(
            format!("Opening persistent RLM context at depth {max_depth}..."),
            AppAction::SendMessage(message),
        )
    }
}
'''
with open(os.path.join(rlm_dir, 'rlm.rs'), 'w', encoding='utf-8') as f:
    f.write(rlm_content)
print('utility/rlm.rs: special inline version')

# Generate undo.rs with special fallback logic
undo_dir = os.path.join(base, 'debug')
undo_content = '''//! Undo command.

use crate::tui::app::App;
use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Undo;
impl Command for Undo {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "undo",
            aliases: &[],
            usage: "/undo",
            description_id: MessageId::CmdUndoDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        let result = crate::commands::back::debug::patch_undo(app);
        if result.message.as_deref().is_none_or(|m| {
            m.starts_with("No snapshots found")
                || m.starts_with("No tool or pre-turn")
                || m.starts_with("Snapshot repo")
        }) {
            crate::commands::back::debug::undo_conversation(app)
        } else {
            result
        }
    }
}
'''
with open(os.path.join(undo_dir, 'undo.rs'), 'w', encoding='utf-8') as f:
    f.write(undo_content)
print('debug/undo.rs: special version')

print('\\nAll done')
