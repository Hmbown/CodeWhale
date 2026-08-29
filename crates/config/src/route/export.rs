//! Owned, serializable sibling of [`super::ProviderDescriptor`].
//!
//! `ProviderDescriptor` holds `&'static dyn Provider` and is deliberately not
//! `Serialize`. This module is the export seam: `codewhale providers export
//! --json` and the cwc generated catalog both consume [`ProvidersExport`].

use serde::{Deserialize, Serialize};

use crate::provider::{self, WireFormat, WirePolicy};
use crate::{ProviderKind, catalog::bundled_catalog_offerings};

use super::auth::AuthMethodExport;
use super::descriptor::{ProviderDescriptor, TransportKind};
use super::ids::RouteId;

/// Schema version of the providers export document.
pub const PROVIDERS_EXPORT_SCHEMA_VERSION: u32 = 1;

/// Top-level `codewhale providers export --json` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersExport {
    /// Document schema.
    pub schema_version: u32,
    /// Runtime version that produced this export.
    pub runtime_version: String,
    /// One row per addressable route id.
    pub routes: Vec<RouteExport>,
}

/// One exported route. This is the owned sibling of [`ProviderDescriptor`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteExport {
    /// Flat kebab route id (open string).
    pub id: String,
    /// Display-grouping family (not a second identity).
    pub family: String,
    /// Human label.
    pub label: String,
    /// Default endpoint URL.
    pub endpoint: String,
    /// Wire format spoken at the default endpoint.
    pub wire: String,
    /// Default wire model id.
    pub default_model: String,
    /// Environment variable candidates for the API key.
    pub env_vars: Vec<String>,
    /// Declared auth methods. OAuth is a type only.
    pub auth: Vec<AuthMethodExport>,
    /// Bundled catalog models for this route, when any exist.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<RouteModelExport>,
    /// Bespoke-transport classification. Catalog rows share `chat-completions`.
    pub transport: TransportKind,
}

/// One bundled model advertised on a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteModelExport {
    /// Provider-owned wire id.
    pub id: String,
    /// Canonical model id when a join exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    /// Whether this is the route default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

impl ProvidersExport {
    /// Build the export from the live descriptor registry.
    #[must_use]
    pub fn from_registry(runtime_version: impl Into<String>) -> Self {
        let offerings = bundled_catalog_offerings();
        let mut routes: Vec<RouteExport> = provider::all_providers()
            .iter()
            .map(|entry| {
                let descriptor = ProviderDescriptor::for_kind(entry.kind());
                route_export(&descriptor, &offerings)
            })
            .collect();
        routes.sort_by(|a, b| a.id.cmp(&b.id));
        Self {
            schema_version: PROVIDERS_EXPORT_SCHEMA_VERSION,
            runtime_version: runtime_version.into(),
            routes,
        }
    }

    /// Stable committed route-id list. Removing or respelling an id is a CI failure.
    #[must_use]
    pub fn route_ids(&self) -> Vec<&str> {
        self.routes.iter().map(|route| route.id.as_str()).collect()
    }
}

fn route_export(
    descriptor: &ProviderDescriptor,
    offerings: &[crate::catalog::CatalogOffering],
) -> RouteExport {
    let id = descriptor.route_id();
    let models = offerings
        .iter()
        .filter(|row| row.provider == id.as_str())
        .map(|row| RouteModelExport {
            id: row.wire_model_id.clone(),
            canonical: row.canonical_model.clone(),
            default: row.default_for_provider,
        })
        .collect();
    RouteExport {
        id: id.as_str().to_string(),
        family: descriptor.family().to_string(),
        label: descriptor.inner.display_name().to_string(),
        endpoint: descriptor.default_base_url().to_string(),
        wire: wire_label(descriptor.wire_policy()),
        default_model: descriptor.default_wire_model().as_str().to_string(),
        env_vars: descriptor
            .env_vars()
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        auth: descriptor
            .auth_methods()
            .iter()
            .copied()
            .map(AuthMethodExport::from)
            .collect(),
        models,
        transport: descriptor.transport(),
    }
}

fn wire_label(policy: WirePolicy) -> String {
    match policy {
        WirePolicy::Fixed(WireFormat::ChatCompletions) => "chat-completions".to_string(),
        WirePolicy::Fixed(WireFormat::Responses) => "responses".to_string(),
        WirePolicy::Fixed(WireFormat::AnthropicMessages) => "anthropic-messages".to_string(),
        WirePolicy::ModelAware => "model-aware".to_string(),
    }
}

/// Parse a CLI `--provider` / route-id string against the catalog.
///
/// Replaces the closed `ProviderArg` enum. Any catalog route id or documented
/// alias resolves; unknown ids fail.
#[must_use]
pub fn parse_route_kind(value: &str) -> Option<ProviderKind> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(kind) = exact_config_identity(trimmed) {
        return Some(kind);
    }
    let folded = trimmed.replace('_', "-");
    if let Some(kind) = exact_config_identity(&folded) {
        return Some(kind);
    }
    if let Some(kind) = clap_compat_alias(trimmed) {
        return Some(kind);
    }
    ProviderKind::parse(trimmed).or_else(|| ProviderKind::parse(&folded.to_ascii_lowercase()))
}

