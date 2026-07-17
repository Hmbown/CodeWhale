//! Config file path resolution and TOML persistence helpers.
//!
//! These helpers are used by command handlers and non-command UI code, so
//! persistence lives outside the command tree.
//!
//! Every `config.toml` mutation funnels through [`mutate_config_document`]:
//! the file is edited in place with `toml_edit` so unrelated comments,
//! ordering, and formatting survive, and the result is replaced atomically
//! (same-directory temp file + rename) with owner-only permissions.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::{ApiProvider, StatusItem, expand_path};

/// Parse the TOML document at `path` (an absent or empty file yields an empty
/// document), apply `mutate`, and atomically persist the result.
///
/// This is the single write path for TUI config mutations: `toml_edit` keeps
/// user comments and formatting intact, and the temp-file + rename write can
/// never leave a half-written config behind.
pub(crate) fn mutate_config_document<F>(path: &Path, mutate: F) -> anyhow::Result<()>
where
    F: FnOnce(&mut toml_edit::DocumentMut) -> anyhow::Result<()>,
{
    let raw = if path.exists() {
        Some(
            fs::read_to_string(path)
                .with_context(|| format!("failed to read config at {}", path.display()))?,
        )
    } else {
        None
    };
    let mut document = match raw.as_deref() {
        Some(raw) if !raw.trim().is_empty() => raw
            .parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("failed to parse config at {}", path.display()))?,
        _ => toml_edit::DocumentMut::new(),
    };
    mutate(&mut document)?;
    write_config_toml_atomic(path, &document.to_string())
}

/// Atomically replace `path` with `body` via a same-directory temp file and
/// rename. On Unix the file lands with 0o600 permissions: config.toml can
/// hold API keys, so this matches `ConfigStore::save` and the auth save path.
pub(crate) fn write_config_toml_atomic(path: &Path, body: &str) -> anyhow::Result<()> {
    use std::io::Write as _;

    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create config directory {}", parent.display()))?;

    let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create temporary config file in {}",
            parent.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| {
                format!(
                    "failed to secure temporary config file for {}",
                    path.display()
                )
            })?;
    }
    temporary
        .write_all(body.as_bytes())
        .with_context(|| format!("failed to write config at {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to sync config at {}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace config at {}", path.display()))?;
    Ok(())
}

/// Set the value at `segments` (parent tables plus the final key), creating
/// missing intermediate tables. Replacing an existing value keeps its decor,
/// so comments above the key and trailing same-line comments survive.
///
/// Segments are separate strings rather than one dotted key, so table names
/// that need quoting (`[providers."my.provider"]`) resolve correctly.
pub(crate) fn set_document_value(
    doc: &mut toml_edit::DocumentMut,
    segments: &[&str],
    value: impl Into<toml_edit::Value>,
) -> anyhow::Result<()> {
    let (key, parents) = segments
        .split_last()
        .context("config value path must not be empty")?;
    let table = table_like_at_path_mut(doc.as_table_mut(), parents, PathLookup::Create)?
        .expect("Create lookups always yield a table");
    match table.get_mut(key) {
        Some(item) => {
            let mut value = value.into();
            if let Some(existing) = item.as_value() {
                *value.decor_mut() = existing.decor().clone();
            }
            *item = toml_edit::Item::Value(value);
        }
        None => {
            table.insert(key, toml_edit::value(value));
        }
    }
    Ok(())
}

/// Remove the value at `segments`. Returns `Ok(true)` when an entry was
/// removed; missing keys and missing (or non-table) parents are a no-op.
pub(crate) fn unset_document_value(
    doc: &mut toml_edit::DocumentMut,
    segments: &[&str],
) -> anyhow::Result<bool> {
    let (key, parents) = segments
        .split_last()
        .context("config value path must not be empty")?;
    let orphaned_root_prefix = (parents.is_empty() && doc.as_table().len() == 1)
        .then(|| leading_prefix_for_key(doc.as_table(), key))
        .flatten();
    let removed = {
        let Some(table) =
            table_like_at_path_mut(doc.as_table_mut(), parents, PathLookup::Existing)?
        else {
            return Ok(false);
        };
        remove_key_preserving_leading_decor(table, key)
    };
    if removed
        && let Some(prefix) = orphaned_root_prefix
        && prefix.as_str().is_some_and(|prefix| !prefix.is_empty())
    {
        let trailing = format!(
            "{}{}",
            prefix.as_str().unwrap_or_default(),
            doc.trailing().as_str().unwrap_or_default()
        );
        doc.set_trailing(trailing);
    }
    Ok(removed)
}

