//! Sanitized environment handling for child processes.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};

#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
#[cfg(windows)]
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
#[cfg(windows)]
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    RegCloseKey, RegEnumValueW, RegOpenKeyExW,
};
#[cfg(windows)]
use windows::core::{PCWSTR, PWSTR};

/// Convert a string env map into owned OS strings for child env helpers.
pub fn string_map_env(
    env: &HashMap<String, String>,
) -> impl Iterator<Item = (OsString, OsString)> + '_ {
    env.iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
}

/// Return the environment for a child process after dropping parent secrets.
///
/// `overrides` are trusted call-site values, such as sandbox markers, hook
/// variables, MCP server config, or RLM context path. They are applied after the
/// parent allowlist so explicit values win.
pub fn sanitized_child_env<I, K, V>(overrides: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut env = Vec::new();
    #[cfg(windows)]
    append_sanitized_child_env_candidates(&mut env, windows_registry_env_vars());
    for (key, value) in std::env::vars_os() {
        append_sanitized_child_env_candidate(&mut env, key, value);
    }
    for (key, value) in overrides {
        upsert_env(
            &mut env,
            key.as_ref().to_os_string(),
            value.as_ref().to_os_string(),
        );
    }
    #[cfg(windows)]
    fill_windows_common_program_files(&mut env);
    env
}

pub fn apply_to_command<I, K, V>(cmd: &mut std::process::Command, overrides: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    cmd.env_clear();
    for (key, value) in sanitized_child_env(overrides) {
        cmd.env(key, value);
    }
}

pub fn apply_to_tokio_command<I, K, V>(cmd: &mut tokio::process::Command, overrides: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    cmd.env_clear();
    for (key, value) in sanitized_child_env(overrides) {
        cmd.env(key, value);
    }
}

#[cfg(not(target_env = "ohos"))]
pub fn apply_to_pty_command<I, K, V>(cmd: &mut portable_pty::CommandBuilder, overrides: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    cmd.env_clear();
    for (key, value) in sanitized_child_env(overrides) {
        cmd.env(key, value);
    }
}

/// Build the sanitized child environment used for MCP stdio servers.
///
/// MCP stdio servers are user-configured integrations declared in
/// `~/.deepseek/mcp.json` (or equivalent). They are not arbitrary processes
/// the agent decided to launch on its own. To avoid breaking common
/// `npx ...` / `uvx ...` / `python -m mcp_server_*` setups (#1244), the
/// MCP-launch allowlist is wider than the base shell-tool allowlist: it
/// also passes through Node, npm, Python, Ruby, Java, proxy, and CA-bundle
/// bootstrap variables. It still drops arbitrary parent env so secret-bearing
/// vars (`AWS_*`, `*_API_KEY`, `GITHUB_TOKEN`, …) are not silently exported.
pub fn sanitized_mcp_env<I, K, V>(overrides: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut env = Vec::new();
    for (key, value) in std::env::vars_os() {
        if is_allowed_mcp_env_key(&key) {
            upsert_env(&mut env, key, value);
        }
    }
    for (key, value) in overrides {
        upsert_env(
            &mut env,
            key.as_ref().to_os_string(),
            value.as_ref().to_os_string(),
        );
    }
    env
}

pub fn apply_to_tokio_command_mcp<I, K, V>(cmd: &mut tokio::process::Command, overrides: I)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    cmd.env_clear();
    for (key, value) in sanitized_mcp_env(overrides) {
        cmd.env(key, value);
    }
}

