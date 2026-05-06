//! AI-friendly Markdown report rendering for the DeepMap analysis engine.
//!
//! All public functions take [`RepoMapEngine`] and produce Markdown
//! strings that are truncated to a caller-specified character limit.

use std::collections::{HashMap, HashSet};

use crate::engine::RepoMapEngine;
use crate::topic;
use crate::types::Symbol;

// ===========================================================================
// Section rendering — public API
// ===========================================================================

/// Full project overview report.
///
/// Sections:
/// 1. Project Map (title + metadata)
/// 2. Scan Statistics
/// 3. Entry Points (file list)
/// 4. Recommended Reading Order (scored entries)
/// 5. Module Summary (top-level directories)
/// 6. Hot Spots (high-density files)
/// 7. Key Symbols (grouped by file)
pub fn render_overview_report(engine: &RepoMapEngine, max_chars: usize) -> String {
    let mut sections: Vec<String> = Vec::new();

    // -- Title --
    sections.push("# Project Map\n".to_string());

    // -- Scan Statistics --
    sections.push("## Scan Statistics\n".to_string());
    let summary_lines = engine.scan_summary_lines();
    for line in &summary_lines {
        sections.push(format!("- {}\n", line));
    }
    sections.push(String::new());

    // -- Entry Points --
    sections.push("## Entry Points\n".to_string());
    let entry_points = engine.entry_points();
    if entry_points.is_empty() {
        sections.push("_No entry points detected._\n".to_string());
    } else {
        for ep in &entry_points {
            sections.push(format!("- `{}`  _{}_\n", ep.file_path, ep.reason));
        }
    }
    sections.push(String::new());

    // -- Recommended Reading Order --
    sections.push("## Recommended Reading Order\n".to_string());
    let reading_order = engine.suggested_reading_order(20);
    if reading_order.is_empty() {
        sections.push("_No suggestions available._\n".to_string());
    } else {
        for (i, entry) in reading_order.iter().enumerate() {
            sections.push(format!(
                "{}. `{}` (score: {:.2})  {}\n",
                i + 1,
                entry.file_path,
                entry.score,
                entry.reason
            ));
        }
    }
    sections.push(String::new());

    // -- Module Summary --
    sections.push("## Module Summary\n".to_string());
    let modules = engine.module_summary();
    if modules.is_empty() {
        sections.push("_No modules detected._\n".to_string());
    } else {
        sections.push("| Directory | Files | Symbols | Lines |\n".to_string());
        sections.push("|-----------|-------|---------|-------|\n".to_string());
        for m in &modules {
            sections.push(format!(
                "| {} | {} | {} | {} |\n",
                m.directory, m.file_count, m.symbol_count, m.lines
            ));
        }
    }
    sections.push(String::new());

    // -- Hot Spots --
    sections.push("## Hot Spots\n".to_string());
    let hotspots = engine.hotspots(10);
    if hotspots.is_empty() {
        sections.push("_No hotspots detected._\n".to_string());
    } else {
        for h in &hotspots {
            sections.push(format!(
                "- `{}`: density {:.2}, complexity {:.2}, PR {:.4}\n",
                h.file_path, h.density, h.complexity_score, h.pagerank
            ));
        }
    }
    sections.push(String::new());

    // -- Key Symbols (grouped by file) --
    sections.push("## Key Symbols\n".to_string());
    let syms = engine.summary_symbols(30);
    if syms.is_empty() {
        sections.push("_No symbols found._\n".to_string());
    } else {
        // Group by file.
        let mut by_file: HashMap<&str, Vec<&crate::ranking::SymbolSummary>> = HashMap::new();
        for s in &syms {
            by_file.entry(s.file.as_str()).or_default().push(s);
        }
        let mut files: Vec<&&str> = by_file.keys().collect();
        files.sort();
        let empty: Vec<&crate::ranking::SymbolSummary> = Vec::new();
        for file in files {
            let entries = by_file.get(*file).unwrap_or(&empty);
            sections.push(format!("### {}\n", file));
            for s in entries {
                sections.push(format!(
                    "- `{}` ({}), PR: {:.4}, sig: `{}`\n",
                    s.name, s.kind, s.pagerank, s.signature
                ));
            }
        }
    }

    truncate(&sections.concat(), max_chars)
}

