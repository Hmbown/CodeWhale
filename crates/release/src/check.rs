//! Throttling and suppression for background "is there a newer release?"
//! checks.
//!
//! Two rules govern when we are allowed to ask GitHub:
//!
//! 1. **Not in CI, and not when the user said no.** A build agent has no one
//!    to tell, and an outbound request from a sandboxed runner is at best
//!    noise and at worst a firewall alert.
//! 2. **At most once per interval.** The answer changes on release cadence,
//!    not on launch cadence, so it is cached on disk and reused.
//!
//! The cache stores the *answer* (the latest tag), not merely a "checked
//! recently" flag. That distinction matters: a stale-binary user who relaunches
//! ten times in an hour should still see the notice every time, while the
//! network is touched at most once. Caching only a timestamp — and returning
//! "no update" on a cache hit — would hide the notice for the whole interval,
//! which is the opposite of what the feature is for.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Filename of the on-disk update-check cache, relative to the CodeWhale home
/// directory (`~/.codewhale/update-check.json` by default).
pub const UPDATE_CHECK_CACHE_FILE: &str = "update-check.json";

/// Default hours between network update checks.
pub const DEFAULT_CHECK_INTERVAL_HOURS: u64 = 1;

/// Explicit opt-out, and the `update-notifier` convention many CLIs honour.
const OPT_OUT_ENV: &[&str] = &["CODEWHALE_NO_UPDATE_CHECK", "NO_UPDATE_NOTIFIER"];

/// Environment variables whose presence means "this is an automated build".
const CI_ENV: &[&str] = &[
    "CI",
    "CONTINUOUS_INTEGRATION",
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "BUILDKITE",
    "CIRCLECI",
    "JENKINS_URL",
    "TEAMCITY_VERSION",
    "TF_BUILD",
];

/// Why an update check was skipped without contacting the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionReason {
    /// The user (or a wrapper script) set an opt-out variable.
    OptOut(&'static str),
    /// A CI marker variable is set.
    ContinuousIntegration(&'static str),
}

impl SuppressionReason {
    /// The environment variable responsible, for logs and `doctor` output.
    #[must_use]
    pub fn variable(self) -> &'static str {
        match self {
            Self::OptOut(var) | Self::ContinuousIntegration(var) => var,
        }
    }
}

/// Returns the reason update checks are suppressed in this process, if any.
///
/// A variable set to an explicitly falsey value (`""`, `"0"`, `"false"`) does
/// not count as set — some shells and CI images export `CI=false`, and taking
/// that as "we are in CI" would disable the check for ordinary users.
#[must_use]
pub fn suppression_reason() -> Option<SuppressionReason> {
    for var in OPT_OUT_ENV {
        if env_flag_is_truthy(var) {
            return Some(SuppressionReason::OptOut(var));
        }
    }
    for var in CI_ENV {
        if env_flag_is_truthy(var) {
            return Some(SuppressionReason::ContinuousIntegration(var));
        }
    }
    None
}

fn env_flag_is_truthy(var: &str) -> bool {
    match std::env::var(var) {
        Ok(value) => flag_value_is_truthy(&value),
        Err(_) => false,
    }
}

fn flag_value_is_truthy(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

/// Cached result of the last network update check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpdateCheckCache {
    /// Unix seconds at which the network was last queried.
    pub checked_at_unix: u64,
    /// The latest release tag seen, or `None` when the check completed but
    /// produced no usable tag (e.g. the release had no publishable assets).
    #[serde(default)]
    pub latest_tag: Option<String>,
    /// The CodeWhale version this machine last launched. Distinct from
    /// [`Self::latest_tag`]: that is the newest *published* release the
    /// network check saw, this is the binary we actually ran. Used to show
    /// a one-shot post-update pointer at `/change`.
    #[serde(default)]
    pub last_seen_version: Option<String>,
}

impl UpdateCheckCache {
    /// Record a check that just completed.
    #[must_use]
    pub fn now(latest_tag: Option<String>) -> Self {
        Self {
            checked_at_unix: now_unix(),
            latest_tag,
            last_seen_version: None,
        }
    }

    /// Copy `last_seen_version` from disk so a network-check write cannot
    /// wipe the post-update notice cursor.
    #[must_use]
    pub fn merging_last_seen_from_disk(mut self, path: &Path) -> Self {
        if let Some(on_disk) = Self::load(path).and_then(|entry| entry.last_seen_version) {
            self.last_seen_version = Some(on_disk);
        }
        self
    }