fn is_allowed_parent_env_key(key: &OsStr) -> bool {
    let key = key.to_string_lossy();
    let normalized = key.to_ascii_uppercase();
    matches!(
        normalized.as_str(),
        "PATH"
            | "HOME"
            | "USER"
            | "USERNAME"
            | "LOGNAME"
            | "LANG"
            | "LANGUAGE"
            | "LC_ALL"
            | "LC_CTYPE"
            | "LC_MESSAGES"
            | "TERM"
            | "COLORTERM"
            | "NO_COLOR"
            | "FORCE_COLOR"
            | "SHELL"
            | "TMPDIR"
            | "TMP"
            | "TEMP"
            | "__CF_USER_TEXT_ENCODING"
            | "SYSTEMROOT"
            | "WINDIR"
            | "COMSPEC"
            | "PATHEXT"
            | "USERPROFILE"
            | "HOMEDRIVE"
            | "HOMEPATH"
            // Preserve Windows toolchain context when the parent shell has
            // already loaded VsDevCmd / vcvars. Without these, `exec_shell`
            // can find `link.exe` via PATH but still fail to resolve
            // SDK/CRT libraries like `kernel32.lib`, so any model-driven
            // `cargo build` from inside the TUI silently breaks on
            // Windows installs that don't run inside a Developer Command
            // Prompt. Harvested from PR #1487.
            | "LIB"
            | "LIBPATH"
            | "INCLUDE"
            | "VSINSTALLDIR"
            | "VCINSTALLDIR"
            | "VCTOOLSINSTALLDIR"
            | "WINDOWSSDKDIR"
            | "WINDOWSSDKVERSION"
            | "UNIVERSALCRTSDKDIR"
            | "UCRTVERSION"
            | "EXTENSIONSDKDIR"
            | "DEVENVDIR"
            | "VISUALSTUDIOVERSION"
            // Windows app-data + .NET/NuGet paths. `dotnet restore` (and npm,
            // pip, etc.) resolve their package caches, HTTP cache, and config
            // under %APPDATA% / %LOCALAPPDATA% / %ProgramData% / %ProgramFiles%.
            // The sanitized child env dropped these, so restore failed through
            // `exec_shell` even though it worked in the user's own shell, where
            // the full environment is present (#1857). `DOTNET_*` (below) covers
            // DOTNET_ROOT and the CLI flags.
            | "APPDATA"
            | "LOCALAPPDATA"
            | "PROGRAMDATA"
            | "ALLUSERSPROFILE"
            | "PROGRAMFILES"
            | "PROGRAMFILES(X86)"
            | "PROGRAMW6432"
            | "COMMONPROGRAMFILES"
            | "COMMONPROGRAMFILES(X86)"
            | "COMMONPROGRAMW6432"
            | "PROCESSOR_ARCHITECTURE"
            | "NUGET_PACKAGES"
            | "NUGET_HTTP_CACHE_PATH"
            // Standard proxy variables are needed by shell tasks in
            // corporate and WSL environments where direct internet egress is
            // blocked. They intentionally exclude token/API-key-shaped vars.
            | "HTTP_PROXY"
            | "HTTPS_PROXY"
            | "NO_PROXY"
            | "ALL_PROXY"
            | "FTP_PROXY"
            // Python uses these to pick stdio/default encodings when stdout is
            // piped instead of attached to a Windows console (#4202).
            | "PYTHONIOENCODING"
            | "PYTHONUTF8"
    ) || normalized.starts_with("LC_")
        // .NET CLI / SDK configuration (DOTNET_ROOT, DOTNET_CLI_*,
        // DOTNET_NOLOGO, DOTNET_CLI_TELEMETRY_OPTOUT, …). Paths and flags
        // only — no secret-shaped values (#1857).
        || normalized.starts_with("DOTNET_")
        || is_allowed_platform_path_like_child_env_key(&normalized)
}

#[cfg(windows)]
fn is_allowed_platform_path_like_child_env_key(normalized: &str) -> bool {
    is_allowed_path_like_child_env_key(normalized)
}

#[cfg(not(windows))]
fn is_allowed_platform_path_like_child_env_key(_normalized: &str) -> bool {
    false
}

