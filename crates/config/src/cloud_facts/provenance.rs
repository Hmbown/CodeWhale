//! Provenance for `/status`: where the facts in use came from and how old they are.

use serde::{Deserialize, Serialize};

/// Where a verified payload was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactsOrigin {
    DiskCache,
    Network,
    LocalFile,
}

/// The state of the cloud facts layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CloudFactsState {
    /// Feature flag off (default). Bundled facts only.
    #[default]
    Off,
    /// Enabled but no active trusted key is pinned; nothing is fetched.
    Inert,
    /// Enabled; no verified payload yet (first launch, or every fetch failed).
    BundledOnly,
    /// A verified payload is merged over bundled facts.
    Verified {
        channel: String,
        facts_version: u64,
        key_id: String,
        fetched_at: u64,
        origin: FactsOrigin,
        stale: bool,
        patches: usize,
        defaults: usize,
        announcements: usize,
    },
    /// The last payload was rejected; bundled facts remain in use.
    Rejected { reason: String, at: u64 },
    /// Verified but not for this binary version.
    NotApplicable { applies_to: String },
    /// Fetch failed; prior verified facts (if any) stay in use.
    Failed {
        last_error: String,
        at: u64,
        keeping: Option<u64>,
    },
}

/// Status snapshot for UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CloudFactsStatus {
    pub state: CloudFactsState,
    pub last_attempt: Option<u64>,
    pub etag: Option<String>,
    pub source_label: String,
}

/// Human-readable age (`12m ago`, `3h ago`, `3d ago`).
#[must_use]
pub fn age_label(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

impl CloudFactsStatus {
    /// One-line `/status` value.
    #[must_use]
    pub fn label(&self, now_unix: u64) -> String {
        match &self.state {
            CloudFactsState::Off => "off (bundled)".to_string(),
            CloudFactsState::Inert => "inert (no trusted keys; bundled)".to_string(),
            CloudFactsState::BundledOnly => "enabled, none verified yet (bundled)".to_string(),
            CloudFactsState::Verified {
                channel,
                facts_version,
                key_id,
                fetched_at,
                origin,
                stale,
                patches,
                defaults,
                announcements,
            } => {
                let origin = match origin {
                    FactsOrigin::DiskCache => "disk cache",
                    FactsOrigin::Network => "network",
                    FactsOrigin::LocalFile => "local file",
                };
                let mut out = format!(
                    "{channel} v{facts_version} · verified {key_id} · fetched {} ({origin})",
                    age_label(*fetched_at, now_unix)
                );
                if *stale {
                    out.push_str(" · stale");
                }
                let _ = std::fmt::Write::write_fmt(
                    &mut out,
                    format_args!(
                        " · {patches} patch{}, {defaults} default{}, {announcements} notice{}",
                        if *patches == 1 { "" } else { "es" },
                        if *defaults == 1 { "" } else { "s" },
                        if *announcements == 1 { "" } else { "s" },
                    ),
                );
                out
            }
            CloudFactsState::Rejected { reason, at } => {
                format!(
                    "rejected: {reason} ({}; bundled in use)",
                    age_label(*at, now_unix)
                )
            }
            CloudFactsState::NotApplicable { applies_to } => {
                format!("not applicable to this build ({applies_to}; bundled in use)")
            }
            CloudFactsState::Failed {
                last_error,
                at,
                keeping,
            } => match keeping {
                Some(version) => format!(
                    "fetch failed {} ({last_error}); keeping v{version}",
                    age_label(*at, now_unix)
                ),
                None => format!(
                    "fetch failed {} ({last_error}); bundled in use",
                    age_label(*at, now_unix)
                ),
            },
        }
    }
}
