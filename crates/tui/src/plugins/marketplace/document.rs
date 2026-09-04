//! Local catalog document loading shared by the `/plugin marketplace add`
//! command and the Runtime API marketplace endpoints (#5311 surface).
//!
//! One loader so both entry points apply the same rules: LOCAL file only
//! (never network), bounded size, no symlink documents, strict per-format
//! parsing, and refusal to persist a document that parsed to nothing but
//! errors. Candidate→install-spec resolution also lives here so the TUI
//! command and the HTTP API can never disagree about what would be fetched.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::parsers::{self, MarketplaceDocument};
use super::store::StoredMarketplaceCatalog;
use super::types::{
    MarketplaceCandidate, MarketplaceCatalogId, MarketplaceFormat, MarketplaceInstallPlan,
    MarketplaceSourceSpec,
};

/// Catalog documents are JSON text; four megabytes is far beyond any real
/// published catalog and caps the parse cost of a user-supplied file.
const MAX_CATALOG_BYTES: u64 = 4 * 1024 * 1024;

/// A parsed catalog ready to store, plus the counts callers render back.
#[derive(Debug)]
pub struct LoadedCatalogDocument {
    pub entry: StoredMarketplaceCatalog,
    pub candidate_count: usize,
    pub warning_count: usize,
}

/// Conservative catalog name: it becomes a key, appears in candidate IDs,
/// and is rendered back to the operator.
#[must_use]
pub fn valid_marketplace_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Read and parse a LOCAL catalog document. Relative paths resolve against
/// `workspace`. No network is touched, here or anywhere in this module.
pub fn load_catalog_document(
    name: &str,
    workspace: &Path,
    raw_path: &str,
) -> Result<LoadedCatalogDocument, String> {
    if !valid_marketplace_name(name) {
        return Err(
            "Marketplace name must be 1-64 characters of letters, digits, `-`, `_`, or `.`"
                .to_string(),
        );
    }
    let path = PathBuf::from(raw_path.trim());
    let path = if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    };
    let path = canonical_document(&path)?;
    let body = read_bounded(&path)?;
    let root = serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|error| format!("Catalog at {} is not valid JSON: {error}", path.display()))?;

    let document = MarketplaceDocument {
        catalog_id: MarketplaceCatalogId::new(name),
        format: MarketplaceFormat::Auto,
        root,
        base: Some(path.display().to_string()),
    };
    let catalog = parsers::parse_catalog(document);

    // A document-level error (unknown/ambiguous format, not-an-object) means
    // nothing useful was parsed; do not persist it.
    if catalog.candidates.is_empty() && catalog.error_count() > 0 {
        return Err(format!(
            "Catalog `{name}` could not be parsed as any known marketplace format (kimi, claude, codex, codewhale):\n{}",
            render_diagnostics_inline(&catalog.diagnostics)
        ));
    }

    let entry = StoredMarketplaceCatalog {
        added_at: chrono::Utc::now().to_rfc3339(),
        source_path: path.display().to_string(),
        catalog,
    };
    Ok(LoadedCatalogDocument {
        candidate_count: entry.catalog.total_candidates(),
        warning_count: entry.catalog.warning_count(),
        entry,
    })
}

/// What installing a stored candidate would do, resolved once for every
/// caller. `Supported.spec` is exactly what the reviewed installer accepts.
pub enum CatalogInstallResolution {
    Supported { spec: String, source_kind: String },
    Unsupported { reason: String },
    HasErrors { diagnostics: String },
}

/// Resolve a stored catalog candidate to its install spec. Relative local
/// paths resolve against the catalog document's own directory, not the
/// caller's working directory.
pub fn resolve_candidate_install(
    entry: &StoredMarketplaceCatalog,
    candidate: &MarketplaceCandidate,
) -> CatalogInstallResolution {
    if candidate.has_errors() {
        return CatalogInstallResolution::HasErrors {
            diagnostics: render_diagnostics_inline(&candidate.diagnostics),
        };
    }
    match &candidate.install_plan {
        MarketplaceInstallPlan::Supported { spec, source_kind } => {
            CatalogInstallResolution::Supported {
                spec: resolve_spec(&entry.source_path, &candidate.source, spec),
                source_kind: source_kind.clone(),
            }
        }
        MarketplaceInstallPlan::Unsupported { reason, .. } => {
            CatalogInstallResolution::Unsupported {
                reason: reason.clone(),
            }
        }
    }
}

fn resolve_spec(source_path: &str, source: &MarketplaceSourceSpec, spec: &str) -> String {
    if let MarketplaceSourceSpec::LocalPath { path } = source
        && path.is_relative()
        && let Some(dir) = Path::new(source_path).parent()
    {
        return format!("path:{}", dir.join(path).display());
    }
    spec.to_string()
}

/// Resolve a user-supplied document path to an existing regular file without
/// following a final symlink (the document is untrusted input).
fn canonical_document(path: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Cannot read catalog at {}: {e}", path.display()))?;
    if metadata.is_symlink() {
        return Err(format!(
            "Catalog path {} is a symlink; marketplace documents must be regular files",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Catalog path {} is not a regular file",
            path.display()
        ));
    }
    Ok(path.to_path_buf())
}

fn read_bounded(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Cannot read catalog at {}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_CATALOG_BYTES {
        return Err(format!(
            "Catalog at {} exceeds the {} byte limit",
            path.display(),
            MAX_CATALOG_BYTES
        ));
    }
    let mut text = String::new();
    let mut limited = file.take(MAX_CATALOG_BYTES + 1);
    limited
        .read_to_string(&mut text)
        .map_err(|e| format!("Cannot read catalog at {}: {e}", path.display()))?;
    Ok(text)
}

fn render_diagnostics_inline(diagnostics: &[super::types::MarketplaceDiagnostic]) -> String {
    use crate::plugins::types::PluginDiagnosticLevel;
    diagnostics
        .iter()
        .map(|d| {
            format!(
                "{} {}: {}",
                match d.level {
                    PluginDiagnosticLevel::Error => "error",
                    PluginDiagnosticLevel::Warning => "warning",
                },
                d.code,
                d.message
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marketplace_names_are_conservative() {
        assert!(valid_marketplace_name("official"));
        assert!(valid_marketplace_name("My-Catalog_2.beta"));
        assert!(!valid_marketplace_name(""));
        assert!(!valid_marketplace_name("has space"));
        assert!(!valid_marketplace_name("a".repeat(65).as_str()));
    }

    #[test]
    fn load_refuses_symlink_documents() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        std::fs::write(&real, "{}").unwrap();
        let _link = dir.path().join("link.json");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &_link).unwrap();
        #[cfg(not(unix))]
        let _link = real.clone();

        let _error = load_catalog_document("test", dir.path(), _link.to_str().unwrap())
            .expect_err("symlink document must be refused");
        #[cfg(unix)]
        assert!(_error.contains("symlink"), "{_error}");
    }

    #[test]
    fn load_refuses_unknown_format_documents() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("catalog.json");
        std::fs::write(&doc, r#"{"totally":"unknown"}"#).unwrap();

        let error = load_catalog_document("test", dir.path(), doc.to_str().unwrap())
            .expect_err("unknown format must be refused");
        assert!(error.contains("could not be parsed"), "{error}");
    }
}
