import os

# Scan all group files for duplicate imports and fix them
groups_dir = r'crates/tui/src/commands/groups'

def dedup_imports(content):
    """Remove duplicate use statements from file content."""
    lines = content.split('\n')
    seen_imports = set()
    new_lines = []
    in_multi_line_use = False
    multi_line_buffer = ''
    
    for line in lines:
        stripped = line.strip()
        
        # Handle multi-line use blocks
        if in_multi_line_use:
            multi_line_buffer += line + '\n'
            if stripped.endswith(';') or stripped == '};' or stripped == '}':
                in_multi_line_use = False
                combined = multi_line_buffer.strip()
                if combined not in seen_imports:
                    seen_imports.add(combined)
                    new_lines.append(multi_line_buffer.rstrip())
                multi_line_buffer = ''
            continue
        
        if stripped.startswith('use ') and (stripped.endswith('{') or stripped.endswith('{')):
            in_multi_line_use = True
            multi_line_buffer = line + '\n'
            continue
        
        if stripped.startswith('use '):
            if stripped not in seen_imports:
                seen_imports.add(stripped)
                new_lines.append(line)
            continue
        
        new_lines.append(line)
    
    return '\n'.join(new_lines)

for root, dirs, files in os.walk(groups_dir):
    for f in files:
        if f.endswith('.rs'):
            path = os.path.join(root, f)
            with open(path, 'r', encoding='utf-8') as fh:
                content = fh.read()
            
            fixed = dedup_imports(content)
            
            if fixed != content:
                with open(path, 'w', encoding='utf-8') as fh:
                    fh.write(fixed)
                rel = os.path.relpath(path, groups_dir)
                print(f'Fixed: {rel}')

print('Done')