    /// Persist `current` as the last-seen running version.
    ///
    /// Returns `Some(current)` exactly once when `current` is newer than the
    /// previously stored version. A missing previous version is a first run
    /// and yields `None` — that launch is not an update. The write happens
    /// before the `Some` is returned, so a later launch of the same version
    /// cannot fire again.
    #[must_use]
    pub fn take_upgrade_to_current(path: &Path, current: &str) -> Option<String> {
        let current = normalize_running_version(current)?;
        let mut cache = Self::load(path).unwrap_or_default();
        let previous = cache
            .last_seen_version
            .as_deref()
            .and_then(normalize_running_version);
        let notice = previous
            .as_deref()
            .and_then(|prev| running_version_is_newer(&current, prev).then(|| current.clone()));
        if previous.as_deref() != Some(current.as_str()) {
            // Re-read so a concurrent network-check write is not clobbered.
            if let Some(fresh) = Self::load(path) {
                cache.checked_at_unix = fresh.checked_at_unix;
                cache.latest_tag = fresh.latest_tag;
            }
            cache.last_seen_version = Some(current);
            if cache.store(path).is_err() {
                return None;
            }
        }
        notice
    }

    /// Read the cache, returning `None` for "absent, unreadable, or corrupt".
    ///
    /// A damaged cache is indistinguishable from no cache for our purposes:
    /// both mean "we do not know, go ask". Nothing here is worth surfacing an
    /// error for.
    #[must_use]
    pub fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// True when this entry is young enough to reuse instead of re-fetching.
    ///
    /// A timestamp in the future is treated as stale rather than
    /// infinitely-fresh, so a clock that jumped forward and back cannot wedge
    /// the check off permanently.
    #[must_use]
    pub fn is_fresh(&self, now_unix: u64, interval_hours: u64) -> bool {
        if self.checked_at_unix > now_unix {
            return false;
        }
        let age = now_unix - self.checked_at_unix;
        age < interval_hours.saturating_mul(3600)
    }

    /// Write the cache atomically (temp file, then rename), creating the
    /// parent directory if needed.
    pub fn store(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(self).context("failed to serialize update cache")?;
        std::fs::write(&tmp, body).with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("failed to install {}", path.display()))?;
        Ok(())
    }
}

/// Resolve the cache path inside a CodeWhale home directory.
#[must_use]
pub fn cache_path_in(codewhale_home: &Path) -> PathBuf {
    codewhale_home.join(UPDATE_CHECK_CACHE_FILE)
}