/// Call-chain report for a single symbol.
///
/// Uses `query_symbol` to find candidates and `call_chain` to walk
/// callers and callees.
pub fn render_call_chain_report(
    engine: &RepoMapEngine,
    symbol_name: &str,
    max_depth: usize,
) -> String {
    let mut out = String::new();

    let matches = engine.query_symbol(symbol_name);
    if matches.is_empty() {
        out.push_str(&format!(
            "## Call Chain: `{}`\n\n_No symbol found._\n",
            symbol_name
        ));
        return out;
    }

    let chain = engine.call_chain(symbol_name, max_depth);

    out.push_str(&format!(
        "## Call Chain: `{}`\n\n- **Kind**: {}\n- **File**: `{}`\n- **PR Score**: {:.4}\n\n",
        chain.symbol_name, chain.symbol_kind, chain.symbol_file,
        matches
            .iter()
            .find(|s| s.name == chain.symbol_name)
            .map(|s| s.pagerank)
            .unwrap_or(0.0)
    ));

    // Callers.
    out.push_str(&format!("### Callers ({} found)\n", chain.callers.len()));
    if chain.callers.is_empty() {
        out.push_str("_None._\n");
    } else {
        for c in &chain.callers {
            out.push_str(&format!(
                "- `{}`  file: `{}`  PR: {:.4}\n",
                c.symbol_name, c.file, c.pagerank
            ));
        }
    }
    out.push('\n');

    // Callees.
    out.push_str(&format!("### Callees ({} found)\n", chain.callees.len()));
    if chain.callees.is_empty() {
        out.push_str("_None._\n");
    } else {
        for c in &chain.callees {
            out.push_str(&format!(
                "- `{}`  file: `{}`  PR: {:.4}\n",
                c.symbol_name, c.file, c.pagerank
            ));
        }
    }

    out
}

/// File detail report showing all symbols in a file with signature, PR
/// score, and visibility.
pub fn render_file_detail_report(
    engine: &RepoMapEngine,
    file_path: &str,
    max_symbols: usize,
    max_chars: usize,
) -> String {
    let mut out = format!("## File Detail: `{}`\n\n", file_path);

    let sym_ids = engine.graph.file_symbols.get(file_path);
    let sym_ids = match sym_ids {
        Some(ids) => ids,
        None => {
            out.push_str("_File not found in graph._\n");
            return truncate(&out, max_chars);
        }
    };

    // Collect symbols, sort by line number, limit.
    let mut symbols: Vec<&Symbol> = sym_ids
        .iter()
        .filter_map(|id| engine.graph.symbols.get(id))
        .collect();
    symbols.sort_by_key(|s| s.line);
    symbols.truncate(max_symbols);

    if symbols.is_empty() {
        out.push_str("_No symbols defined in this file._\n");
        return truncate(&out, max_chars);
    }

    out.push_str(&format!("Total symbols: {}\n\n", sym_ids.len()));
    out.push_str("| Line | Kind | Name | Visibility | PR Score | Signature |\n");
    out.push_str("|------|------|------|------------|----------|-----------|\n");

    for s in &symbols {
        out.push_str(&format!(
            "| {} | {} | `{}` | {} | {:.4} | `{}` |\n",
            s.line, s.kind, s.name, s.visibility, s.pagerank, s.signature
        ));
    }

    truncate(&out, max_chars)
}