/// Remove every entry named `key` from `table` and, recursively, from nested
/// tables, inline tables, and arrays of tables. Used by `/logout` to strip
/// `api_key` everywhere without disturbing keys like `api_key_env`.
pub(crate) fn remove_document_key_recursive(table: &mut dyn toml_edit::TableLike, key: &str) {
    remove_key_preserving_leading_decor(table, key);
    for (_, item) in table.iter_mut() {
        if let toml_edit::Item::ArrayOfTables(tables) = item {
            for nested in tables.iter_mut() {
                remove_document_key_recursive(nested, key);
            }
        } else if let Some(nested) = item.as_table_like_mut() {
            remove_document_key_recursive(nested, key);
        }
    }
}

fn remove_key_preserving_leading_decor(table: &mut dyn toml_edit::TableLike, key: &str) -> bool {
    let mut found = false;
    let next_key = table.iter().find_map(|(candidate, _)| {
        if found {
            Some(candidate.to_owned())
        } else {
            found = candidate == key;
            None
        }
    });
    let leading_prefix = leading_prefix_for_key(table, key);
    if table.remove(key).is_none() {
        return false;
    }
    let Some(prefix) = leading_prefix else {
        return true;
    };
    let Some(next_key) = next_key else {
        return true;
    };
    if prefix.as_str() == Some("") {
        return true;
    }
    if let Some(mut next_key_decor) = table.key_mut(&next_key)
        && decor_prefix_is_empty(next_key_decor.leaf_decor())
    {
        next_key_decor.leaf_decor_mut().set_prefix(prefix);
    }
    true
}

fn decor_prefix_is_empty(decor: &toml_edit::Decor) -> bool {
    match decor.prefix() {
        Some(prefix) => prefix.as_str() == Some(""),
        None => true,
    }
}

