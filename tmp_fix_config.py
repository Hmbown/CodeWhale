import os

impl_dirs = [
    'config/config', 'config/settings', 'config/statusline',
    'config/mode', 'config/theme', 'config/verbose', 'config/trust',
    'config/logout', 'project/lsp', 'utility/slop',
]

base_imports = 'use crate::commands::CommandResult;\nuse crate::tui::app::App;\n'

for d in impl_dirs:
    parts = d.split('/')
    fname = f'crates/tui/src/commands/groups/{d}/{parts[1]}_impl.rs'
    with open(fname, 'r', encoding='utf-8') as f:
        content = f.read()
    if 'use crate::commands::CommandResult;' not in content:
        content = base_imports + '\n' + content
        with open(fname, 'w', encoding='utf-8') as f:
            f.write(content)
        print(f'{d}: added base imports')

# Fix mode_impl.rs helper references
mode_impl = 'crates/tui/src/commands/groups/config/mode/mode_impl.rs'
with open(mode_impl, 'r', encoding='utf-8') as f:
    content = f.read()
content = content.replace('match parse_mode_arg(arg)', 'match crate::commands::back::config::parse_mode_arg(arg)')
content = content.replace('switch_mode_with_status(app, mode)', 'crate::commands::back::config::switch_mode_with_status(app, mode)')
with open(mode_impl, 'w', encoding='utf-8') as f:
    f.write(content)
print('mode: fixed helper references')

# Fix logout_impl.rs - needs AppAction import
logout_impl = 'crates/tui/src/commands/groups/config/logout/logout_impl.rs'
with open(logout_impl, 'r', encoding='utf-8') as f:
    content = f.read()
if 'use crate::tui::app::AppAction;' not in content:
    content = content.replace('use crate::tui::app::App;', 'use crate::tui::app::{App, AppAction};')
    with open(logout_impl, 'w', encoding='utf-8') as f:
        f.write(content)
print('logout: added AppAction import')

# Clean up orphaned doc comments in back/config.rs
back_config = 'crates/tui/src/commands/back/config.rs'
with open(back_config, 'r', encoding='utf-8') as f:
    lines = f.readlines()

# Find and remove standalone /// lines that don't precede any item
cleaned = []
i = 0
while i < len(lines):
    line = lines[i]
    stripped = line.strip()
    
    # Check if this line starts a doc comment that leads to nothing
    if stripped.startswith('///') and i + 1 < len(lines):
        j = i
        # Collect all consecutive doc comment lines
        doc_lines = []
        while j < len(lines) and (lines[j].strip().startswith('///') or lines[j].strip().startswith('//!')):
            doc_lines.append(lines[j])
            j += 1
        # Check if next non-blank, non-comment line after doc is an item
        k = j
        while k < len(lines) and (lines[k].strip() == '' or lines[k].strip().startswith('//')):
            k += 1
        if k < len(lines) and not lines[k].strip().startswith('pub ') and not lines[k].strip().startswith('fn ') and not lines[k].strip().startswith('use ') and not lines[k].strip().startswith('#['):
            # Orphaned doc comment - skip it
            i = j
            continue
    
    cleaned.append(line)
    i += 1

with open(back_config, 'w', encoding='utf-8') as f:
    f.writelines(cleaned)
print('Cleaned orphaned doc comments in back/config.rs')

print('All fixes done')
