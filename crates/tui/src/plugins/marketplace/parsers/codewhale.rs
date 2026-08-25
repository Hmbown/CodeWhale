//! Codewhale native catalog parser.
//!
//! This is Codewhale's own documented catalog format. It is deliberately
//! the smallest of the supported formats: entries carry a `name` and a
//! `source` that is exactly the install spec `/plugin install` accepts
//! (`github:owner/repo`, `path:dir`, an http(s) tarball URL).
//!
//! ```json
//! {
//!   "name": "my-catalog",
//!   "description": "Team plugins",
//!   "version": "1",
//!   "plugins": [
//!     { "name": "formatter", "source": "github:owner/repo", "version": "2.1.0" }
//!   ]
//! }
//! ```

use serde_json::Value;

use crate::plugins::install::PluginInstallSource;

use super::super::types::{
    CatalogProvenance, CatalogTier, MarketplaceCandidate, MarketplaceCandidateId,
    MarketplaceCatalog, MarketplaceDiagnostic, MarketplaceFormat, MarketplaceInstallPlan,
    MarketplaceSourceSpec,
};
use super::{MarketplaceDocument, str_field, unknown_fields_warning};

const TOP_LEVEL_FIELDS: &[&str] = &["name", "description", "version", "plugins"];
const ENTRY_FIELDS: &[&str] = &["name", "source", "description", "version", "homepage"];

pub fn parse_codewhale_catalog(document: MarketplaceDocument) -> MarketplaceCatalog {
    let MarketplaceDocument {
        catalog_id,
        root,
        base,
        ..
    } = document;
    let mut diagnostics = Vec::new();

    let Some(obj) = root.as_object() else {
        return MarketplaceCatalog {
            id: catalog_id,
            format: MarketplaceFormat::Codewhale,
            name: String::new(),
            display_name: None,
            description: None,
            version: None,
            base,
            provenance: CatalogProvenance::default(),
            candidates: Vec::new(),
            diagnostics: vec![MarketplaceDiagnostic::error(
                "NOT_AN_OBJECT",
                "Codewhale catalog must be a JSON object",
                None,
                None,
            )],
        };
    };

    if let Some(diag) = unknown_fields_warning(obj, TOP_LEVEL_FIELDS) {
        diagnostics.push(diag);
    }

    let (name, bad_name) = str_field(obj, "name");
    if let Some(diag) = bad_name {
        diagnostics.push(diag);
    }
    let name = name
        .map(ToString::to_string)
        .unwrap_or_else(|| catalog_id.as_str().to_string());
    let (description, bad_desc) = str_field(obj, "description");
    if let Some(diag) = bad_desc {
        diagnostics.push(diag);
    }
    let (version, bad_version) = str_field(obj, "version");
    if let Some(diag) = bad_version {
        diagnostics.push(diag);
    }

    let Some(entries) = obj.get("plugins").and_then(Value::as_array) else {
        diagnostics.push(MarketplaceDiagnostic::error(
            "MISSING_PLUGINS",
            "Codewhale catalog must contain a `plugins` array",
            None,
            None,
        ));
        return MarketplaceCatalog {
            id: catalog_id,
            format: MarketplaceFormat::Codewhale,
            name,
            display_name: None,
            description: description.map(ToString::to_string),
            version: version.map(ToString::to_string),
            base,
            provenance: CatalogProvenance::default(),
            candidates: Vec::new(),
            diagnostics,
        };
    };

    let mut candidates = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if let Some(candidate) = parse_codewhale_entry(&catalog_id, index, entry, &mut diagnostics)
        {
            candidates.push(candidate);
        }
    }

    MarketplaceCatalog {
        id: catalog_id,
        format: MarketplaceFormat::Codewhale,
        name,
        display_name: None,
        description: description.map(ToString::to_string),
        version: version.map(ToString::to_string),
        base,
        provenance: CatalogProvenance {
            tier: CatalogTier::Community,
            publisher: None,
            source_url: None,
        },
        candidates,
        diagnostics,
    }
}

