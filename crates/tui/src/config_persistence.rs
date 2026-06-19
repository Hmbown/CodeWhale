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

pub(crate) fn persist_feature_bool_key(
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
    let features_entry = table
        .entry("features".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let features = features_entry
        .as_table_mut()
        .context("`features` section in config.toml must be a table")?;
    features.insert(key.to_string(), toml::Value::Boolean(value));
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
        ApiProvider::OpenaiCodex => Ok("openai_codex"),
        ApiProvider::Zai => Ok("zai"),
        ApiProvider::Stepfun => Ok("stepfun"),
        ApiProvider::Minimax => Ok("minimax"),
    }
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
    fn persist_feature_bool_key_writes_features_table_and_preserves_comments() {
        let temp_root = temp_root("codewhale-persist-feature-comments");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "# my note\nmodel = \"deepseek-v4-flash\"\n[features]\n# keep this nearby\nweb_search = true\n",
        )
        .unwrap();

        let written = persist_feature_bool_key(Some(&path), "subagents", false)
            .expect("persist should succeed");
        let body = fs::read_to_string(&written).expect("written file should be readable");
        assert!(body.contains("# my note"), "prefix comment lost: {body}");
        assert!(
            body.contains("# keep this nearby"),
            "features comment lost: {body}"
        );
        assert!(
            body.contains("web_search = true"),
            "existing feature lost: {body}"
        );
        assert!(
            body.contains("subagents = false"),
            "subagent feature not written: {body}"
        );
    }
}
