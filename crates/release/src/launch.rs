//! Remembering which version last started, so the first launch after an
//! update can say what changed.
//!
//! An update is invisible from inside the TUI: the user runs `codewhale
//! update` in a shell, restarts, and lands in a session that looks exactly
//! like the one before it. The changelog exists (`/change` renders it, already
//! localized) but nothing points at it at the one moment it is relevant.
//!
//! This module supplies the missing edge. It keeps a single version string on
//! disk next to the update-check cache and compares it to the running binary
//! at startup.
//!
//! Three deliberate choices about when to stay quiet:
//!
//! * **A fresh install is not an update.** With no record on disk we write one
//!   and say nothing: a first-run user has no previous version to have moved
//!   from, and pointing them at a changelog for software they have never run
//!   is noise.
//! * **Going backwards is not an update.** Bisecting a bug, or having two
//!   installs on `PATH`, means older binaries run after newer ones. That is
//!   not an event to celebrate, and a "what's new" pointer that appears while
//!   you downgrade is actively confusing.
//! * **Unparseable versions are not compared.** A `-pre`/`-dev` suffix or a
//!   locally patched version string means the comparison cannot be trusted;
//!   an exact string change is still recorded, but only a real, parseable
//!   increase produces a hint.
//!
//! Recording is best-effort: a read-only or full home directory costs the user
//! a hint, which is not worth an error dialog on startup. It is not, however,
//! silent -- [`record_launch`] hands the failure back in its result so the
//! caller can log it through whatever channel it already has. This crate is
//! reachable from the CLI updater before logging is initialized, which is why
//! it takes no `tracing` dependency of its own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};

/// Filename of the last-launch record, relative to the CodeWhale home
/// directory (`~/.codewhale/last-launch.json` by default).
pub const LAST_LAUNCH_FILE: &str = "last-launch.json";

/// What a startup recording found and whether it managed to persist.
#[derive(Debug)]
pub struct LaunchOutcome {
    /// The upgrade the user just completed, if this launch was one.
    pub change: Option<VersionChange>,
    /// Why the record could not be written, if it could not be.
    ///
    /// The only consequence is a hint that will be offered again after the
    /// next update; callers should log this, not surface it.
    pub record_error: Option<anyhow::Error>,
}

/// The versions either side of an update the user has just completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionChange {
    /// The version recorded by the previous run.
    pub previous: String,
    /// The version running now.
    pub current: String,
}

/// The version that last started CodeWhale on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastLaunch {
    /// The `CARGO_PKG_VERSION` of the binary that last wrote this file.
    pub version: String,
}

impl LastLaunch {
    /// Read the record, returning `None` for "absent, unreadable, or corrupt".
    ///
    /// As with the update-check cache, a damaged file is indistinguishable
    /// from no file: both mean "we do not know what ran last", and the safe
    /// answer to that is silence plus a fresh record.
    #[must_use]
    pub fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Write the record atomically (temp file, then rename), creating the
    /// parent directory if needed.
    pub fn store(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(self).context("failed to serialize launch record")?;
        std::fs::write(&tmp, body).with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("failed to install {}", path.display()))?;
        Ok(())
    }
}

/// Resolve the record path inside a CodeWhale home directory.
#[must_use]
pub fn record_path_in(codewhale_home: &Path) -> PathBuf {
    codewhale_home.join(LAST_LAUNCH_FILE)
}

/// Compare a stored version against the running one.
///
/// Split out from [`record_launch`] so the decision is testable without
/// touching the filesystem.
#[must_use]
pub fn version_change(previous: Option<&str>, current: &str) -> Option<VersionChange> {
    let previous = previous?;
    if previous == current {
        return None;
    }
    // Both sides must parse for the comparison to mean anything. A version we
    // cannot order is a version we cannot claim moved forward.
    let (Ok(before), Ok(now)) = (Version::parse(previous), Version::parse(current)) else {
        return None;
    };
    if now <= before {
        return None;
    }
    Some(VersionChange {
        previous: previous.to_string(),
        current: current.to_string(),
    })
}

