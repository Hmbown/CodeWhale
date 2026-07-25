//! Slash commands for the persistent network allow/deny list.

use std::fs;
use std::path::Path;

use anyhow::{Context, bail};
use toml::Value;

use crate::commands::CommandResult;
use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::network_policy::host_from_url;
use crate::tui::app::App;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "network",
    aliases: &[],
    usage: "/network [list|allow <host>|deny <host>|remove <host>|default <allow|deny|prompt>]",
    description_id: MessageId::CmdNetworkDescription,
};

pub(in crate::commands) struct NetworkCmd;

impl RegisterCommand for NetworkCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        network(app, arg)
    }
}

fn network(_app: &mut App, arg: Option<&str>) -> CommandResult {
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
    let path = crate::config_persistence::config_toml_path(None)?;
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
    let path = crate::config_persistence::config_toml_path(None)?;
    crate::config_persistence::mutate_config_document(&path, |doc| {
        ensure_network_defaults(doc)?;
        let mut allow = document_string_array(doc, "allow")?;
        let mut deny = document_string_array(doc, "deny")?;
        match edit {
            NetworkEdit::Allow => {
                remove_host(&mut deny, host);
                add_host(&mut allow, host);
            }
            NetworkEdit::Deny => {
                remove_host(&mut allow, host);
                add_host(&mut deny, host);
            }
            NetworkEdit::Remove => {
                remove_host(&mut allow, host);
                remove_host(&mut deny, host);
            }
        }
        crate::config_persistence::set_document_value(
            doc,
            &["network", "allow"],
            string_array_value(&allow),
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &["network", "deny"],
            string_array_value(&deny),
        )
    })?;
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

    let path = crate::config_persistence::config_toml_path(None)?;
    crate::config_persistence::mutate_config_document(&path, |doc| {
        ensure_network_defaults(doc)?;
        crate::config_persistence::set_document_value(doc, &["network", "default"], normalized)
    })?;

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
    toml::from_str(&raw).map_err(|_| {
        anyhow::anyhow!(
            "failed to parse config at {}; file contents were omitted",
            codewhale_config::quote_os_path(path)
        )
    })
}

fn ensure_network_defaults(doc: &mut toml_edit::DocumentMut) -> anyhow::Result<()> {
    if doc
        .get("network")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|table| table.get("default"))
        .is_none()
    {
        crate::config_persistence::set_document_value(doc, &["network", "default"], "prompt")?;
    }
    if doc
        .get("network")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|table| table.get("audit"))
        .is_none()
    {
        crate::config_persistence::set_document_value(doc, &["network", "audit"], true)?;
    }
    Ok(())
}

fn document_string_array(doc: &toml_edit::DocumentMut, key: &str) -> anyhow::Result<Vec<String>> {
    let Some(item) = doc
        .get("network")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|table| table.get(key))
    else {
        return Ok(Vec::new());
    };
    let array = item
        .as_array()
        .with_context(|| format!("`network.{key}` must be an array of strings"))?;
    array
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .with_context(|| format!("`network.{key}` must be an array of strings"))
        })
        .collect()
}

fn string_array_value(values: &[String]) -> toml_edit::Array {
    values.iter().map(String::as_str).collect()
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

fn add_host(list: &mut Vec<String>, host: &str) {
    if !list
        .iter()
        .any(|existing| normalize_host_for_compare(existing) == host)
    {
        list.push(host.to_string());
    }
}

fn remove_host(list: &mut Vec<String>, host: &str) {
    list.retain(|existing| normalize_host_for_compare(existing) != host);
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
        _lock: crate::test_support::TestEnvLock,
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
            skills_dir: home.join("skills"),
            memory_path: home.join("memory.md"),
            notes_path: home.join("notes.txt"),
            mcp_config_path: home.join("mcp.json"),
            ..crate::test_support::test_tui_options(home.to_path_buf())
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

    #[test]
    fn network_config_parse_error_omits_secret_contents_and_keys() {
        let home = temp_home("parse-redaction");
        let path = home.join("config.toml");
        let secret = "cw-secret-network-config-4507";
        fs::write(
            &path,
            format!("[providers.xai]\napi_key = \"{secret}\" trailing-junk\n"),
        )
        .unwrap();

        let error = load_config_doc(&path).expect_err("malformed config must fail");
        let diagnostic = format!("{error:#}");
        assert!(!diagnostic.contains(secret), "{diagnostic}");
        assert!(!diagnostic.contains("api_key"), "{diagnostic}");
        assert!(
            diagnostic.contains("file contents were omitted"),
            "{diagnostic}"
        );
    }
}