/// Query / topic-search report.
///
/// Uses `topic::topic_score` to rank files and includes related tests
/// and key symbols.
pub fn render_query_report(
    engine: &RepoMapEngine,
    query: &str,
    max_files: usize,
    max_chars: usize,
) -> String {
    let mut out = format!("## Query Results: `{}`\n\n", query);

    // Gather candidate files.
    let candidate_files: Vec<String> = engine.graph.file_symbols.keys().cloned().collect();
    if candidate_files.is_empty() {
        out.push_str("_No files to search._\n");
        return truncate(&out, max_chars);
    }

    // Compute keyword weights.
    let query_tokens: Vec<String> = topic::split_identifier(query);
    let keyword_weights = topic::compute_keyword_weights(&query_tokens, &candidate_files, &engine.graph);

    // Score all files.
    let mut scored: Vec<(&str, f64)> = candidate_files
        .iter()
        .map(|f| {
            let score = topic::topic_score(query, f, &engine.graph, &keyword_weights);
            (f.as_str(), score)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_files);

    // Top Files section.
    out.push_str(&format!(
        "### Top Files ({} shown)\n\n",
        scored.len()
    ));
    if scored.is_empty() {
        out.push_str("_No matching files._\n");
    } else {
        for (i, (file, score)) in scored.iter().enumerate() {
            let role = topic::classify_file_role(file);
            out.push_str(&format!(
                "{}. `{}` (score: {:.2}, role: {})\n",
                i + 1,
                file,
                score,
                role
            ));
        }
    }
    out.push('\n');

    // Related Tests section.
    let target_files: Vec<String> = scored.iter().map(|(f, _)| f.to_string()).collect();
    let related = topic::find_related_tests(
        &target_files,
        &engine.graph,
        &engine.project_path,
    );
    out.push_str(&format!(
        "### Related Tests ({} found)\n\n",
        related.len()
    ));
    if related.is_empty() {
        out.push_str("_No related tests found._\n");
    } else {
        // Deduplicate top 10.
        let mut seen: HashSet<&str> = HashSet::new();
        let mut count = 0;
        for tm in &related {
            if seen.insert(tm.test_file.as_str()) {
                count += 1;
                if count > 10 {
                    break;
                }
                out.push_str(&format!(
                    "- `{}` -> `{}` (confidence: {:.2}, {})\n",
                    tm.test_file, tm.target_file, tm.confidence, tm.reason
                ));
            }
        }
    }
    out.push('\n');

    // Key Symbols section (highlight matches).
    out.push_str("### Key Symbols\n\n");
    let all_syms = engine.summary_symbols(30);
    let query_lower = query.to_lowercase();
    let matched_syms: Vec<&crate::ranking::SymbolSummary> = all_syms
        .iter()
        .filter(|s| {
            s.name.to_lowercase().contains(&query_lower)
                || s.signature.to_lowercase().contains(&query_lower)
        })
        .collect();

    if matched_syms.is_empty() {
        out.push_str("_No matching symbols._\n");
    } else {
        out.push_str("| Name | Kind | File | PR |\n");
        out.push_str("|------|------|------|----|\n");
        for s in &matched_syms {
            out.push_str(&format!(
                "| `{}` ★ | {} | `{}` | {:.4} |\n",
                s.name, s.kind, s.file, s.pagerank
            ));
        }
    }

    truncate(&out, max_chars)
}

/// Impact report for a set of changed files.
///
/// Lists each changed file, its dependents (files that import it), and
/// a summary of how many files are transitively affected.
pub fn render_impact_report(
    engine: &RepoMapEngine,
    target_files: &[String],
    max_chars: usize,
) -> String {
    let mut out = String::from("## Impact Analysis\n\n");

    if target_files.is_empty() {
        out.push_str("_No target files provided._\n");
        return truncate(&out, max_chars);
    }

    let mut all_affected: HashSet<String> = HashSet::new();

    for target in target_files {
        out.push_str(&format!("### Changed File: `{}`\n\n", target));

        // Find direct dependents (files that import this one).
        let mut dependents: Vec<String> = Vec::new();
        for (file, imports) in &engine.graph.file_imports {
            let target_lower = target.to_lowercase();
            if imports.iter().any(|imp| {
                imp.to_lowercase().contains(&target_lower)
                    || target_lower.contains(&imp.to_lowercase())
            }) {
                dependents.push(file.clone());
                all_affected.insert(file.clone());
            }
        }

        // Also check incoming edges from symbol level.
        if let Some(sym_ids) = engine.graph.file_symbols.get(target) {
            for sym_id in sym_ids {
                if let Some(incoming) = engine.graph.incoming.get(sym_id) {
                    for edge in incoming {
                        if let Some(sym) = engine.graph.symbols.get(&edge.source) {
                            if sym.file != *target {
                                dependents.push(sym.file.clone());
                                all_affected.insert(sym.file.clone());
                            }
                        }
                    }
                }
            }
        }

        dependents.sort();
        dependents.dedup();

        if dependents.is_empty() {
            out.push_str("_No direct dependents found._\n\n");
        } else {
            out.push_str(&format!(
                "Direct dependents ({}):\n",
                dependents.len()
            ));
            for d in &dependents {
                out.push_str(&format!("- `{}`\n", d));
            }
            out.push('\n');
        }

        // File metrics.
        let metrics = engine.get_file_metrics(target);
        out.push_str(&format!(
            "- Lines: {}\n- Symbols: {}\n- Complexity: {:.2}\n- PageRank: {:.4}\n\n",
            metrics.lines, metrics.symbols, metrics.complexity, metrics.pagerank
        ));
    }

    out.push_str(&format!(
        "### Summary\n- **Changed files**: {}\n- **Transitively affected files**: {}\n",
        target_files.len(),
        all_affected.len()
    ));

    truncate(&out, max_chars)
}

/// Diff / risk report for a list of changed files.
///
/// Combines impact analysis with risk assessment and verification
/// suggestions.
pub fn render_diff_risk_report(
    engine: &RepoMapEngine,
    changed_files: &[String],
    max_chars: usize,
) -> String {
    let mut out = String::from("## Diff Risk Assessment\n\n");

    // --- Risk assessment ---
    let (level, reasons) = assess_risk(changed_files);
    out.push_str(&format!("**Risk Level**: {}\n\n", level));

    if !reasons.is_empty() {
        out.push_str("Risk reasons:\n");
        for r in &reasons {
            out.push_str(&format!("- {}\n", r));
        }
        out.push('\n');
    }

    // --- Impact ---
    let mut all_affected: HashSet<String> = HashSet::new();
    for target in changed_files {
        // Symbol-level dependents via incoming edges.
        if let Some(sym_ids) = engine.graph.file_symbols.get(target) {
            for sym_id in sym_ids {
                if let Some(incoming) = engine.graph.incoming.get(sym_id) {
                    for edge in incoming {
                        if let Some(sym) = engine.graph.symbols.get(&edge.source) {
                            if sym.file != *target {
                                all_affected.insert(sym.file.clone());
                            }
                        }
                    }
                }
            }
        }
        // Import-level dependents.
        for (file, imports) in &engine.graph.file_imports {
            if imports.iter().any(|imp| {
                imp.to_lowercase() == target.to_lowercase()
                    || imp.to_lowercase().contains(
                        &Path::new(target)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_lowercase(),
                    )
            }) {
                all_affected.insert(file.clone());
            }
        }
    }

    out.push_str("### Affected Files\n\n");
    let mut affected_sorted: Vec<&String> = all_affected.iter().collect();
    affected_sorted.sort();
    if affected_sorted.is_empty() {
        out.push_str("_No affected files detected beyond the changed set._\n\n");
    } else {
        for f in &affected_sorted {
            out.push_str(&format!("- `{}`\n", f));
        }
        out.push('\n');
    }

    // --- Changed files detail ---
    out.push_str("### Changed Files Detail\n\n");
    for f in changed_files {
        let metrics = engine.get_file_metrics(f);
        out.push_str(&format!(
            "- `{}`: {} symbols, {} lines, complexity {:.2}\n",
            f, metrics.symbols, metrics.lines, metrics.complexity
        ));
    }
    out.push('\n');

    // --- Verification suggestions ---
    out.push_str("### Suggested Verification\n\n");
    out.push_str("1. Run existing tests for affected files:\n");
    let test_files: Vec<String> = changed_files
        .iter()
        .chain(all_affected.iter())
        .map(|f| f.clone())
        .collect();
    let related_tests = topic::find_related_tests(&test_files, &engine.graph, &engine.project_path);
    if related_tests.is_empty() {
        out.push_str("   _No specific test files identified._\n");
    } else {
        let mut seen: HashSet<&str> = HashSet::new();
        for tm in &related_tests {
            if seen.insert(tm.test_file.as_str()) {
                out.push_str(&format!("   - `{}`\n", tm.test_file));
            }
        }
    }
    out.push('\n');

    out.push_str("2. Suggested test commands:\n");
    out.push_str("   ```bash\n");
    out.push_str("   cargo test\n");
    out.push_str("   ```\n");

    truncate(&out, max_chars)
}

// ===========================================================================
// Private helpers
// ===========================================================================

/// Truncate `text` to at most `max_chars` characters, preserving whole
/// words when possible (stops at the last space before the limit).
fn truncate(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }

    let truncated = &text[..max_chars];
    // Try to break at a space boundary.
    if let Some(last_space) = truncated.rfind(' ') {
        format!("{}\n..._truncated_", &truncated[..last_space])
    } else {
        format!("{}\n..._truncated_", truncated)
    }
}

