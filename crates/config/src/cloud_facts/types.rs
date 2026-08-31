//! Signed payload shape for cloud facts (`schema_version` 1).
//!
//! Every section is optional and defaulted so a payload that only carries
//! release truth still parses. Unknown fields are preserved in `unknown` so
//! `/status` debugging can show what a newer server sent, but they are never
//! acted on.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_applies_to() -> String {
    "*".to_string()
}

/// The signed facts payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CloudFacts {
    pub schema_version: u32,
    pub channel: String,
    pub facts_version: u64,
    #[serde(default)]
    pub published_at: String,
    /// RFC 3339 UTC timestamp after which the facts are considered stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<String>,
    /// Cargo-style semver requirement the whole payload applies to.
    #[serde(default = "default_applies_to")]
    pub applies_to: String,
    #[serde(default)]
    pub models: Vec<ModelFact>,
    #[serde(default)]
    pub provider_defaults: BTreeMap<String, ProviderDefaultFact>,
    #[serde(default)]
    pub release: Option<ReleaseFact>,
    #[serde(default)]
    pub announcements: Vec<Announcement>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

/// What a model patch does to the catalog row it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelOp {
    /// Patch the fields that are set; create the row when enough is known.
    #[default]
    Upsert,
    /// Annotate as deprecated; never removes the row.
    Deprecate,
    /// Remove the row, but only when it came from the bundled/models.dev layers.
    Hide,
}

/// Per-million-token pricing patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PricingFact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_per_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_per_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_per_m: Option<f64>,
}

/// A field-level patch to one `(provider, wire id)` catalog row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelFact {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub op: ModelOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<PricingFact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies_to: Option<String>,
}

/// Provider default overrides. Only consulted when config.toml sets nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProviderDefaultFact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// Accepted only when `https` and on the provider's official host family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies_to: Option<String>,
}

/// Release truth: what the newest install is and which versions are yanked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReleaseFact {
    pub latest: String,
    #[serde(default)]
    pub yanked: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_supported: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies_to: Option<String>,
}

/// Announcement severity. There is deliberately no `Critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementLevel {
    #[default]
    Info,
    Warn,
}

/// Where an announcement may be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Tui,
    Desktop,
    Web,
}

/// A one-line banner. Text only; no actions, no executable content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Announcement {
    pub id: String,
    #[serde(default)]
    pub level: AnnouncementLevel,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Maximum announcement text length accepted by the client.
pub const MAX_ANNOUNCEMENT_CHARS: usize = 200;