/// Current Unix time in seconds, saturating at 0 for pre-epoch clocks.
#[must_use]
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn normalize_running_version(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_start_matches('v').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn running_version_is_newer(current: &str, previous: &str) -> bool {
    match (
        crate::parse_release_version(current),
        crate::parse_release_version(previous),
    ) {
        (Ok(current), Ok(previous)) => current > previous,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_is_fresh_inside_the_interval_and_stale_outside_it() {
        let entry = UpdateCheckCache {
            checked_at_unix: 1_000_000,
            latest_tag: Some("v0.9.5".to_string()),
            last_seen_version: None,
        };
        // One second before the one-hour boundary: still fresh.
        assert!(entry.is_fresh(1_000_000 + 3599, 1));
        // One second beyond the boundary: stale.
        assert!(!entry.is_fresh(1_000_000 + 3601, 1));
        // Exactly at the boundary counts as stale, so the interval is a true
        // upper bound on cache age.
        assert!(!entry.is_fresh(1_000_000 + 3600, 1));
    }

    #[test]
    fn a_zero_interval_always_refetches() {
        let entry = UpdateCheckCache {
            checked_at_unix: 1_000_000,
            latest_tag: None,
            last_seen_version: None,
        };
        assert!(!entry.is_fresh(1_000_000, 0));
    }

    #[test]
    fn a_future_timestamp_is_stale_not_permanently_fresh() {
        let entry = UpdateCheckCache {
            checked_at_unix: 2_000_000,
            latest_tag: Some("v9.9.9".to_string()),
            last_seen_version: None,
        };
        assert!(!entry.is_fresh(1_000_000, 24));
    }

    #[test]
    fn store_then_load_round_trips_and_survives_an_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path_in(dir.path());
        assert_eq!(path.file_name().unwrap(), UPDATE_CHECK_CACHE_FILE);

        let first = UpdateCheckCache {
            checked_at_unix: 42,
            latest_tag: Some("v0.9.5".to_string()),
            last_seen_version: Some("0.9.4".to_string()),
        };
        first.store(&path).expect("store");
        assert_eq!(UpdateCheckCache::load(&path), Some(first));

        // Overwriting in place must not leave the temp file behind.
        let second = UpdateCheckCache {
            checked_at_unix: 99,
            latest_tag: None,
            last_seen_version: Some("0.9.5".to_string()),
        };
        second.store(&path).expect("overwrite");
        assert_eq!(UpdateCheckCache::load(&path), Some(second));
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn store_creates_a_missing_home_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path_in(&dir.path().join("nested").join("home"));
        UpdateCheckCache::now(Some("v1.0.0".to_string()))
            .store(&path)
            .expect("store into a fresh directory");
        assert!(path.exists());
    }

    #[test]
    fn a_corrupt_or_absent_cache_reads_as_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path_in(dir.path());
        assert_eq!(UpdateCheckCache::load(&path), None);
        std::fs::write(&path, b"{ not json").expect("write junk");
        assert_eq!(UpdateCheckCache::load(&path), None);
    }

    #[test]
    fn falsey_flag_values_do_not_count_as_set() {
        for value in ["", "0", "false", "FALSE", " no ", "off"] {
            assert!(
                !flag_value_is_truthy(value),
                "{value:?} should not read as set"
            );
        }
        for value in ["1", "true", "yes", "azure-pipelines"] {
            assert!(flag_value_is_truthy(value), "{value:?} should read as set");
        }
    }

    #[test]
    fn suppression_reason_names_the_responsible_variable() {
        assert_eq!(
            SuppressionReason::OptOut("CODEWHALE_NO_UPDATE_CHECK").variable(),
            "CODEWHALE_NO_UPDATE_CHECK"
        );
        assert_eq!(
            SuppressionReason::ContinuousIntegration("CI").variable(),
            "CI"
        );
    }

    #[test]
    fn a_cache_written_before_last_seen_version_existed_loads_as_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path_in(dir.path());
        std::fs::write(
            &path,
            r#"{
  "checked_at_unix": 42,
  "latest_tag": "v0.9.5"
}"#,
        )
        .expect("write legacy cache");
        let loaded = UpdateCheckCache::load(&path).expect("legacy cache still loads");
        assert_eq!(loaded.checked_at_unix, 42);
        assert_eq!(loaded.latest_tag.as_deref(), Some("v0.9.5"));
        assert_eq!(loaded.last_seen_version, None);
    }

    #[test]
    fn first_run_is_not_an_upgrade_and_records_the_current_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path_in(dir.path());
        assert_eq!(
            UpdateCheckCache::take_upgrade_to_current(&path, "0.9.11"),
            None,
            "no stored version is a first run, not an update"
        );
        let stored = UpdateCheckCache::load(&path).expect("first run still records the version");
        assert_eq!(stored.last_seen_version.as_deref(), Some("0.9.11"));
        assert_eq!(stored.checked_at_unix, 0);
        assert_eq!(stored.latest_tag, None);
    }

    #[test]
    fn an_upgrade_notice_fires_once_per_version_and_not_on_the_same_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path_in(dir.path());
        UpdateCheckCache {
            checked_at_unix: 7,
            latest_tag: Some("v0.9.11".to_string()),
            last_seen_version: Some("0.9.10".to_string()),
        }
        .store(&path)
        .expect("seed previous version");

        assert_eq!(
            UpdateCheckCache::take_upgrade_to_current(&path, "0.9.11"),
            Some("0.9.11".to_string())
        );
        let after = UpdateCheckCache::load(&path).expect("upgrade records the new version");
        assert_eq!(after.last_seen_version.as_deref(), Some("0.9.11"));
        assert_eq!(
            after.latest_tag.as_deref(),
            Some("v0.9.11"),
            "recording last-seen must not wipe the network-check tag"
        );
        assert_eq!(after.checked_at_unix, 7);

        assert_eq!(
            UpdateCheckCache::take_upgrade_to_current(&path, "0.9.11"),
            None,
            "the same version must not fire again"
        );
        assert_eq!(
            UpdateCheckCache::take_upgrade_to_current(&path, "v0.9.11"),
            None,
            "a v-prefix on the running version is the same version"
        );
    }

    #[test]
    fn a_downgrade_is_not_an_upgrade() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path_in(dir.path());
        UpdateCheckCache {
            checked_at_unix: 1,
            latest_tag: None,
            last_seen_version: Some("0.9.11".to_string()),
        }
        .store(&path)
        .expect("seed");
        assert_eq!(
            UpdateCheckCache::take_upgrade_to_current(&path, "0.9.10"),
            None
        );
        assert_eq!(
            UpdateCheckCache::load(&path)
                .expect("load")
                .last_seen_version
                .as_deref(),
            Some("0.9.10"),
            "a downgrade still updates the cursor so a later return is an upgrade"
        );
    }

    #[test]
    fn merging_last_seen_from_disk_survives_a_network_check_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path_in(dir.path());
        UpdateCheckCache {
            checked_at_unix: 1,
            latest_tag: Some("v0.9.10".to_string()),
            last_seen_version: Some("0.9.11".to_string()),
        }
        .store(&path)
        .expect("seed last-seen");

        UpdateCheckCache::now(Some("v0.9.12".to_string()))
            .merging_last_seen_from_disk(&path)
            .store(&path)
            .expect("network check store");
        let stored = UpdateCheckCache::load(&path).expect("load");
        assert_eq!(stored.latest_tag.as_deref(), Some("v0.9.12"));
        assert_eq!(
            stored.last_seen_version.as_deref(),
            Some("0.9.11"),
            "network-check store must keep the last-seen cursor"
        );
    }
}
