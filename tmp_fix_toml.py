import re

p = r'C:\myWork\AboimPintoConsulting\CodeWhale-worktrees\feat\command-strategy\crates\tui\Cargo.toml'
with open(p, 'r', encoding='utf-8') as f:
    lines = f.readlines()

# Remove any line containing 'linkme' that is under [target.'cfg(unix)'.dependencies]
result = []
in_unix_section = False
for line in lines:
    if line.strip().startswith("[target.") and "unix" in line:
        in_unix_section = True
    elif line.strip().startswith("[target.") or line.strip().startswith("["):
        in_unix_section = False
    if in_unix_section and 'linkme' in line:
        continue
    result.append(line)

# Add linkme to main dependencies (after a stable anchor point)
content = ''.join(result)
# Find 'itertools' line and add linkme after it
anchor = 'itertools = "0.14"'
if anchor in content:
    content = content.replace(anchor, anchor + '\nlinkme = "0.3"', 1)

with open(p, 'w', encoding='utf-8') as f:
    f.write(content)
print('Done')
