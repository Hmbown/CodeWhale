//! Process-wide overlay: the scoped facts currently merged over bundled facts,
//! plus the status `/status` renders. Set by the fetch layer
//! (`codewhale-cloud-facts`), read by the catalog compiler and provider
//! default resolution.

use std::sync::{Arc, RwLock};

use super::provenance::{CloudFactsState, CloudFactsStatus};
use super::scope::ScopedFacts;

static OVERLAY: RwLock<Option<Arc<ScopedFacts>>> = RwLock::new(None);
static STATUS: RwLock<Option<CloudFactsStatus>> = RwLock::new(None);

/// Where a resolved provider default came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultSource {
    /// Compiled-in constant.
    Compiled,
    /// Cloud facts overlay (verified payload at `facts_version`).
    CloudFacts { facts_version: u64 },
}

/// Install (or clear with `None`) the scoped facts overlay.
pub fn set_overlay(facts: Option<ScopedFacts>) {
    if let Ok(mut guard) = OVERLAY.write() {
        *guard = facts.map(Arc::new);
    }
}

/// The current overlay, if a verified payload is installed.
#[must_use]
pub fn overlay() -> Option<Arc<ScopedFacts>> {
    OVERLAY.read().ok().and_then(|guard| guard.clone())
}

/// Replace the status snapshot.
pub fn set_status(status: CloudFactsStatus) {
    if let Ok(mut guard) = STATUS.write() {
        *guard = Some(status);
    }
}

/// Current status; `Off` until the fetch layer reports anything.
#[must_use]
pub fn status() -> CloudFactsStatus {
    STATUS
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}

/// Whether a verified payload is currently merged.
#[must_use]
pub fn is_verified() -> bool {
    matches!(status().state, CloudFactsState::Verified { .. })
}

/// Cloud default model for `provider`, when the overlay carries one.
///
/// Only consulted after every configured source (CLI, env, config.toml) has
/// declined, so a cloud fact can never override an explicit choice.
#[must_use]
pub fn cloud_default_model(provider: &str) -> Option<(String, DefaultSource)> {
    let overlay = overlay()?;
    let fact = overlay.provider_defaults.get(provider)?;
    let model = fact.default_model.clone()?;
    Some((
        model,
        DefaultSource::CloudFacts {
            facts_version: overlay.facts_version,
        },
    ))
}

/// Cloud base URL for `provider`, already allowlist-filtered by `scope`.
#[must_use]
pub fn cloud_default_base_url(provider: &str) -> Option<(String, DefaultSource)> {
    let overlay = overlay()?;
    let fact = overlay.provider_defaults.get(provider)?;
    let url = fact.base_url.clone()?;
    Some((
        url,
        DefaultSource::CloudFacts {
            facts_version: overlay.facts_version,
        },
    ))
}

/// Reset both the overlay and the status (tests and shutdown).
pub fn clear() {
    set_overlay(None);
    if let Ok(mut guard) = STATUS.write() {
        *guard = None;
    }
}
