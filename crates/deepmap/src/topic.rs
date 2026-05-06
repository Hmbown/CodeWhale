//! Topic / relevance scoring for code-search queries.
//!
//! Provides heuristic strategies for matching a natural-language or
//! code-level query against files and symbols in a [`RepoGraph`]:
//!
//! - Identifier splitting (camelCase / PascalCase / snake_case / kebab-case).
//! - File-role classification.
//! - Weighted topic scoring with IDF-like keyword weighting.
//! - Fuzzy symbol suggestion (edit distance <= 3).
//! - Related-test discovery.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::types::RepoGraph;

// ---------------------------------------------------------------------------
// TestMatch
// ---------------------------------------------------------------------------

/// The result of a related-test lookup.
#[derive(Debug, Clone)]
pub struct TestMatch {
    pub test_file: String,
    pub target_file: String,
    pub confidence: f64,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Identifier splitting
// ---------------------------------------------------------------------------

/// Split a compound identifier into lowercased tokens.
///
/// Handles:
/// - camelCase / PascalCase   -> `splitCamel` => `["split", "camel"]`
/// - snake_case               -> `split_snake` => `["split", "snake"]`
/// - kebab-case               -> `split-kebab` => `["split", "kebab"]`
/// - Mixed separators         -> any of `-`, `_`, `.` act as delimiters
pub fn split_identifier(name: &str) -> Vec<String> {
    if name.is_empty() {
        return Vec::new();
    }

    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::with_capacity(name.len());

    // Phase 1: split on explicit separators.
    let segments: Vec<&str> = name.split(|c: char| c == '_' || c == '-' || c == '.').collect();

    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        // Phase 2: split camelCase / PascalCase boundaries.
        let chars: Vec<char> = segment.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i].is_uppercase() && i > 0 {
                // Check for acronym: multiple uppercase in a row.
                if i + 1 < chars.len() && chars[i + 1].is_lowercase() {
                    // Transition: uppercase + lowercase => boundary.
                    if !current.is_empty() {
                        tokens.push(current.clone());
                        current.clear();
                    }
                } else if !current.is_empty() && current.chars().all(|c| c.is_uppercase()) {
                    // Still in acronym segment: keep going.
                } else if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            current.push(chars[i].to_lowercase().next().unwrap_or(chars[i]));
            i += 1;
        }
        if !current.is_empty() {
            tokens.push(current.clone());
            current.clear();
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// File-role classification
// ---------------------------------------------------------------------------

/// Classify a file path into a high-level role string.
///
/// Returns one of: `"test"`, `"frontend-ui"`, `"frontend-state"`,
/// `"backend"`, `"config"`.
pub fn classify_file_role(path: &str) -> &str {
    let lower = path.to_lowercase();

    // Test files.
    if is_test_like_file(path) {
        return "test";
    }

    // Config files.
    if lower.ends_with(".json")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".toml")
        || lower.contains(".config.")
        || lower.contains("/config/")
    {
        return "config";
    }

    // Frontend UI.
    if lower.contains("/components/")
        || lower.contains("/pages/")
        || lower.contains("/views/")
        || lower.contains("/ui/")
        || lower.contains(".component.")
        || lower.ends_with(".vue")
        || lower.ends_with(".svelte")
        || lower.ends_with(".jsx")
        || lower.ends_with(".tsx")
    {
        return "frontend-ui";
    }

    // Frontend state / store.
    if lower.contains("/store/")
        || lower.contains("/state/")
        || lower.contains("/reducers/")
        || lower.contains("/actions/")
        || lower.ends_with("store.ts")
        || lower.ends_with("store.js")
    {
        return "frontend-state";
    }

    // Backend / API.
    if lower.contains("/api/")
        || lower.contains("/routes/")
        || lower.contains("/handlers/")
        || lower.contains("/controllers/")
        || lower.contains("/services/")
        || lower.contains("/middleware/")
        || lower.ends_with("_pb2.py")
        || lower.ends_with("_grpc.py")
    {
        return "backend";
    }

    "backend"
}

/// Check whether a file path looks like a test file.
pub fn is_test_like_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("test")
        || lower.contains("spec")
        || lower.contains("__test__")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.rs")
        || lower.ends_with("test.rs")
        || lower.contains("/tests/")
        || lower.contains("__snapshots__")
}

/// Check whether a file path is likely noise and should be down-weighted.
pub fn is_noise_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("__pycache__")
        || lower.contains("node_modules")
        || lower.contains(".min.")
        || lower.ends_with(".lock")
        || lower.ends_with(".map")
        || lower.contains("/vendor/")
        || lower.contains("/third_party/")
        || lower.contains("/.cache/")
        || lower.contains("/dist/")
        || lower.contains("/build/")
        || lower.contains(".generated.")
        || lower.contains("/generated/")
}

// ---------------------------------------------------------------------------
// Topic scoring
// ---------------------------------------------------------------------------

