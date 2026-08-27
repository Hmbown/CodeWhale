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
//!
//! FEAT-020: the marketplace store/parse/install machinery runs host-side in
//! the TUI adapter; the handler consumes portable marketplace values and
//! renders them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use codewhale_command_contract::facets::{
    CommandPluginContext, CommandPresentationContext, PluginMarketplaceCatalog,
    PluginMarketplaceInstallPlan,
};

use crate::commands::CommandResult;

const USAGE: &str = "Usage: /plugin marketplace add|list|show|remove|install\n\
     \x20 add <name> <path>    read a local catalog file (kimi/claude/codex/codewhale)\n\
     \x20 list                 show catalogs and their candidates\n\
     \x20 show <name>          one catalog in detail\n\
     \x20 remove <name>        forget a catalog (installed plugins unaffected)\n\
     \x20 install <catalog> <candidate>  install via the reviewed installer";

pub(super) fn dispatch(
    presentation: &mut dyn CommandPresentationContext,
    plugin: &mut dyn CommandPluginContext,
    words: &[&str],
) -> CommandResult {
    match words {
        [] | ["list"] => list(presentation, plugin),
        ["add", name, path] => add(presentation, plugin, name, path),
        ["show", name] => show(presentation, plugin, name),
        ["remove", name] => remove(presentation, plugin, name),
        ["install", catalog, candidate] => install(presentation, plugin, catalog, candidate),
        _ => CommandResult::error(USAGE),
    }
}

fn add(
    _presentation: &mut dyn CommandPresentationContext,
    plugin: &mut dyn CommandPluginContext,
    name: &str,
    raw_path: &str,
) -> CommandResult {
    let path = PathBuf::from(raw_path.trim());
    let path = if path.is_absolute() {
        path
    } else {
        PathBuf::from(".").join(path)
    };
    match plugin.marketplace_add(name, &path) {
        Ok(receipt) => {
            let summary = render_catalog_summary(name, &receipt.catalog);
            CommandResult::message(format!(
                "Added marketplace `{}` ({} candidate(s), {} warning(s)).\n{summary}\n\
                 Tiers and provenance are display-only. Nothing was installed, trusted, or enabled.",
                escape_review_text(name),
                receipt.candidate_count,
                receipt.warning_count,
            ))
        }
        Err(error) => CommandResult::error(error),
    }
}

fn list(
    presentation: &mut dyn CommandPresentationContext,
    plugin: &dyn CommandPluginContext,
) -> CommandResult {
    let state = match plugin.marketplace_state() {
        Ok(state) => state,
        Err(error) => {
            return CommandResult::error(format!(
                "Marketplace state is fail-closed and will not be rewritten: {error}"
            ));
        }
    };
    let mut output = String::from("Marketplace catalogs:\n");
    output.push('\n');
    output.push_str(&render_catalog_summary("official", &state.official));
    output.push_str("  built into this Codewhale release; nothing is downloaded\n");
    output.push_str(&render_candidates(presentation, &state.official, false));
    if state.stored.is_empty() {
        output.push_str(&format!(
            "\nNo other catalogs are registered.\n{USAGE}\n\
             `add` reads a LOCAL catalog file; nothing is fetched over the network.\n"
        ));
    }
    for catalog in &state.stored {
        output.push('\n');
        output.push_str(&render_catalog_summary(&catalog.id, catalog));
        output.push_str(&render_candidates(presentation, catalog, false));
    }
    output.push_str(
        "\nTiers and provenance are display-only. Install with /plugin marketplace install <catalog> <candidate>; \
         installs go through the reviewed installer and start disabled and untrusted.",
    );
    CommandResult::message(output)
}

fn show(
    presentation: &mut dyn CommandPresentationContext,
    plugin: &dyn CommandPluginContext,
    name: &str,
) -> CommandResult {
    let state = match plugin.marketplace_state() {
        Ok(state) => state,
        Err(error) => {
            return CommandResult::error(format!(
                "Marketplace state is fail-closed and will not be rewritten: {error}"
            ));
        }
    };
    let catalog = if name == "official" {
        Some(&state.official)
    } else {
        state.stored.iter().find(|catalog| catalog.id == name)
    };
    let Some(catalog) = catalog else {
        return CommandResult::error(format!(
            "No marketplace named `{}`. Use /plugin marketplace list.",
            escape_review_text(name)
        ));
    };
    let mut output = render_catalog_summary(name, catalog);
    output.push_str("\n  added from: ");
    let _ = writeln!(
        output,
        "{}",
        escape_review_path(Path::new(catalog.source_path.as_deref().unwrap_or(name)))
    );
    output.push_str(&render_candidates(presentation, catalog, true));
    CommandResult::message(output)
}