#[cfg(windows)]
fn is_allowed_path_like_child_env_key(normalized: &str) -> bool {
    if is_secret_like_child_env_key(normalized) {
        return false;
    }
    normalized.ends_with("_ROOT")
        || normalized.ends_with("_DIR")
        || normalized.ends_with("_HOME")
        || normalized.ends_with("_PATH")
        || normalized.ends_with("_PATHS")
        || normalized.ends_with("SDKROOT")
}

#[cfg(windows)]
fn is_secret_like_child_env_key(normalized: &str) -> bool {
    normalized.contains("SECRET")
        || normalized.contains("TOKEN")
        || normalized.contains("PASSWORD")
        || normalized.contains("PASSWD")
        || normalized.contains("CREDENTIAL")
        || normalized.contains("API_KEY")
        || normalized.contains("ACCESS_KEY")
        || normalized.contains("PRIVATE_KEY")
        || normalized.ends_with("_KEY")
}

/// Allowlist for MCP stdio launches. Strict superset of
/// `is_allowed_parent_env_key`. See `sanitized_mcp_env` for rationale.
fn is_allowed_mcp_env_key(key: &OsStr) -> bool {
    if is_allowed_parent_env_key(key) {
        return true;
    }
    let key_str = key.to_string_lossy();
    let normalized = key_str.to_ascii_uppercase();
    if matches!(
        normalized.as_str(),
        // Node.js / npm / npx / pnpm / yarn / volta / corepack
        "NVM_DIR"
            | "NVM_BIN"
            | "NVM_INC"
            | "VOLTA_HOME"
            | "COREPACK_HOME"
            | "NODE_PATH"
            | "NODE_OPTIONS"
            | "NODE_EXTRA_CA_CERTS"
            // Python ecosystem
            | "PYTHONPATH"
            | "PYTHONHOME"
            | "PYTHONDONTWRITEBYTECODE"
            | "PYTHONUNBUFFERED"
            | "VIRTUAL_ENV"
            | "POETRY_HOME"
            | "PIPX_HOME"
            | "PIPX_BIN_DIR"
            // Ruby ecosystem
            | "GEM_HOME"
            | "GEM_PATH"
            | "BUNDLE_PATH"
            | "BUNDLE_GEMFILE"
            // Java
            | "JAVA_HOME"
            // Network proxies (uppercase form; lowercase handled below)
            | "HTTP_PROXY"
            | "HTTPS_PROXY"
            | "NO_PROXY"
            | "ALL_PROXY"
            | "FTP_PROXY"
            // Custom CA bundles for corporate TLS interception
            | "SSL_CERT_FILE"
            | "SSL_CERT_DIR"
            | "REQUESTS_CA_BUNDLE"
            | "CURL_CA_BUNDLE"
    ) {
        return true;
    }
    // npm config namespace (NPM_CONFIG_PREFIX, NPM_CONFIG_CACHE, …) and
    // uv (UV_CACHE_DIR, UV_PYTHON, …) — both ecosystems use a stable prefix
    // for their bootstrap configuration, so allow the whole namespace.
    if normalized.starts_with("NPM_CONFIG_") || normalized.starts_with("UV_") {
        return true;
    }
    false
}

#[cfg(windows)]
fn append_sanitized_child_env_candidates<I, K, V>(
    env: &mut Vec<(OsString, OsString)>,
    candidates: I,
) where
    I: IntoIterator<Item = (K, V)>,
    K: Into<OsString>,
    V: Into<OsString>,
{
    for (key, value) in candidates {
        append_sanitized_child_env_candidate(env, key.into(), value.into());
    }
}

fn append_sanitized_child_env_candidate(
    env: &mut Vec<(OsString, OsString)>,
    key: OsString,
    value: OsString,
) {
    if is_allowed_parent_env_key(&key) {
        upsert_env(env, key, value);
    }
}

fn upsert_env(env: &mut Vec<(OsString, OsString)>, key: OsString, value: OsString) {
    let normalized = normalize_key(&key);
    env.retain(|(existing, _)| normalize_key(existing) != normalized);
    env.push((key, value));
}