fn leading_prefix_for_key(
    table: &dyn toml_edit::TableLike,
    key: &str,
) -> Option<toml_edit::RawString> {
    table
        .key(key)
        .and_then(|key| key.leaf_decor().prefix().cloned())
        .or_else(|| {
            table
                .get(key)
                .and_then(|item| item.as_value())
                .and_then(|value| value.decor().prefix().cloned())
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathLookup {
    /// Create missing intermediate tables; error when a segment exists but is
    /// not table-like.
    Create,
    /// Return `None` when a segment is missing or not table-like.
    Existing,
}

fn table_like_at_path_mut<'a>(
    root: &'a mut toml_edit::Table,
    segments: &[&str],
    lookup: PathLookup,
) -> anyhow::Result<Option<&'a mut dyn toml_edit::TableLike>> {
    let mut current: &mut dyn toml_edit::TableLike = root;
    for segment in segments {
        if current.get(segment).is_none() {
            match lookup {
                PathLookup::Create => {
                    // Implicit, so creating `providers.foo.base_url` does not
                    // emit an empty `[providers]` header.
                    let mut table = toml_edit::Table::new();
                    table.set_implicit(true);
                    current.insert(segment, toml_edit::Item::Table(table));
                }
                PathLookup::Existing => return Ok(None),
            }
        }
        let item = current
            .get_mut(segment)
            .expect("segment exists or was inserted above");
        match item.as_table_like_mut() {
            Some(table) => current = table,
            None => match lookup {
                PathLookup::Create => {
                    anyhow::bail!("`{segment}` in config.toml must be a table")
                }
                PathLookup::Existing => return Ok(None),
            },
        }
    }
    Ok(Some(current))
}

pub(crate) fn persist_status_items(items: &[StatusItem]) -> anyhow::Result<PathBuf> {
    let path = config_toml_path(None)?;
    let items: toml_edit::Array = items.iter().map(|item| item.key()).collect();
    mutate_config_document(&path, |doc| {
        set_document_value(doc, &["tui", "status_items"], items)
    })?;
    Ok(path)
}

pub(crate) fn persist_root_string_key(
    config_path: Option<&Path>,
    key: &str,
    value: &str,
) -> anyhow::Result<PathBuf> {
    let path = config_toml_path(config_path)?;
    mutate_config_document(&path, |doc| set_document_value(doc, &[key], value))?;
    Ok(path)
}

pub(crate) fn persist_unset_root_key(
    config_path: Option<&Path>,
    key: &str,
) -> anyhow::Result<PathBuf> {
    let path = config_toml_path(config_path)?;
    mutate_config_document(&path, |doc| unset_document_value(doc, &[key]).map(|_| ()))?;
    Ok(path)
}

pub(crate) fn persist_root_bool_key(
    config_path: Option<&Path>,
    key: &str,
    value: bool,
) -> anyhow::Result<PathBuf> {
    let path = config_toml_path(config_path)?;
    mutate_config_document(&path, |doc| set_document_value(doc, &[key], value))?;
    Ok(path)
}

pub(crate) fn persist_tui_integer_key(
    config_path: Option<&Path>,
    key: &str,
    value: u64,
) -> anyhow::Result<PathBuf> {
    let value = i64::try_from(value).context("integer value is too large for TOML")?;
    persist_table_value_key(config_path, "tui", key, value.into())
}

pub(crate) fn persist_subagents_bool_key(
    config_path: Option<&Path>,
    key: &str,
    value: bool,
) -> anyhow::Result<PathBuf> {
    persist_table_value_key(config_path, "subagents", key, value.into())
}

pub(crate) fn persist_subagents_integer_key(
    config_path: Option<&Path>,
    key: &str,
    value: u64,
) -> anyhow::Result<PathBuf> {
    let value = i64::try_from(value).context("integer value is too large for TOML")?;
    persist_table_value_key(config_path, "subagents", key, value.into())
}

pub(crate) fn persist_table_bool_key(
    config_path: Option<&Path>,
    table_name: &str,
    key: &str,
    value: bool,
) -> anyhow::Result<PathBuf> {
    persist_table_value_key(config_path, table_name, key, value.into())
}

pub(crate) fn persist_table_string_key(
    config_path: Option<&Path>,
    table_name: &str,
    key: &str,
    value: &str,
) -> anyhow::Result<PathBuf> {
    persist_table_value_key(config_path, table_name, key, value.into())
}

fn persist_table_value_key(
    config_path: Option<&Path>,
    table_name: &str,
    key: &str,
    value: toml_edit::Value,
) -> anyhow::Result<PathBuf> {
    let path = config_toml_path(config_path)?;
    mutate_config_document(&path, |doc| {
        set_document_value(doc, &[table_name, key], value)
    })?;
    Ok(path)
}

pub(crate) fn persist_provider_base_url_key(
    config_path: Option<&Path>,
    provider: ApiProvider,
    value: &str,
) -> anyhow::Result<PathBuf> {
    let provider_key = provider_base_url_table_key(provider)?;
    let path = config_toml_path(config_path)?;
    mutate_config_document(&path, |doc| {
        set_document_value(doc, &["providers", provider_key, "base_url"], value)
    })?;
    Ok(path)
}

/// Persist the model for one exact provider route without rewriting the
/// legacy root DeepSeek fallback used by unrelated providers.
///
/// First-party DeepSeek retains its historical `default_text_model` root key.
/// Every other built-in provider writes to its typed `[providers.<name>]`
/// table, while named custom routes use their exact user-owned table id.
pub(crate) fn persist_provider_model_key(
    config_path: Option<&Path>,
    provider: ApiProvider,
    provider_identity: &str,
    value: &str,
) -> anyhow::Result<PathBuf> {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        return persist_root_string_key(config_path, "default_text_model", value);
    }

    let provider_key = if provider == ApiProvider::Custom {
        normalize_custom_provider_id(provider_identity)?
    } else {
        provider
            .metadata()
            .context("provider config metadata")?
            .provider_config_key()
            .to_string()
    };
    let path = config_toml_path(config_path)?;
    mutate_config_document(&path, |doc| {
        set_document_value(doc, &["providers", &provider_key, "model"], value)
    })?;
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
        ApiProvider::MinimaxAnthropic => Ok("minimax_anthropic"),
        ApiProvider::Sakana => Ok("sakana"),
        ApiProvider::LongCat => Ok("longcat"),
        ApiProvider::OpencodeGo => Ok("opencode_go"),
        ApiProvider::OpencodeZen => Ok("opencode_zen"),
        ApiProvider::Meta => Ok("meta"),
        ApiProvider::Xai => Ok("xai"),
        // Custom providers live under a user-chosen `[providers.<name>]` table,
        // not a fixed key. Persisting base_url through this static-key path is
        // out of scope for the #1519 constrained slice; users edit the named
        // table directly.
        ApiProvider::Custom => {
            anyhow::bail!("custom providers store base_url in their named [providers.<name>] table")
        }
    }
}

