import os

path = r'crates/tui/src/commands/back/skills.rs'
with open(path, 'r') as f:
    content = f.read()

# Replace struct fields
old_struct = """    struct IsolatedHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        home_prev: Option<OsString>,
        userprofile_prev: Option<OsString>,
    }"""

new_struct = """    struct IsolatedHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        home_prev: Option<OsString>,
        userprofile_prev: Option<OsString>,
        homedrive_prev: Option<OsString>,
        homepath_prev: Option<OsString>,
    }"""

if old_struct in content:
    content = content.replace(old_struct, new_struct, 1)
    print("Replaced struct")
else:
    print("WARN: struct not found, trying CRLF version")
    old_struct_crlf = old_struct.replace('\n', '\r\n')
    new_struct_crlf = new_struct.replace('\n', '\r\n')
    content = content.replace(old_struct_crlf, new_struct_crlf, 1)
    print("Replaced struct (CRLF)")

# Replace the new() method - add HOMEDRIVE/HOMEPATH save and set
old_new = """            let home_prev = std::env::var_os("HOME");
            let userprofile_prev = std::env::var_os("USERPROFILE");
            // SAFETY: tests that mutate process env hold the shared test env
            // mutex for the full lifetime of this guard.
            unsafe {
                std::env::set_var("HOME", &home);
                std::env::set_var("USERPROFILE", &home);
            }"""

new_new = """            let home_prev = std::env::var_os("HOME");
            let userprofile_prev = std::env::var_os("USERPROFILE");
            let homedrive_prev = std::env::var_os("HOMEDRIVE");
            let homepath_prev = std::env::var_os("HOMEPATH");
            // SAFETY: tests that mutate process env hold the shared test env
            // mutex for the full lifetime of this guard.
            //
            // Override both Unix (HOME) and Windows (USERPROFILE, HOMEDRIVE,
            // HOMEPATH) home-directory env vars so that dirs::home_dir()
            // returns the isolated path on both platforms.
            unsafe {
                std::env::set_var("HOME", &home);
                std::env::set_var("USERPROFILE", &home);
                std::env::set_var("HOMEDRIVE", home.parent().unwrap_or(&home));
                std::env::set_var("HOMEPATH", home.file_name().unwrap_or_default());
            }"""

if old_new in content:
    content = content.replace(old_new, new_new, 1)
    print("Replaced new()")
else:
    old_new_crlf = old_new.replace('\n', '\r\n')
    new_new_crlf = new_new.replace('\n', '\r\n')
    content = content.replace(old_new_crlf, new_new_crlf, 1)
    print("Replaced new() (CRLF)")

# Replace the Self { ... } construction
old_self = """            Self {
                _lock: lock,
                home_prev,
                userprofile_prev,
            }"""

new_self = """            Self {
                _lock: lock,
                home_prev,
                userprofile_prev,
                homedrive_prev,
                homepath_prev,
            }"""

if old_self in content:
    content = content.replace(old_self, new_self, 1)
    print("Replaced Self{}")
else:
    old_self_crlf = old_self.replace('\n', '\r\n')
    new_self_crlf = new_self.replace('\n', '\r\n')
    content = content.replace(old_self_crlf, new_self_crlf, 1)
    print("Replaced Self{} (CRLF)")

# Replace the Drop impl
old_drop = """                Self::restore_var("HOME", self.home_prev.take());
                Self::restore_var("USERPROFILE", self.userprofile_prev.take());"""

new_drop = """                Self::restore_var("HOME", self.home_prev.take());
                Self::restore_var("USERPROFILE", self.userprofile_prev.take());
                Self::restore_var("HOMEDRIVE", self.homedrive_prev.take());
                Self::restore_var("HOMEPATH", self.homepath_prev.take());"""

if old_drop in content:
    content = content.replace(old_drop, new_drop, 1)
    print("Replaced Drop")
else:
    old_drop_crlf = old_drop.replace('\n', '\r\n')
    new_drop_crlf = new_drop.replace('\n', '\r\n')
    content = content.replace(old_drop_crlf, new_drop_crlf, 1)
    print("Replaced Drop (CRLF)")

with open(path, 'w') as f:
    f.write(content)
print("Done")