#[cfg(windows)]
fn windows_registry_env_vars() -> Vec<(OsString, OsString)> {
    let mut env = Vec::new();
    append_windows_registry_env_key(
        &mut env,
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    );
    append_windows_registry_env_key(&mut env, HKEY_CURRENT_USER, "Environment");
    env
}

#[cfg(windows)]
fn append_windows_registry_env_key(env: &mut Vec<(OsString, OsString)>, root: HKEY, subkey: &str) {
    let mut key = HKEY::default();
    let subkey_wide = windows_wide_null(OsStr::new(subkey));
    let open =
        unsafe { RegOpenKeyExW(root, PCWSTR(subkey_wide.as_ptr()), None, KEY_READ, &mut key) };
    if open != ERROR_SUCCESS {
        return;
    }

    let mut index = 0;
    loop {
        match read_windows_registry_env_value(key, index) {
            RegistryEnvValue::Value(name, value) => {
                upsert_env(env, name, value);
                index += 1;
            }
            RegistryEnvValue::Skip => {
                index += 1;
            }
            RegistryEnvValue::Done => break,
        }
    }

    let _ = unsafe { RegCloseKey(key) };
}

#[cfg(windows)]
enum RegistryEnvValue {
    Value(OsString, OsString),
    Skip,
    Done,
}

#[cfg(windows)]
fn read_windows_registry_env_value(key: HKEY, index: u32) -> RegistryEnvValue {
    let mut name = vec![0u16; 32_767];
    let mut data = vec![0u8; 65_536];

    loop {
        let mut name_len = name.len() as u32;
        let mut data_len = data.len() as u32;
        let mut value_type = 0u32;
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut value_type),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };

        if status == ERROR_NO_MORE_ITEMS {
            return RegistryEnvValue::Done;
        }
        if status == ERROR_MORE_DATA && resize_registry_data_buffer(&mut data, data_len) {
            continue;
        }
        if status != ERROR_SUCCESS {
            return RegistryEnvValue::Skip;
        }
        if value_type != REG_SZ.0 && value_type != REG_EXPAND_SZ.0 {
            return RegistryEnvValue::Skip;
        }

        let name = OsString::from_wide(&name[..name_len as usize]);
        let value = registry_utf16_value_from_bytes(&data[..data_len as usize]);
        let value = if REG_VALUE_TYPE(value_type) == REG_EXPAND_SZ {
            expand_windows_env_string(&value).unwrap_or(value)
        } else {
            value
        };
        return RegistryEnvValue::Value(name, value);
    }
}

#[cfg(windows)]
fn resize_registry_data_buffer(data: &mut Vec<u8>, required_len: u32) -> bool {
    let Ok(required_len) = usize::try_from(required_len) else {
        return false;
    };
    if required_len <= data.len() {
        return false;
    }
    data.resize(required_len, 0);
    true
}

#[cfg(windows)]
fn registry_utf16_value_from_bytes(data: &[u8]) -> OsString {
    let mut wide = data
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    while wide.last() == Some(&0) {
        wide.pop();
    }
    OsString::from_wide(&wide)
}

#[cfg(windows)]
fn expand_windows_env_string(value: &OsStr) -> Option<OsString> {
    let src = windows_wide_null(value);
    let required_len = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), None) };
    if required_len == 0 {
        return None;
    }

    let mut expanded = vec![0u16; required_len as usize];
    let written = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut expanded)) };
    if written == 0 || written > required_len {
        return None;
    }

    let len = usize::try_from(written).ok()?.saturating_sub(1);
    Some(OsString::from_wide(&expanded[..len]))
}

