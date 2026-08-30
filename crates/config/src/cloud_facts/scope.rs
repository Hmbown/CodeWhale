//! Scope a verified payload to the running binary: per-item `applies_to`,
//! announcement windows, and the provider `base_url` allowlist all live here so
//! consumers see one already-filtered view.

use std::collections::BTreeMap;

use super::types::{
    Announcement, MAX_ANNOUNCEMENT_CHARS, ModelFact, ProviderDefaultFact, ReleaseFact,
};
use super::verify::{VerifiedFacts, item_applies, parse_rfc3339_utc};
use crate::provider_kind::ProviderKind;

/// A verified payload filtered to what applies to this binary right now.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScopedFacts {
    pub channel: String,
    pub facts_version: u64,
    pub key_id: String,
    pub sha256: String,
    pub published_at: String,
    pub stale: bool,
    pub models: Vec<ModelFact>,
    pub provider_defaults: BTreeMap<String, ProviderDefaultFact>,
    pub release: Option<ReleaseFact>,
    pub announcements: Vec<Announcement>,
    /// Human-readable receipts for every item that was dropped and why.
    pub dropped: Vec<String>,
}

impl ScopedFacts {
    /// Count of applied patch/default/announcement items, for `/status`.
    #[must_use]
    pub fn item_counts(&self) -> (usize, usize, usize) {
        (
            self.models.len(),
            self.provider_defaults.len(),
            self.announcements.len(),
        )
    }
}

/// Whether `candidate` (an https URL) stays on `provider`'s official host family.
///
/// The allowlist is derived from the compiled default base URL: the exact
/// host, or any subdomain of its registrable-ish domain (last two labels).
/// Providers whose compiled default is not https (local runtimes) accept no
/// cloud base URL at all.
#[must_use]
pub fn base_url_allowed(provider: &str, candidate: &str) -> bool {
    let Some(kind) = ProviderKind::parse(provider) else {
        return false;
    };
    let Some(default_host) = https_host(kind.provider().default_base_url()) else {
        return false;
    };
    let Some(candidate_host) = https_host(candidate) else {
        return false;
    };
    if candidate_host == default_host {
        return true;
    }
    let labels: Vec<&str> = default_host.rsplit('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let base = format!("{}.{}", labels[1], labels[0]);
    candidate_host == base || candidate_host.ends_with(&format!(".{base}"))
}

fn https_host(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let host = authority.rsplit_once(':').map_or(authority, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            authority
        }
    });
    Some(host.to_ascii_lowercase())
}

/// Build the scoped view for `current` at `now_unix`.
#[must_use]
pub fn scoped_view(
    verified: &VerifiedFacts,
    current: &semver::Version,
    now_unix: u64,
) -> ScopedFacts {
    let facts = &verified.facts;
    let mut out = ScopedFacts {
        channel: facts.channel.clone(),
        facts_version: facts.facts_version,
        key_id: verified.key_id.clone(),
        sha256: verified.sha256.clone(),
        published_at: facts.published_at.clone(),
        stale: verified.stale,
        ..ScopedFacts::default()
    };

    for model in &facts.models {
        if model.provider.trim().is_empty() || model.id.trim().is_empty() {
            out.dropped.push("model patch without provider/id".into());
            continue;
        }
        if !item_applies(model.applies_to.as_deref(), current) {
            out.dropped.push(format!(
                "model {}/{}: applies_to {:?} does not match",
                model.provider,
                model.id,
                model.applies_to.as_deref().unwrap_or("")
            ));
            continue;
        }
        out.models.push(model.clone());
    }

    for (provider, fact) in &facts.provider_defaults {
        if !item_applies(fact.applies_to.as_deref(), current) {
            out.dropped.push(format!(
                "provider_defaults.{provider}: applies_to {:?} does not match",
                fact.applies_to.as_deref().unwrap_or("")
            ));
            continue;
        }
        let mut kept = ProviderDefaultFact {
            default_model: fact
                .default_model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            base_url: None,
            applies_to: None,
        };
        if let Some(url) = fact.base_url.as_deref().map(str::trim) {
            if base_url_allowed(provider, url) {
                kept.base_url = Some(url.to_string());
            } else {
                out.dropped.push(format!(
                    "provider_defaults.{provider}.base_url {url:?}: not https on the official host family"
                ));
            }
        }
        if kept.default_model.is_none() && kept.base_url.is_none() {
            continue;
        }
        out.provider_defaults.insert(provider.clone(), kept);
    }

    if let Some(release) = &facts.release {
        if item_applies(release.applies_to.as_deref(), current) {
            out.release = Some(release.clone());
        } else {
            out.dropped.push(format!(
                "release: applies_to {:?} does not match",
                release.applies_to.as_deref().unwrap_or("")
            ));
        }
    }

    for announcement in &facts.announcements {
        let id = announcement.id.trim();
        if id.is_empty() || announcement.text.trim().is_empty() {
            out.dropped.push("announcement without id/text".into());
            continue;
        }
        if announcement.text.chars().count() > MAX_ANNOUNCEMENT_CHARS {
            out.dropped
                .push(format!("announcement {id}: text too long"));
            continue;
        }
        if !item_applies(announcement.applies_to.as_deref(), current) {
            out.dropped
                .push(format!("announcement {id}: applies_to does not match"));
            continue;
        }
        if let Some(starts) = announcement
            .starts_at
            .as_deref()
            .and_then(parse_rfc3339_utc)
            && now_unix < starts
        {
            out.dropped.push(format!("announcement {id}: not started"));
            continue;
        }
        if let Some(expires) = announcement
            .expires_at
            .as_deref()
            .and_then(parse_rfc3339_utc)
            && now_unix >= expires
        {
            out.dropped.push(format!("announcement {id}: expired"));
            continue;
        }
        out.announcements.push(announcement.clone());
    }

    out
}