/// Exact id / config-table key only. Does not collapse dialect aliases onto
/// the vendor primary — that is what orphaned `[providers.minimax_anthropic]`
/// tables. Call [`ProviderKind::parse`] only after this and clap-compat aliases.
fn exact_config_identity(value: &str) -> Option<ProviderKind> {
    crate::provider::all_providers()
        .iter()
        .find(|entry| {
            value.eq_ignore_ascii_case(entry.id())
                || value.eq_ignore_ascii_case(entry.provider_config_key())
        })
        .map(|entry| entry.kind())
}

fn clap_compat_alias(value: &str) -> Option<ProviderKind> {
    let key = value.replace('_', "-").to_ascii_lowercase();
    Some(match key.as_str() {
        "agy" => ProviderKind::Antigravity,
        "opencodego" | "opencode-go" => ProviderKind::OpencodeGo,
        "ollama-cloud" => ProviderKind::OllamaCloud,
        "mini-max-anthropic" => ProviderKind::MinimaxAnthropic,
        "siliconflow-china" | "silicon-flow-cn" => ProviderKind::SiliconflowCN,
        "deep-infra" => ProviderKind::Deepinfra,
        "fugu" | "sakana-ai" => ProviderKind::Sakana,
        "long-cat" | "meituan-longcat" | "meituan" => ProviderKind::LongCat,
        "meta-ai" | "meta-model-api" | "muse" | "muse-spark" => ProviderKind::Meta,
        "x-ai" | "grok" => ProviderKind::Xai,
        "mistral-ai" | "mistralai" | "la-plateforme" => ProviderKind::Mistral,
        "eden-ai" => ProviderKind::Edenai,
        _ => return None,
    })
}

/// [`RouteId`] for a known kind.
#[must_use]
pub fn route_id_for(kind: ProviderKind) -> RouteId {
    RouteId::from(kind.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_covers_every_registry_route_exactly_once() {
        let export = ProvidersExport::from_registry("0.9.12");
        let ids = export.route_ids();
        let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "route ids must be unique");
        assert_eq!(ids.len(), provider::all_providers().len());
        assert!(ids.contains(&"deepseek"));
        assert!(ids.contains(&"custom"));
    }

    #[test]
    fn export_has_no_artificial_analysis_fields() {
        let json = serde_json::to_string(&ProvidersExport::from_registry("0.9.12")).unwrap();
        let lowered = json.to_ascii_lowercase();
        assert!(
            !lowered.contains("artificialanalysis")
                && !lowered.contains("artificial_analysis")
                && !lowered.contains("artificial-analysis"),
            "export must never carry Artificial Analysis fields"
        );
    }

    #[test]
    fn parse_route_kind_accepts_aliases() {
        assert_eq!(
            parse_route_kind("deepseek-anthropic"),
            Some(ProviderKind::DeepseekAnthropic)
        );
        assert_eq!(
            parse_route_kind("siliconflow-CN"),
            Some(ProviderKind::SiliconflowCN)
        );
        assert_eq!(
            parse_route_kind("minimax_anthropic"),
            Some(ProviderKind::MinimaxAnthropic)
        );
        assert_eq!(
            parse_route_kind("mini-max-anthropic"),
            Some(ProviderKind::MinimaxAnthropic)
        );
        assert_eq!(parse_route_kind(""), None);
        assert_eq!(parse_route_kind("not-a-provider"), None);
    }

    #[test]
    fn golden_route_ids_are_stable() {
        let export = ProvidersExport::from_registry("0.9.12");
        let actual: Vec<&str> = export.route_ids();
        let expected: Vec<&str> = include_str!("golden_route_ids.txt")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        assert_eq!(
            actual, expected,
            "route ids are a committed contract; do not remove or respell an id"
        );
    }

    #[test]
    #[ignore = "set WRITE_GOLDEN=1 to regenerate providers-export.golden.json"]
    fn write_golden_providers_export_when_requested() {
        if std::env::var("WRITE_GOLDEN").ok().as_deref() != Some("1") {
            return;
        }
        let export = ProvidersExport::from_registry("0.9.12");
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/route/providers-export.golden.json"
        );
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&export).unwrap()),
        )
        .expect("write providers-export.golden.json");
    }

    #[test]
    fn golden_providers_export_matches_registry() {
        let export = ProvidersExport::from_registry("0.9.12");
        let golden: ProvidersExport =
            serde_json::from_str(include_str!("providers-export.golden.json"))
                .expect("providers-export.golden.json must parse");
        assert_eq!(
            export.routes, golden.routes,
            "update providers-export.golden.json from ProvidersExport::from_registry"
        );
        assert!(
            !serde_json::to_string(&export)
                .unwrap()
                .to_ascii_lowercase()
                .contains("artificialanalysis"),
            "export must never carry Artificial Analysis fields"
        );
    }
}