#[cfg(windows)]
fn windows_wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(any(windows, test))]
fn fill_windows_common_program_files(env: &mut Vec<(OsString, OsString)>) {
    for (key, default) in [
        ("CommonProgramFiles", r"C:\Program Files\Common Files"),
        (
            "CommonProgramFiles(x86)",
            r"C:\Program Files (x86)\Common Files",
        ),
        ("CommonProgramW6432", r"C:\Program Files\Common Files"),
    ] {
        let existing = env
            .iter()
            .find(|(existing, _)| normalize_key(existing) == normalize_key(OsStr::new(key)))
            .map(|(_, value)| value.to_string_lossy().trim().is_empty());
        if existing.unwrap_or(true) {
            upsert_env(env, OsString::from(key), OsString::from(default));
        }
    }
}

fn normalize_key(key: &OsStr) -> String {
    key.to_string_lossy().to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn mcp_env_allowlist_inherits_base_keys() {
        for key in [
            "PATH",
            "HOME",
            "USER",
            "TERM",
            "LANG",
            "SHELL",
            "LIB",
            "LIBPATH",
            "INCLUDE",
            "VCTOOLSINSTALLDIR",
            "WINDOWSSDKDIR",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should inherit base key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_node_bootstrap_keys() {
        for key in [
            "NVM_DIR",
            "NVM_BIN",
            "NVM_INC",
            "NODE_PATH",
            "NODE_OPTIONS",
            "NODE_EXTRA_CA_CERTS",
            "VOLTA_HOME",
            "COREPACK_HOME",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_npm_config_prefix() {
        for key in [
            "NPM_CONFIG_PREFIX",
            "NPM_CONFIG_CACHE",
            "NPM_CONFIG_REGISTRY",
            "NPM_CONFIG_USERCONFIG",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include npm config key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_proxy_keys_either_case() {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "NO_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "no_proxy",
            "all_proxy",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include proxy key {key}"
            );
        }
    }

    #[test]
    fn child_env_allowlist_includes_proxy_keys_either_case() {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "NO_PROXY",
            "ALL_PROXY",
            "FTP_PROXY",
            "http_proxy",
            "https_proxy",
            "no_proxy",
            "all_proxy",
            "ftp_proxy",
        ] {
            assert!(
                is_allowed_parent_env_key(OsStr::new(key)),
                "child env allowlist should include proxy key {key}"
            );
        }
    }

    #[test]
    fn child_env_allowlist_includes_dotnet_and_windows_appdata_keys() {
        // #1857: dotnet restore / NuGet need these to find caches and config.
        for key in [
            "APPDATA",
            "LOCALAPPDATA",
            "PROGRAMDATA",
            "ALLUSERSPROFILE",
            "PROGRAMFILES",
            "PROGRAMFILES(X86)",
            "PROGRAMW6432",
            "COMMONPROGRAMFILES",
            "COMMONPROGRAMFILES(X86)",
            "COMMONPROGRAMW6432",
            "PROCESSOR_ARCHITECTURE",
            "NUGET_PACKAGES",
            "DOTNET_ROOT",
            "DOTNET_CLI_TELEMETRY_OPTOUT",
            "DOTNET_NOLOGO",
            // Case-insensitive: the real Windows var is `ProgramFiles`.
            "ProgramFiles",
            "dotnet_root",
        ] {
            assert!(
                is_allowed_parent_env_key(OsStr::new(key)),
                "child env allowlist should include {key}"
            );
        }
        // Guard: NuGet credential env vars must still be dropped.
        assert!(
            !is_allowed_parent_env_key(OsStr::new("NuGetPackageSourceCredentials_feed")),
            "NuGet credential vars must not be exported to child processes"
        );
    }

    #[test]
    fn child_env_allowlist_includes_python_stdio_encoding_vars() {
        for key in ["PYTHONIOENCODING", "PYTHONUTF8", "pythonioencoding"] {
            assert!(
                is_allowed_parent_env_key(OsStr::new(key)),
                "child env allowlist should include Python stdio encoding key {key}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn child_env_allowlist_includes_custom_path_like_vars_without_secrets() {
        // #3572: SDK/toolchain roots created through Windows Environment
        // Variables are often project-specific and cannot be exhaustively
        // named in the static allowlist.
        for key in [
            "BIMRV_SDK_ROOT",
            "ACME_TOOLCHAIN_HOME",
            "PROJECT_SDK_DIR",
            "CMAKE_PREFIX_PATH",
            "ANDROID_SDKROOT",
        ] {
            assert!(
                is_allowed_parent_env_key(OsStr::new(key)),
                "child env allowlist should include path-like key {key}"
            );
        }

        for key in [
            "OPENAI_API_KEY",
            "GITHUB_TOKEN",
            "MY_SECRET_ROOT",
            "SERVICE_PASSWORD_DIR",
            "AWS_ACCESS_KEY_ID",
            "PRIVATE_KEY_PATH",
            "NuGetPackageSourceCredentials_feed",
        ] {
            assert!(
                !is_allowed_parent_env_key(OsStr::new(key)),
                "secret-like key {key} must not be exported to child processes"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn sanitized_child_env_preserves_custom_sdk_root_vars() {
        let _guard = env_lock().lock().expect("env lock");
        let previous_sdk = std::env::var_os("BIMRV_SDK_ROOT");
        let previous_secret = std::env::var_os("MY_SECRET_ROOT");
        unsafe {
            std::env::set_var("BIMRV_SDK_ROOT", r"F:\Lib\BimRv27.5");
            std::env::set_var("MY_SECRET_ROOT", r"F:\Secrets");
        }

        let env = sanitized_child_env(std::iter::empty::<(OsString, OsString)>());

        unsafe {
            match previous_sdk {
                Some(value) => std::env::set_var("BIMRV_SDK_ROOT", value),
                None => std::env::remove_var("BIMRV_SDK_ROOT"),
            }
            match previous_secret {
                Some(value) => std::env::set_var("MY_SECRET_ROOT", value),
                None => std::env::remove_var("MY_SECRET_ROOT"),
            }
        }

        assert!(
            env.iter()
                .any(|(key, value)| key == "BIMRV_SDK_ROOT" && value == r"F:\Lib\BimRv27.5"),
            "child env should preserve custom SDK roots"
        );
        assert!(
            env.iter().all(|(key, _)| key != "MY_SECRET_ROOT"),
            "secret-like path vars must still be dropped"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_registry_env_candidates_preserve_custom_sdk_roots() {
        use windows::Win32::System::Registry::{
            HKEY_CURRENT_USER, REG_SZ, RegCreateKeyW, RegDeleteTreeW, RegSetValueExW,
        };

        let subkey = format!(r"Software\CodeWhaleTest\child_env_{}", std::process::id());
        let subkey_wide = windows_wide_null(OsStr::new(&subkey));
        let mut key = HKEY::default();
        let created =
            unsafe { RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(subkey_wide.as_ptr()), &mut key) };
        assert_eq!(created, ERROR_SUCCESS);

        set_registry_string_value(key, "BIMRV_SDK_ROOT", r"F:\Lib\BimRv27.5");
        set_registry_string_value(key, "MY_SECRET_ROOT", r"F:\Secrets");
        let _ = unsafe { RegCloseKey(key) };

        let mut candidates = Vec::new();
        append_windows_registry_env_key(&mut candidates, HKEY_CURRENT_USER, &subkey);
        let mut env = Vec::new();
        append_sanitized_child_env_candidates(&mut env, candidates);

        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(subkey_wide.as_ptr())) };

        assert!(
            env.iter()
                .any(|(key, value)| key == "BIMRV_SDK_ROOT" && value == r"F:\Lib\BimRv27.5"),
            "registry child env should preserve custom SDK roots"
        );
        assert!(
            env.iter().all(|(key, _)| key != "MY_SECRET_ROOT"),
            "secret-like registry vars must still be dropped"
        );

        fn set_registry_string_value(key: HKEY, name: &str, value: &str) {
            let name_wide = windows_wide_null(OsStr::new(name));
            let data = value
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>();
            let status = unsafe {
                RegSetValueExW(key, PCWSTR(name_wide.as_ptr()), None, REG_SZ, Some(&data))
            };
            assert_eq!(status, ERROR_SUCCESS);
        }
    }

    #[test]
    fn windows_common_program_files_defaults_replace_empty_values() {
        let mut env = vec![
            (OsString::from("CommonProgramFiles"), OsString::new()),
            (
                OsString::from("CommonProgramFiles(x86)"),
                OsString::from(" "),
            ),
            (
                OsString::from("CommonProgramW6432"),
                OsString::from(r"D:\Common Files"),
            ),
        ];

        fill_windows_common_program_files(&mut env);

        let get = |name: &str| {
            env.iter()
                .find(|(key, _)| normalize_key(key) == normalize_key(OsStr::new(name)))
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };
        assert_eq!(
            get("CommonProgramFiles").as_deref(),
            Some(r"C:\Program Files\Common Files")
        );
        assert_eq!(
            get("CommonProgramFiles(x86)").as_deref(),
            Some(r"C:\Program Files (x86)\Common Files")
        );
        assert_eq!(
            get("CommonProgramW6432").as_deref(),
            Some(r"D:\Common Files")
        );
    }

    #[test]
    fn mcp_env_allowlist_includes_python_bootstrap_keys() {
        for key in [
            "PYTHONPATH",
            "PYTHONHOME",
            "VIRTUAL_ENV",
            "PIPX_HOME",
            "PIPX_BIN_DIR",
            "POETRY_HOME",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include python bootstrap key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_uv_prefixed_keys() {
        for key in ["UV_CACHE_DIR", "UV_INDEX_URL", "UV_PYTHON"] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include uv prefixed key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_includes_ca_bundles() {
        for key in [
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "REQUESTS_CA_BUNDLE",
            "CURL_CA_BUNDLE",
        ] {
            assert!(
                is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist should include CA bundle key {key}"
            );
        }
    }

    #[test]
    fn mcp_env_allowlist_excludes_secrets_and_creds() {
        for key in [
            "AWS_SECRET_ACCESS_KEY",
            "AWS_ACCESS_KEY_ID",
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "DEEPSEEK_API_KEY",
            "SLACK_TOKEN",
            "MY_RANDOM_SECRET",
        ] {
            assert!(
                !is_allowed_mcp_env_key(OsStr::new(key)),
                "MCP allowlist must NOT include {key}"
            );
        }
    }

    #[test]
    fn sanitized_mcp_env_passes_through_node_bootstrap() {
        let _guard = env_lock().lock().expect("env lock");
        let prev = std::env::var_os("NVM_DIR");
        unsafe {
            std::env::set_var("NVM_DIR", "/tmp/test-nvm");
        }

        let env = sanitized_mcp_env(std::iter::empty::<(OsString, OsString)>());

        match prev {
            Some(value) => unsafe { std::env::set_var("NVM_DIR", value) },
            None => unsafe { std::env::remove_var("NVM_DIR") },
        }

        let nvm_dir = env
            .iter()
            .find(|(key, _)| normalize_key(key) == "NVM_DIR")
            .map(|(_, value)| value.clone());
        assert_eq!(nvm_dir, Some(OsString::from("/tmp/test-nvm")));
    }

    #[test]
    fn sanitized_mcp_env_drops_unrelated_secret_like_values() {
        let _guard = env_lock().lock().expect("env lock");
        let prev = std::env::var_os("DEEPSEEK_MCP_TEST_SECRET");
        unsafe {
            std::env::set_var("DEEPSEEK_MCP_TEST_SECRET", "should-not-leak");
        }

        let env = sanitized_mcp_env(std::iter::empty::<(OsString, OsString)>());

        match prev {
            Some(value) => unsafe {
                std::env::set_var("DEEPSEEK_MCP_TEST_SECRET", value);
            },
            None => unsafe {
                std::env::remove_var("DEEPSEEK_MCP_TEST_SECRET");
            },
        }

        assert!(
            env.iter().all(|(key, _)| key != "DEEPSEEK_MCP_TEST_SECRET"),
            "MCP env should not pass arbitrary parent vars"
        );
    }

    #[test]
    fn sanitized_child_env_drops_parent_secret_like_values() {
        let _guard = env_lock().lock().expect("env lock");
        let previous = std::env::var_os("DEEPSEEK_CHILD_ENV_TEST_SECRET");
        unsafe {
            std::env::set_var("DEEPSEEK_CHILD_ENV_TEST_SECRET", "parent-secret");
        }

        let env = sanitized_child_env(std::iter::empty::<(OsString, OsString)>());

        match previous {
            Some(value) => unsafe {
                std::env::set_var("DEEPSEEK_CHILD_ENV_TEST_SECRET", value);
            },
            None => unsafe {
                std::env::remove_var("DEEPSEEK_CHILD_ENV_TEST_SECRET");
            },
        }

        assert!(
            env.iter()
                .all(|(key, _)| key != "DEEPSEEK_CHILD_ENV_TEST_SECRET")
        );
    }

    #[test]
    fn explicit_child_env_values_win_over_parent_allowlist() {
        let _guard = env_lock().lock().expect("env lock");
        let previous = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", "/parent/bin");
        }

        let env = sanitized_child_env([(OsString::from("PATH"), OsString::from("/explicit/bin"))]);

        match previous {
            Some(value) => unsafe {
                std::env::set_var("PATH", value);
            },
            None => unsafe {
                std::env::remove_var("PATH");
            },
        }

        let path = env
            .iter()
            .find(|(key, _)| normalize_key(key) == "PATH")
            .map(|(_, value)| value);
        assert_eq!(path, Some(&OsString::from("/explicit/bin")));
    }

    #[test]
    fn sanitized_child_env_preserves_windows_toolchain_vars() {
        let _guard = env_lock().lock().expect("env lock");
        let prev_lib = std::env::var_os("LIB");
        let prev_include = std::env::var_os("INCLUDE");
        let prev_sdk = std::env::var_os("WINDOWSSDKDIR");
        // SAFETY: serialised by env_lock above. Restoring after the
        // assertion is also under the same guard so concurrent tests
        // never see our staged values.
        unsafe {
            std::env::set_var("LIB", r"C:\sdk\lib");
            std::env::set_var("INCLUDE", r"C:\sdk\include");
            std::env::set_var("WINDOWSSDKDIR", r"C:\sdk");
        }

        let env = sanitized_child_env(std::iter::empty::<(OsString, OsString)>());

        // Restore prior state before asserting so a panic still leaves
        // the process env clean for the next test.
        unsafe {
            match prev_lib {
                Some(value) => std::env::set_var("LIB", value),
                None => std::env::remove_var("LIB"),
            }
            match prev_include {
                Some(value) => std::env::set_var("INCLUDE", value),
                None => std::env::remove_var("INCLUDE"),
            }
            match prev_sdk {
                Some(value) => std::env::set_var("WINDOWSSDKDIR", value),
                None => std::env::remove_var("WINDOWSSDKDIR"),
            }
        }

        assert!(
            env.iter()
                .any(|(key, value)| key == "LIB" && value == r"C:\sdk\lib"),
            "child env should preserve LIB"
        );
        assert!(
            env.iter()
                .any(|(key, value)| key == "INCLUDE" && value == r"C:\sdk\include"),
            "child env should preserve INCLUDE"
        );
        assert!(
            env.iter()
                .any(|(key, value)| key == "WINDOWSSDKDIR" && value == r"C:\sdk"),
            "child env should preserve WINDOWSSDKDIR"
        );
    }
}
