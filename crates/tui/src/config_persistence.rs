//! Config file path resolution and TOML persistence helpers.
//!
//! These helpers are used by command handlers and non-command UI code, so
//! persistence lives outside the command tree.

use std::path::{Path, PathBuf};

use crate::config::{ApiProvider, StatusItem, effective_home_dir, expand_path};

pub(crate) fn persist_status_items(items: &[StatusItem]) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path(None)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let (mut doc, original_raw) = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let doc: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;
        (doc, Some(raw))
    } else {
        (toml::Value::Table(toml::value::Table::new()), None)
    };

    let table = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    let tui_entry = table
        .entry("tui".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let tui_table = tui_entry
        .as_table_mut()
        .context("`tui` section in config.toml must be a table")?;
    let array = items
        .iter()
        .map(|item| toml::Value::String(item.key().to_string()))
        .collect::<Vec<_>>();
    tui_table.insert("status_items".to_string(), toml::Value::Array(array));

    if let Some(raw) = original_raw {
        save_toml_preserving_comments(&path, &doc, &raw)?;
    } else {
        let body = toml::to_string_pretty(&doc).context("failed to serialize config.toml")?;
        fs::write(&path, body)
            .with_context(|| format!("failed to write config at {}", path.display()))?;
    }
    Ok(path)
}

pub(crate) fn persist_root_string_key(
    config_path: Option<&Path>,
    key: &str,
    value: &str,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path(config_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let (mut doc, original_raw) = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let doc: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;
        (doc, Some(raw))
    } else {
        (toml::Value::Table(toml::value::Table::new()), None)
    };
    let table = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    table.insert(key.to_string(), toml::Value::String(value.to_string()));
    if let Some(raw) = original_raw {
        save_toml_preserving_comments(&path, &doc, &raw)?;
    } else {
        let body = toml::to_string_pretty(&doc).context("failed to serialize config.toml")?;
        fs::write(&path, body)
            .with_context(|| format!("failed to write config at {}", path.display()))?;
    }
    Ok(path)
}

pub(crate) fn persist_root_bool_key(
    config_path: Option<&Path>,
    key: &str,
    value: bool,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path(config_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let (mut doc, original_raw) = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let doc: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;
        (doc, Some(raw))
    } else {
        (toml::Value::Table(toml::value::Table::new()), None)
    };
    let table = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    table.insert(key.to_string(), toml::Value::Boolean(value));
    if let Some(raw) = original_raw {
        save_toml_preserving_comments(&path, &doc, &raw)?;
    } else {
        let body = toml::to_string_pretty(&doc).context("failed to serialize config.toml")?;
        fs::write(&path, body)
            .with_context(|| format!("failed to write config at {}", path.display()))?;
    }
    Ok(path)
}

