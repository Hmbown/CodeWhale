import subprocess, os

# Map each sub-folder back to its original file group/cmd.rs
cmds = [
    'config/status', 'core/feedback', 'core/provider', 'debug/balance',
    'memory/attach', 'memory/memory', 'memory/note', 'project/change',
    'project/goal', 'project/init', 'session/rename', 'skills/restore',
    'skills/review', 'utility/anchor', 'utility/hooks', 'utility/jobs',
    'utility/mcp', 'utility/network', 'utility/queue', 'utility/stash',
    'utility/task',
]

for path in cmds:
    group, cmd = path.split('/')
    git_path = f'crates/tui/src/commands/groups/{group}/{cmd}.rs'
    
    # Get original file from HEAD
    result = subprocess.run(
        ['git', 'show', f'HEAD:{git_path}'],
        capture_output=True
    )
    original = result.stdout.decode('utf-8', errors='replace')
    
    if not original:
        print(f'{path}: not found in git')
        continue
    
    # Extract test section
    if '#[cfg(test)]' not in original:
        print(f'{path}: no tests in original')
        continue
    
    test_start = original.find('#[cfg(test)]')
    test_section = original[test_start:]
    
    # Read current command file
    cur_path = f'crates/tui/src/commands/groups/{group}/{cmd}/{cmd}_command.rs'
    with open(cur_path, 'r', encoding='utf-8') as f:
        current = f.read()
    
    # Append tests if not already present
    if '#[cfg(test)]' not in current:
        current = current.rstrip() + '\n\n' + test_section
        with open(cur_path, 'w', encoding='utf-8') as f:
            f.write(current)
        print(f'{path}: added tests')
    else:
        print(f'{path}: already has tests')

print('Done')
