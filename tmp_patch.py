p = r'crates/tui/src/commands/groups/project/change/change_impl.rs'
with open(p, 'r', encoding='utf-8') as f:
    c = f.read()
old = 'include_str!("../../../CHANGELOG.md")'
new = 'include_str!("../../../../../CHANGELOG.md")'
c = c.replace(old, new)
with open(p, 'w', encoding='utf-8') as f:
    f.write(c)
print('Fixed')
