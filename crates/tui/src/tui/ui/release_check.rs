//! The startup update check: resolving a latest-release tag from the
//! configured source and turning it into a version hint.
//!
//! Moved verbatim out of `ui.rs`.

use super::*;

pub(crate) fn startup_version_check_source(config: &UpdateConfig) -> StartupVersionCheckSource {
    resolve_version_check_source(config, codewhale_release::suppression_reason())
}

/// Pure form of [`startup_version_check_source`]: the environment is read once
/// by the caller and passed in, so the decision itself is testable without
/// mutating process-global state (and so this crate's own CI run does not
/// change the answer).
pub(crate) fn resolve_version_check_source(
    config: &UpdateConfig,
    suppression: Option<codewhale_release::SuppressionReason>,
) -> StartupVersionCheckSource {
    if !config.check_for_updates {
        return StartupVersionCheckSource::Disabled;
    }
    // CI runners and explicit opt-outs never reach the network: there is
    // nobody at the terminal to read the notice, and an unexplained outbound
    // request from a build agent is a support ticket waiting to happen.
    if let Some(reason) = suppression {
        tracing::debug!(
            variable = reason.variable(),
            "skipping startup update check"
        );
        return StartupVersionCheckSource::Disabled;
    }
    if let Some(update_uri) = config.update_uri() {
        return StartupVersionCheckSource::ConfiguredUrl(update_uri.to_string());
    }
    StartupVersionCheckSource::ReleaseResolver
}

/// Where the throttling cache for the startup check lives.
///
/// `None` when the CodeWhale home cannot be resolved — the check still runs,
/// it just cannot be throttled, which is the right tradeoff for a homeless
/// install (rare, and better than never checking).
pub(crate) fn update_check_cache_path() -> Option<PathBuf> {
    codewhale_config::codewhale_home()
        .ok()
        .map(|home| codewhale_release::check::cache_path_in(&home))
}

pub(crate) fn spawn_startup_version_check(
    config: UpdateConfig,
) -> Option<tokio::task::JoinHandle<Option<UpdateNotice>>> {
    let source = startup_version_check_source(&config);
    if source == StartupVersionCheckSource::Disabled {
        return None;
    }

    let current = env!("CARGO_PKG_VERSION").to_string();
    let cache_path = update_check_cache_path();
    let interval_hours = config.check_interval_hours;
    Some(tokio::spawn(async move {
        cached_version_hint(source, &current, cache_path.as_deref(), interval_hours).await
    }))
}

/// Resolve the update notice, reaching for the network at most once per
/// configured interval.
///
/// The cache holds the *tag we last saw*, not a "checked recently" flag, so a
/// user who relaunches all afternoon still sees the notice every time while
/// GitHub is asked once. A cached `None` means "we asked, there was nothing" —
/// also a valid answer worth not re-asking for.
pub(crate) async fn cached_version_hint(
    source: StartupVersionCheckSource,
    current: &str,
    cache_path: Option<&Path>,
    interval_hours: u64,
) -> Option<UpdateNotice> {
    let now = codewhale_release::check::now_unix();
    if let Some(path) = cache_path
        && let Some(entry) = codewhale_release::UpdateCheckCache::load(path)
        && entry.is_fresh(now, interval_hours)
    {
        return entry
            .latest_tag
            .as_deref()
            .and_then(|tag| version_hint_from_latest_tag(tag, current));
    }

    let latest_tag = latest_tag_from_startup_source(source).await;

    // Only record a completed check. A network failure leaves the cache
    // untouched so the next launch retries instead of caching an outage for a
    // whole day.
    if let Some(path) = cache_path
        && latest_tag.is_some()
        && let Err(err) = codewhale_release::UpdateCheckCache::now(latest_tag.clone())
            .merging_last_seen_from_disk(path)
            .store(path)
    {
        tracing::debug!(error = %err, "failed to persist update-check cache");
    }

    latest_tag
        .as_deref()
        .and_then(|tag| version_hint_from_latest_tag(tag, current))
}

/// The latest publishable release tag for this source, or `None` when the
/// lookup failed or the release is not one we would offer.
pub(crate) async fn latest_tag_from_startup_source(
    source: StartupVersionCheckSource,
) -> Option<String> {
    match source {
        StartupVersionCheckSource::Disabled => None,
        StartupVersionCheckSource::ConfiguredUrl(url) => {
            match latest_tag_from_configured_update_uri(&url).await {
                Ok(tag) => tag,
                Err(_) => latest_tag_from_release_mirror_env().await,
            }
        }
        StartupVersionCheckSource::ReleaseResolver => {
            if release_mirror_env_configured() {
                return latest_tag_from_release_mirror_env().await;
            }

            let body = codewhale_release::fetch_release_json_async(
                codewhale_release::LATEST_RELEASE_URL,
                "latest release",
            )
            .await
            .ok()?;
            let json: serde_json::Value = serde_json::from_str(&body).ok()?;
            publishable_release_tag(&json).map(str::to_string)
        }
    }
}

