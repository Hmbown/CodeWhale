//! RepoMapEngine — top-level analysis orchestrator with session caching,
//! file scanning, tree-sitter parsing, import/call resolution, and
//! PageRank-based ranking.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use ignore::WalkBuilder;

use crate::parser::TreeSitterAdapter;
use crate::ranking::{GraphAnalyzer, HotspotInfo, FileMetrics, ModuleInfo, ReadingOrderEntry, SymbolSummary};
use crate::resolver::ImportResolver;
use crate::types::*;

// ---------------------------------------------------------------------------
// Edge weight constants
// ---------------------------------------------------------------------------

const IMPORT_WEIGHT: f64 = 0.35;
const CALL_WEIGHT: f64 = 0.50;
const MAX_SCAN_TIME_SECS: f64 = 300.0;

// ---------------------------------------------------------------------------
// Session cache
// ---------------------------------------------------------------------------

/// Cached scan result for a single canonical workspace path.
#[derive(Clone)]
struct CachedScan {
    graph: RepoGraph,
    pagerank: HashMap<String, f64>,
}

/// Workspace-level cache keyed by canonical project root path.
/// Reuse scan results across multiple tool calls within the same process.
static SCAN_CACHE: std::sync::LazyLock<Mutex<HashMap<PathBuf, CachedScan>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Scan state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum ScanState {
    Idle,
    Scanning,
    Scanned,
}

// ---------------------------------------------------------------------------
// RepoMapEngine
// ---------------------------------------------------------------------------

/// The top-level analysis engine that owns the scanned dependency graph,
/// the tree-sitter parser, import resolver, and PageRank analyzer.
pub struct RepoMapEngine {
    /// Absolute or canonical path to the project root.
    pub project_root: PathBuf,
    /// Lazily initialised tree-sitter parser (behind a Mutex so that
    /// read-only methods can check parser availability).
    ts: Mutex<Option<TreeSitterAdapter>>,
    /// The scanned symbol-level dependency graph.
    pub graph: RepoGraph,
    /// File modification-time cache for fast re-scans.
    mtime_cache: HashMap<PathBuf, std::time::SystemTime>,
    /// Current scanning state.
    scan_state: ScanState,
    /// Maximum file size (in bytes) that the engine will read.
    max_file_bytes: u64,
    /// Statistics from the most recent scan.
    pub scan_stats: ScanStats,
    /// Import-path resolver (built after scanning).
    resolver: Option<ImportResolver>,
    /// PageRank analyser (built after scanning).
    analyzer: Option<GraphAnalyzer>,
}

impl RepoMapEngine {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a new engine for the given project root.
    ///
    /// The tree-sitter parser is loaded lazily on the first call to
    /// [`scan`].  `DEEPMAP_MAX_FILE_BYTES` env-var overrides the default
    /// 512 KB per-file limit.
    pub fn new(project_root: &Path) -> Self {
        let max_file_bytes = std::env::var("DEEPMAP_MAX_FILE_BYTES")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_FILE_BYTES);

