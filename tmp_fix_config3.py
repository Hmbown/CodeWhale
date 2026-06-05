import os

groups = 'crates/tui/src/commands/groups'

# Fix 1: config_impl.rs needs imports for shared helpers in back/config.rs
path = groups + '/config/config/config_impl.rs'
with open(path, 'r', encoding='utf-8') as f:
    content = f.read()

additions = [
    'use crate::config::Config;',
    'use crate::settings::Settings;',
    'use crate::commands::back::config::{show_config, set_config_value};',
]
for a in additions:
    if a not in content:
        content = content.replace('use crate::commands::CommandResult;\n', 'use crate::commands::CommandResult;\n' + a + '\n')
with open(path, 'w', encoding='utf-8') as f:
    f.write(content)
print('Fixed config_impl.rs imports')

# Fix 2: expand_tilde needs to stay in back/config.rs - put it back
back_cfg = 'crates/tui/src/commands/back/config.rs'
with open(back_cfg, 'r', encoding='utf-8') as f:
    lines = f.readlines()

# Find where expand_tilde was — after the function extracted from trust_impl
# Make sure it's defined somewhere accessible
has_expand_tilde = any('fn expand_tilde' in l for l in lines)
if not has_expand_tilde:
    # Add it back as pub(crate)
    expand_fn = '''
pub(crate) fn expand_tilde(raw: &str) -> String {
    if !raw.starts_with('~') {
        return raw.to_string();
    }
    let trimmed = raw.trim_start_matches('~');
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) => PathBuf::from(home).join(trimmed).to_string_lossy().to_string(),
        None => raw.to_string(),
    }
}
'''
    # Find a good insertion point - after imports, before the first function
    insert_after = 0
    for i, line in enumerate(lines):
        if line.strip().startswith('pub fn ') or line.strip().startswith('pub(crate) fn '):
            insert_after = i - 1  # Insert before first function
            break
    lines.insert(insert_after, expand_fn)
    with open(back_cfg, 'w', encoding='utf-8') as f:
        f.writelines(lines)
    print('Added expand_tilde back to back/config.rs')

# Fix 3: show_settings, status_line, logout take 1 arg - fix callers
for f, fn in [
    ('config/settings/settings_command.rs', 'show_settings'),
    ('config/statusline/statusline_command.rs', 'status_line'),
    ('config/logout/logout_command.rs', 'logout'),
]:
    path = groups + '/' + f
    with open(path, 'r', encoding='utf-8') as fh:
        content = fh.read()
    old = f'{fn}(app, args)'
    new = f'{fn}(app)'
    if old in content:
        content = content.replace(old, new)
        with open(path, 'w', encoding='utf-8') as fh:
            fh.write(content)
        print(f'Fixed {f}: removed args from {fn}()')

# Fix 4: Remove unused re-exports from mod.rs files
for group, cmd in [
    ('config', 'config'), ('config', 'settings'), ('config', 'statusline'),
    ('config', 'theme'), ('config', 'verbose'), ('config', 'trust'),
    ('project', 'lsp'), ('utility', 'slop'),
]:
    mod_path = groups + '/' + group + '/' + cmd + '/mod.rs'
    with open(mod_path, 'r', encoding='utf-8') as f:
        content = f.read()
    # Remove pub use XXX_impl::YYY line
    content = content.split('\n')
    content = [l for l in content if not (l.strip().startswith('pub use') and '_impl::' in l)]
    with open(mod_path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(content))
    print(f'Removed re-export from {group}/{cmd}/mod.rs')

# Fix 5: Remove unused AppAction import from logout_impl
path = groups + '/config/logout/logout_impl.rs'
with open(path, 'r', encoding='utf-8') as f:
    content = f.read()
content = content.replace('use crate::tui::app::{App, AppAction};', 'use crate::tui::app::App;')
with open(path, 'w', encoding='utf-8') as f:
    f.write(content)
print('Fixed unused AppAction in logout_impl.rs')

# Fix 6: Remove unused OnboardingState from back/config.rs
with open(back_cfg, 'r', encoding='utf-8') as f:
    content = f.read()
content = content.replace(', OnboardingState', '')
with open(back_cfg, 'w', encoding='utf-8') as f:
    f.write(content)
print('Removed unused OnboardingState from back/config.rs')

print('All fixes done')