pub(crate) async fn latest_tag_from_release_mirror_env() -> Option<String> {
    if !release_mirror_env_configured() {
        return None;
    }
    codewhale_release::latest_release_tag_async(codewhale_release::ReleaseChannel::Stable)
        .await
        .ok()
}

pub(crate) fn release_mirror_env_configured() -> bool {
    let version = codewhale_release::update_version_from_env()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    codewhale_release::release_base_url_from_env(&version).is_some()
}

pub(crate) async fn latest_tag_from_configured_update_uri(
    update_uri: &str,
) -> Result<Option<String>> {
    let body = codewhale_release::fetch_release_json_async(update_uri, "configured latest release")
        .await?;
    let json: serde_json::Value = serde_json::from_str(&body).with_context(|| {
        format!("failed to parse release JSON from configured URI {update_uri}")
    })?;
    Ok(custom_release_tag(&json).map(str::to_string))
}

/// Tag of a GitHub release we would actually offer: published, and with every
/// asset the updater needs already uploaded.
pub(crate) fn publishable_release_tag(json: &serde_json::Value) -> Option<&str> {
    if !release_has_required_assets(json) {
        return None;
    }
    json["tag_name"].as_str()
}

/// Tag from a user-configured release endpoint. Asset completeness is only
/// enforced when the payload advertises assets at all, since a custom mirror
/// may legitimately publish metadata in a different shape.
pub(crate) fn custom_release_tag(json: &serde_json::Value) -> Option<&str> {
    if !release_is_publishable(json) {
        return None;
    }
    if json.get("assets").is_some() && !release_has_required_assets(json) {
        return None;
    }
    json["tag_name"].as_str()
}

/// Test-only shorthand pairing tag extraction with the newness comparison.
/// Production code splits the two so the tag can be cached independently of
/// the version we happen to be running.
#[cfg(test)]
pub(crate) fn version_hint_from_release_json(
    json: &serde_json::Value,
    current: &str,
) -> Option<UpdateNotice> {
    version_hint_from_latest_tag(publishable_release_tag(json)?, current)
}

#[cfg(test)]
pub(crate) fn version_hint_from_custom_release_json(
    json: &serde_json::Value,
    current: &str,
) -> Option<UpdateNotice> {
    version_hint_from_latest_tag(custom_release_tag(json)?, current)
}

pub(crate) fn version_hint_from_latest_tag(tag: &str, current: &str) -> Option<UpdateNotice> {
    let latest = tag.trim_start_matches('v');
    if !is_newer_version(latest, current) {
        return None;
    }

    Some(UpdateNotice {
        current: current.to_string(),
        latest: latest.to_string(),
    })
}

pub(crate) fn release_has_required_assets(json: &serde_json::Value) -> bool {
    if !release_is_publishable(json) {
        return false;
    }

    REQUIRED_RELEASE_ASSETS
        .iter()
        .all(|required| release_has_uploaded_asset(json, required))
}

pub(crate) fn release_is_publishable(json: &serde_json::Value) -> bool {
    !json
        .get("draft")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        && !json
            .get("prerelease")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

pub(crate) fn release_has_uploaded_asset(json: &serde_json::Value, required: &str) -> bool {
    let Some(assets) = json.get("assets").and_then(serde_json::Value::as_array) else {
        return false;
    };
    assets.iter().any(|asset| {
        asset.get("name").and_then(serde_json::Value::as_str) == Some(required)
            && asset.get("state").and_then(serde_json::Value::as_str) == Some("uploaded")
    })
}

pub(crate) fn is_newer_version(latest: &str, current: &str) -> bool {
    // Compare semver so dev builds (e.g. "0.8.46-pre") don't trigger false
    // hints. Falls back to string compare on unparseable versions.
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => latest != current,
    }
}

/// Parse a `major.minor.patch` version string into a comparable tuple.
/// Returns `None` on any parse failure (non-semver, dev suffixes, etc.).
pub(crate) fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    Some((major, minor, patch))
}

/// One quiet transcript line when this launch is running a newer version
/// than the last one recorded in `update-check.json`. First run (no stored
/// version) is not an update and yields `None`.
pub(crate) fn maybe_push_local_upgrade_notice(app: &mut App, path: &Path, current: &str) {
    let Some(version) = codewhale_release::UpdateCheckCache::take_upgrade_to_current(path, current)
    else {
        return;
    };
    let line = tr(app.ui_locale, MessageId::StartupUpdatedNotice).replace("{version}", &version);
    app.add_message(HistoryCell::System { content: line });
}

/// Resolve the cache and, if this launch is an upgrade, push the `/change`
/// pointer. Missing home is a no-op: we cannot record that we showed it.
pub(crate) fn apply_local_upgrade_notice(app: &mut App) {
    let Some(path) = update_check_cache_path() else {
        return;
    };
    maybe_push_local_upgrade_notice(app, &path, env!("CARGO_PKG_VERSION"));
}