pub(crate) fn persist_custom_provider(
    config_path: Option<&Path>,
    provider_id: &str,
    base_url: &str,
    model: Option<&str>,
    api_key_env: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let provider_id = normalize_custom_provider_id(provider_id)?;
    let base_url = normalize_custom_provider_base_url(base_url)?;
    let model = model.and_then(normalize_optional_custom_provider_field);
    let api_key_env = api_key_env.and_then(normalize_optional_custom_provider_field);

    let path = config_toml_path(config_path)?;
    mutate_config_document(&path, |doc| {
        let entry = ["providers", provider_id.as_str()];
        set_document_value(doc, &["provider"], provider_id.as_str())?;
        set_document_value(doc, &[entry[0], entry[1], "kind"], "openai-compatible")?;
        set_document_value(doc, &[entry[0], entry[1], "base_url"], base_url.as_str())?;
        match model.as_deref() {
            Some(model) => set_document_value(doc, &[entry[0], entry[1], "model"], model)?,
            None => {
                unset_document_value(doc, &[entry[0], entry[1], "model"])?;
            }
        }
        match api_key_env.as_deref() {
            Some(env) => set_document_value(doc, &[entry[0], entry[1], "api_key_env"], env)?,
            None => {
                unset_document_value(doc, &[entry[0], entry[1], "api_key_env"])?;
            }
        }
        Ok(())
    })?;
    Ok(path)
}

fn normalize_custom_provider_id(raw: &str) -> anyhow::Result<String> {
    use anyhow::bail;

    let value = raw.trim();
    if value.is_empty() {
        bail!("custom provider name is required");
    }
    if value == "__custom__" {
        bail!("custom provider name is reserved");
    }
    if crate::config::ApiProvider::parse(value).is_some() {
        bail!("custom provider name must not shadow a built-in provider");
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        bail!("custom provider name may only use letters, numbers, '-' and '_'");
    }
    Ok(value.to_string())
}