        Self {
            project_root: project_root.to_path_buf(),
            ts: Mutex::new(None),
            graph: RepoGraph::default(),
            mtime_cache: HashMap::new(),
            scan_state: ScanState::Idle,
            max_file_bytes,
            scan_stats: ScanStats::default(),
            resolver: None,
            analyzer: None,
        }
    }

    /// Return a cached engine for `project_root` if one exists, otherwise
    /// scan and cache the result.
    ///
    /// The cache is keyed by the canonical path of `project_root` so that
    /// equivalent paths (e.g. `./foo` and `/absolute/path/to/foo`) map to
    /// the same entry.
    pub fn get_or_scan(project_root: &Path, max_files: usize, max_scan_secs: f64) -> Self {
        let canonical = match project_root.canonicalize() {
            Ok(p) => p,
            Err(_) => project_root.to_path_buf(),
        };

        // Check cache.
        {
            let cache = match SCAN_CACHE.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };

            if let Some(cached) = cache.get(&canonical) {
                let graph = cached.graph.clone();
                let pagerank = cached.pagerank.clone();
                let mut engine = Self {
                    project_root: canonical,
                    ts: Mutex::new(None),
                    graph,
                    mtime_cache: HashMap::new(),
                    scan_state: ScanState::Scanned,
                    max_file_bytes: DEFAULT_MAX_FILE_BYTES,
                    scan_stats: ScanStats::default(),
                    resolver: None,
                    analyzer: None,
                };
                // Restore PageRank scores into symbols.
                for (sym_id, score) in &pagerank {
                    if let Some(sym) = engine.graph.symbols.get_mut(sym_id) {
                        sym.pagerank = *score;
                    }
                }
                // Rebuild resolver from restored graph.
                engine.resolver = Some(ImportResolver::new(&engine.project_root, &engine.graph));
                // Rebuild analyzer with stored scores.
                let mut analyzer = GraphAnalyzer::new();
                analyzer.pagerank = pagerank.clone();
                engine.analyzer = Some(analyzer);
                return engine;
            }
        }

        // Cache miss — scan and store.
        let mut engine = Self::new(&canonical);
        engine.scan(max_files, max_scan_secs);

        let pagerank = engine
            .analyzer
            .as_ref()
            .map(|a| a.pagerank_scores().clone())
            .unwrap_or_default();

        if let Ok(mut cache) = SCAN_CACHE.lock() {
            cache.insert(
                canonical,
                CachedScan {
                    graph: engine.graph.clone(),
                    pagerank,
                },
            );
        }

        engine
    }

    /// Whether the engine has completed at least one scan successfully.
    pub fn is_scanned(&self) -> bool {
        self.scan_state == ScanState::Scanned
    }

    // -----------------------------------------------------------------------
    // Scanning
    // -----------------------------------------------------------------------

    /// Run a full scan: list source files, parse each file, resolve
    /// imports and calls, build the edge graph, and compute PageRank.
    ///
    /// If the parser has not been initialised yet it will be created
    /// lazily on the first call.
    pub fn scan(&mut self, max_files: usize, max_scan_secs: f64) {
        let start = Instant::now();
        let deadline = max_scan_secs.min(MAX_SCAN_TIME_SECS);
        self.scan_state = ScanState::Scanning;
        let mut stats = ScanStats::default();

        // --- initialise parser lazily ---
        {
            let mut guard = match self.ts.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            if guard.is_none() {
                *guard = Some(TreeSitterAdapter::new());
            }
        }
        let ts_guard = match self.ts.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        // SAFETY: we just ensured the parser exists above.
        let ts: &TreeSitterAdapter = ts_guard.as_ref().expect("parser initialised above");

        // --- list source files ---
        let all_files: Vec<PathBuf> = list_source_files(&self.project_root, max_files);
        stats.listed_source_files = all_files.len();

        // --- filter by language / parser availability ---
        let files: Vec<PathBuf> = all_files
            .into_iter()
            .filter(|path| {
                let ext = match path.extension().and_then(|e| e.to_str()) {
                    Some(e) => format!(".{}", e),
                    None => return false,
                };
                let lang = match ext_to_lang(&ext) {
                    Some(l) => l,
                    None => return false,
                };
                if ts.has_parser(lang) {
                    true
                } else {
                    stats.filtered_path_files += 1;
                    false
                }
            })
            .collect();
        stats.selected_source_files = files.len();

        // We are done with the parser guard for now; drop it so we can
        // use it mutably inside the file loop.
        drop(ts_guard);

        // --- file processing loop ---
        let mut graph = RepoGraph::default();
        let mut processed_count: usize = 0;

        for path in &files {
            if start.elapsed().as_secs_f64() >= deadline {
                stats.timeout_triggered = true;
                break;
            }

            // MTime check — restore unchanged files from old graph.
            let mtime = match path.metadata().and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let file_str = path.to_string_lossy().to_string();
            if let Some(cached_mtime) = self.mtime_cache.get(path) {
                if *cached_mtime == mtime {
                    // Restore symbols and metadata from the previous scan.
                    if let Some(sym_ids) = self.graph.file_symbols.get(&file_str) {
                        for sid in sym_ids {
                            if let Some(sym) = self.graph.symbols.get(sid) {
                                graph.symbols.insert(sid.clone(), sym.clone());
                            }
                        }
                        graph.file_symbols.insert(file_str.clone(), sym_ids.clone());
                    }
                    if let Some(imports) = self.graph.file_imports.get(&file_str) {
                        graph.file_imports.insert(file_str.clone(), imports.clone());
                    }
                    if let Some(calls) = self.graph.file_calls.get(&file_str) {
                        graph.file_calls.insert(file_str.clone(), calls.clone());
                    }
                    if let Some(bindings) = self.graph.file_import_bindings.get(&file_str) {
                        graph.file_import_bindings.insert(file_str.clone(), bindings.clone());
                    }
                    if let Some(exports) = self.graph.file_exports.get(&file_str) {
                        graph.file_exports.insert(file_str.clone(), exports.clone());
                    }
                    self.mtime_cache.insert(path.clone(), mtime);
                    processed_count += 1;
                    continue;
                }
            }

            // Size check.
            let file_size = match path.metadata() {
                Ok(meta) => meta.len(),
                Err(_) => continue,
            };
            if file_size > self.max_file_bytes {
                stats.filtered_large_files += 1;
                continue;
            }

            // Read content.
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => {
                    stats
                        .failed_files
                        .push(path.to_string_lossy().to_string());
                    continue;
                }
            };

            // Truncate oversize content.
            let max_content_bytes = self.max_file_bytes as usize;
            let truncated_content = if content.len() > max_content_bytes {
                stats.truncated_files += 1;
                &content[..max_content_bytes]
            } else {
                &content[..]
            };

            // Determine language from extension.
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => format!(".{}", e),
                None => continue,
            };
            let lang = match ext_to_lang(&ext) {
                Some(l) => l,
                None => continue,
            };

            // Parse.
            let file_str = path.to_string_lossy().to_string();
            let mut ts_guard = match self.ts.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let ts_mut = match ts_guard.as_mut() {
                Some(t) => t,
                None => continue,
            };

            match ts_mut.parse(path, truncated_content, lang) {
                Ok(()) => {
                    // Symbols.
                    for sym in ts_mut.symbols() {
                        let id = sym.id.clone();
                        graph
                            .file_symbols
                            .entry(file_str.clone())
                            .or_default()
                            .push(id.clone());
                        graph.symbols.insert(id, sym);
                    }

                    // Imports.
                    let imports = ts_mut.imports();
                    if !imports.is_empty() {
                        graph.file_imports.insert(file_str.clone(), imports);
                    }

                    // Calls.
                    let calls = ts_mut.calls();
                    if !calls.is_empty() {
                        graph.file_calls.insert(file_str.clone(), calls);
                    }

                    // JS/TS import bindings.
                    let bindings = ts_mut.import_bindings();
                    if !bindings.is_empty() {
                        graph
                            .file_import_bindings
                            .insert(file_str.clone(), bindings);
                    }

                    // JS/TS exports.
                    let exports = ts_mut.exports();
                    if !exports.is_empty() {
                        graph.file_exports.insert(file_str.clone(), exports);
                    }

                    // Update mtime cache.
                    self.mtime_cache.insert(path.clone(), mtime);

                    processed_count += 1;
                }
                Err(e) => {
                    stats
                        .failed_files
                        .push(format!("{}: {}", file_str, e));
                }
            }
            drop(ts_guard);
        }

        stats.processed_files = processed_count;

        // Clean stale mtime entries — run ONCE after the file loop.
        let current_paths: HashSet<PathBuf> = files.iter().cloned().collect();
        self.mtime_cache
            .retain(|p, _| current_paths.contains(p));

        // --- build resolver ---
        self.resolver = Some(ImportResolver::new(&self.project_root, &graph));
        self.graph = graph;

        // --- build edges ---
        Self::build_edges_internal(
            self.resolver.as_ref().expect("resolver just built"),
            &mut self.graph,
        );

        // --- compute PageRank ---
        self.calculate_pagerank();

        // --- finalise stats ---
        stats.scan_duration_ms = start.elapsed().as_millis() as u64;
        self.scan_stats = stats;
        self.scan_state = ScanState::Scanned;
    }

    // -----------------------------------------------------------------------
    // Edge construction
    // -----------------------------------------------------------------------

    /// Build call edges (weighted by CALL_WEIGHT) and import edges
    /// (weighted by IMPORT_WEIGHT) using the resolver.
    ///
    /// Iterates `file_calls` and `file_imports` by reference — no cloning
    /// of the original vectors.
    fn build_edges_internal(resolver: &ImportResolver, graph: &mut RepoGraph) {
        let mut dedup: HashSet<(String, String)> = HashSet::new();
        let mut pending: Vec<Edge> = Vec::new();

        // --- call edges ---
        for (file, calls) in &graph.file_calls {
            // Build local-name map from JS/TS import bindings.
            let local_names: HashMap<String, String> = graph
                .file_import_bindings
                .get(file)
                .map(|bindings| {
                    let mut map = HashMap::with_capacity(bindings.len());
                    for b in bindings {
                        map.insert(b.local_name.clone(), b.imported_name.clone());
                    }
                    map
                })
                .unwrap_or_default();

            for call in calls {
                let (ref call_name, call_line, ref call_kind) = *call;
                let target_id = match resolver.resolve_call_target(
                    file,
                    call_name,
                    call_line,
                    call_kind,
                    &local_names,
                ) {
                    Some(id) => id,
                    None => continue,
                };
                let source_id = match resolver.resolve_calling_symbol_with_graph(
                    file,
                    call_line,
                    graph,
                ) {
                    Some(id) => id,
                    None => continue,
                };

                let key = (source_id.clone(), target_id.clone());
                if dedup.insert(key) {
                    pending.push(Edge {
                        source: source_id,
                        target: target_id,
                        weight: CALL_WEIGHT,
                        kind: "call".to_string(),
                    });
                }
            }
        }

        // --- import edges ---
        for (file, imports) in &graph.file_imports {
            let source_sym_ids: Vec<String> = graph
                .file_symbols
                .get(file)
                .cloned()
                .unwrap_or_default();

            for import_path in imports {
                let targets = resolver.resolve_import_targets(file, import_path);
                for target_file in &targets {
                    let target_sym_ids = match graph.file_symbols.get(target_file) {
                        Some(ids) => ids,
                        None => continue,
                    };
                    // Use one representative per file to avoid O(N×M) Cartesian explosion.
                    // Import edges are a weak signal (weight 0.35); one edge per
                    // import path preserves the dependency relationship without
                    // flooding the graph with N×M edges.
                    if let (Some(src), Some(tgt)) = (source_sym_ids.first(), target_sym_ids.first()) {
                        let key = (src.clone(), tgt.clone());
                        if dedup.insert(key) {
                            pending.push(Edge {
                                source: src.clone(),
                                target: tgt.clone(),
                                weight: IMPORT_WEIGHT,
                                kind: "import".to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Insert into graph (outgoing + incoming).
        for edge in pending {
            let src = edge.source.clone();
            let tgt = edge.target.clone();
            graph.outgoing.entry(src).or_default().push(edge.clone());
            graph.incoming.entry(tgt).or_default().push(edge);
        }
    }

    // -----------------------------------------------------------------------
    // PageRank computation
    // -----------------------------------------------------------------------

    /// Compute PageRank scores for all symbols in the current graph and
    /// write them back into the symbol entries.
    fn calculate_pagerank(&mut self) {
        let mut analyzer = GraphAnalyzer::new();
        analyzer.calculate_pagerank(&self.graph, 0.85, 50, 1e-6);

        // Write scores back into symbols.
        let scores: Vec<(String, f64)> = analyzer
            .pagerank_scores()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for (sym_id, score) in &scores {
            if let Some(sym) = self.graph.symbols.get_mut(sym_id) {
                sym.pagerank = *score;
            }
        }

        self.analyzer = Some(analyzer);
    }

    // -----------------------------------------------------------------------
    // Parser query
    // -----------------------------------------------------------------------

    /// Whether the engine has a parser available for `lang`.
    ///
    /// If the parser has not been initialised yet this returns `false`.
    pub fn has_parser(&self, lang: &str) -> bool {
        match self.ts.lock() {
            Ok(guard) => guard
                .as_ref()
                .map(|t| t.has_parser(lang))
                .unwrap_or(false),
            Err(poisoned) => poisoned
                .into_inner()
                .as_ref()
                .map(|t| t.has_parser(lang))
                .unwrap_or(false),
        }
    }

    // -----------------------------------------------------------------------
    // Query interface — delegates to GraphAnalyzer
    // -----------------------------------------------------------------------

    /// Case-insensitive substring symbol search.
    pub fn query_symbol(&self, name: &str) -> Vec<&Symbol> {
        match self.analyzer.as_ref() {
            Some(a) => a.query_symbol(name, &self.graph),
            None => Vec::new(),
        }
    }

    /// BFS call-chain traversal returning depth-grouped symbols.
    pub fn call_chain(
        &self,
        symbol_id: &str,
        direction: &str,
        max_depth: usize,
    ) -> HashMap<String, Vec<Symbol>> {
        match self.analyzer.as_ref() {
            Some(a) => a.call_chain(symbol_id, direction, max_depth, &self.graph),
            None => HashMap::new(),
        }
    }

    /// Files with the highest `symbol_count * avg_pagerank`.
    pub fn hotspots(&self, limit: usize) -> Vec<HotspotInfo> {
        match self.analyzer.as_ref() {
            Some(a) => a.hotspots(limit, &self.graph),
            None => Vec::new(),
        }
    }

    /// Entry-point files detected by stem or path pattern.
    pub fn entry_points(&self) -> Vec<String> {
        match self.analyzer.as_ref() {
            Some(a) => a.entry_points(&self.graph),
            None => Vec::new(),
        }
    }

    /// Per-file analysis: symbol count, edge counts, average PageRank.
    pub fn file_analysis(&self) -> HashMap<String, FileMetrics> {
        match self.analyzer.as_ref() {
            Some(a) => a.file_analysis(&self.graph),
            None => HashMap::new(),
        }
    }

    /// Group symbols by top-level directory, sorted by total PageRank.
    pub fn module_summary(&self, limit: usize) -> Vec<ModuleInfo> {
        match self.analyzer.as_ref() {
            Some(a) => a.module_summary(limit, &self.graph),
            None => Vec::new(),
        }
    }

    /// Suggested reading order based on PageRank, symbol density, and
    /// entry-point boost.
    pub fn suggested_reading_order(&self, limit: usize) -> Vec<ReadingOrderEntry> {
        match self.analyzer.as_ref() {
            Some(a) => a.suggested_reading_order(limit, &self.graph),
            None => Vec::new(),
        }
    }

    /// Compact symbol summary for the top files.
    pub fn summary_symbols(&self, limit_files: usize, per_file: usize) -> Vec<SymbolSummary> {
        match self.analyzer.as_ref() {
            Some(a) => a.summary_symbols(limit_files, per_file, &self.graph),
            None => Vec::new(),
        }
    }

    /// Human-readable scan-statistic lines.
    pub fn scan_summary_lines(&self) -> Vec<String> {
        let s = &self.scan_stats;
        let mut lines = Vec::new();

        lines.push(format!("Scan completed in {} ms", s.scan_duration_ms));
        lines.push(format!("Listed source files: {}", s.listed_source_files));
        lines.push(format!(
            "Selected source files: {}",
            s.selected_source_files
        ));
        lines.push(format!("Processed files: {}", s.processed_files));
        lines.push(format!(
            "Filtered path/files: {}",
            s.filtered_path_files
        ));
        lines.push(format!(
            "Filtered large files: {}",
            s.filtered_large_files
        ));
        lines.push(format!("Truncated files: {}", s.truncated_files));
        lines.push(format!("Failed files: {}", s.failed_files.len()));

        if s.timeout_triggered {
            lines.push("Scan was interrupted by timeout.".to_string());
        }

        lines
    }
}

// ===========================================================================
// Private helpers
// ===========================================================================

/// Walk the project root and collect source files (respecting .gitignore
/// via the `ignore` crate), up to `max_files`.
fn list_source_files(project_root: &Path, max_files: usize) -> Vec<PathBuf> {
    let walker = WalkBuilder::new(project_root)
        .standard_filters(true)
        .filter_entry(|entry| {
            let path = entry.path();
            let file_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => return true,
            };

            match entry.file_type() {
                Some(ft) if ft.is_dir() => {
                    // Do not descend into skipped directories.
                    !SKIP_DIR_NAMES.contains(&file_name)
                }
                Some(ft) if ft.is_file() => {
                    // Skip noise file names.
                    if SKIP_FILE_NAMES.contains(&file_name) {
                        return false;
                    }
                    // Only keep files with a recognised extension.
                    let ext = match path.extension().and_then(|e| e.to_str()) {
                        Some(e) => format!(".{}", e),
                        None => return false,
                    };
                    ext_to_lang(&ext).is_some()
                }
                _ => true,
            }
        })
        .build();

    let mut files: Vec<PathBuf> = Vec::new();
    for result in walker {
        if files.len() >= max_files {
            break;
        }
        match result {
            Ok(entry) => {
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    files.push(entry.into_path());
                }
            }
            Err(_) => continue,
        }
    }

    files
}