/// Record that `current` is running, and report whether that is an upgrade
/// over whatever ran last.
///
/// The record is rewritten whenever it does not already name `current`,
/// including on downgrade and on the unparseable versions that never produce a
/// hint. Storing the version that actually ran — rather than the highest ever
/// seen — keeps the file a truthful answer to "what ran last", and stops a
/// single downgrade-then-upgrade cycle from re-announcing a version the user
/// has already been shown.
///
/// A failed write is reported alongside the answer rather than replacing it:
/// the comparison has already been made by that point and is still true.
pub fn record_launch(codewhale_home: &Path, current: &str) -> LaunchOutcome {
    let path = record_path_in(codewhale_home);
    let previous = LastLaunch::load(&path).map(|record| record.version);
    let change = version_change(previous.as_deref(), current);

    let record_error = if previous.as_deref() == Some(current) {
        None
    } else {
        LastLaunch {
            version: current.to_string(),
        }
        .store(&path)
        .err()
    };

    LaunchOutcome {
        change,
        record_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forward_version_move_is_an_update() {
        assert_eq!(
            version_change(Some("0.9.10"), "0.9.11"),
            Some(VersionChange {
                previous: "0.9.10".to_string(),
                current: "0.9.11".to_string(),
            })
        );
    }

    #[test]
    fn the_same_version_is_not_an_update() {
        assert_eq!(version_change(Some("0.9.11"), "0.9.11"), None);
    }

    #[test]
    fn a_first_ever_launch_is_not_an_update() {
        assert_eq!(version_change(None, "0.9.11"), None);
    }

    #[test]
    fn a_downgrade_is_not_an_update() {
        assert_eq!(version_change(Some("0.9.11"), "0.9.10"), None);
    }

    #[test]
    fn a_major_or_minor_move_counts_as_much_as_a_patch() {
        assert!(version_change(Some("0.9.11"), "0.10.0").is_some());
        assert!(version_change(Some("0.9.11"), "1.0.0").is_some());
    }

    // Ordering by string would rank "0.9.9" above "0.9.10" and suppress the
    // hint on the release where the patch number gains a digit.
    #[test]
    fn double_digit_patches_order_numerically_not_lexically() {
        assert!(version_change(Some("0.9.9"), "0.9.10").is_some());
        assert_eq!(version_change(Some("0.9.10"), "0.9.9"), None);
    }

    #[test]
    fn a_prerelease_sorts_below_the_release_it_precedes() {
        assert!(version_change(Some("0.9.11-pre"), "0.9.11").is_some());
        assert_eq!(version_change(Some("0.9.11"), "0.9.11-pre"), None);
    }

    #[test]
    fn an_unparseable_version_on_either_side_produces_no_hint() {
        assert_eq!(version_change(Some("not-a-version"), "0.9.11"), None);
        assert_eq!(version_change(Some("0.9.10"), "also-not-a-version"), None);
    }

    #[test]
    fn a_first_launch_records_the_version_without_claiming_an_update() {
        let home = tempfile::tempdir().expect("tempdir");
        let outcome = record_launch(home.path(), "0.9.10");
        assert_eq!(outcome.change, None);
        assert!(outcome.record_error.is_none());
        assert_eq!(
            LastLaunch::load(&record_path_in(home.path())).map(|r| r.version),
            Some("0.9.10".to_string())
        );
    }

    #[test]
    fn the_hint_fires_once_and_not_again_on_the_next_launch() {
        let home = tempfile::tempdir().expect("tempdir");
        record_launch(home.path(), "0.9.10");
        assert!(record_launch(home.path(), "0.9.11").change.is_some());
        assert_eq!(record_launch(home.path(), "0.9.11").change, None);
    }

    // The record answers "what ran last", so a downgrade must overwrite it.
    // Keeping the high-water mark instead would swallow the hint when the user
    // returns to the newer build.
    #[test]
    fn a_downgrade_rewrites_the_record_so_the_return_trip_still_hints() {
        let home = tempfile::tempdir().expect("tempdir");
        record_launch(home.path(), "0.9.11");
        assert_eq!(record_launch(home.path(), "0.9.10").change, None);
        assert_eq!(
            LastLaunch::load(&record_path_in(home.path())).map(|r| r.version),
            Some("0.9.10".to_string())
        );
        assert!(record_launch(home.path(), "0.9.11").change.is_some());
    }

    #[test]
    fn a_corrupt_record_is_replaced_rather_than_reported() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = record_path_in(home.path());
        std::fs::write(&path, b"{ not json").expect("seed corrupt record");
        assert_eq!(record_launch(home.path(), "0.9.11").change, None);
        assert_eq!(
            LastLaunch::load(&path).map(|r| r.version),
            Some("0.9.11".to_string())
        );
    }

    // A home that cannot be written costs the user a hint, not a startup
    // failure -- the version comparison itself must still be answered.
    #[cfg(unix)]
    #[test]
    fn an_unwritable_home_reports_the_failure_and_still_answers() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("tempdir");
        let path = record_path_in(home.path());
        LastLaunch {
            version: "0.9.10".to_string(),
        }
        .store(&path)
        .expect("seed record");
        let mut perms = std::fs::metadata(home.path())
            .expect("metadata")
            .permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(home.path(), perms).expect("chmod");

        let outcome = record_launch(home.path(), "0.9.11");

        let mut perms = std::fs::metadata(home.path())
            .expect("metadata")
            .permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(home.path(), perms).expect("restore chmod");

        assert!(outcome.change.is_some(), "the comparison still holds");
        assert!(
            outcome.record_error.is_some(),
            "an unwritable home must be reported, not swallowed"
        );
    }
}
