//! RepoMapEngine — top-level analysis engine built on top of RepoGraph.
//!
//! Provides PageRank computation, ranking, symbol lookup, call-chain
//! traversal, and entry-point / hotspot detection.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ranking::{FileMetrics, HotspotInfo, ModuleInfo, ReadingOrderEntry, SymbolSummary};
use crate::types::{RepoGraph, ScanStats, Symbol};

// ---------------------------------------------------------------------------
// Call-chain result types
// ---------------------------------------------------------------------------

/// One node in a call-chain listing.
#[derive(Debug, Clone)]
pub struct CallChainEntry {
    pub symbol_name: String,
    pub symbol_id: String,
    pub file: String,
    pub pagerank: f64,
}

/// Complete call-chain report for a single symbol.
#[derive(Debug, Clone)]
pub struct CallChainResult {
    pub symbol_name: String,
    pub symbol_file: String,
    pub symbol_kind: String,
    pub callers: Vec<CallChainEntry>,
    pub callees: Vec<CallChainEntry>,
}

// ---------------------------------------------------------------------------
// Entry-point candidate
// ---------------------------------------------------------------------------

/// A single entry-point candidate with a confidence score.
#[derive(Debug, Clone)]
pub struct EntryPoint {
    pub file_path: String,
    pub score: f64,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// RepoMapEngine
// ---------------------------------------------------------------------------

/// The main analysis engine, wrapping a [`RepoGraph`] and providing
/// ranking, query, and reporting methods.
pub struct RepoMapEngine {
    pub graph: RepoGraph,
    pub project_path: PathBuf,
    pub stats: ScanStats,
}

impl RepoMapEngine {
    /// Build a new engine, run PageRank, and pre-compute file-level
    /// aggregates.
    pub fn new(graph: RepoGraph, project_path: PathBuf, stats: ScanStats) -> Self {
        let mut engine = Self {
            graph,
            project_path,
            stats,
        };
        calculate_pagerank(&mut engine.graph, 0.85, 50);
        engine
    }

    // ------------------------------------------------------------------
    // Summary helpers
    // ------------------------------------------------------------------

    /// Return human-readable scan-statistic lines.
    pub fn scan_summary_lines(&self) -> Vec<String> {
        let s = &self.stats;
        vec![
            format!("Source files listed: {}", s.listed_source_files),
            format!("Source files selected: {}", s.selected_source_files),
            format!("Files processed: {}", s.processed_files),
            format!("Failed files: {}", s.failed_files.len()),
            format!("Scan duration: {} ms", s.scan_duration_ms),
        ]
    }

