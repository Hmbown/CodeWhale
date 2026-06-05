import os, re, subprocess

groups_dir = r'crates/tui/src/commands/groups'
back_dir = r'crates/tui/src/commands/back'

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
    back_rs = f'crates/tui/src/commands/back/{bmod}.rs'
    
    # Show git HEAD version of the file
    result = subprocess.run(
        ['git', 'show', f'HEAD:{back_rs}'],
        capture_output=True, text=True, cwd=r'C:\myWork\AboimPintoConsulting\CodeWhale-worktrees\feat\command-strategy'
    )
    
    if result.returncode != 0 or not result.stdout:
        # Try from the parent commit (HEAD~1) since it was deleted in HEAD
        pass
    
    original = result.stdout
    
    if not original or 'pub fn ' not in original:
        print(f'{bmod}: could not get original')
        continue
    
    # Extract all use statements from the original
    use_lines = []
    for line in original.split('\n'):
        stripped = line.strip()
        if stripped.startswith('use '):
            use_lines.append(stripped)
    
    # Remove the standard ones that cmd files already have
    skip_patterns = [
        'use crate::commands::CommandResult',
        'use crate::commands::traits',
        'use crate::localization::MessageId',
        'use crate::tui::app::App',
    ]
    needed = []
    for ul in use_lines:
        should_skip = False
        for sp in skip_patterns:
            if sp in ul:
                should_skip = True
                break
        if not should_skip and ul not in needed:
            needed.append(ul)
    
    if not needed:
        print(f'{bmod}: no extra imports needed')
        continue
    
    # Read the command file
    cmd_path = os.path.join(groups_dir, group, cmd + '.rs')
    with open(cmd_path, 'r', encoding='utf-8') as f:
        cmd_content = f.read()
    
    # Add imports after the existing import block (find the last use statement)
    last_use = 0
    for i, line in enumerate(cmd_content.split('\n')):
        if line.strip().startswith('use '):
            last_use = i
    
    if last_use > 0:
        lines = cmd_content.split('\n')
        insert_pos = last_use + 1
        for imp in reversed(needed):
            lines.insert(insert_pos, imp)
        cmd_content = '\n'.join(lines)
    
        with open(cmd_path, 'w', encoding='utf-8') as f:
            f.write(cmd_content)
        
        print(f'{bmod}: added {len(needed)} imports to {group}/{cmd}.rs')
    else:
        print(f'{bmod}: no use statements found in cmd file')

print('Done')
