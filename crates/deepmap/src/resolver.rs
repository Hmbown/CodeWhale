use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;

use crate::types::{RepoGraph, ProjectImportConfig, PathAliasRule};

/// Resolves import paths, tsconfig / jsconfig aliases, and call targets
/// within a project graph. Used by the analysis engine to connect
/// symbol references across files.
pub struct ImportResolver {
    pub project_root: PathBuf,
    pub import_configs: Vec<ProjectImportConfig>,
    /// File stem -> candidate file paths.
    pub file_map: HashMap<String, Vec<String>>,
    /// Symbol name -> symbol IDs.
    pub name_index: HashMap<String, Vec<String>>,
    /// All known relative file paths for O(1) existence check.
    known_paths: std::collections::HashSet<String>,
}

// ---------------------------------------------------------------------------
// Construction & index building
// ---------------------------------------------------------------------------

impl ImportResolver {
    /// Build an empty resolver, populate indices from `graph`, then scan
    /// the project root for TypeScript / JavaScript config files.
    pub fn new(project_root: &Path, graph: &RepoGraph) -> Self {
        let mut this = Self {
            project_root: project_root.to_path_buf(),
            import_configs: Vec::new(),
            file_map: HashMap::new(),
            name_index: HashMap::new(),
            known_paths: std::collections::HashSet::new(),
        };
        this.build_indices(graph);
        this.discover_import_configs();
        this
    }

    /// Populate `file_map` (stem -> paths) from `file_symbols` and
    /// `file_imports`; populate `name_index` (name -> symbol IDs) from
    /// `symbols`.
    ///
    /// Files that appear only in `file_imports` (e.g. a file that only
    /// imports without defining its own symbols) are still indexed so
    /// that downstream resolution can find them.
    pub fn build_indices(&mut self, graph: &RepoGraph) {
        // file_map: stem -> candidate paths
        for file_path in graph.file_symbols.keys() {
            if let Some(stem) = file_stem(file_path) {
                self.file_map
                    .entry(stem.to_string())
                    .or_default()
                    .push(file_path.clone());
            }
        }
        // also index files that appear only in file_imports
        for file_path in graph.file_imports.keys() {
            if !graph.file_symbols.contains_key(file_path) {
                if let Some(stem) = file_stem(file_path) {
                    self.file_map
                        .entry(stem.to_string())
                        .or_default()
                        .push(file_path.clone());
                }
            }
        }

        // name_index: name -> symbol IDs
        for (sym_id, symbol) in &graph.symbols {
            self.name_index
                .entry(symbol.name.clone())
                .or_default()
                .push(sym_id.clone());
        }

        // Pre-compute set of all known file paths for O(1) existence check.
        for file in graph.file_symbols.keys().chain(graph.file_imports.keys()) {
            self.known_paths.insert(file.clone());
        }
    }

