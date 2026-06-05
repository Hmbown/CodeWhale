import os

back_cfg = 'crates/tui/src/commands/back/config.rs'
with open(back_cfg, 'r', encoding='utf-8') as f:
    content = f.read()

# Find the test module
test_marker = '#[cfg(test)]\nmod tests {'
test_pos = content.find(test_marker)

# Extract test-only functions (defined before test module, used only in tests)
# We'll move them into the test module
test_only_fns = [
    'auto_model_heuristic_selection_with_bias',
    'auto_model_heuristic_with_bias',
    'auto_route_from_heuristic',
    'auto_route_prompt',
    'extract_first_json_object',
    'mode_display_name',
    'normalize_auto_route_model',
    'parse_auto_route_reasoning_effort',
    'parse_config_bool',
    'persist_provider_base_url_key',
    'persist_root_bool_key',
    'provider_base_url_table_key',
    'resolve_provider_url_value',
    'set_config',
]

# For each function, move it into the test module
production_part = content[:test_pos]
test_part = content[test_pos:]

moved = []
for fn in test_only_fns:
    # Find the function in production code
    patterns = [
        f'pub fn {fn}(',
        f'pub(crate) fn {fn}(',
        f'fn {fn}(',
    ]
    fn_start = -1
    for p in patterns:
        pos = production_part.find(p)
        if pos >= 0:
            fn_start = pos
            break
    
    if fn_start < 0:
        print(f'{fn}: not found')
        continue
    
    # Find where this function ends (next function definition or end of production)
    rest = production_part[fn_start:]
    fn_end = len(rest)
    for p in ['\npub fn ', '\npub(crate) fn ', '\nfn ']:
        pos = rest.find(p, 1)
        if pos > 0 and pos < fn_end:
            fn_end = pos
    
    fn_body = rest[:fn_end]
    
    # Remove from production (also clean up preceding blank lines)
    pre_start = fn_start
    while pre_start > 0 and production_part[pre_start-1] in '\n\r ':
        pre_start -= 1
    
    production_part = production_part[:pre_start] + production_part[fn_start + fn_end:]
    # Clean up triple blank lines
    while '\n\n\n' in production_part:
        production_part = production_part.replace('\n\n\n', '\n\n')
    
    # Add into test module (right after the opening {)
    brace_pos = test_part.find('{')
    insert_pos = test_part.find('\n', brace_pos) + 1
    
    # Make it private since it's only used in tests
    fn_body_moved = fn_body.replace('pub fn ', 'fn ').replace('pub(crate) fn ', 'fn ')
    test_part = test_part[:insert_pos] + '\n' + fn_body_moved + '\n' + test_part[insert_pos:]
    
    moved.append(fn)
    print(f'Moved {fn} into test module')

# Write back
content = production_part + '\n\n' + test_part
with open(back_cfg, 'w', encoding='utf-8') as f:
    f.write(content)

print(f'\nMoved {len(moved)} functions into test module')