    /// Return a list of entry-point candidates sorted by confidence.
    pub fn entry_points(&self) -> Vec<EntryPoint> {
        let mut candidates: Vec<EntryPoint> = Vec::new();

        for file in self.graph.file_imports.keys() {
            // Entry-point heuristic: a file that is not imported by
            // anything else but does export symbols.
            let imported_by = self
                .graph
                .file_imports
                .iter()
                .filter(|(_, deps)| deps.contains(file))
                .count();

            let has_symbols = self
                .graph
                .file_symbols
                .get(file)
                .map(|ids| !ids.is_empty())
                .unwrap_or(false);

            if imported_by == 0 && has_symbols {
                candidates.push(EntryPoint {
                    file_path: file.clone(),
                    score: 1.0,
                    reason: "No incoming imports, has exported symbols".to_string(),
                });
                continue;
            }

            // Also flag files named main/index/entry.
            let lower = file.to_lowercase();
            let is_named_entry = lower.ends_with("main.ts")
                || lower.ends_with("main.rs")
                || lower.ends_with("main.py")
                || lower.ends_with("index.ts")
                || lower.ends_with("index.js")
                || lower.ends_with("index.tsx")
                || lower.ends_with("entry.ts")
                || lower.ends_with("entry.rs")
                || lower.ends_with("entry.py")
                || lower.ends_with("app.ts")
                || lower.ends_with("app.rs")
                || lower.ends_with("app.py")
                || lower.ends_with("lib.rs");

            if is_named_entry {
                candidates.push(EntryPoint {
                    file_path: file.clone(),
                    score: 0.9,
                    reason: "Conventional entry-point filename".to_string(),
                });
            }
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates
    }

    /// Return the top-N symbols ranked by PageRank.
    pub fn top_symbols(&self, n: usize) -> Vec<&Symbol> {
        let mut syms: Vec<&Symbol> = self.graph.symbols.values().collect();
        syms.sort_by(|a, b| b.pagerank.partial_cmp(&a.pagerank).unwrap_or(std::cmp::Ordering::Equal));
        syms.truncate(n);
        syms
    }

    /// Return files sorted by a combined importance score.
    pub fn suggested_reading_order(&self, max_results: usize) -> Vec<ReadingOrderEntry> {
        let file_scores = compute_file_pagerank(&self.graph);
        let mut entries: Vec<ReadingOrderEntry> = Vec::new();

        // Score each file that has at least one symbol.
        for (file, pr) in &file_scores {
            let sym_count = self
                .graph
                .file_symbols
                .get(file)
                .map(|v| v.len())
                .unwrap_or(0);
            if sym_count == 0 {
                continue;
            }

            // Boost files with many outgoing edges (explains a lot).
            let outgoing = self
                .graph
                .outgoing
                .values()
                .flat_map(|edges| edges.iter())
                .filter(|e| {
                    self.graph
                        .symbols
                        .get(&e.source)
                        .map(|s| s.file.as_str() == file.as_str())
                        .unwrap_or(false)
                })
                .count();

            let centrality = 1.0 + (outgoing as f64).ln_1p();
            let score = pr * centrality;

            let reason = if outgoing > 5 {
                format!("Core module with {} outgoing references", outgoing)
            } else {
                "Provides foundational types or utilities".to_string()
            };

            entries.push(ReadingOrderEntry {
                file_path: file.clone(),
                score,
                reason,
            });
        }

        entries.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        entries.truncate(max_results);
        entries
    }

    /// Aggregate per-directory statistics at the top level.
    pub fn module_summary(&self) -> Vec<ModuleInfo> {
        let mut dirs: HashMap<String, (usize, usize, usize)> = HashMap::new(); // (files, symbols, lines)

        for (file, sym_ids) in &self.graph.file_symbols {
            let dir = top_level_dir(file, &self.project_path);
            let entry = dirs.entry(dir).or_default();
            entry.0 += 1;
            entry.1 += sym_ids.len();

            // Approximate lines from symbol ranges.
            let file_lines: usize = sym_ids
                .iter()
                .filter_map(|id| self.graph.symbols.get(id))
                .map(|s| {
                    if s.end_line > s.line {
                        s.end_line - s.line
                    } else {
                        1
                    }
                })
                .sum();
            entry.2 += file_lines;
        }

        let mut modules: Vec<ModuleInfo> = dirs
            .into_iter()
            .map(|(dir, (fc, sc, lines))| ModuleInfo {
                directory: dir,
                file_count: fc,
                symbol_count: sc,
                lines,
            })
            .collect();

        modules.sort_by(|a, b| b.file_count.cmp(&a.file_count));
        modules
    }

    /// Return the top-N high-density files (hotspots).
    pub fn hotspots(&self, top_n: usize) -> Vec<HotspotInfo> {
        let mut hotspots: Vec<HotspotInfo> = Vec::new();

        for (file, sym_ids) in &self.graph.file_symbols {
            if sym_ids.is_empty() {
                continue;
            }

            // Estimate line count from the last symbol's end_line.
            let max_line = sym_ids
                .iter()
                .filter_map(|id| self.graph.symbols.get(id))
                .map(|s| s.end_line)
                .max()
                .unwrap_or(0);

            let line_count = max_line.max(1);
            let symbol_count = sym_ids.len();
            let density = symbol_count as f64 / line_count as f64 * 1000.0;

            // Aggregate PageRank and edge degree for this file.
            let mut file_pr = self.graph.symbols[self.graph.file_symbols[file].first().unwrap_or(&String::new())]
                .pagerank;
            if let Some(first_sym) = sym_ids.first().and_then(|id| self.graph.symbols.get(id)) {
                file_pr = first_sym.pagerank;
            }

            // Count all incoming + outgoing edges touching symbols in this file.
            let sym_set: HashSet<&str> = sym_ids.iter().map(|s| s.as_str()).collect();
            let mut edge_degree = 0usize;
            for edges in self.graph.outgoing.values() {
                for e in edges {
                    if sym_set.contains(e.source.as_str())
                        || sym_set.contains(e.target.as_str())
                    {
                        edge_degree += 1;
                    }
                }
            }
            let complexity = if edge_grade(edge_degree) > 0.0 {
                1.0 + edge_grade(edge_degree).ln_1p()
            } else {
                1.0
            };

            hotspots.push(HotspotInfo {
                file_path: file.clone(),
                density,
                complexity_score: complexity,
                pagerank: file_pr,
                line_count,
                symbol_count,
            });
        }

        hotspots.sort_by(|a, b| {
            b.density
                .partial_cmp(&a.density)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hotspots.truncate(top_n);
        hotspots
    }

    /// Return the top-N symbols summarised.
    pub fn summary_symbols(&self, max_symbols: usize) -> Vec<SymbolSummary> {
        self.top_symbols(max_symbols)
            .iter()
            .map(|s| SymbolSummary {
                name: s.name.clone(),
                kind: s.kind.clone(),
                file: s.file.clone(),
                pagerank: s.pagerank,
                signature: s.signature.clone(),
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Symbol & call-chain queries
    // ------------------------------------------------------------------

    /// Find all symbols whose name contains `name` (substring match).
    pub fn query_symbol(&self, name: &str) -> Vec<&Symbol> {
        let lower = name.to_lowercase();
        self.graph
            .symbols
            .values()
            .filter(|s| s.name.to_lowercase().contains(&lower))
            .collect()
    }

    /// Walk the call graph for `symbol_name` up to `max_depth` levels,
    /// returning callers (incoming) and callees (outgoing).
    pub fn call_chain(&self, symbol_name: &str, max_depth: usize) -> CallChainResult {
        // Find the symbol with highest PageRank matching the name.
        let matches = self.query_symbol(symbol_name);
        let target = match matches.into_iter().max_by(|a, b| {
            a.pagerank
                .partial_cmp(&b.pagerank)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Some(s) => s,
            None => {
                return CallChainResult {
                    symbol_name: symbol_name.to_string(),
                    symbol_file: String::new(),
                    symbol_kind: String::new(),
                    callers: Vec::new(),
                    callees: Vec::new(),
                };
            }
        };

        let target_id = &target.id;

        // --- Collect callers (incoming edges) ---
        let callers: Vec<CallChainEntry> = self
            .graph
            .incoming
            .get(target_id)
            .map(|edges| {
                let mut seen = HashSet::new();
                edges
                    .iter()
                    .filter(|e| seen.insert(e.source.clone()))
                    .take(max_depth)
                    .filter_map(|e| {
                        self.graph
                            .symbols
                            .get(&e.source)
                            .map(|s| CallChainEntry {
                                symbol_name: s.name.clone(),
                                symbol_id: s.id.clone(),
                                file: s.file.clone(),
                                pagerank: s.pagerank,
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // --- Collect callees (outgoing edges) ---
        let callees: Vec<CallChainEntry> = self
            .graph
            .outgoing
            .get(target_id)
            .map(|edges| {
                let mut seen = HashSet::new();
                edges
                    .iter()
                    .filter(|e| seen.insert(e.target.clone()))
                    .take(max_depth)
                    .filter_map(|e| {
                        self.graph
                            .symbols
                            .get(&e.target)
                            .map(|s| CallChainEntry {
                                symbol_name: s.name.clone(),
                                symbol_id: s.id.clone(),
                                file: s.file.clone(),
                                pagerank: s.pagerank,
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();

        CallChainResult {
            symbol_name: target.name.clone(),
            symbol_file: target.file.clone(),
            symbol_kind: target.kind.clone(),
            callers,
            callees,
        }
    }

    /// Compute metrics for a single file.
    pub fn get_file_metrics(&self, file_path: &str) -> FileMetrics {
        let sym_ids = self
            .graph
            .file_symbols
            .get(file_path)
            .cloned()
            .unwrap_or_default();
        let symbol_count = sym_ids.len();

        let max_line = sym_ids
            .iter()
            .filter_map(|id| self.graph.symbols.get(id))
            .map(|s| s.end_line)
            .max()
            .unwrap_or(0);

        let avg_pr: f64 = if symbol_count > 0 {
            sym_ids
                .iter()
                .filter_map(|id| self.graph.symbols.get(id))
                .map(|s| s.pagerank)
                .sum::<f64>()
                / symbol_count as f64
        } else {
            0.0
        };

        FileMetrics {
            file_path: file_path.to_string(),
            lines: max_line,
            symbols: symbol_count,
            complexity: 1.0 + (symbol_count as f64).ln_1p(),
            pagerank: avg_pr,
        }
    }
}

// ---------------------------------------------------------------------------
// PageRank
// ---------------------------------------------------------------------------

/// Compute PageRank for every symbol in the graph using the standard
/// power-iteration method.
///
/// `damping` is the teleport probability (typically 0.85).  `max_iter`
/// caps the number of iterations.
pub fn calculate_pagerank(graph: &mut RepoGraph, damping: f64, max_iter: usize) {
    let n = graph.symbols.len();
    if n == 0 {
        return;
    }

    // Pre-compute out-degree for each node.
    let out_deg: HashMap<&str, usize> = graph
        .outgoing
        .iter()
        .map(|(k, v)| (k.as_str(), v.len()))
        .collect();

    let keys: Vec<String> = graph.symbols.keys().cloned().collect();
    let keys_str: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
    let uniform = 1.0 / n as f64;
    let mut rank: HashMap<&str, f64> = keys_str.iter().map(|k| (*k, uniform)).collect();
    let mut next: HashMap<&str, f64> = HashMap::with_capacity(n);

    for _ in 0..max_iter {
        next.clear();
        let mut diff = 0.0_f64;

        // Dangling-node mass (nodes with no outgoing edges).
        let dangling = rank
            .iter()
            .filter(|(k, _)| out_deg.get(*k).map(|d| *d == 0).unwrap_or(true))
            .map(|(_, v)| *v)
            .sum::<f64>()
            * damping
            / n as f64;

        let base = (1.0 - damping) / n as f64;

        for node_id in &keys_str {
            let mut pr = base + dangling;

            // Distribute rank from incoming edges.
            if let Some(incoming) = graph.incoming.get(*node_id) {
                for edge in incoming {
                    let src = edge.source.as_str();
                    let deg = out_deg.get(src).copied().unwrap_or(1).max(1);
                    pr += damping * rank.get(src).copied().unwrap_or(0.0) / deg as f64;
                }
            }

            next.insert(*node_id, pr);
            diff += (pr - rank.get(node_id).copied().unwrap_or(0.0)).abs();
        }

        std::mem::swap(&mut rank, &mut next);

        // Convergence threshold.
        if diff < 1e-8 {
            break;
        }
    }

    // Write ranks back into symbols.
    // Collect target IDs first to avoid borrow conflicts.
    let target_ranks: Vec<(String, f64)> = graph
        .symbols
        .keys()
        .map(|id| {
            let pr = rank.get(id.as_str()).copied().unwrap_or(0.0);
            (id.clone(), pr)
        })
        .collect();
    for (id, pr) in target_ranks {
        if let Some(symbol) = graph.symbols.get_mut(&id) {
            symbol.pagerank = pr;
        }
    }
}

// ---------------------------------------------------------------------------
// File-level PageRank aggregate
// ---------------------------------------------------------------------------

/// Aggregate per-symbol PageRank values to the file level.
///
/// Returns the mean PR of all symbols in each file.
pub fn compute_file_pagerank(graph: &RepoGraph) -> HashMap<String, f64> {
    let mut file_pr: HashMap<String, (f64, usize)> = HashMap::new();

    for symbol in graph.symbols.values() {
        let entry = file_pr.entry(symbol.file.clone()).or_default();
        entry.0 += symbol.pagerank;
        entry.1 += 1;
    }

    file_pr
        .into_iter()
        .map(|(f, (sum, count))| (f, sum / count.max(1) as f64))
        .collect()
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Extract the top-level directory name from a file path relative to
/// `project_root`.
fn top_level_dir(file_path: &str, _project_root: &Path) -> String {
    let rel = Path::new(file_path);
    // If the path is already relative, it may not contain project_root prefix.
    // Try to get the first component.
    let first = rel
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "root".to_string());

    if first == "." || first.is_empty() {
        "root".to_string()
    } else {
        first
    }
}

/// Convert an edge count to a simple numeric grade for complexity
/// scoring.
fn edge_grade(count: usize) -> f64 {
    if count > 50 {
        100.0
    } else if count > 20 {
        50.0
    } else if count > 10 {
        20.0
    } else if count > 5 {
        10.0
    } else {
        count as f64
    }
}
