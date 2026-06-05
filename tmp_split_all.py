import os, shutil

back_dir = r'crates/tui/src/commands/back'
groups_dir = r'crates/tui/src/commands/groups'

# (back_module, group, cmd_name) — single-command back modules to migrate
merges = [
    ('anchor',     'utility',  'anchor'),
    ('attachment', 'memory',   'attach'),
    ('balance',    'debug',    'balance'),
    ('change',     'project',  'change'),
    ('feedback',   'core',     'feedback'),
    ('goal',       'project',  'goal'),
    ('hooks',      'utility',  'hooks'),
    ('init',       'project',  'init'),
    ('jobs',       'utility',  'jobs'),
    ('mcp',        'utility',  'mcp'),
    ('memory',     'memory',   'memory'),
    ('network',    'utility',  'network'),
    ('note',       'memory',   'note'),
    ('provider',   'core',     'provider'),
    ('queue',      'utility',  'queue'),
    ('rename',     'session',  'rename'),
    ('restore',    'skills',   'restore'),
    ('review',     'skills',   'review'),
    ('stash',      'utility',  'stash'),
    ('status',     'config',   'status'),
    ('task',       'utility',  'task'),
]

for bmod, group, cmd in merges:
    back_path = os.path.join(back_dir, bmod + '.rs')
    cmd_path = os.path.join(groups_dir, group, cmd + '.rs')
    sub_dir = os.path.join(groups_dir, group, cmd)
    
    if not os.path.exists(back_path):
        print(f'{bmod}: back file missing')
        continue
    if not os.path.exists(cmd_path):
        print(f'{bmod}: cmd file missing')
        continue
    
    # Read files
    with open(back_path, 'r', encoding='utf-8') as f:
        back_content = f.read()
    with open(cmd_path, 'r', encoding='utf-8') as f:
        cmd_content = f.read()
    
    # Create sub-directory
    os.makedirs(sub_dir, exist_ok=True)
    
    # ── Split command file ──
    # Move the Command struct + impl into command_file.rs
    # Keep imports needed by the command struct
    
    # Extract function name from back
    fn_name = None
    for line in back_content.split('\n'):
        s = line.strip()
        if s.startswith('pub fn '):
            fn_name = s.split('(')[0].replace('pub fn ', '').strip()
            break
    if not fn_name:
        print(f'{bmod}: no pub fn found')
        continue
    
    # Build command_file.rs: extract Command impl from cmd_content
    # Take only the top portion (imports + struct + impl Command)
    cmd_lines = cmd_content.split('\n')
    command_lines = []
    in_test = False
    for line in cmd_lines:
        if '#[cfg(test)]' in line:
            break
        command_lines.append(line)
    
    # Change the delegation call to use super::impl_file::fn_name
    for pattern in [
        f'crate::commands::back::{bmod}::{fn_name}(app, args)',
        f'crate::commands::back::{bmod}::{fn_name}(app, _args)',
        f'crate::commands::back::{bmod}::{fn_name}(app)',
    ]:
        if pattern in '\n'.join(command_lines):
            new_call = pattern.replace(f'crate::commands::back::{bmod}::', 'crate::commands::groups::' + group + '::' + cmd + '::')
            command_text = '\n'.join(command_lines)
            command_text = command_text.replace(pattern, new_call)
            command_lines = command_text.split('\n')
            break
    
    command_file = '\n'.join(command_lines)
    
    # Write command file
    cmd_file_path = os.path.join(sub_dir, f'{cmd}_command.rs')
    with open(cmd_file_path, 'w', encoding='utf-8') as f:
        f.write(command_file)
    
    # ── Build impl_file.rs ──
    # Extract the implementation from back file, rename fn to not be pub
    impl_content = back_content.strip()
    # Add "use crate::commands::groups::{group}::{cmd}::CommandResult;" etc if needed
    # Actually, the impl file is standalone — it just needs its own imports
    # The back file already has the right imports
    
    impl_file_path = os.path.join(sub_dir, f'{cmd}_impl.rs')
    with open(impl_file_path, 'w', encoding='utf-8') as f:
        f.write(impl_content)
    
    # ── Build mod.rs barrel ──
    struct_name = cmd.capitalize()
    # Handle special cases
    if cmd == 'mcp': struct_name = 'Mcp'
    
    mod_rs = f'''//! {struct_name} command.
//!
//! This module separates the command handler from the implementation.

pub mod {cmd}_command;
pub mod {cmd}_impl;
pub use {cmd}_command::{struct_name};
'''
    mod_rs_path = os.path.join(sub_dir, 'mod.rs')
    with open(mod_rs_path, 'w', encoding='utf-8') as f:
        f.write(mod_rs)
    
    # ── Remove old files ──
    os.remove(cmd_path)
    os.remove(back_path)
    
    print(f'{bmod}: -> {group}/{cmd}/')

# Update back/mod.rs to remove merged modules
back_mod_path = os.path.join(back_dir, 'mod.rs')
with open(back_mod_path, 'r', encoding='utf-8') as f:
    content = f.read()

for bmod, _, _ in merges:
    content = content.replace(f'pub(crate) mod {bmod};\n', '')
    content = content.replace(f'pub(crate) mod {bmod};\r\n', '')

with open(back_mod_path, 'w', encoding='utf-8') as f:
    f.write(content)

print()
print('back/mod.rs updated')
print('All done')
