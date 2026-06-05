import os

# Fix memory/mod.rs group barrel
path = r'crates/tui/src/commands/groups/memory/mod.rs'
with open(path, 'r', encoding='utf-8') as f:
    content = f.read()
content = content.replace('\npub use memory_impl::memory;', '')
with open(path, 'w', encoding='utf-8') as f:
    f.write(content)
print('Fixed memory/mod.rs group barrel')

# Fix change_impl.rs include_str path
path = r'crates/tui/src/commands/groups/project/change/change_impl.rs'
with open(path, 'r', encoding='utf-8') as f:
    content = f.read()
old = 'include_str!("../../../CHANGELOG.md")'
new = 'include_str!("../../../../../CHANGELOG.md")'
content = content.replace(old, new)
with open(path, 'w', encoding='utf-8') as f:
    f.write(content)
print('Fixed change_impl.rs path')

# Make config_toml_path pub(crate)
path = r'crates/tui/src/commands/back/config.rs'
with open(path, 'r', encoding='utf-8') as f:
    content = f.read()
old = 'pub(super) fn config_toml_path(config_path: Option<&Path>) -> anyhow::Result<PathBuf> {'
new = 'pub(crate) fn config_toml_path(config_path: Option<&Path>) -> anyhow::Result<PathBuf> {'
content = content.replace(old, new)
with open(path, 'w', encoding='utf-8') as f:
    f.write(content)
print('Fixed config_toml_path visibility')

print('All done')