    /// Walk `project_root` looking for `tsconfig.json` / `jsconfig.json`
    /// at the top level and in each immediate subdirectory. Parsed
    /// configs are appended to `self.import_configs`.
    pub fn discover_import_configs(&mut self) {
        let config_names = ["tsconfig.json", "jsconfig.json"];

        // top-level
        for name in &config_names {
            let path = self.project_root.join(name);
            if path.is_file() {
                if let Ok(cfg) = Self::parse_tsconfig(&path) {
                    self.import_configs.push(cfg);
                }
            }
        }

        // one level deep
        if let Ok(entries) = fs::read_dir(&self.project_root) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    for name in &config_names {
                        let path = entry.path().join(name);
                        if path.is_file() {
                            if let Ok(cfg) = Self::parse_tsconfig(&path) {
                                self.import_configs.push(cfg);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Read the JSON(C) file at `path`, strip comments and trailing
    /// commas, then extract `compilerOptions.baseUrl` and
    /// `compilerOptions.paths` into a [`ProjectImportConfig`].
    pub fn parse_tsconfig(path: &Path) -> Result<ProjectImportConfig, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let cleaned = Self::strip_jsonc(&content);
        let value: serde_json::Value = serde_json::from_str(&cleaned)
            .map_err(|e| format!("Failed to parse JSON in {}: {}", path.display(), e))?;

        let config_path = Some(path.to_string_lossy().to_string());
        let config_dir = path.parent().map(|p| p.to_string_lossy().to_string());

        let base_url = value
            .get("compilerOptions")
            .and_then(|co| co.get("baseUrl"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let alias_rules: Vec<PathAliasRule> = value
            .get("compilerOptions")
            .and_then(|co| co.get("paths"))
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .map(|(pattern, targets)| {
                        let target_patterns = targets
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        PathAliasRule {
                            alias_pattern: pattern.clone(),
                            target_patterns,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(ProjectImportConfig {
            config_path,
            config_dir,
            base_url,
            alias_rules,
        })
    }
}

// ---------------------------------------------------------------------------
// JSONC comment / trailing-comma stripper (public for downstream reuse)
// ---------------------------------------------------------------------------

impl ImportResolver {
    /// Strip JSONC comments and trailing commas from `text`.
    ///
    /// - `//` line comments: removed along with trailing whitespace
    /// - `/* */` block comments: removed without replacement
    /// - Trailing comma before `}` or `]`: removed
    /// - String literals are preserved verbatim (no stripping inside "..")
    pub fn strip_jsonc(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;

        while i < len {
            // string literal -- copy verbatim
            if chars[i] == '"' {
                out.push('"');
                i += 1;
                while i < len {
                    let c = chars[i];
                    out.push(c);
                    if c == '\\' {
                        // escaped character -- skip the next char too
                        i += 1;
                        if i < len {
                            out.push(chars[i]);
                            i += 1;
                        }
                        continue;
                    }
                    if c == '"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                continue;
            }

            // // line comment
            if chars[i] == '/' && i + 1 < len && chars[i + 1] == '/' {
                // trim trailing whitespace that sits before the //
                let trim = out.trim_end().len();
                out.truncate(trim);

                i += 2;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                if i < len {
                    out.push('\n');
                    i += 1;
                }
                continue;
            }

            // /* block comment */
            if chars[i] == '/' && i + 1 < len && chars[i + 1] == '*' {
                i += 2;
                while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                if i + 1 < len {
                    i += 2; // skip past */
                }
                continue; // no replacement character
            }

            // Trailing comma before } or ] — skip (string-safe: we're not inside a string here).
            if chars[i] == ',' {
                let mut j = i + 1;
                while j < len && chars[j].is_whitespace() { j += 1; }
                if j < len && (chars[j] == '}' || chars[j] == ']') {
                    i += 1; // skip comma
                    continue;
                }
            }

            out.push(chars[i]);
            i += 1;
        }

        out
    }
}

// ---------------------------------------------------------------------------
// Import-target resolution
// ---------------------------------------------------------------------------

impl ImportResolver {
    /// Resolve an import statement to concrete file paths.
    ///
    /// Strategy (in order):
    ///
    /// 1. **Relative import** (starts with `.`): canonicalize the path
    ///    relative to `source_file`'s directory, then probe exact match,
    ///    well-known extensions, and `index` files.
    ///
    /// 2. **Alias resolution**: match `import_path` against each
    ///    tsconfig `paths` rule. On match, substitute the wildcard,
    ///    resolve the target(s) against `baseUrl` and probe candidates.
    ///
    /// 3. **Stem lookup**: extract the filename stem from
    ///    `import_path` and consult `file_map`.
    pub fn resolve_import_targets(&self, source_file: &str, import_path: &str) -> Vec<String> {
        let mut results = Vec::new();

        // 1. relative import
        if import_path.starts_with('.') {
            let source_dir = Path::new(source_file).parent().unwrap_or(Path::new(""));
            let normalised = normalise_path(&source_dir.join(import_path));
            results.extend(self.resolve_path_candidates(&normalised));
        }

        // 2. alias resolution
        if results.is_empty() {
            for config in &self.import_configs {
                if let Some(matched) = self.resolve_alias(import_path, config) {
                    results.extend(matched);
                    break; // first matching config wins
                }
            }
        }

        // 3. direct stem lookup
        if results.is_empty() {
            let stem = Path::new(import_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(import_path);
            if let Some(paths) = self.file_map.get(stem) {
                results.extend(paths.clone());
            }
        }

        results
    }

    // -- helpers -----------------------------------------------------------

    /// Probe `base_path` (exact, +extensions, +/index.ext) against the
    /// known file set.
    fn resolve_path_candidates(&self, base_path: &Path) -> Vec<String> {
        let base = base_path.to_string_lossy().to_string();
        let mut out = Vec::new();

        if self.known_paths.contains(&base) {
            out.push(base);
            return out;
        }

        const EXTENSIONS: &[&str] = &[
            ".ts", ".tsx", ".js", ".jsx", ".py", ".rs", ".go", ".json", ".html", ".css",
        ];

        for ext in EXTENSIONS {
            let candidate = format!("{}{}", base, ext);
            if self.known_paths.contains(&candidate) {
                out.push(candidate);
            }
        }

        for ext in EXTENSIONS {
            let candidate = format!("{}/index{}", base, ext);
            if self.known_paths.contains(&candidate) {
                out.push(candidate);
            }
        }

        out
    }

    /// Try to match `import_path` against one alias rule set (all rules
    /// from a single config). Returns `Some(targets)` when at least one
    /// target resolves to a known file.
    fn resolve_alias(
        &self,
        import_path: &str,
        config: &ProjectImportConfig,
    ) -> Option<Vec<String>> {
        for rule in &config.alias_rules {
            if let Some(captured) = match_alias_pattern(import_path, &rule.alias_pattern) {
                let mut results = Vec::new();
                for target_pattern in &rule.target_patterns {
                    let substituted = target_pattern.replace('*', &captured);
                    let full = match &config.base_url {
                        Some(base) => self.project_root.join(base).join(&substituted),
                        None => self.project_root.join(&substituted),
                    };
                    results.extend(self.resolve_path_candidates(&full));
                }
                if !results.is_empty() {
                    return Some(results);
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Symbol / call-target resolution
// ---------------------------------------------------------------------------

impl ImportResolver {
    /// Given a file and line number, find the symbol whose declared
    /// line range `(line .. end_line)` contains `call_line`.
    ///
    /// Returns the symbol ID if found, `None` otherwise.
    pub fn resolve_calling_symbol_with_graph(
        &self,
        file: &str,
        call_line: usize,
        graph: &RepoGraph,
    ) -> Option<String> {
        if let Some(sym_ids) = graph.file_symbols.get(file) {
            for sym_id in sym_ids {
                if let Some(symbol) = graph.symbols.get(sym_id) {
                    if call_line >= symbol.line && call_line <= symbol.end_line {
                        return Some(sym_id.clone());
                    }
                }
            }
        }
        None
    }

    /// Resolve a call expression to a target symbol ID.
    ///
    /// 1. Resolve `call_name` through `local_names` (import-renaming map
    ///    built from `JsImportBinding`).
    /// 2. Look up the resolved name in `name_index`.
    /// 3. Return the first matching symbol ID (caller should prefer
    ///    same-file matches when available).
    ///
    /// The `_call_line` and `_call_kind` parameters are reserved for
    /// future scope-narrowing heuristics.
    pub fn resolve_call_target(
        &self,
        _file: &str,
        call_name: &str,
        _call_line: usize,
        _call_kind: &str,
        local_names: &HashMap<String, String>,
    ) -> Option<String> {
        let resolved_name = local_names
            .get(call_name)
            .map(|s| s.as_str())
            .unwrap_or(call_name);
        self.name_index
            .get(resolved_name)
            .and_then(|ids| ids.first().cloned())
    }
}

// ===========================================================================
// Free helper functions
// ===========================================================================

/// Return the file stem (filename without extension) of a path string.
fn file_stem(file_path: &str) -> Option<&str> {
    Path::new(file_path).file_stem().and_then(|s| s.to_str())
}

/// Remove `.` and resolve `..` path components without touching disk.
fn normalise_path(path: &Path) -> PathBuf {
    let mut buf = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => { /* skip */ }
            std::path::Component::ParentDir => {
                buf.pop();
            }
            other => buf.push(other),
        }
    }
    buf
}

/// Match an import path against a tsconfig alias pattern.
///
/// Supports:
/// - Exact match (`pattern == import_path` -> `Some("")`)
/// - Wildcard suffix (`@/lib/*` matches `@/lib/foo` -> `Some("foo")`)
fn match_alias_pattern(import_path: &str, pattern: &str) -> Option<String> {
    if pattern == import_path {
        return Some(String::new());
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        if import_path.starts_with(prefix) {
            let captured = &import_path[prefix.len()..];
            // require non-empty capture (e.g. `@/` alone should not match `@/`)
            if !captured.is_empty() {
                return Some(captured.to_string());
            }
        }
    }
    None
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Symbol;

    fn make_test_graph() -> RepoGraph {
        let mut g = RepoGraph::default();
        g.file_symbols
            .insert("src/bar/foo.ts".to_string(), vec![]);
        g.file_symbols
            .insert("src/bar/foo.js".to_string(), vec![]);
        g.file_symbols
            .insert("src/utils/helper.ts".to_string(), vec![]);

        g.symbols.insert(
            "sym_helper".to_string(),
            Symbol::new(
                "sym_helper".to_string(),
                "helper".to_string(),
                "function".to_string(),
                "src/utils/helper.ts".to_string(),
                1,
                10,
                0,
                "public".to_string(),
                String::new(),
                "helper()".to_string(),
            ),
        );
        g.file_symbols
            .get_mut("src/utils/helper.ts")
            .unwrap()
            .push("sym_helper".to_string());

        g
    }

    // ---- strip_jsonc tests -----------------------------------------------

    #[test]
    fn strip_simple_line_comment() {
        let text = "{\"a\": 1 // comment\n}";
        let got = ImportResolver::strip_jsonc(text);
        assert_eq!(got, "{\"a\": 1\n}");
    }

    #[test]
    fn strip_block_comment() {
        let text = "{\"a\": /* block */ 1}";
        let got = ImportResolver::strip_jsonc(text);
        assert_eq!(got, "{\"a\":  1}");
    }

    #[test]
    fn strip_trailing_comma() {
        let text = "{\"a\": 1,}";
        let got = ImportResolver::strip_jsonc(text);
        assert_eq!(got, "{\"a\": 1}");
    }

    #[test]
    fn strip_string_keeps_comment_like_content() {
        let text = "{\"a\": \"// not a comment\"}";
        let got = ImportResolver::strip_jsonc(text);
        assert_eq!(got, "{\"a\": \"// not a comment\"}");
    }

    // ---- resolve_import_targets tests ------------------------------------

    #[test]
    fn normalise_dot() {
        let graph = make_test_graph();
        let resolver = ImportResolver::new(Path::new("/tmp/test"), &graph);
        let results = resolver.resolve_import_targets("src/bar/index.ts", "./foo");
        assert!(!results.is_empty(), "should resolve to at least one file");
        assert!(
            results.contains(&"src/bar/foo.ts".to_string()),
            "results should contain foo.ts, got: {:?}",
            results
        );
    }

    #[test]
    fn normalise_multiple() {
        let graph = make_test_graph();
        let resolver = ImportResolver::new(Path::new("/tmp/test"), &graph);
        let results = resolver.resolve_import_targets("src/bar/index.ts", "./foo");
        assert!(
            results.contains(&"src/bar/foo.ts".to_string()),
            "should contain foo.ts, got: {:?}",
            results
        );
        assert!(
            results.contains(&"src/bar/foo.js".to_string()),
            "should contain foo.js, got: {:?}",
            results
        );
    }

    // ---- resolve_calling_symbol_with_graph tests -------------------------

    #[test]
    fn symbol_range_contains_call_line() {
        let graph = make_test_graph();
        let resolver = ImportResolver::new(Path::new("/tmp/test"), &graph);
        let result =
            resolver.resolve_calling_symbol_with_graph("src/utils/helper.ts", 5, &graph);
        assert_eq!(result, Some("sym_helper".to_string()));
    }

    #[test]
    fn symbol_range_outside_call_line() {
        let graph = make_test_graph();
        let resolver = ImportResolver::new(Path::new("/tmp/test"), &graph);
        // sym_helper spans lines 1-10, so line 42 should not match
        let result =
            resolver.resolve_calling_symbol_with_graph("src/utils/helper.ts", 42, &graph);
        assert_eq!(result, None);
    }

    // ---- resolve_call_target tests ---------------------------------------

    #[test]
    fn call_target_lookup_by_name() {
        let graph = make_test_graph();
        let resolver = ImportResolver::new(Path::new("/tmp/test"), &graph);
        let local_names = HashMap::new();
        let result = resolver.resolve_call_target("src/bar/foo.ts", "helper", 0, "call", &local_names);
        assert_eq!(result, Some("sym_helper".to_string()));
    }

    #[test]
    fn call_target_uses_local_rename() {
        let graph = make_test_graph();
        let resolver = ImportResolver::new(Path::new("/tmp/test"), &graph);
        let mut local_names = HashMap::new();
        local_names.insert("h".to_string(), "helper".to_string());
        let result = resolver.resolve_call_target("src/bar/foo.ts", "h", 0, "call", &local_names);
        assert_eq!(result, Some("sym_helper".to_string()));
    }

    // ---- path helper tests -----------------------------------------------

    #[test]
    fn normalise_path_removes_dot_and_dotdot() {
        let p = normalise_path(Path::new("a/b/./c/d/../e"));
        assert_eq!(p, Path::new("a/b/c/e"));
    }
}