fn remove(
    _presentation: &mut dyn CommandPresentationContext,
    plugin: &mut dyn CommandPluginContext,
    name: &str,
) -> CommandResult {
    if name == "official" {
        return CommandResult::error("`official` is built into Codewhale and cannot be removed.");
    }
    match plugin.marketplace_remove(name) {
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

fn install(
    presentation: &mut dyn CommandPresentationContext,
    plugin: &mut dyn CommandPluginContext,
    catalog_name: &str,
    candidate_name: &str,
) -> CommandResult {
    match plugin.marketplace_install(catalog_name, candidate_name) {
        Ok(receipt) => {
            use codewhale_command_contract::facets::PluginMutationOutcome;
            match receipt.outcome {
                PluginMutationOutcome::Installed => {
                    // Marketplace installs route through the same reviewed
                    // installer as `/plugin install`: the result is disabled
                    // and untrusted and drops into the trust review.
                    let name = receipt.name;
                    let mut output = format!(
                        "Installed plugin '{name}' from marketplace `{}`.\n\
                         It is disabled and untrusted. Review its requested authority below, then trust and enable it.\n",
                        escape_review_text(catalog_name)
                    );
                    if let Some(review) = super::review_bundle(presentation, plugin, &name).message
                    {
                        output.push('\n');
                        output.push_str(&review);
                    }
                    CommandResult::with_message_and_action(
                        output,
                        crate::tui::app::AppAction::PluginRegistryChanged,
                    )
                }
                PluginMutationOutcome::NeedsApproval(host) => {
                    CommandResult::error(needs_approval_message(&host))
                }
                PluginMutationOutcome::NetworkDenied(host) => {
                    CommandResult::error(network_denied_message(&host))
                }
                _ => CommandResult::message(format!(
                    "Installed `{}` from marketplace `{}`.",
                    escape_review_text(candidate_name),
                    escape_review_text(catalog_name)
                )),
            }
        }
        Err(error) => CommandResult::error(error),
    }
}

fn needs_approval_message(host: &str) -> String {
    format!(
        "Network policy requires approval for {host}.\n\
         Add it to your allow list with `/network allow {host}` (or set [network].default = \"allow\" in ~/.codewhale/config.toml), then retry."
    )
}

fn network_denied_message(host: &str) -> String {
    format!(
        "Network policy denied access to {host}.\n\
         Remove the deny entry from ~/.codewhale/config.toml under [network] or contact your administrator."
    )
}

fn render_catalog_summary(name: &str, catalog: &PluginMarketplaceCatalog) -> String {
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
        catalog.total_candidates,
        catalog.tier
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

fn render_candidates(
    presentation: &mut dyn CommandPresentationContext,
    catalog: &PluginMarketplaceCatalog,
    detailed: bool,
) -> String {
    let mut out = String::new();
    for candidate in &catalog.candidates {
        let status = if candidate.has_errors {
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
        let _ = write!(out, " · tier={}", candidate.tier);
        let _ = writeln!(out);
        let compatibility = candidate
            .compatibility
            .clone()
            .unwrap_or_else(|| "decided at install review".to_string());
        let _ = writeln!(out, "    compatibility: {compatibility}");
        match &candidate.install_plan {
            PluginMarketplaceInstallPlan::Supported {
                spec: _,
                source_kind,
            } => {
                let source_kind = localized_plan_text(presentation, source_kind);
                let _ = writeln!(
                    out,
                    "    installable via {source_kind}: /plugin marketplace install {} {}",
                    escape_review_text(&catalog.id),
                    escape_review_text(&candidate.name)
                );
            }
            PluginMarketplaceInstallPlan::Unsupported { reason } => {
                let reason = localized_plan_text(presentation, reason);
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
                let _ = writeln!(out, "    when: {when}");
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

/// Resolve a marketplace plan code through the presentation facet, falling
/// back to the raw code when unknown (mirrors the legacy localized plan text).
fn localized_plan_text(presentation: &mut dyn CommandPresentationContext, value: &str) -> String {
    presentation
        .translate(value, &[])
        .unwrap_or_else(|_| value.to_string())
}

fn render_diagnostics_inline(
    diagnostics: &[codewhale_command_contract::facets::PluginDiagnostic],
) -> String {
    diagnostics
        .iter()
        .map(|d| {
            format!(
                "{} {}: {}",
                match d.level {
                    codewhale_command_contract::facets::PluginDiagnosticLevel::Error => "error",
                    codewhale_command_contract::facets::PluginDiagnosticLevel::Warning => {
                        "warning"
                    }
                },
                d.code,
                escape_review_text(&d.message)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

pub(super) fn escape_review_text(value: &str) -> String {
    super::escape_review_text(value)
}

pub(super) fn escape_review_path(path: &Path) -> String {
    super::escape_review_path(path)
}
