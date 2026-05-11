//! Slash commands for the persistent network allow/deny list.

use std::fs;
use std::path::Path;

use anyhow::{Context, bail};
use toml::Value;

use super::CommandResult;
use crate::network_policy::host_from_url;
use crate::tui::app::App;

pub fn network(_app: &mut App, arg: Option<&str>) -> CommandResult {
    match network_inner(arg) {
        Ok(message) => CommandResult::message(message),
        Err(err) => CommandResult::error(err.to_string()),
    }
}

fn network_inner(arg: Option<&str>) -> anyhow::Result<String> {
    let raw = arg.map(str::trim).unwrap_or("");
    if raw.is_empty() || raw.eq_ignore_ascii_case("list") {
        return list_policy();
    }

    let mut parts = raw.split_whitespace();
    let Some(command) = parts.next() else {
        return list_policy();
    };
    let command = command.to_ascii_lowercase();

    match command.as_str() {
        "allow" | "deny" | "remove" | "forget" => {
            let Some(host_arg) = parts.next() else {
                bail!("Usage: /network {command} <host>");
            };
            if parts.next().is_some() {
                bail!("Usage: /network {command} <host>");
            }
            let host = normalize_host_arg(host_arg)?;
            let edit = match command.as_str() {
                "allow" => NetworkEdit::Allow,
                "deny" => NetworkEdit::Deny,
                _ => NetworkEdit::Remove,
            };
            update_host(edit, &host)
        }
        "default" => {
            let Some(value) = parts.next() else {
                bail!("Usage: /network default <allow|deny|prompt>");
            };
            if parts.next().is_some() {
                bail!("Usage: /network default <allow|deny|prompt>");
            }
            update_default(value)
        }
        _ => bail!(usage()),
    }
}

fn usage() -> &'static str {
    "Usage: /network [list|allow <host>|deny <host>|remove <host>|default <allow|deny|prompt>]"
}

#[derive(Clone, Copy)]
enum NetworkEdit {
    Allow,
    Deny,
    Remove,
}

fn list_policy() -> anyhow::Result<String> {
    let path = super::config::config_toml_path()?;
    let doc = load_config_doc(&path)?;
    let network = doc.get("network").and_then(Value::as_table);
    let default = network
        .and_then(|table| table.get("default"))
        .and_then(Value::as_str)
        .unwrap_or("prompt");
    let allow = network
        .map(|table| string_array(table, "allow"))
        .unwrap_or_default();
    let deny = network
        .map(|table| string_array(table, "deny"))
        .unwrap_or_default();

    Ok(format!(
        "Network policy ({})\n\
         default = {default}\n\
         allow = {}\n\
         deny = {}\n\n\
         Use `/network allow <host>` to allow a host, `/network deny <host>` to block it, or `/network remove <host>` to clear an entry.",
        path.display(),
        display_list(&allow),
        display_list(&deny)
    ))
}

fn update_host(edit: NetworkEdit, host: &str) -> anyhow::Result<String> {
    let path = super::config::config_toml_path()?;
    let mut doc = load_config_doc(&path)?;
    let network = network_table_mut(&mut doc)?;

    match edit {
        NetworkEdit::Allow => {
            remove_host(network, "deny", host)?;
            add_host(network, "allow", host)?;
        }
        NetworkEdit::Deny => {
            remove_host(network, "allow", host)?;
            add_host(network, "deny", host)?;
        }
        NetworkEdit::Remove => {
            remove_host(network, "allow", host)?;
            remove_host(network, "deny", host)?;
        }
    }

    save_config_doc(&path, &doc)?;
    let action = match edit {
        NetworkEdit::Allow => "allowed",
        NetworkEdit::Deny => "denied",
        NetworkEdit::Remove => "removed",
    };
    Ok(format!(
        "Network host {action}: {host}\nSaved to {}. Retry the command now.",
        path.display()
    ))
}

fn update_default(value: &str) -> anyhow::Result<String> {
    let normalized = match value.trim().to_ascii_lowercase().as_str() {
        "allow" => "allow",
        "deny" | "block" => "deny",
        "prompt" | "ask" => "prompt",
        _ => bail!("Usage: /network default <allow|deny|prompt>"),
    };

    let path = super::config::config_toml_path()?;
    let mut doc = load_config_doc(&path)?;
    let network = network_table_mut(&mut doc)?;
    network.insert("default".to_string(), Value::String(normalized.to_string()));
    save_config_doc(&path, &doc)?;

    Ok(format!(
        "Network default set to {normalized}\nSaved to {}.",
        path.display()
    ))
}

fn load_config_doc(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(Value::Table(toml::value::Table::new()));
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse config at {}", path.display()))
}

fn save_config_doc(path: &Path, doc: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(doc).context("failed to serialize config.toml")?;
    fs::write(path, body).with_context(|| format!("failed to write config at {}", path.display()))
}