pub(crate) fn persist_tui_integer_key(
    config_path: Option<&Path>,
    key: &str,
    value: u64,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path(config_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let (mut doc, original_raw) = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let doc: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;
        (doc, Some(raw))
    } else {
        (toml::Value::Table(toml::value::Table::new()), None)
    };
    let table = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    let tui_entry = table
        .entry("tui".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let tui_table = tui_entry
        .as_table_mut()
        .context("`tui` section in config.toml must be a table")?;
    let value = i64::try_from(value).context("integer value is too large for TOML")?;
    tui_table.insert(key.to_string(), toml::Value::Integer(value));
    if let Some(raw) = original_raw {
        save_toml_preserving_comments(&path, &doc, &raw)?;
    } else {
        let body = toml::to_string_pretty(&doc).context("failed to serialize config.toml")?;
        fs::write(&path, body)
            .with_context(|| format!("failed to write config at {}", path.display()))?;
    }
    Ok(path)
}

pub(crate) fn persist_subagents_bool_key(
    config_path: Option<&Path>,
    key: &str,
    value: bool,
) -> anyhow::Result<PathBuf> {
    persist_subagents_value_key(config_path, key, toml::Value::Boolean(value))
}

pub(crate) fn persist_subagents_integer_key(
    config_path: Option<&Path>,
    key: &str,
    value: u64,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;

    let value = i64::try_from(value).context("integer value is too large for TOML")?;
    persist_subagents_value_key(config_path, key, toml::Value::Integer(value))
}

fn persist_subagents_value_key(
    config_path: Option<&Path>,
    key: &str,
    value: toml::Value,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path(config_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let (mut doc, original_raw) = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let doc: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;
        (doc, Some(raw))
    } else {
        (toml::Value::Table(toml::value::Table::new()), None)
    };
    let table = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    let subagents_entry = table
        .entry("subagents".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let subagents_table = subagents_entry
        .as_table_mut()
        .context("`subagents` section in config.toml must be a table")?;
    subagents_table.insert(key.to_string(), value);

    if let Some(raw) = original_raw {
        save_toml_preserving_comments(&path, &doc, &raw)?;
    } else {
        let body = toml::to_string_pretty(&doc).context("failed to serialize config.toml")?;
        fs::write(&path, body)
            .with_context(|| format!("failed to write config at {}", path.display()))?;
    }
    Ok(path)
}

pub(crate) fn persist_provider_base_url_key(
    config_path: Option<&Path>,
    provider: ApiProvider,
    value: &str,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path(config_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let (mut doc, original_raw) = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let doc: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;
        (doc, Some(raw))
    } else {
        (toml::Value::Table(toml::value::Table::new()), None)
    };
    let table = doc
        .as_table_mut()
        .context("config.toml root must be a table")?;
    let providers = table
        .entry("providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .context("`providers` must be a table")?;
    let provider_key = provider_base_url_table_key(provider)?;
    let entry = providers
        .entry(provider_key.to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .with_context(|| format!("`providers.{provider_key}` must be a table"))?;
    entry.insert(
        "base_url".to_string(),
        toml::Value::String(value.to_string()),
    );

    if let Some(raw) = original_raw {
        save_toml_preserving_comments(&path, &doc, &raw)?;
    } else {
        let body = toml::to_string_pretty(&doc).context("failed to serialize config.toml")?;
        fs::write(&path, body)
            .with_context(|| format!("failed to write config at {}", path.display()))?;
    }
    Ok(path)
}

fn provider_base_url_table_key(provider: ApiProvider) -> anyhow::Result<&'static str> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
            anyhow::bail!("DeepSeek uses the root base_url setting")
        }
        ApiProvider::DeepseekAnthropic => Ok("deepseek_anthropic"),
        ApiProvider::NvidiaNim => Ok("nvidia_nim"),
        ApiProvider::Openai => Ok("openai"),
        ApiProvider::Anthropic => Ok("anthropic"),
        ApiProvider::Atlascloud => Ok("atlascloud"),
        ApiProvider::WanjieArk => Ok("wanjie_ark"),
        ApiProvider::Volcengine => Ok("volcengine"),
        ApiProvider::Openrouter => Ok("openrouter"),
        ApiProvider::XiaomiMimo => Ok("xiaomi_mimo"),
        ApiProvider::Novita => Ok("novita"),
        ApiProvider::Fireworks => Ok("fireworks"),
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => Ok("siliconflow"),
        ApiProvider::Arcee => Ok("arcee"),
        ApiProvider::Huggingface => Ok("huggingface"),
        ApiProvider::Deepinfra => Ok("deepinfra"),
        ApiProvider::Moonshot => Ok("moonshot"),
        ApiProvider::Sglang => Ok("sglang"),
        ApiProvider::Vllm => Ok("vllm"),
        ApiProvider::Ollama => Ok("ollama"),
        ApiProvider::Together => Ok("together"),
        ApiProvider::Qianfan => Ok("qianfan"),
        ApiProvider::OpenaiCodex => Ok("openai_codex"),
        ApiProvider::Openmodel => Ok("openmodel"),
        ApiProvider::Zai => Ok("zai"),
        ApiProvider::Stepfun => Ok("stepfun"),
        ApiProvider::Minimax => Ok("minimax"),
        ApiProvider::Sakana => Ok("sakana"),
        // Custom providers live under a user-chosen `[providers.<name>]` table,
        // not a fixed key. Persisting base_url through this static-key path is
        // out of scope for the #1519 constrained slice; users edit the named
        // table directly.
        ApiProvider::Custom => {
            anyhow::bail!("custom providers store base_url in their named [providers.<name>] table")
        }
    }
}

pub(crate) fn persist_hotbar_bindings(
    config_path: Option<&Path>,
    bindings: &[codewhale_config::HotbarBindingToml],
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path(config_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let raw = if path.exists() {
        Some(
            fs::read_to_string(&path)
                .with_context(|| format!("failed to read config at {}", path.display()))?,
        )
    } else {
        None
    };
    let mut document = match raw.as_deref() {
        Some(raw) if !raw.trim().is_empty() => raw
            .parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("failed to edit config at {}", path.display()))?,
        _ => toml_edit::DocumentMut::new(),
    };

    let table = document.as_table_mut();
    table.remove("hotbar");
    if bindings.is_empty() {
        table.insert(
            "hotbar",
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
        );
    } else {
        let mut hotbar = toml_edit::ArrayOfTables::new();
        for binding in bindings {
            let mut table = toml_edit::Table::new();
            table["slot"] = toml_edit::value(i64::from(binding.slot));
            table["action"] = toml_edit::value(binding.action.clone());
            if let Some(label) = binding.label.as_deref() {
                table["label"] = toml_edit::value(label);
            }
            hotbar.push(table);
        }
        table.insert("hotbar", toml_edit::Item::ArrayOfTables(hotbar));
    }

    fs::write(&path, document.to_string())
        .with_context(|| format!("failed to write config at {}", path.display()))?;
    Ok(path)
}

/// Remove the `hotbar` key from the resolved config so the resolver falls back
/// to the built-in default slots (`/hotbar on` / `/hotbar reset`). Unlike
/// `persist_hotbar_bindings(&[])` — which writes an explicit `hotbar = []` and
/// therefore *disables* the hotbar — this deletes the key entirely, restoring
/// defaults. If the file or key is absent there is nothing to restore; we still
/// return the resolved path. Surrounding keys are preserved via `toml_edit`;
/// a comment used as the removed key's own leading decor is removed with it
/// (standard `toml_edit` key-prefix behavior).
pub(crate) fn remove_hotbar_from_config(config_path: Option<&Path>) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::fs;

    let path = config_toml_path(config_path)?;
    if !path.exists() {
        // No config file ⇒ no `hotbar` key ⇒ defaults already apply.
        return Ok(path);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(path);
    }
    let mut document = raw
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to edit config at {}", path.display()))?;
    let table = document.as_table_mut();
    if table.remove("hotbar").is_none() {
        // Key wasn't present ⇒ defaults already apply; avoid a needless rewrite.
        return Ok(path);
    }
    fs::write(&path, document.to_string())
        .with_context(|| format!("failed to write config at {}", path.display()))?;
    Ok(path)
}

