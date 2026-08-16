//! `/plugin marketplace` — the #5311 user journey over the catalog parsers.
//!
//! `add` reads a LOCAL catalog document (no network here, ever), parses it
//! with the strict per-format parsers, and persists the parsed result next to
//! the plugin registry state. `list`/`show` render candidates with their
//! honest install plans and per-entry diagnostics. `install` routes a
//! candidate through the EXISTING reviewed installer — the same code path as
//! `/plugin install`, so installed bundles still enter disabled and untrusted.
//!
//! Catalog-declared tiers and provenance are display-only: nothing in this
//! module grants trust, enables anything, or auto-installs (Codex
//! `INSTALLED_BY_DEFAULT` is visibly ignored).

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::render::{escape_review_path, escape_review_text};
use crate::commands::CommandResult;
use crate::localization::{Locale, MessageId, tr};
use crate::plugins::marketplace::parsers::MarketplaceDocument;
use crate::plugins::marketplace::parsers::kimi::{
    KIMI_GZIP_TARBALL_SOURCE_KIND, KIMI_REMOTE_UNSUPPORTED_REASON, KIMI_ZIP_UNSUPPORTED_REASON,
};
use crate::plugins::marketplace::store::{MarketplaceStore, StoredMarketplaceCatalog};
use crate::plugins::marketplace::types::{
    MarketplaceCatalog, MarketplaceFormat, MarketplaceInstallPlan, MarketplaceSourceSpec,
};
use crate::plugins::types::PluginDiagnosticLevel;
use crate::tui::app::App;

/// Catalog documents are JSON text; four megabytes is far beyond any real
/// published catalog and caps the parse cost of a user-supplied file.
const MAX_CATALOG_BYTES: u64 = 4 * 1024 * 1024;

const USAGE: &str = "Usage: /plugin marketplace add|list|show|remove|install\n\
     \x20 add <name> <path>    read a local catalog file (kimi/claude/codex/codewhale)\n\
     \x20 list                 show catalogs and their candidates\n\
     \x20 show <name>          one catalog in detail\n\
     \x20 remove <name>        forget a catalog (installed plugins unaffected)\n\
     \x20 install <catalog> <candidate>  install via the reviewed installer";

pub(super) fn dispatch(app: &mut App, words: &[&str]) -> CommandResult {
    match words {
        [] | ["list"] => list(app),
        ["add", name, path] => add(app, name, path),
        ["show", name] => show(app, name),
        ["remove", name] => remove(app, name),
        ["install", catalog, candidate] => install(app, catalog, candidate),
        _ => CommandResult::error(USAGE),
    }
}

fn open_store(app: &App) -> Result<MarketplaceStore, Box<CommandResult>> {
    MarketplaceStore::open(app.plugin_registry.state_path()).ok_or_else(|| {
        Box::new(CommandResult::error(
            "This plugin registry has no persistence store, so marketplace catalogs cannot be saved.",
        ))
    })
}

/// Conservative catalog name: it becomes a key, appears in candidate IDs,
/// and is rendered back to the operator.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn add(app: &mut App, name: &str, raw_path: &str) -> CommandResult {
    if !valid_name(name) {
        return CommandResult::error(
            "Marketplace name must be 1-64 characters of letters, digits, `-`, `_`, or `.`",
        );
    }
    let store = match open_store(app) {
        Ok(store) => store,
        Err(result) => return *result,
    };
    let path = PathBuf::from(raw_path.trim());
    let path = if path.is_absolute() {
        path
    } else {
        app.workspace.join(path)
    };
    let path = match canonical_document(&path) {
        Ok(path) => path,
        Err(error) => return CommandResult::error(error),
    };

    let body = match read_bounded(&path) {
        Ok(text) => text,
        Err(error) => return CommandResult::error(error),
    };
    let root = match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(root) => root,
        Err(error) => {
            return CommandResult::error(format!(
                "Catalog at {} is not valid JSON: {error}",
                escape_review_path(&path)
            ));
        }
    };

    let document = MarketplaceDocument {
        catalog_id: crate::plugins::marketplace::types::MarketplaceCatalogId::new(name),
        format: MarketplaceFormat::Auto,
        root,
        base: Some(path.display().to_string()),
    };
    let catalog = crate::plugins::marketplace::parsers::parse_catalog(document);

    // A document-level error (unknown/ambiguous format, not-an-object) means
    // nothing useful was parsed; do not persist it.
    if catalog.candidates.is_empty() && catalog.error_count() > 0 {
        return CommandResult::error(format!(
            "Catalog `{}` could not be parsed as any known marketplace format (kimi, claude, codex, codewhale):\n{}",
            escape_review_text(name),
            render_diagnostics_inline(&catalog.diagnostics)
        ));
    }

    let entry = StoredMarketplaceCatalog {
        added_at: chrono::Utc::now().to_rfc3339(),
        source_path: path.display().to_string(),
        catalog,
    };
    let candidate_count = entry.catalog.total_candidates();
    let warning_count = entry.catalog.warning_count();
    let summary = render_catalog_summary(name, &entry.catalog);
    match store.add(&entry.catalog.id.clone(), entry) {
        Ok(()) => CommandResult::message(format!(
            "Added marketplace `{}` ({} candidate(s), {} warning(s)).\n{summary}\n\
             Tiers and provenance are display-only. Nothing was installed, trusted, or enabled.",
            escape_review_text(name),
            candidate_count,
            warning_count,
        )),
        Err(error) => CommandResult::error(error),
    }
}