fn network_table_mut(doc: &mut Value) -> anyhow::Result<&mut toml::value::Table> {
    let root = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    let entry = root
        .entry("network".to_string())
        .or_insert_with(|| Value::Table(toml::value::Table::new()));
    let table = entry
        .as_table_mut()
        .context("`network` section in config.toml must be a table")?;
    table
        .entry("default".to_string())
        .or_insert_with(|| Value::String("prompt".to_string()));
    table
        .entry("audit".to_string())
        .or_insert_with(|| Value::Boolean(true));
    Ok(table)
}

fn string_array(table: &toml::value::Table, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn string_array_mut<'a>(
    table: &'a mut toml::value::Table,
    key: &str,
) -> anyhow::Result<&'a mut Vec<Value>> {
    let value = table
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    value
        .as_array_mut()
        .with_context(|| format!("`network.{key}` must be an array of strings"))
}

fn add_host(table: &mut toml::value::Table, key: &str, host: &str) -> anyhow::Result<()> {
    let list = string_array_mut(table, key)?;
    if !list
        .iter()
        .filter_map(Value::as_str)
        .any(|existing| normalize_host_for_compare(existing) == host)
    {
        list.push(Value::String(host.to_string()));
    }
    Ok(())
}

fn remove_host(table: &mut toml::value::Table, key: &str, host: &str) -> anyhow::Result<()> {
    let list = string_array_mut(table, key)?;
    list.retain(|value| {
        value
            .as_str()
            .is_none_or(|existing| normalize_host_for_compare(existing) != host)
    });
    Ok(())
}

fn normalize_host_arg(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    let host = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        host_from_url(trimmed).context("URL must include a host")?
    } else {
        if trimmed.contains("://") || trimmed.contains('/') {
            bail!("Pass a host like `github.com`, not a URL path");
        }
        trimmed.to_string()
    };

    let normalized = normalize_host_for_compare(&host);
    if normalized.is_empty() {
        bail!("host cannot be empty");
    }
    Ok(normalized)
}

fn normalize_host_for_compare(host: &str) -> String {
    let trimmed = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if let Some(rest) = trimmed.strip_prefix("*.") {
        format!(".{rest}")
    } else {
        trimmed
    }
}

fn display_list(values: &[String]) -> String {
    if values.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", values.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::env;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct EnvGuard {
        home: Option<OsString>,
        userprofile: Option<OsString>,
        deepseek_config_path: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(home: &Path) -> Self {
            let lock = crate::test_support::lock_test_env();
            let config_path = home.join(".deepseek").join("config.toml");
            let home_prev = env::var_os("HOME");
            let userprofile_prev = env::var_os("USERPROFILE");
            let deepseek_config_prev = env::var_os("DEEPSEEK_CONFIG_PATH");

            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                env::set_var("HOME", home.as_os_str());
                env::set_var("USERPROFILE", home.as_os_str());
                env::set_var("DEEPSEEK_CONFIG_PATH", config_path.as_os_str());
            }

            Self {
                home: home_prev,
                userprofile: userprofile_prev,
                deepseek_config_path: deepseek_config_prev,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore_env("HOME", self.home.take());
            restore_env("USERPROFILE", self.userprofile.take());
            restore_env("DEEPSEEK_CONFIG_PATH", self.deepseek_config_path.take());
        }
    }

    fn restore_env(key: &str, value: Option<OsString>) {
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            if let Some(value) = value {
                env::set_var(key, value);
            } else {
                env::remove_var(key);
            }
        }
    }

    fn temp_home(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "deepseek-network-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn create_test_app(home: &Path) -> App {
        let options = TuiOptions {
            model: "test-model".to_string(),
            workspace: home.to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: home.join("skills"),
            memory_path: home.join("memory.md"),
            notes_path: home.join("notes.txt"),
            mcp_config_path: home.join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn network_allow_persists_host_and_removes_exact_deny() {
        let home = temp_home("allow");
        let _guard = EnvGuard::new(&home);
        let config_path = home.join(".deepseek").join("config.toml");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            "[network]\ndefault = \"prompt\"\ndeny = [\"github.com\"]\n",
        )
        .unwrap();

        let mut app = create_test_app(&home);
        let result = network(&mut app, Some("allow GitHub.COM"));

        assert!(!result.is_error, "{:?}", result.message);
        let body = fs::read_to_string(config_path).unwrap();
        assert!(body.contains("allow = [\"github.com\"]"), "{body}");
        assert!(body.contains("deny = []"), "{body}");
    }

    #[test]
    fn network_allow_extracts_host_from_url() {
        let home = temp_home("url");
        let _guard = EnvGuard::new(&home);

        let mut app = create_test_app(&home);
        let result = network(&mut app, Some("allow https://github.com/obra/superpowers"));

        assert!(!result.is_error, "{:?}", result.message);
        let body = fs::read_to_string(home.join(".deepseek").join("config.toml")).unwrap();
        assert!(body.contains("allow = [\"github.com\"]"), "{body}");
    }

    #[test]
    fn network_default_rejects_unknown_value() {
        let home = temp_home("default");
        let _guard = EnvGuard::new(&home);

        let mut app = create_test_app(&home);
        let result = network(&mut app, Some("default maybe"));

        assert!(result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("/network default <allow|deny|prompt>")
        );
    }
}
