//! Secret-free OpenAI Codex / ChatGPT OAuth model roster discovery.
//!
//! The Codex CLI keeps its account-scoped roster in `models_cache.json`.
//! CodeWhale reads only the cache timestamp and model identifiers; it never
//! opens the adjacent OAuth credential file and never logs cache contents.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::config::DEFAULT_OPENAI_CODEX_MODEL;

const MODEL_CACHE_FILE: &str = "models_cache.json";
const MAX_MODEL_CACHE_BYTES: u64 = 4 * 1024 * 1024;
/// Codex refreshes its own cache much more frequently. CodeWhale is an offline
/// consumer, so it accepts a last-known account roster for one day before
/// falling back to the single conservative compatibility model.
const MODEL_CACHE_MAX_AGE: Duration = Duration::hours(24);
const MAX_FUTURE_CLOCK_SKEW: Duration = Duration::minutes(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexModelCacheFreshness {
    Fresh,
    Missing,
    Stale,
    Invalid,
}

impl CodexModelCacheFreshness {
    #[must_use]
    pub(crate) const fn picker_label(self) -> &'static str {
        match self {
            Self::Fresh => "ChatGPT OAuth",
            Self::Missing => "OAuth roster missing · fallback",
            Self::Stale => "OAuth roster stale · fallback",
            Self::Invalid => "OAuth roster invalid · fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexModelRoster {
    pub(crate) models: Vec<CodexModelMetadata>,
    pub(crate) freshness: CodexModelCacheFreshness,
    pub(crate) fetched_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexModelMetadata {
    pub(crate) id: String,
    pub(crate) context_window: Option<u32>,
    pub(crate) reasoning: Option<bool>,
}

impl CodexModelRoster {
    fn fallback(freshness: CodexModelCacheFreshness, fetched_at: Option<DateTime<Utc>>) -> Self {
        Self {
            models: vec![CodexModelMetadata {
                id: DEFAULT_OPENAI_CODEX_MODEL.to_string(),
                context_window: None,
                reasoning: None,
            }],
            freshness,
            fetched_at,
        }
    }

    #[must_use]
    pub(crate) fn model_ids(&self) -> Vec<String> {
        self.models.iter().map(|model| model.id.clone()).collect()
    }

    #[must_use]
    pub(crate) fn metadata_for(&self, id: &str) -> Option<&CodexModelMetadata> {
        self.models
            .iter()
            .find(|model| model.id.eq_ignore_ascii_case(id.trim()))
    }
}

#[derive(Debug, Deserialize)]
struct CacheFile {
    fetched_at: DateTime<Utc>,
    #[serde(default)]
    models: Vec<CacheModel>,
}

#[derive(Debug, Deserialize)]
struct CacheModel {
    slug: String,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    supported_reasoning_levels: Option<Vec<CacheReasoningLevel>>,
}

#[derive(Debug, Deserialize)]
struct CacheReasoningLevel {}

/// Resolve the Codex home without consulting OAuth-file overrides.
///
/// `OPENAI_CODEX_AUTH_FILE` intentionally does not participate: it may point
/// at a standalone test/credential file while the model roster still belongs
/// to `$CODEX_HOME` (or the default `~/.codex`).
#[must_use]
pub(crate) fn codex_home_path() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::config::effective_home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        })
}

#[must_use]
pub(crate) fn model_roster() -> CodexModelRoster {
    load_model_roster_from_home_at(&codex_home_path(), Utc::now())
}

fn load_model_roster_from_home_at(home: &Path, now: DateTime<Utc>) -> CodexModelRoster {
    let path = home.join(MODEL_CACHE_FILE);
    let path_metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CodexModelRoster::fallback(CodexModelCacheFreshness::Missing, None);
        }
        Err(_) => return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, None),
    };
    if !path_metadata.file_type().is_file() || path_metadata.len() > MAX_MODEL_CACHE_BYTES {
        return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, None);
    }
    let mut file = match open_cache_file(&path) {
        Ok(file) => file,
        Err(_) => return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, None),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, None),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_MODEL_CACHE_BYTES {
        return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, None);
    }

    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_MODEL_CACHE_BYTES) as usize);
    if file
        .by_ref()
        .take(MAX_MODEL_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_MODEL_CACHE_BYTES
    {
        return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, None);
    }
    let cache: CacheFile = match serde_json::from_slice(&bytes) {
        Ok(cache) => cache,
        Err(_) => return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, None),
    };

    let age = now.signed_duration_since(cache.fetched_at);
    if age < -MAX_FUTURE_CLOCK_SKEW {
        return CodexModelRoster::fallback(
            CodexModelCacheFreshness::Invalid,
            Some(cache.fetched_at),
        );
    }
    if age > MODEL_CACHE_MAX_AGE {
        return CodexModelRoster::fallback(CodexModelCacheFreshness::Stale, Some(cache.fetched_at));
    }

    let mut indexed: Vec<_> = cache.models.into_iter().enumerate().collect();
    indexed.sort_by_key(|(index, model)| (model.priority.unwrap_or(i64::MAX), *index));

    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for (_, model) in indexed {
        let slug = model.slug.trim();
        if !valid_model_id(slug) {
            continue;
        }
        let identity = slug.to_ascii_lowercase();
        if seen.insert(identity) {
            models.push(CodexModelMetadata {
                id: slug.to_string(),
                context_window: model
                    .context_window
                    .filter(|window| (1..=16_000_000).contains(window)),
                reasoning: model
                    .supported_reasoning_levels
                    .map(|levels| !levels.is_empty()),
            });
        }
    }
    if models.is_empty() {
        return CodexModelRoster::fallback(
            CodexModelCacheFreshness::Invalid,
            Some(cache.fetched_at),
        );
    }

    CodexModelRoster {
        models,
        freshness: CodexModelCacheFreshness::Fresh,
        fetched_at: Some(cache.fetched_at),
    }
}