fn normalize_custom_provider_base_url(raw: &str) -> anyhow::Result<String> {
    use anyhow::bail;

    let value = raw.trim().trim_end_matches('/');
    if value.is_empty() {
        bail!("custom provider base URL is required");
    }
    let parsed = reqwest::Url::parse(value)
        .map_err(|err| anyhow::anyhow!("custom provider base URL is invalid: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("custom provider base URL must be an http(s) URL with a host");
    }
    Ok(value.to_string())
}

fn normalize_optional_custom_provider_field(raw: &str) -> Option<String> {
    let value = raw.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub(crate) fn persist_hotbar_bindings(
    config_path: Option<&Path>,
    bindings: &[codewhale_config::HotbarBindingToml],
) -> anyhow::Result<PathBuf> {
    let path = config_toml_path(config_path)?;
    mutate_config_document(&path, |doc| {
        let table = doc.as_table_mut();
        table.remove("hotbar");
        if bindings.is_empty() {
            table.insert(
                "hotbar",
                toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
            );
        } else {
            let mut hotbar = toml_edit::ArrayOfTables::new();
            for binding in bindings {
                let mut entry = toml_edit::Table::new();
                entry["slot"] = toml_edit::value(i64::from(binding.slot));
                entry["action"] = toml_edit::value(binding.action.clone());
                if let Some(label) = binding.label.as_deref() {
                    entry["label"] = toml_edit::value(label);
                }
                hotbar.push(entry);
            }
            table.insert("hotbar", toml_edit::Item::ArrayOfTables(hotbar));
        }
        Ok(())
    })?;
    Ok(path)
}

pub(crate) fn config_toml_path(config_path: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(path) = config_path {
        return Ok(expand_path(path.to_string_lossy().as_ref()));
    }
    crate::config::resolve_load_config_path(None)
        .context("failed to resolve the active config.toml path")
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
        codewhale_home: Option<OsString>,
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
            let codewhale_home_prev = env::var_os("CODEWHALE_HOME");
            let codewhale_config_prev = env::var_os("CODEWHALE_CONFIG_PATH");
            let deepseek_config_prev = env::var_os("DEEPSEEK_CONFIG_PATH");

            // Safety: test-only environment mutation guarded by process-wide mutex.
            unsafe {
                env::set_var("HOME", &home_str);
                env::set_var("USERPROFILE", &home_str);
                env::remove_var("CODEWHALE_HOME");
                env::remove_var("CODEWHALE_CONFIG_PATH");
                env::set_var("DEEPSEEK_CONFIG_PATH", &config_str);
            }

            Self {
                home: home_prev,
                userprofile: userprofile_prev,
                codewhale_home: codewhale_home_prev,
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

            if let Some(value) = self.codewhale_home.take() {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::set_var("CODEWHALE_HOME", value);
                }
            } else {
                // Safety: test-only environment mutation guarded by a global mutex.
                unsafe {
                    env::remove_var("CODEWHALE_HOME");
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
    fn config_toml_path_ignores_legacy_config_when_codewhale_home_is_explicit() {
        let temp_root = temp_root("codewhale-config-path-explicit-home");
        let explicit_home = temp_root.join("isolated-codewhale");
        let legacy_config = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(legacy_config.parent().unwrap()).unwrap();
        fs::write(&legacy_config, "").unwrap();
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::remove_var("DEEPSEEK_CONFIG_PATH");
            env::set_var("CODEWHALE_HOME", &explicit_home);
        }

        assert_eq!(
            config_toml_path(None).unwrap(),
            explicit_home.join("config.toml")
        );
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
    fn config_toml_path_uses_existing_home_fallback_when_env_target_is_missing() {
        let temp_root = temp_root("codewhale-config-path-missing-env-fallback");
        let home_config = temp_root.join(".codewhale").join("config.toml");
        fs::create_dir_all(home_config.parent().unwrap()).unwrap();
        fs::write(&home_config, "# existing fallback\n").unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let missing_env = temp_root.join("override").join("missing.toml");

        unsafe {
            env::set_var("DEEPSEEK_CONFIG_PATH", &missing_env);
        }

        assert_eq!(config_toml_path(None).unwrap(), home_config);
        assert!(!missing_env.exists());
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
    fn persist_table_bool_key_updates_existing_memory_enabled() {
        let temp_root = temp_root("codewhale-persist-memory-update");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "allow_shell = true\n\n[memory]\nenabled = true\n").unwrap();

        let written = persist_table_bool_key(Some(&path), "memory", "enabled", false)
            .expect("persist should succeed");
        let body = fs::read_to_string(&written).expect("written file should be readable");
        assert!(
            body.contains("enabled = false"),
            "memory enabled should be false: {body}"
        );
        assert!(
            !body.contains("enabled = true"),
            "memory enabled should not still be true: {body}"
        );
    }

    #[test]
    fn persist_memory_enabled_round_trips_through_config_load() {
        let temp_root = temp_root("codewhale-persist-memory-roundtrip");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".deepseek").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Initial config has memory enabled = true
        fs::write(&path, "allow_shell = true\n\n[memory]\nenabled = true\n").unwrap();

        // Verify initial state
        let cfg0 = crate::config::Config::load(Some(path.clone()), None)
            .expect("initial config should load");
        assert!(cfg0.memory_enabled(), "memory should be enabled initially");

        // Persist memory.enabled = false (what the GUI's set_config endpoint does)
        persist_table_bool_key(Some(&path), "memory", "enabled", false)
            .expect("persist should succeed");

        // Reload config from disk and verify memory_enabled() reflects the change
        let cfg1 = crate::config::Config::load(Some(path.clone()), None)
            .expect("reloaded config should load");
        assert!(
            !cfg1.memory_enabled(),
            "memory should be disabled after persisting false"
        );
    }

    #[test]
    fn persist_custom_provider_writes_named_openai_compatible_table() {
        let temp_root = temp_root("codewhale-custom-provider-persist");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let path = temp_root.join(".codewhale").join("config.toml");
        let written = persist_custom_provider(
            Some(&path),
            "acme_ai",
            "https://api.acme.example/v1/",
            Some("acme/code-1"),
            Some("ACME_API_KEY"),
        )
        .expect("custom provider should persist");
        let body = fs::read_to_string(&written).expect("written file should be readable");

        assert!(body.contains("provider = \"acme_ai\""), "{body}");
        assert!(body.contains("[providers.acme_ai]"), "{body}");
        assert!(body.contains("kind = \"openai-compatible\""), "{body}");
        assert!(
            body.contains("base_url = \"https://api.acme.example/v1\""),
            "{body}"
        );
        assert!(body.contains("model = \"acme/code-1\""), "{body}");
        assert!(body.contains("api_key_env = \"ACME_API_KEY\""), "{body}");
        assert!(
            !body.contains("sk-"),
            "helper must not persist raw secret values: {body}"
        );

        let loaded =
            crate::config::Config::load(Some(written.clone()), None).expect("config should load");
        assert_eq!(loaded.provider.as_deref(), Some("acme_ai"));
        assert_eq!(loaded.api_provider(), crate::config::ApiProvider::Custom);
        let entry = loaded
            .providers
            .as_ref()
            .and_then(|providers| providers.custom_provider_config("acme_ai"))
            .expect("custom provider entry");
        assert!(entry.is_openai_compatible_custom());
        assert_eq!(
            entry.base_url.as_deref(),
            Some("https://api.acme.example/v1")
        );
        assert_eq!(entry.model.as_deref(), Some("acme/code-1"));
        assert_eq!(entry.api_key_env.as_deref(), Some("ACME_API_KEY"));
    }

    #[test]
    fn persist_custom_provider_rejects_builtin_or_invalid_names() {
        let temp_root = temp_root("codewhale-custom-provider-invalid");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let path = temp_root.join(".codewhale").join("config.toml");

        let builtin = persist_custom_provider(
            Some(&path),
            "openrouter",
            "https://api.example.invalid/v1",
            None,
            None,
        )
        .expect_err("built-in names should be rejected");
        assert!(builtin.to_string().contains("built-in provider"));

        let bad_chars = persist_custom_provider(
            Some(&path),
            "my provider",
            "https://api.example.invalid/v1",
            None,
            None,
        )
        .expect_err("space in name should be rejected");
        assert!(bad_chars.to_string().contains("letters, numbers"));
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
    fn persist_default_hotbar_bindings_round_trips_for_hotbar_on() {
        // #3807: `/hotbar on` persists the explicit default slots (an absent key
        // now means hidden), and they read back as the eight recommended slots.
        let temp_root = temp_root("codewhale-hotbar-on-defaults");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);

        let defaults = codewhale_config::default_hotbar_bindings_toml();
        assert_eq!(defaults.len(), codewhale_config::HOTBAR_SLOT_COUNT as usize);

        let path = persist_hotbar_bindings(None, &defaults).expect("persist should succeed");
        let body = fs::read_to_string(&path).expect("written file should be readable");
        assert!(body.contains("[[hotbar]]"), "hotbar table missing: {body}");

        let parsed: codewhale_config::ConfigToml =
            toml::from_str(&body).expect("written hotbar config should parse");
        assert_eq!(parsed.hotbar, Some(defaults));

        // The persisted defaults resolve back to all eight recommended slots.
        let resolved = parsed.resolve_hotbar_bindings(&codewhale_config::DEFAULT_HOTBAR_ACTIONS);
        assert_eq!(
            resolved.bindings,
            codewhale_config::default_hotbar_bindings()
        );
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

    // ------------------------------------------------------------------
    // Golden-file coverage for the shared toml_edit mutation path
    // (findings #18/#19/#20): unrelated comments, ordering, and quoted
    // provider tables must survive every supported mutation.
    // ------------------------------------------------------------------

    const GOLDEN_CONFIG: &str = r#"# CodeWhale golden config fixture, top note.
# api_key = "sk-placeholder" (uncomment to set the key by hand)
model = "deepseek-v4-pro" # pinned for release QA

# workspace trust note
[projects."/Users/example/work"]
trust_level = "trusted" # granted manually

# providers note
[providers.openrouter]
base_url = "https://openrouter.ai/api/v1" # keep in sync with docs

[providers."quoted.provider"]
base_url = "https://quoted.example/v1"

[[hotbar]]
slot = 1
action = "mode.plan"
"#;

    fn write_golden_config(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, GOLDEN_CONFIG).unwrap();
    }

    #[test]
    fn golden_replacing_existing_root_value_only_touches_that_value() {
        let temp_root = temp_root("codewhale-golden-root-value");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let path = temp_root.join(".deepseek").join("config.toml");
        write_golden_config(&path);

        persist_root_string_key(Some(&path), "model", "deepseek-v4-flash")
            .expect("persist should succeed");

        let body = fs::read_to_string(&path).unwrap();
        let expected = GOLDEN_CONFIG.replace(
            "model = \"deepseek-v4-pro\" # pinned for release QA",
            "model = \"deepseek-v4-flash\" # pinned for release QA",
        );
        assert_eq!(body, expected, "only the model value may change");
    }

    #[test]
    fn golden_mutations_preserve_unrelated_comments_order_and_quoted_tables() {
        let temp_root = temp_root("codewhale-golden-mutations");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let path = temp_root.join(".deepseek").join("config.toml");
        write_golden_config(&path);

        persist_root_bool_key(Some(&path), "allow_shell", true).unwrap();
        persist_tui_integer_key(Some(&path), "scrollback_lines", 4000).unwrap();
        persist_table_string_key(Some(&path), "memory", "backend", "sqlite").unwrap();
        persist_subagents_bool_key(Some(&path), "enabled", true).unwrap();
        persist_provider_base_url_key(
            Some(&path),
            crate::config::ApiProvider::Openrouter,
            "https://openrouter.example/v2",
        )
        .unwrap();
        persist_status_items(&[crate::config::StatusItem::Mode]).unwrap();
        persist_hotbar_bindings(
            Some(&path),
            &[codewhale_config::HotbarBindingToml {
                slot: 2,
                action: "session.compact".to_string(),
                label: None,
            }],
        )
        .unwrap();

        let body = fs::read_to_string(&path).unwrap();
        for comment in [
            "# CodeWhale golden config fixture, top note.",
            "# api_key = \"sk-placeholder\" (uncomment to set the key by hand)",
            "# pinned for release QA",
            "# workspace trust note",
            "# granted manually",
            "# providers note",
            "# keep in sync with docs",
        ] {
            assert!(body.contains(comment), "comment lost: {comment}\n{body}");
        }
        // Updated in place, keeping the trailing comment on the same line.
        assert!(
            body.contains("base_url = \"https://openrouter.example/v2\" # keep in sync with docs"),
            "{body}"
        );
        assert!(body.contains("[providers.\"quoted.provider\"]"), "{body}");
        assert!(
            !body.contains("mode.plan"),
            "old hotbar entry must be replaced: {body}"
        );

        // Original section order is intact.
        let model_at = body.find("model = ").unwrap();
        let projects_at = body.find("[projects.").unwrap();
        let providers_at = body.find("[providers.openrouter]").unwrap();
        assert!(
            model_at < projects_at && projects_at < providers_at,
            "{body}"
        );

        let parsed: toml::Value = toml::from_str(&body).unwrap();
        assert_eq!(
            parsed.get("allow_shell").and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("tui")
                .and_then(|t| t.get("scrollback_lines"))
                .and_then(toml::Value::as_integer),
            Some(4000)
        );
        assert_eq!(
            parsed
                .get("memory")
                .and_then(|t| t.get("backend"))
                .and_then(toml::Value::as_str),
            Some("sqlite")
        );
        assert_eq!(
            parsed
                .get("subagents")
                .and_then(|t| t.get("enabled"))
                .and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn set_document_value_inserts_api_key_even_when_a_comment_mentions_it() {
        // Finding #20 at the primitive level: the old string scan treated a
        // comment mentioning api_key as an existing assignment and skipped
        // the insert entirely.
        let temp_root = temp_root("codewhale-golden-api-key-comment");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let path = temp_root.join(".deepseek").join("config.toml");
        write_golden_config(&path);

        mutate_config_document(&path, |doc| {
            set_document_value(doc, &["api_key"], "sk-fresh")
        })
        .expect("mutation should succeed");

        let body = fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("# api_key = \"sk-placeholder\""),
            "comment lost: {body}"
        );
        let parsed: toml::Value = toml::from_str(&body).unwrap();
        assert_eq!(
            parsed.get("api_key").and_then(toml::Value::as_str),
            Some("sk-fresh"),
            "real key must be inserted despite the comment: {body}"
        );
    }

    #[test]
    fn unset_document_value_reports_removal_and_tolerates_missing_parents() {
        let mut doc = "model = \"deepseek-v4-pro\"\n"
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert!(!unset_document_value(&mut doc, &["providers", "openrouter", "api_key"]).unwrap());
        assert!(!unset_document_value(&mut doc, &["model", "nested"]).unwrap());
        assert!(unset_document_value(&mut doc, &["model"]).unwrap());
        assert!(!unset_document_value(&mut doc, &["model"]).unwrap());
    }

    #[test]
    fn unset_last_root_value_preserves_its_leading_comment() {
        let mut doc = "# keep this explanation\napproval_policy = \"on-request\"\n"
            .parse::<toml_edit::DocumentMut>()
            .unwrap();

        assert!(unset_document_value(&mut doc, &["approval_policy"]).unwrap());

        let saved = doc.to_string();
        assert!(saved.contains("# keep this explanation"), "{saved:?}");
        assert!(!saved.contains("approval_policy"), "{saved:?}");
    }

    #[test]
    fn set_document_value_rejects_non_table_parents() {
        let mut doc = "model = \"deepseek-v4-pro\"\n"
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        let err = set_document_value(&mut doc, &["model", "nested"], "x")
            .expect_err("scalar parent must be rejected");
        assert!(err.to_string().contains("must be a table"), "{err}");
    }

    #[test]
    fn remove_document_key_recursive_strips_nested_and_quoted_tables() {
        let mut doc = r#"# root note
api_key = "root"
api_key_env = "KEEP_ENV"

[providers.openrouter]
api_key = "or"
base_url = "https://openrouter.ai/api/v1"

[providers."quoted.provider"]
api_key = "quoted"

[[hotbar]]
slot = 1
"#
        .parse::<toml_edit::DocumentMut>()
        .unwrap();

        remove_document_key_recursive(doc.as_table_mut(), "api_key");

        let body = doc.to_string();
        assert!(!body.contains("api_key = "), "{body}");
        assert!(body.contains("# root note"), "{body}");
        assert!(body.contains("api_key_env = \"KEEP_ENV\""), "{body}");
        assert!(body.contains("base_url"), "{body}");
        assert!(body.contains("[[hotbar]]"), "{body}");
    }

    #[test]
    fn persist_custom_provider_unsets_removed_optional_fields() {
        let temp_root = temp_root("codewhale-custom-provider-unset");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let path = temp_root.join(".codewhale").join("config.toml");

        persist_custom_provider(
            Some(&path),
            "acme_ai",
            "https://api.acme.example/v1",
            Some("acme/code-1"),
            Some("ACME_API_KEY"),
        )
        .expect("first persist should succeed");
        persist_custom_provider(
            Some(&path),
            "acme_ai",
            "https://api.acme.example/v2",
            None,
            None,
        )
        .expect("second persist should succeed");

        let body = fs::read_to_string(&path).unwrap();
        let parsed: toml::Value = toml::from_str(&body).unwrap();
        let entry = parsed
            .get("providers")
            .and_then(|providers| providers.get("acme_ai"))
            .expect("provider entry");
        assert_eq!(
            entry.get("base_url").and_then(toml::Value::as_str),
            Some("https://api.acme.example/v2")
        );
        assert!(entry.get("model").is_none(), "model must be unset: {body}");
        assert!(
            entry.get("api_key_env").is_none(),
            "api_key_env must be unset: {body}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_writes_land_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp_root = temp_root("codewhale-persist-perms");
        fs::create_dir_all(&temp_root).unwrap();
        let _guard = EnvGuard::new(&temp_root);
        let path = temp_root.join(".deepseek").join("config.toml");
        write_golden_config(&path);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        persist_root_bool_key(Some(&path), "allow_shell", true).expect("persist should succeed");

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config.toml can hold api keys");
    }
}