use std::path::Path;

/// Assess the risk level of a set of changed files based on keyword
/// patterns in their paths.
///
/// Scoring:
/// - `auth`, `login`, `token` => +3 each
/// - `db`, `sql`             => +3 each
/// - `config`                => +2 each
///
/// Returns `("low" | "medium" | "high" | "critical", reasons)`.
fn assess_risk(files: &[String]) -> (&'static str, Vec<String>) {
    let mut score = 0usize;
    let mut reasons: Vec<String> = Vec::new();

    for f in files {
        let lower = f.to_lowercase();

        // Security-sensitive keywords (+3).
        for kw in &["auth", "login", "token"] {
            if lower.contains(kw) {
                let count = lower.matches(kw).count();
                for _ in 0..count {
                    reasons.push(format!(
                        "`{}` contains security keyword '{}'",
                        f, kw
                    ));
                }
                score += count * 3;
            }
        }

        // Data-layer keywords (+3).
        for kw in &["db", "sql"] {
            if lower.contains(kw) {
                let count = lower.matches(kw).count();
                for _ in 0..count {
                    reasons.push(format!(
                        "`{}` contains data-layer keyword '{}'",
                        f, kw
                    ));
                }
                score += count * 3;
            }
        }

        // Configuration keywords (+2).
        if lower.contains("config") {
            reasons.push(format!("`{}` contains configuration keyword", f));
            score += 2;
        }
    }

    reasons.dedup();

    let level = match score {
        0 => "low",
        1..=2 => "medium",
        3..=5 => "high",
        _ => "critical",
    };

    (level, reasons)
}