fn open_cache_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().any(|byte| byte.is_ascii_alphanumeric())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/codex_models_cache.json");
    const FIXTURE_TIME: &str = "2030-01-02T03:04:05Z";

    fn fixture_time() -> DateTime<Utc> {
        FIXTURE_TIME.parse().expect("fixture timestamp")
    }

    fn write_fixture(home: &Path) {
        std::fs::write(home.join(MODEL_CACHE_FILE), FIXTURE).expect("write fixture");
    }

    #[test]
    fn valid_cache_uses_priority_order_and_keeps_route_available_rows() {
        let home = tempfile::tempdir().expect("temp CODEX_HOME");
        write_fixture(home.path());

        let roster =
            load_model_roster_from_home_at(home.path(), fixture_time() + Duration::minutes(30));

        assert_eq!(roster.freshness, CodexModelCacheFreshness::Fresh);
        assert_eq!(roster.fetched_at, Some(fixture_time()));
        assert_eq!(
            roster.model_ids(),
            [
                "gpt-test-primary",
                "gpt-test-secondary",
                "codex-test-review"
            ]
        );
        let primary = roster
            .metadata_for("gpt-test-primary")
            .expect("primary metadata");
        assert_eq!(primary.context_window, Some(372_000));
        assert_eq!(primary.reasoning, Some(true));
        let secondary = roster
            .metadata_for("gpt-test-secondary")
            .expect("secondary metadata");
        assert_eq!(secondary.context_window, Some(128_000));
    }

    #[test]
    fn missing_cache_falls_back_conservatively() {
        let home = tempfile::tempdir().expect("temp CODEX_HOME");
        let roster = load_model_roster_from_home_at(home.path(), fixture_time());

        assert_eq!(roster.freshness, CodexModelCacheFreshness::Missing);
        assert_eq!(roster.model_ids(), [DEFAULT_OPENAI_CODEX_MODEL]);
    }

    #[test]
    fn malformed_cache_falls_back_conservatively() {
        let home = tempfile::tempdir().expect("temp CODEX_HOME");
        std::fs::write(home.path().join(MODEL_CACHE_FILE), b"{not-json")
            .expect("write malformed cache");

        let roster = load_model_roster_from_home_at(home.path(), fixture_time());

        assert_eq!(roster.freshness, CodexModelCacheFreshness::Invalid);
        assert_eq!(roster.model_ids(), [DEFAULT_OPENAI_CODEX_MODEL]);
    }

    #[test]
    fn oversized_cache_is_rejected_without_unbounded_read() {
        let home = tempfile::tempdir().expect("temp CODEX_HOME");
        let file = std::fs::File::create(home.path().join(MODEL_CACHE_FILE)).expect("cache file");
        file.set_len(MAX_MODEL_CACHE_BYTES + 1)
            .expect("sparse oversized cache");

        let roster = load_model_roster_from_home_at(home.path(), fixture_time());

        assert_eq!(roster.freshness, CodexModelCacheFreshness::Invalid);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cache_is_rejected_as_non_regular_input() {
        let home = tempfile::tempdir().expect("temp CODEX_HOME");
        let target = home.path().join("target.json");
        std::fs::write(&target, FIXTURE).expect("target fixture");
        std::os::unix::fs::symlink(&target, home.path().join(MODEL_CACHE_FILE))
            .expect("cache symlink");

        let roster = load_model_roster_from_home_at(home.path(), fixture_time());

        assert_eq!(roster.freshness, CodexModelCacheFreshness::Invalid);
    }

    #[test]
    fn stale_cache_falls_back_conservatively() {
        let home = tempfile::tempdir().expect("temp CODEX_HOME");
        write_fixture(home.path());

        let roster =
            load_model_roster_from_home_at(home.path(), fixture_time() + Duration::hours(25));

        assert_eq!(roster.freshness, CodexModelCacheFreshness::Stale);
        assert_eq!(roster.model_ids(), [DEFAULT_OPENAI_CODEX_MODEL]);
        assert_eq!(roster.fetched_at, Some(fixture_time()));
    }

    #[test]
    fn invalid_and_duplicate_model_ids_are_filtered() {
        let home = tempfile::tempdir().expect("temp CODEX_HOME");
        let cache = format!(
            r#"{{
  "fetched_at": "{FIXTURE_TIME}",
  "models": [
    {{"slug": "gpt-good", "priority": 3}},
    {{"slug": "GPT-GOOD", "priority": 4}},
    {{"slug": "bad model", "priority": 1}},
    {{"slug": "../bad\\path", "priority": 2}}
  ]
}}"#
        );
        std::fs::write(home.path().join(MODEL_CACHE_FILE), cache).expect("write cache");

        let roster = load_model_roster_from_home_at(home.path(), fixture_time());

        assert_eq!(roster.freshness, CodexModelCacheFreshness::Fresh);
        assert_eq!(roster.model_ids(), ["gpt-good"]);
    }

    #[test]
    fn codex_home_respects_environment_override() {
        let lock = crate::test_support::lock_test_env();
        let home = tempfile::tempdir().expect("temp CODEX_HOME");
        let guard = crate::test_support::EnvVarGuard::set("CODEX_HOME", home.path());

        assert_eq!(codex_home_path(), home.path());

        drop(guard);
        drop(lock);
    }
}
