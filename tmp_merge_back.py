import os, shutil

back_dir = r'crates/tui/src/commands/back'
groups_dir = r'crates/tui/src/commands/groups'

# Map: back_module -> (group_name, cmd_name)
# Only back modules that are SINGLE-CMD per the audit above
merges = {
    'anchor': ('utility', 'anchor'),
    'attachment': ('memory', 'attach'),
    'balance': ('debug', 'balance'),
    'change': ('project', 'change'),
    'feedback': ('core', 'feedback'),
    'goal': ('project', 'goal'),
    'hooks': ('utility', 'hooks'),
    'init': ('project', 'init'),
    'jobs': ('utility', 'jobs'),
    'mcp': ('utility', 'mcp'),
    'memory': ('memory', 'memory'),
    'network': ('utility', 'network'),
    'note': ('memory', 'note'),
    'provider': ('core', 'provider'),
    'queue': ('utility', 'queue'),
    'rename': ('session', 'rename'),
    'restore': ('skills', 'restore'),
    'review': ('skills', 'review'),
    'stash': ('utility', 'stash'),
    'status': ('config', 'status'),
    'task': ('utility', 'task'),
}

for bmod, (group, cmd) in sorted(merges.items()):
    back_path = os.path.join(back_dir, bmod + '.rs')
    cmd_path = os.path.join(groups_dir, group, cmd + '.rs')
    
    if not os.path.exists(back_path):
        print(f'{bmod}: back file missing, skipping')
        continue
    if not os.path.exists(cmd_path):
        print(f'{bmod}: cmd file missing ({cmd_path}), skipping')
        continue
    
    # Read both files
    with open(back_path, 'r', encoding='utf-8') as f:
        back_content = f.read()
    with open(cmd_path, 'r', encoding='utf-8') as f:
        cmd_content = f.read()
    
    # Extract the main pub fn from back_content (everything after imports/doc)
    # Find the pub fn definition
    fn_start = back_content.find('pub fn ')
    if fn_start < 0:
        print(f'{bmod}: no pub fn found')
        continue
    
    # Get imports (anything before the fn)
    imports = back_content[:fn_start].strip()
    
    # Get the full function body (from pub fn to EOF)
    fn_body = back_content[fn_start:].strip()
    
    # Determine the function name
    fn_name = fn_body.split('(')[0].replace('pub fn ', '').strip()
    
    # Determine the old backend path pattern
    old_path = f'crate::commands::back::{bmod}::{fn_name}(app, args)'
    old_path_noargs = f'crate::commands::back::{bmod}::{fn_name}(app)'
    
    # Check which pattern exists in cmd content
    if old_path in cmd_content:
        new_call = f'{fn_name}(app, args)'
        cmd_content = cmd_content.replace(old_path, new_call)
    elif old_path_noargs in cmd_content:
        new_call = f'{fn_name}(app)'
        cmd_content = cmd_content.replace(old_path_noargs, new_call)
    else:
        # Try with _args
        old_path_noargs2 = f'crate::commands::back::{bmod}::{fn_name}(app, _args)'
        if old_path_noargs2 in cmd_content:
            new_call = f'{fn_name}(app, _args)'
            cmd_content = cmd_content.replace(old_path_noargs2, new_call)
        else:
            print(f'{bmod}: could not find reference pattern in {cmd}')
            continue
    
    # Append function body after the cmd file content
    # Insert imports + fn body before the #[cfg(test)] block
    # Actually, just append at end - fn will be visible to the module
    merged = cmd_content.rstrip() + '\n\n\n// ── Implementation ─────────────────────────────────────────────────────\n\n' + fn_body + '\n'
    
    with open(cmd_path, 'w', encoding='utf-8') as f:
        f.write(merged)
    
    print(f'{bmod}: merged {fn_name}() into {group}/{cmd}.rs')

print('\nNow removing back files and updating mod.rs...')

# Now remove merged back files
back_mod_path = os.path.join(back_dir, 'mod.rs')
with open(back_mod_path, 'r', encoding='utf-8') as f:
    back_mod = f.read()

for bmod in sorted(merges.keys()):
    # Remove the file
    os.remove(os.path.join(back_dir, bmod + '.rs'))
    
    # Remove from back/mod.rs
    # Pattern: "pub(crate) mod xxx;\n" or "pub(crate) mod xxx;\r\n"
    back_mod = back_mod.replace(f'pub(crate) mod {bmod};\n', '')
    back_mod = back_mod.replace(f'pub(crate) mod {bmod};\r\n', '')
    
    print(f'  Removed {bmod}.rs and updated mod.rs')

with open(back_mod_path, 'w', encoding='utf-8') as f:
    f.write(back_mod)

print('All done')