pub(crate) fn config_toml_path(config_path: Option<&Path>) -> anyhow::Result<PathBuf> {
    use anyhow::Context;

    if let Some(path) = config_path {
        return Ok(expand_path(path.to_string_lossy().as_ref()));
    }
    if let Ok(env) = std::env::var("CODEWHALE_CONFIG_PATH") {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if let Ok(env) = std::env::var("DEEPSEEK_CONFIG_PATH") {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home =
        effective_home_dir().context("failed to resolve home directory for config.toml path")?;
    let primary = home.join(".codewhale").join("config.toml");
    if primary.exists() {
        return Ok(primary);
    }
    let legacy = home.join(".deepseek").join("config.toml");
    if legacy.exists() {
        return Ok(legacy);
    }
    Ok(primary)
}

/// Write `doc` to `path`, merging comments from `original_raw` so user
/// annotations survive the rewrite.
fn save_toml_preserving_comments(
    path: &Path,
    doc: &toml::Value,
    original_raw: &str,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let serialized = toml::to_string_pretty(doc).context("failed to serialize config.toml")?;
    let body = codewhale_config::merge_and_preserve_comments(&serialized, original_raw)
        .unwrap_or_else(|e| {
            tracing::warn!("failed to merge config comments, saving without them: {e:#}");
            serialized
        });
    std::fs::write(path, body)
        .with_context(|| format!("failed to write config at {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct EnvGuard {
        home: Option<OsString>,
        userprofile: Option<OsString>,
        codewhale_config_path: Option<OsString>,
        deepseek_config_path: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(home: &Path) -> Self {
            let lock = crate::test_support::lock_test_env();
            let home_str = OsString::from(home.as_os_str());
            let config_path = home.join(".deepseek").join("config.toml");
            let config_str = OsString::from(config_path.as_os_str());
            let home_prev = env::var_os("HOME");
            let userprofile_prev = env::var_os("USERPROFILE");
            let codewhale_config_prev = env::var_os("CODEWHALE_CONFIG_PATH");
            let deepseek_config_prev = env::var_os("DEEPSEEK_CONFIG_PATH");

            // Safety: test-only environment mutation guarded by process-wide mutex.
            unsafe {
                env::set_var("HOME", &home_str);
                env::set_var("USERPROFILE", &home_str);
                env::remove_var("CODEWHALE_CONFIG_PATH");
                env::set_var("DEEPSEEK_CONFIG_PATH", &config_str);
            }

            Self {
                home: home_prev,
                userprofile: userprofile_prev,
                codewhale_config_path: codewhale_config_prev,
                deepseek_config_path: deepseek_config_prev,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.home.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("HOME", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("HOME");
                }
            }

            if let Some(value) = self.userprofile.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("USERPROFILE", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("USERPROFILE");
                }
            }

            if let Some(value) = self.codewhale_config_path.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("CODEWHALE_CONFIG_PATH", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("CODEWHALE_CONFIG_PATH");
                }
            }

            if let Some(value) = self.deepseek_config_path.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("DEEPSEEK_CONFIG_PATH", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("DEEPSEEK_CONFIG_PATH");
                }
            }
        }
    }

    fn temp_root(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn persist_status_items_writes_tui_section_to_config_toml() {
        let temp_root = temp_root("codewhale-statusline-persist");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let items = vec![
            crate::config::StatusItem::Mode,
            crate::config::StatusItem::Model,
            crate::config::StatusItem::Cost,
        ];

        let path = persist_status_items(&items).expect("persist should succeed");
        let body = fs::read_to_string(&path).expect("written file should be readable");
        assert!(body.contains("[tui]"), "expected [tui] section in {body}");
        assert!(
            body.contains("status_items"),
            "expected status_items key in {body}"
        );
        assert!(body.contains("\"mode\""), "expected mode key in {body}");
        assert!(body.contains("\"cost\""), "expected cost key in {body}");
    }

    #[test]
    fn config_toml_path_uses_codewhale_home_for_fresh_installs() {
        let temp_root = temp_root("codewhale-config-path-fresh");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::remove_var("DEEPSEEK_CONFIG_PATH");
        }

        assert_eq!(
            config_toml_path(None).unwrap(),
            temp_root.join(".codewhale").join("config.toml")
        );
    }

    #[test]
    fn config_toml_path_preserves_legacy_config_when_it_exists() {
        let temp_root = temp_root("codewhale-config-path-legacy");
        let legacy_config = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(legacy_config.parent().unwrap()).unwrap();
        fs::write(&legacy_config, "").unwrap();
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::remove_var("DEEPSEEK_CONFIG_PATH");
        }

        assert_eq!(config_toml_path(None).unwrap(), legacy_config);
    }

    #[test]
    fn config_toml_path_prefers_codewhale_env_over_legacy_env() {
        let temp_root = temp_root("codewhale-config-path-env");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let preferred = temp_root.join("preferred.toml");
        let legacy = temp_root.join("legacy.toml");

        unsafe {
            env::set_var("CODEWHALE_CONFIG_PATH", &preferred);
            env::set_var("DEEPSEEK_CONFIG_PATH", &legacy);
        }

        assert_eq!(config_toml_path(None).unwrap(), preferred);
    }

    #[test]
    fn persist_status_items_preserves_existing_unrelated_keys() {
        let temp_root = temp_root("codewhale-statusline-preserve");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "api_key = \"sentinel-key\"\nmodel = \"deepseek-v4-pro\"\n",
        )
        .unwrap();

        let written = persist_status_items(&[crate::config::StatusItem::Mode])
            .expect("persist should succeed");
        let body = fs::read_to_string(&written).expect("written file should be readable");
        assert!(
            body.contains("api_key = \"sentinel-key\""),
            "round-trip lost api_key: {body}"
        );
        assert!(
            body.contains("model = \"deepseek-v4-pro\""),
            "round-trip lost model: {body}"
        );
        assert!(
            body.contains("status_items"),
            "expected status_items in {body}"
        );
    }

    #[test]
    fn persist_bool_key_preserves_comments() {
        let temp_root = temp_root("codewhale-persist-comments");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "# my note\nmodel = \"deepseek-v4-flash\"\n# disabled = true\n",
        )
        .unwrap();

        let written = persist_root_bool_key(Some(&path), "allow_shell", true)
            .expect("persist should succeed");
        let body = fs::read_to_string(&written).expect("written file should be readable");
        assert!(body.contains("# my note"), "prefix comment lost: {body}");
        assert!(
            body.contains("# disabled = true"),
            "disabled key lost: {body}"
        );
        assert!(
            body.contains("allow_shell = true"),
            "new key not written: {body}"
        );
    }

    #[test]
    fn persist_hotbar_bindings_writes_primary_config_path_for_fresh_installs() {
        let temp_root = temp_root("codewhale-hotbar-persist-fresh");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::remove_var("DEEPSEEK_CONFIG_PATH");
        }

        let bindings = vec![codewhale_config::HotbarBindingToml {
            slot: 1,
            action: "mode.plan".to_string(),
            label: Some("Plan".to_string()),
        }];
        let path = persist_hotbar_bindings(None, &bindings).expect("persist should succeed");

        assert_eq!(path, temp_root.join(".codewhale").join("config.toml"));
        let body = fs::read_to_string(&path).expect("written file should be readable");
        assert!(body.contains("[[hotbar]]"), "hotbar table missing: {body}");
        let parsed: codewhale_config::ConfigToml =
            toml::from_str(&body).expect("written hotbar config should parse");
        assert_eq!(parsed.hotbar, Some(bindings));
    }

    #[test]
    fn persist_hotbar_bindings_preserves_comments_and_replaces_existing_tables() {
        let temp_root = temp_root("codewhale-hotbar-persist-comments");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".codewhale").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"# model note
