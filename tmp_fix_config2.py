import os

groups = 'crates/tui/src/commands/groups'

# Fix 1: Add use super::XXX_impl::yyy imports to command files
fixes = {
    'config/config/config_command.rs': 'use super::config_impl::config_command;',
    'config/settings/settings_command.rs': 'use super::settings_impl::show_settings;',
    'config/statusline/statusline_command.rs': 'use super::statusline_impl::status_line;',
    'config/theme/theme_command.rs': 'use super::theme_impl::theme;',
    'config/verbose/verbose_command.rs': 'use super::verbose_impl::verbose;',
    'config/trust/trust_command.rs': 'use super::trust_impl::trust;',
    'project/lsp/lsp_command.rs': 'use super::lsp_impl::lsp_command;',
    'utility/slop/slop_command.rs': 'use super::slop_impl::slop;',
}

for fpath, imp in fixes.items():
    full = groups + '/' + fpath
    with open(full, 'r', encoding='utf-8') as f:
        content = f.read()
    lines = content.split('\n')
    last_use = 0
    for i, line in enumerate(lines):
        if line.strip().startswith('use '):
            last_use = i
    lines.insert(last_use + 1, imp)
    with open(full, 'w', encoding='utf-8') as f:
        f.write('\n'.join(lines))
    print(f'Fixed {fpath}')

# Fix logout_impl.rs: add missing imports
logout = f'{groups}/config/logout/logout_impl.rs'
with open(logout, 'r', encoding='utf-8') as f:
    content = f.read()
for additional in ['use crate::config::clear_active_provider_api_key;', 'use crate::tui::app::OnboardingState;']:
    if additional not in content:
        content = content.replace('use crate::tui::app::{App, AppAction};',
                                  'use crate::tui::app::{App, AppAction};\n' + additional)
with open(logout, 'w', encoding='utf-8') as f:
    f.write(content)
print('Fixed logout_impl.rs imports')

# Remove unused re-exports from mod.rs files
for group, cmd in [('config', 'mode'), ('config', 'logout')]:
    mod_rs = f'{groups}/{group}/{cmd}/mod.rs'
    with open(mod_rs, 'r', encoding='utf-8') as f:
        content = f.read()
    content = content.replace(f'pub use {cmd}_impl::{cmd};\n', '')
    with open(mod_rs, 'w', encoding='utf-8') as f:
        f.write(content)
    print(f'Removed unused re-export from {group}/{cmd}/mod.rs')

# Remove orphaned doc comments
target_files = [
    f'{groups}/config/config/config_impl.rs',
    f'{groups}/config/settings/settings_impl.rs',
    f'{groups}/config/statusline/statusline_impl.rs',
    f'{groups}/config/theme/theme_impl.rs',
    f'{groups}/config/verbose/verbose_impl.rs',
    f'{groups}/config/trust/trust_impl.rs',
    f'{groups}/project/lsp/lsp_impl.rs',
    f'{groups}/utility/slop/slop_impl.rs',
    'crates/tui/src/commands/back/config.rs',
]

for fname in target_files:
    with open(fname, 'r', encoding='utf-8') as f:
        lines = f.readlines()
    cleaned = []
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        if stripped.startswith('///'):
            j = i
            while j < len(lines) and (lines[j].strip().startswith('///') or lines[j].strip().startswith('//')):
                j += 1
            k = j
            while k < len(lines) and lines[k].strip() == '':
                k += 1
            if k < len(lines):
                next_line = lines[k].strip()
                starts = ['pub ', 'fn ', 'use ', '#[', 'const ', 'let ', 'struct ', 'enum ',
                          'trait ', 'type ', 'impl ', 'mod ', 'static ', 'unsafe ', 'macro_rules']
                if not any(next_line.startswith(p) for p in starts):
                    i = j
                    continue
        cleaned.append(line)
        i += 1
    with open(fname, 'w', encoding='utf-8') as f:
        f.writelines(cleaned)

print('Cleaned orphaned docs')
print('All fixes done')