fn list(app: &mut App) -> CommandResult {
    let store = match open_store(app) {
        Ok(store) => store,
        Err(result) => return *result,
    };
    let state = match store.load() {
        Ok(state) => state,
        Err(error) => {
            return CommandResult::error(format!(
                "Marketplace state is fail-closed and will not be rewritten: {error}"
            ));
        }
    };
    if state.catalogs().is_empty() {
        return CommandResult::message(format!(
            "No marketplace catalogs are registered.\n{}\n\
             Reads a LOCAL catalog file; nothing is fetched over the network.",
            USAGE
        ));
    }
    let mut output = String::from("Marketplace catalogs:\n");
    for (name, entry) in state.catalogs() {
        output.push('\n');
        output.push_str(&render_catalog_summary(name, &entry.catalog));
        output.push_str(&render_candidates(app.ui_locale, &entry.catalog, false));
    }
    output.push_str(
        "\nTiers and provenance are display-only. Install with /plugin marketplace install <catalog> <candidate>; \
         installs go through the reviewed installer and start disabled and untrusted.",
    );
    CommandResult::message(output)
}

fn show(app: &mut App, name: &str) -> CommandResult {
    let store = match open_store(app) {
        Ok(store) => store,
        Err(result) => return *result,
    };
    let state = match store.load() {
        Ok(state) => state,
        Err(error) => {
            return CommandResult::error(format!(
                "Marketplace state is fail-closed and will not be rewritten: {error}"
            ));
        }
    };
    let Some(entry) = state.get(name) else {
        return CommandResult::error(format!(
            "No marketplace named `{}`. Use /plugin marketplace list.",
            escape_review_text(name)
        ));
    };
    let mut output = render_catalog_summary(name, &entry.catalog);
    output.push_str("\n  added from: ");
    let _ = writeln!(
        output,
        "{}",
        escape_review_path(Path::new(&entry.source_path))
    );
    output.push_str(&render_candidates(app.ui_locale, &entry.catalog, true));
    CommandResult::message(output)
}

fn remove(app: &mut App, name: &str) -> CommandResult {
    let store = match open_store(app) {
        Ok(store) => store,
        Err(result) => return *result,
    };
    match store.remove(name) {
        Ok(true) => CommandResult::message(format!(
            "Removed marketplace `{}`. Installed plugins and their trust state are unaffected.",
            escape_review_text(name)
        )),
        Ok(false) => CommandResult::error(format!(
            "No marketplace named `{}`. Use /plugin marketplace list.",
            escape_review_text(name)
        )),
        Err(error) => CommandResult::error(error),
    }
}

fn install(app: &mut App, catalog_name: &str, candidate_name: &str) -> CommandResult {
    let store = match open_store(app) {
        Ok(store) => store,
        Err(result) => return *result,
    };
    let state = match store.load() {
        Ok(state) => state,
        Err(error) => {
            return CommandResult::error(format!(
                "Marketplace state is fail-closed and will not be rewritten: {error}"
            ));
        }
    };
    let Some(entry) = state.get(catalog_name) else {
        return CommandResult::error(format!(
            "No marketplace named `{}`. Use /plugin marketplace list.",
            escape_review_text(catalog_name)
        ));
    };
    let Some(candidate) = entry.catalog.candidate_by_name(candidate_name) else {
        return CommandResult::error(format!(
            "No candidate `{}` in marketplace `{}`.",
            escape_review_text(candidate_name),
            escape_review_text(catalog_name)
        ));
    };
    if candidate.has_errors() {
        return CommandResult::error(format!(
            "Candidate `{}` has parse errors and cannot be installed:\n{}",
            escape_review_text(candidate_name),
            render_diagnostics_inline(&candidate.diagnostics)
        ));
    }
    let MarketplaceInstallPlan::Supported { spec, .. } = &candidate.install_plan else {
        let MarketplaceInstallPlan::Unsupported { reason, .. } = &candidate.install_plan else {
            unreachable!("is_supported and this match agree");
        };
        return CommandResult::error(format!(
            "Candidate `{}` cannot be installed by Codewhale: {}",
            escape_review_text(candidate_name),
            escape_review_text(&localized_marketplace_plan_text(app.ui_locale, reason))
        ));
    };
    let spec = spec.as_str();
    // Relative local paths resolve against the catalog document's own
    // directory, not the TUI's working directory.
    let spec = resolve_spec(&entry.source_path, &candidate.source, spec);
    super::install_bundle(app, &spec)
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

fn render_catalog_summary(name: &str, catalog: &MarketplaceCatalog) -> String {
    let mut out = String::new();
    let display = catalog
        .display_name
        .as_deref()
        .filter(|d| !d.trim().is_empty());
    let _ = writeln!(
        out,
        "`{}` — {} format, {} candidate(s), tier={} (display only)",
        escape_review_text(name),
        catalog.format,
        catalog.total_candidates(),
        catalog.provenance.tier
    );
    if let Some(display) = display {
        let _ = writeln!(out, "  display name: {}", escape_review_text(display));
    }
    if let Some(description) = catalog
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        let _ = writeln!(out, "  {}", escape_review_text(description));
    }
    if !catalog.diagnostics.is_empty() {
        let _ = writeln!(
            out,
            "  catalog diagnostics: {}",
            render_diagnostics_inline(&catalog.diagnostics)
        );
    }
    out
}

