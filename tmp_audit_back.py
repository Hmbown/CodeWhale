import os, re

back_dir = r'crates/tui/src/commands/back'
groups_dir = r'crates/tui/src/commands/groups'
mod_rs = r'crates/tui/src/commands/mod.rs'

back_modules = [f.replace('.rs','') for f in os.listdir(back_dir) if f.endswith('.rs') and f != 'mod.rs']

# Read mod.rs once
with open(mod_rs, 'r') as f:
    mod_content = f.read()

for bmod in sorted(back_modules):
    cmd_files = []
    for root, dirs, files in os.walk(groups_dir):
        for f in files:
            if f.endswith('.rs'):
                path = os.path.join(root, f)
                with open(path, 'r') as fh:
                    content = fh.read()
                pattern = f'back::{bmod}::'
                if pattern in content:
                    rel = os.path.relpath(path, groups_dir)
                    cmd_files.append(rel)
    
    back_refs = mod_content.count(f'back::{bmod}::')
    
    if len(cmd_files) == 1 and back_refs == 0:
        print(f"[MERGE]  {bmod}: only used by {cmd_files[0]}")
    elif len(cmd_files) > 0 or back_refs > 0:
        callers = cmd_files.copy()
        if back_refs > 0:
            callers.append("commands/mod.rs")
        print(f"[KEEP]   {bmod}: shared by {len(callers)} callers: {callers}")
    else:
        print(f"[UNUSED] {bmod}: no references found")