/// Score a single file for relevance to `query`.
///
/// The score is a weighted combination of:
/// - `path_score` (30%): does the path itself contain query tokens?
/// - `name_score` (25%): do symbol names in the file contain query
///   tokens?
/// - `symbol_score` (15%): do symbol kinds / docstrings match?
///
/// A noise-file penalty (5% reduction) and test-weight boost (55% of
/// original) are applied where appropriate.
///
/// The final value is clamped to `[0.0, 100.0]`.
pub fn topic_score(
    query: &str,
    file_path: &str,
    graph: &RepoGraph,
    keyword_weights: &HashMap<String, f64>,
) -> f64 {
    let query_tokens: Vec<String> = split_identifier(query);

    if query_tokens.is_empty() {
        return 0.0;
    }

    let path_tokens = split_identifier(file_path);
    let path_score = token_overlap(&query_tokens, &path_tokens, keyword_weights);

    // Symbol-name overlap (look at all symbols in this file).
    let mut name_score = 0.0_f64;
    if let Some(sym_ids) = graph.file_symbols.get(file_path) {
        let mut max_ns = 0.0_f64;
        for sym_id in sym_ids {
            if let Some(sym) = graph.symbols.get(sym_id) {
                let sym_tokens = split_identifier(&sym.name);
                let score = token_overlap(&query_tokens, &sym_tokens, keyword_weights);
                if score > max_ns {
                    max_ns = score;
                }
            }
        }
        name_score = max_ns;
    }

    // Symbol-kind / signature overlap.
    let mut symbol_score = 0.0_f64;
    if let Some(sym_ids) = graph.file_symbols.get(file_path) {
        for sym_id in sym_ids {
            if let Some(sym) = graph.symbols.get(sym_id) {
                let haystack = format!("{} {} {}", sym.kind, sym.signature, sym.docstring);
                for token in &query_tokens {
                    if haystack.to_lowercase().contains(&token.to_lowercase()) {
                        symbol_score += keyword_weights.get(token).copied().unwrap_or(1.0);
                    }
                }
            }
        }
    }

    let raw = path_score * 0.30 + name_score * 0.25 + symbol_score * 0.15;

    // Apply noise penalty.
    let after_noise = if is_noise_file(file_path) {
        raw * 0.95
    } else {
        raw
    };

    // Apply test weight.
    let final_score = if is_test_like_file(file_path) {
        after_noise * 0.55
    } else {
        after_noise
    };

    final_score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// IDF-like keyword weight computation
// ---------------------------------------------------------------------------

/// Compute per-keyword weights using an IDF-like heuristic.
///
/// Tokens that appear in many candidate files receive lower weight
/// (they are less discriminative).  Tokens that appear in few files
/// receive higher weight.
pub fn compute_keyword_weights(
    keywords: &[String],
    candidate_files: &[String],
    graph: &RepoGraph,
) -> HashMap<String, f64> {
    let n = candidate_files.len().max(1);

    let mut doc_freq: HashMap<String, usize> = HashMap::new();
    for kw in keywords {
        for file in candidate_files {
            if let Some(sym_ids) = graph.file_symbols.get(file) {
                let lower_kw = kw.to_lowercase();
                let matches: bool = split_identifier(file).iter().any(|t| *t == lower_kw)
                    || sym_ids.iter().any(|id| {
                        graph
                            .symbols
                            .get(id)
                            .map(|s| {
                                s.name.to_lowercase().contains(&lower_kw)
                                    || s.kind.to_lowercase().contains(&lower_kw)
                                    || s.docstring.to_lowercase().contains(&lower_kw)
                            })
                            .unwrap_or(false)
                    });
                if matches {
                    *doc_freq.entry(kw.clone()).or_insert(0) += 1;
                }
            }
        }
    }

    keywords
        .iter()
        .map(|kw| {
            let df = doc_freq.get(kw).copied().unwrap_or(0).max(1);
            let weight = (n as f64 / df as f64).ln_1p() + 0.5;
            (kw.clone(), weight)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Fuzzy symbol suggestion (edit distance <= 3)
// ---------------------------------------------------------------------------

/// Find symbol names in the graph whose Levenshtein distance to `query`
/// is at most 3, returning up to `max_results` matches.
pub fn fuzzy_symbol_suggest(
    query: &str,
    graph: &RepoGraph,
    max_results: usize,
) -> Vec<String> {
    let query_lower = query.to_lowercase();
    let mut candidates: Vec<(String, f64, f64)> = Vec::new(); // (name, pagerank, dist_score)

    for symbol in graph.symbols.values() {
        let dist = levenshtein_distance(&query_lower, &symbol.name.to_lowercase());
        if dist <= 3 {
            // Score: closer distance is better; PageRank breaks ties.
            let dist_score = 1.0 - (dist as f64 / (query.len().max(symbol.name.len()).max(1) as f64));
            candidates.push((symbol.name.clone(), symbol.pagerank, dist_score));
        }
    }

    candidates.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    candidates.truncate(max_results);
    candidates.into_iter().map(|(name, _, _)| name).collect()
}

// ---------------------------------------------------------------------------
// Related-test discovery
// ---------------------------------------------------------------------------

/// Find test files that are likely related to `targets`.
///
/// Three strategies, applied in order:
/// 1. **Same-directory**: test files in the same directory as a target.
/// 2. **Naming convention**: test files whose name contains a target's
///    stem.
/// 3. **Cross-reference**: test files whose imports reference a target.
pub fn find_related_tests(
    targets: &[String],
    graph: &RepoGraph,
    _project_root: &Path,
) -> Vec<TestMatch> {
    let all_files: Vec<&String> = graph.file_symbols.keys().collect();
    let test_files: Vec<&String> = all_files
        .iter()
        .filter(|f| is_test_like_file(f))
        .copied()
        .collect();

    let target_stems: Vec<String> = targets
        .iter()
        .filter_map(|t| {
            Path::new(t)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
        })
        .collect();

    let mut results: Vec<TestMatch> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for target in targets {
        let target_dir = Path::new(target)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        for test_file in &test_files {
            // Strategy 1: same directory.
            let test_dir = Path::new(test_file)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if test_dir == target_dir && !test_dir.is_empty() {
                let key = (test_file.to_string(), target.clone());
                if seen.insert(key) {
                    results.push(TestMatch {
                        test_file: test_file.to_string(),
                        target_file: target.clone(),
                        confidence: 0.9,
                        reason: "Same directory as target file".to_string(),
                    });
                    continue;
                }
            }

            // Strategy 2: name contains target stem.
            let tf_stem = Path::new(test_file)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            for stem in &target_stems {
                let tf_stem_test = tf_stem
                    .strip_prefix("test_")
                    .or_else(|| tf_stem.strip_suffix("_test"))
                    .or_else(|| tf_stem.strip_prefix("spec_"))
                    .or_else(|| tf_stem.strip_suffix("_spec"))
                    .unwrap_or(tf_stem)
                    .to_lowercase();
                if tf_stem_test == *stem
                    || tf_stem_test.contains(stem.as_str())
                    || stem.contains(&tf_stem_test)
                {
                    let key = (test_file.to_string(), target.clone());
                    if seen.insert(key) {
                        results.push(TestMatch {
                            test_file: test_file.to_string(),
                            target_file: target.clone(),
                            confidence: 0.75,
                            reason: format!(
                                "Test name '{}' matches target stem '{}'",
                                tf_stem, stem
                            ),
                        });
                        break;
                    }
                }
            }
        }
    }

    // Strategy 3: cross-reference (test file imports reference target).
    if results.len() < targets.len() * 2 {
        for target in targets {
            let target_lower = target.to_lowercase();
            for test_file in &test_files {
                if let Some(imports) = graph.file_imports.get(*test_file) {
                    for imp in imports {
                        if imp.contains(&target_lower) || target_lower.contains(imp) {
                            let key = (test_file.to_string(), target.clone());
                            if seen.insert(key) {
                                results.push(TestMatch {
                                    test_file: test_file.to_string(),
                                    target_file: target.clone(),
                                    confidence: 0.6,
                                    reason: format!(
                                        "Test file imports '{}' which relates to '{}'",
                                        imp, target
                                    ),
                                });
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Compute the token-level overlap score between query tokens and
/// haystack tokens, weighted by `keyword_weights`.
fn token_overlap(
    query_tokens: &[String],
    haystack_tokens: &[String],
    keyword_weights: &HashMap<String, f64>,
) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }

    let mut score = 0.0_f64;
    for qt in query_tokens {
        let weight = keyword_weights.get(qt).copied().unwrap_or(1.0);
        for ht in haystack_tokens {
            if ht == qt {
                score += weight;
            } else if ht.contains(qt.as_str()) || qt.contains(ht.as_str()) {
                score += weight * 0.5;
            }
        }
    }

    // Normalise by query length so longer queries don't dominate.
    score / query_tokens.len() as f64
}

/// Classic Levenshtein distance (iterative, O(n*m)).
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    // Use two rows to keep memory O(min(n,m)).
    if a_len < b_len {
        return levenshtein_distance(b, a);
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr: Vec<usize> = vec![0; b_len + 1];

    for (i, ac) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, bc) in b_chars.iter().enumerate() {
            let cost = if ac == bc { 0 } else { 1 };
            curr[j + 1] = min_three(
                curr[j] + 1,         // insertion
                prev[j + 1] + 1,     // deletion
                prev[j] + cost,      // substitution
            );
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

#[inline]
fn min_three(a: usize, b: usize, c: usize) -> usize {
    if a <= b && a <= c {
        a
    } else if b <= a && b <= c {
        b
    } else {
        c
    }
}