model = "deepseek-v4-flash"

[[hotbar]]
slot = 1
action = "mode.plan"
label = "Plan"

# notification note
[notifications]
enabled = true
"#,
        )
        .unwrap();

        let bindings = vec![codewhale_config::HotbarBindingToml {
            slot: 2,
            action: "session.compact".to_string(),
            label: Some("Compact".to_string()),
        }];
        let written =
            persist_hotbar_bindings(Some(&path), &bindings).expect("persist should succeed");
        let body = fs::read_to_string(&written).expect("written file should be readable");

        assert!(body.contains("# model note"), "prefix comment lost: {body}");
        assert!(
            body.contains("# notification note"),
            "section comment lost: {body}"
        );
        assert!(
            !body.contains("mode.plan"),
            "old hotbar table was not replaced: {body}"
        );
        assert!(body.contains("[[hotbar]]"), "hotbar table missing: {body}");
        assert!(
            body.contains("action = \"session.compact\""),
            "new action missing: {body}"
        );
        let parsed: codewhale_config::ConfigToml =
            toml::from_str(&body).expect("written hotbar config should parse");
        assert_eq!(parsed.hotbar, Some(bindings));
    }

    #[test]
    fn persist_hotbar_bindings_writes_empty_array_to_disable_defaults() {
        let temp_root = temp_root("codewhale-hotbar-persist-empty");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".codewhale").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        let written = persist_hotbar_bindings(Some(&path), &[]).expect("persist should succeed");
        let body = fs::read_to_string(&written).expect("written file should be readable");

        assert!(body.contains("hotbar = []"), "empty hotbar missing: {body}");
        let parsed: codewhale_config::ConfigToml =
            toml::from_str(&body).expect("written hotbar config should parse");
        assert_eq!(parsed.hotbar, Some(Vec::new()));
    }

    #[test]
    fn remove_hotbar_from_config_deletes_key_to_restore_defaults() {
        let temp_root = temp_root("codewhale-hotbar-remove");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".codewhale").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Seed a config with a hotbar binding and an unrelated key whose own
        // trailing comment is NOT the hotbar key's decor (so it must survive).
        fs::write(
            &path,
            "hotbar = [{ slot = 1, action = \"mode.plan\" }]\nother = true  # keep me\n",
        )
        .unwrap();

        let returned = remove_hotbar_from_config(Some(&path)).expect("remove should succeed");
        assert_eq!(returned, path);

        let body = fs::read_to_string(&path).expect("written file should be readable");
        assert!(
            !body.contains("hotbar"),
            "hotbar key should be gone: {body}"
        );
        // An unrelated key and its own trailing comment must survive the rewrite.
        assert!(
            body.contains("other = true"),
            "unrelated key should survive: {body}"
        );
        assert!(
            body.contains("keep me"),
            "comment that is not the hotbar key's decor should survive: {body}"
        );

        let parsed: codewhale_config::ConfigToml =
            toml::from_str(&body).expect("restored config should parse");
        assert_eq!(
            parsed.hotbar, None,
            "removing the key reads back as None so the resolver falls back to defaults"
        );
    }

    #[test]
    fn remove_hotbar_from_config_is_a_noop_when_key_absent() {
        let temp_root = temp_root("codewhale-hotbar-remove-absent");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".codewhale").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "other = true\n").unwrap();

        let before = fs::read_to_string(&path).unwrap();
        remove_hotbar_from_config(Some(&path)).expect("noop should succeed");
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(
            before, after,
            "an absent hotbar key should not trigger a rewrite"
        );
    }
}
