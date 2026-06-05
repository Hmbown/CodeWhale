import os, shutil

groups_dir = r'crates/tui/src/commands/groups'
back_config = r'crates/tui/src/commands/back/config.rs'

# Commands that need sub-folders and their impl functions from back/config.rs
# (group, cmd, function_name_in_back_config)
config_cmds = [
    ('config', 'config', 'config_command'),
    ('config', 'settings', 'show_settings'),
    ('config', 'statusline', 'status_line'),
    ('config', 'mode', 'mode'),
    ('config', 'theme', 'theme'),
    ('config', 'verbose', 'verbose'),
    ('config', 'trust', 'trust'),
    ('config', 'logout', 'logout'),
    ('project', 'lsp', 'lsp_command'),
    ('utility', 'slop', 'slop'),
]

# Also: project/share.rs delegates to crate::commands::share::share (not back/config)
# utility/rlm.rs has inline logic

# Read back/config.rs to get function bodies
with open(back_config, 'r', encoding='utf-8') as f:
    config_content = f.read()

# For each command, extract the function and move it
extracted_fns = []

for group, cmd, fn_name in config_cmds:
    src_path = os.path.join(groups_dir, group, cmd + '.rs')
    sub_dir = os.path.join(groups_dir, group, cmd)
    
    # Create sub-directory
    os.makedirs(sub_dir, exist_ok=True)
    
    # Read the existing command file
    if os.path.exists(src_path):
        with open(src_path, 'r', encoding='utf-8') as f:
            cmd_content = f.read()
        
        # Extract command struct part (before #[cfg(test)])
        test_pos = cmd_content.find('#[cfg(test)]')
        cmd_part = cmd_content[:test_pos] if test_pos > 0 else cmd_content
        
        # Change delegation to use local impl
        for pattern in [
            f'crate::commands::back::config::{fn_name}(app, args)',
            f'crate::commands::back::config::{fn_name}(app, _args)',
            f'crate::commands::back::config::{fn_name}(app)',
        ]:
            if pattern in cmd_part:
                cmd_part = cmd_part.replace(pattern, f'{fn_name}(app, args)')
                break
        
        # Also try with just fn_name call
        # Write command file
        cmd_file = os.path.join(sub_dir, f'{cmd}_command.rs')
        with open(cmd_file, 'w', encoding='utf-8') as f:
            f.write(cmd_part)
        
        # Remove old file
        os.remove(src_path)
    else:
        # Create a minimal command file
        struct_name = cmd.capitalize()
        overrides = {'lsp': 'Lsp', 'slop': 'Slop'}
        struct_name = overrides.get(cmd, cmd.capitalize())
        
        cmd_file_content = f'''//! {struct_name} command.

use crate::commands::traits::{{Command, CommandInfo}};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct {struct_name};
impl Command for {struct_name} {{
    fn info(&self) -> &'static CommandInfo {{
        &CommandInfo {{
            name: "{cmd}",
            aliases: &[],
            usage: "/{cmd}",
            description_id: MessageId::Cmd{struct_name}Description,
        }}
    }}
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {{
        {fn_name}(app, args)
    }}
}}
'''
        cmd_file = os.path.join(sub_dir, f'{cmd}_command.rs')
        with open(cmd_file, 'w', encoding='utf-8') as f:
            f.write(cmd_file_content)
    
    # Extract function from back/config.rs
    fn_pattern = f'pub fn {fn_name}('
    fn_start = config_content.find(fn_pattern)
    if fn_start < 0:
        # Try pub(crate) fn
        fn_pattern = f'pub(crate) fn {fn_name}('
        fn_start = config_content.find(fn_pattern)
    
    if fn_start >= 0:
        # Find the end: look for next "pub fn" or "pub(crate) fn" or "pub(super) fn" or EOF
        rest = config_content[fn_start:]
        next_fn = len(rest)
        for p in ['\npub fn ', '\npub(crate) fn ', '\npub(super) fn ']:
            pos = rest.find(p, 1)
            if pos > 0 and pos < next_fn:
                next_fn = pos
        fn_body = rest[:next_fn].strip()
        
        # Write impl file
        impl_file = os.path.join(sub_dir, f'{cmd}_impl.rs')
        with open(impl_file, 'w', encoding='utf-8') as f:
            f.write(fn_body)
        
        # Add to extraction list for removal from back/config.rs
        extracted_fns.append(fn_pattern)
        print(f'{group}/{cmd}: extracted {fn_name}()')
    else:
        print(f'{group}/{cmd}: WARNING - {fn_name}() not found in back/config.rs')
    
    # Create mod.rs
    struct_name = cmd.capitalize()
    overrides = {'lsp': 'Lsp', 'slop': 'Slop'}
    struct_name = overrides.get(cmd, cmd.capitalize())
    
    mod_rs = f'''//! {struct_name} command.

pub mod {cmd}_command;
pub mod {cmd}_impl;
pub use {cmd}_command::{struct_name};
pub use {cmd}_impl::{fn_name};
'''
    mod_file = os.path.join(sub_dir, 'mod.rs')
    with open(mod_file, 'w', encoding='utf-8') as f:
        f.write(mod_rs)

# Remove extracted functions from back/config.rs
print()
print('Removing extracted functions from back/config.rs...')
with open(back_config, 'r', encoding='utf-8') as f:
    content = f.read()

for fn_name in [c[2] for c in config_cmds]:
    # Find the function
    fn_pattern = f'pub fn {fn_name}('
    fn_start = content.find(fn_pattern)
    if fn_start < 0:
        fn_pattern = f'pub(crate) fn {fn_name}('
        fn_start = content.find(fn_pattern)
    if fn_start < 0:
        continue
    
    # Find the end
    rest = content[fn_start:]
    next_fn = len(rest)
    for p in ['\npub fn ', '\npub(crate) fn ', '\npub(super) fn ']:
        pos = rest.find(p, 1)
        if pos > 0 and pos < next_fn:
            next_fn = pos
    
    # Remove from fn_start to next_fn (inclusive of the trailing newline)
    end_pos = fn_start + next_fn
    # Also remove leading blank lines before the function
    while fn_start > 0 and content[fn_start-1] in '\n\r ':
        fn_start -= 1
    
    removed = content[fn_start:end_pos]
    content = content[:fn_start] + content[end_pos:]
    # Clean up multiple blank lines
    while '\n\n\n' in content:
        content = content.replace('\n\n\n', '\n\n')
    
    print(f'  Removed {fn_name}() from back/config.rs ({len(removed)} bytes)')

with open(back_config, 'w', encoding='utf-8') as f:
    f.write(content)

print(f'back/config.rs now {len(content)} bytes')
print('Done')
