path = r'crates/tui/src/commands/back/skills.rs'
with open(path, 'rb') as f:
    raw = f.read()

# The duplicate Drop impl (old version, omits HOMEDRIVE/HOMEPATH)
old = (
    b'    impl Drop for IsolatedHome {\n'
    b'        fn drop(&mut self) {\n'
    b'            // SAFETY: the shared test env mutex is still held while Drop runs.\n'
    b'            unsafe {\n'
    b'                Self::restore_var("HOME", self.home_prev.take());\n'
    b'                Self::restore_var("USERPROFILE", self.userprofile_prev.take());\n'
    b'            }\n'
    b'        }\n'
    b'    }\n'
    b'\n'
)

idx = raw.find(old)
if idx >= 0:
    raw = raw[:idx] + raw[idx+len(old):]
    with open(path, 'wb') as f:
        f.write(raw)
    print(f'Removed duplicate Drop impl at byte {idx}, new size: {len(raw)}')
else:
    old_crlf = old.replace(b'\n', b'\r\n')
    idx = raw.find(old_crlf)
    if idx >= 0:
        raw = raw[:idx] + raw[idx+len(old_crlf):]
        with open(path, 'wb') as f:
            f.write(raw)
        print(f'Removed duplicate Drop impl at byte {idx} (CRLF), new size: {len(raw)}')
    else:
        # Maybe it's surrounded differently - find by content
        print('Searching for partial match...')
        # Find all occurrences
        count = 0
        pos = 0
        while True:
            idx = raw.find(b'impl Drop for IsolatedHome', pos)
            if idx < 0:
                break
            count += 1
            print(f'  Found at byte {idx}')
            pos = idx + 1
        print(f'Total occurrences: {count}')