fn localized_marketplace_plan_text(locale: Locale, value: &str) -> std::borrow::Cow<'_, str> {
    match value {
        KIMI_ZIP_UNSUPPORTED_REASON => tr(locale, MessageId::PluginKimiMarketplaceZipUnsupported),
        KIMI_REMOTE_UNSUPPORTED_REASON => {
            tr(locale, MessageId::PluginKimiMarketplaceRemoteUnsupported)
        }
        KIMI_GZIP_TARBALL_SOURCE_KIND => tr(locale, MessageId::PluginKimiMarketplaceGzipTarball),
        _ => std::borrow::Cow::Borrowed(value),
    }
}

fn render_candidates(locale: Locale, catalog: &MarketplaceCatalog, detailed: bool) -> String {
    let mut out = String::new();
    for candidate in &catalog.candidates {
        let status = if candidate.has_errors() {
            "unusable"
        } else {
            "candidate"
        };
        let _ = write!(
            out,
            "  • {} [{}] — {}",
            escape_review_text(&candidate.name),
            status,
            candidate
                .display_name
                .as_deref()
                .map(escape_review_text)
                .as_deref()
                .unwrap_or("no display name")
        );
        if let Some(version) = &candidate.version {
            let _ = write!(out, " · v{}", escape_review_text(version));
        }
        let _ = write!(out, " · tier={}", candidate.provenance.tier);
        let _ = writeln!(out);
        let compatibility = candidate
            .compatibility
            .as_ref()
            .map(|c| c.as_str().to_string())
            .unwrap_or_else(|| "decided at install review".to_string());
        let _ = writeln!(out, "    compatibility: {compatibility}");
        match &candidate.install_plan {
            MarketplaceInstallPlan::Supported { source_kind, .. } => {
                let source_kind = localized_marketplace_plan_text(locale, source_kind);
                let _ = writeln!(
                    out,
                    "    installable via {source_kind}: /plugin marketplace install {} {}",
                    escape_review_text(catalog.id.as_str()),
                    escape_review_text(&candidate.name)
                );
            }
            MarketplaceInstallPlan::Unsupported { reason, .. } => {
                let reason = localized_marketplace_plan_text(locale, reason);
                let _ = writeln!(out, "    not installable: {}", escape_review_text(&reason));
            }
        }
        if detailed {
            if let Some(description) = candidate
                .description
                .as_deref()
                .filter(|d| !d.trim().is_empty())
            {
                let _ = writeln!(out, "    {}", escape_review_text(description));
            }
            if let Some(homepage) = &candidate.homepage {
                let _ = writeln!(out, "    homepage: {}", escape_review_text(homepage));
            }
            if let Some(repository) = &candidate.repository {
                let _ = writeln!(out, "    repository: {}", escape_review_text(repository));
            }
            if let Some(author) = &candidate.author {
                let _ = writeln!(out, "    author: {}", escape_review_text(author));
            }
            if let Some(license) = &candidate.license {
                let _ = writeln!(out, "    license: {}", escape_review_text(license));
            }
            if !candidate.keywords.is_empty() {
                let _ = writeln!(
                    out,
                    "    keywords: {}",
                    escape_review_text(&candidate.keywords.join(", "))
                );
            }
            if let Some(when) = &candidate.when {
                let _ = writeln!(out, "    when: {when:?}");
            }
        }
        if !candidate.diagnostics.is_empty() {
            let _ = writeln!(
                out,
                "    diagnostics: {}",
                render_diagnostics_inline(&candidate.diagnostics)
            );
        }
    }
    out
}

fn render_diagnostics_inline(
    diagnostics: &[crate::plugins::marketplace::types::MarketplaceDiagnostic],
) -> String {
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
                escape_review_text(&d.message)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod localized_plan_tests {
    use super::*;

    #[test]
    fn kimi_plan_codes_resolve_at_render_time() {
        let zip = localized_marketplace_plan_text(Locale::Es419, KIMI_ZIP_UNSUPPORTED_REASON);
        let remote = localized_marketplace_plan_text(Locale::Es419, KIMI_REMOTE_UNSUPPORTED_REASON);
        let gzip = localized_marketplace_plan_text(Locale::Es419, KIMI_GZIP_TARBALL_SOURCE_KIND);
        assert!(zip.contains("no admite paquetes ZIP"), "{zip}");
        assert!(remote.contains("deben terminar en .tar.gz"), "{remote}");
        assert_eq!(gzip, "URL de tarball gzip");
    }
}