fn parse_codewhale_entry(
    catalog_id: &super::super::types::MarketplaceCatalogId,
    index: usize,
    entry: &Value,
    diagnostics: &mut Vec<MarketplaceDiagnostic>,
) -> Option<MarketplaceCandidate> {
    let Some(obj) = entry.as_object() else {
        diagnostics.push(MarketplaceDiagnostic::error(
            "MALFORMED_ENTRY",
            format!("Codewhale plugin at index {index} must be a JSON object"),
            None,
            Some(index),
        ));
        return None;
    };

    let mut entry_diags = Vec::new();
    if let Some(diag) = unknown_fields_warning(obj, ENTRY_FIELDS) {
        entry_diags.push(diag);
    }

    let (name, bad_name) = str_field(obj, "name");
    if let Some(diag) = bad_name {
        entry_diags.push(diag);
    }
    let Some(name) = name else {
        diagnostics.push(MarketplaceDiagnostic::error(
            "MISSING_NAME",
            format!("Codewhale plugin at index {index} is missing required `name`"),
            None,
            Some(index),
        ));
        return None;
    };
    let name = name.to_string();

    let (source, bad_source) = str_field(obj, "source");
    if let Some(diag) = bad_source {
        entry_diags.push(diag);
    }
    let Some(source) = source else {
        diagnostics.push(MarketplaceDiagnostic::error(
            "MISSING_SOURCE",
            format!("Codewhale plugin `{name}` is missing required `source`"),
            Some(name.clone()),
            Some(index),
        ));
        return None;
    };

    let install_plan = match PluginInstallSource::parse(source) {
        Ok(parsed) => {
            let source_kind = match &parsed {
                PluginInstallSource::Remote(crate::skills::install::InstallSource::GitHubRepo(
                    _,
                )) => "GitHub repository".to_string(),
                PluginInstallSource::Remote(crate::skills::install::InstallSource::DirectUrl(
                    _,
                )) => "Tarball URL".to_string(),
                PluginInstallSource::LocalPath { .. } => "Local directory".to_string(),
                PluginInstallSource::Builtin(_) => "Built into this Codewhale".to_string(),
                PluginInstallSource::Remote(crate::skills::install::InstallSource::Registry(_)) => {
                    "Registry".to_string()
                }
            };
            MarketplaceInstallPlan::Supported {
                spec: source.to_string(),
                source_kind,
            }
        }
        Err(err) => MarketplaceInstallPlan::Unsupported {
            reason: format!("invalid Codewhale install spec: {err}"),
            raw: source.to_string(),
        },
    };
    let normalized = normalize_native_source(source);

    let (description, bad_desc) = str_field(obj, "description");
    if let Some(diag) = bad_desc {
        entry_diags.push(diag);
    }
    let (version, bad_version) = str_field(obj, "version");
    if let Some(diag) = bad_version {
        entry_diags.push(diag);
    }
    let (homepage, bad_home) = str_field(obj, "homepage");
    if let Some(diag) = bad_home {
        entry_diags.push(diag);
    }

    Some(MarketplaceCandidate {
        id: MarketplaceCandidateId::new(catalog_id, &name),
        catalog_id: catalog_id.clone(),
        name,
        display_name: None,
        description: description.map(ToString::to_string),
        version: version.map(ToString::to_string),
        author: None,
        homepage: homepage.map(ToString::to_string),
        repository: None,
        license: None,
        keywords: Vec::new(),
        categories: Vec::new(),
        source: normalized,
        install_plan,
        declared_components: None,
        compatibility: None,
        provenance: CatalogProvenance {
            tier: CatalogTier::Community,
            publisher: None,
            source_url: None,
        },
        when: None,
        diagnostics: entry_diags,
    })
}

fn normalize_native_source(spec: &str) -> MarketplaceSourceSpec {
    match PluginInstallSource::parse(spec) {
        Ok(PluginInstallSource::LocalPath(path)) => MarketplaceSourceSpec::LocalPath { path },
        Ok(PluginInstallSource::Remote(crate::skills::install::InstallSource::GitHubRepo(
            repo,
        ))) => {
            let (owner, name) = repo.split_once('/').unwrap_or((&repo, ""));
            MarketplaceSourceSpec::GitHub {
                owner: owner.to_string(),
                repo: name.to_string(),
                git_ref: None,
                sha: None,
            }
        }
        Ok(PluginInstallSource::Remote(crate::skills::install::InstallSource::DirectUrl(url))) => {
            MarketplaceSourceSpec::ArchiveUrl { url, sha256: None }
        }
        Ok(PluginInstallSource::Builtin(name)) => MarketplaceSourceSpec::Builtin { name },
        Ok(other) => MarketplaceSourceSpec::Invalid {
            reason: format!("registry source {other:?} is not a marketplace install"),
        },
        Err(err) => MarketplaceSourceSpec::Invalid {
            reason: err.to_string(),
        },
    }
}
