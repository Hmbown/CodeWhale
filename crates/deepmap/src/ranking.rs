//! Ranking types used throughout the DeepMap analysis pipeline.
//!
//! These structs represent computed metrics and rankings for files,
//! modules, symbols, and reading-order suggestions. All types derive
//! common traits so they can be serialised, inspected, and compared.

use serde::{Deserialize, Serialize};

/// Per-file density and complexity snapshot.
///
/// Density = symbol_count / max(1, line_count).  Complexity is a
/// composite (edge-degree normalised by file size).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotspotInfo {
    pub file_path: String,
    /// Symbol-per-line ratio (higher = denser).
    pub density: f64,
    /// Aggregated structural complexity score.
    pub complexity_score: f64,
    /// Sum of PageRank values for symbols in this file.
    pub pagerank: f64,
    pub line_count: usize,
    pub symbol_count: usize,
}

/// Aggregate metrics for a single source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetrics {
    pub file_path: String,
    pub lines: usize,
    pub symbols: usize,
    /// Composite complexity (same as hotspot complexity).
    pub complexity: f64,
    /// File-level PageRank (mean of symbol PR values).
    pub pagerank: f64,
}

/// Summary statistics for a top-level module (directory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub directory: String,
    pub file_count: usize,
    pub symbol_count: usize,
    /// Total lines summed across all files in the module.
    pub lines: usize,
}

/// A single entry in the suggested reading order.
///
/// Higher score means the file is more important to read first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadingOrderEntry {
    pub file_path: String,
    pub score: f64,
    /// Human-readable explanation of why this file is recommended.
    pub reason: String,
}

/// Compact representation of an important symbol for summary views.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolSummary {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub pagerank: f64,
    pub signature: String,
}
