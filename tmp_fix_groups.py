import os

base = r'crates/tui/src/commands/groups'

# Fix 1: Add App import to utility/mod.rs
mod_path = os.path.join(base, 'utility', 'mod.rs')
with open(mod_path, 'r', encoding='utf-8') as f:
    content = f.read()

content = content.replace(
    'use crate::commands::traits::{Command, CommandGroup};',
    'use crate::commands::traits::{Command, CommandGroup};\nuse crate::tui::app::App;'
)

with open(mod_path, 'w', encoding='utf-8') as f:
    f.write(content)
print('utility/mod.rs: added App import')

# Fix 2: Change unused `args` to `_args` in execute methods
# These are files where backend takes no args
fix_args = [
    'session/fork.rs',
    'session/compact.rs',
    'session/purge.rs',
    'config/settings.rs',
    'config/status.rs',
    'config/statusline.rs',
    'config/logout.rs',
    'debug/translate.rs',
    'debug/tokens.rs',
    'debug/cost.rs',
    'debug/balance.rs',
    'debug/system.rs',
    'debug/context.rs',
    'debug/edit.rs',
    'debug/diff.rs',
    'debug/retry.rs',
    'project/init.rs',
]

for fpath in fix_args:
    full = os.path.join(base, fpath.replace('/', os.sep))
    with open(full, 'r', encoding='utf-8') as f:
        content = f.read()
    
    old = 'app: &mut App, args: Option<&str>)'
    new = 'app: &mut App, _args: Option<&str>)'
    if old in content:
        content = content.replace(old, new)
        with open(full, 'w', encoding='utf-8') as f:
            f.write(content)
    print(f'{fpath}: fixed args -> _args')

print('All fixes done')
