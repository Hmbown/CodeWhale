//! Multi-language code parser backed by tree-sitter.
//!
//! Provides [`TreeSitterAdapter`] which manages per-language parsers, compiled
//! queries, and per-file extraction of symbols, imports, calls, and JS/TS
//! import/export bindings.

use std::collections::HashMap;
use std::path::Path;

use crate::types::{JsExportBinding, JsImportBinding, Symbol};
use crate::queries;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator, Tree};

// ---------------------------------------------------------------------------
// Raw query capture
// ---------------------------------------------------------------------------

/// A single capture produced by a tree-sitter query match.
#[derive(Debug, Clone)]
struct RawCapture {
    name: String,
    kind: String,
    start_byte: usize,
    end_byte: usize,
    start_row: usize,
    end_row: usize,
    start_col: usize,
}

// ---------------------------------------------------------------------------
// TreeSitterAdapter
// ---------------------------------------------------------------------------

/// Multi-language adapter that owns per-language parsers, compiled queries, and
/// per-file extraction buffers.
pub struct TreeSitterAdapter {
    parsers: HashMap<String, Parser>,
    queries: HashMap<String, HashMap<String, Query>>,
    languages: HashMap<String, Language>,

    current_symbols: Vec<Symbol>,
    current_imports: Vec<String>,
    current_calls: Vec<(String, usize, String)>,
    current_import_bindings: Vec<JsImportBinding>,
    current_exports: Vec<JsExportBinding>,
    current_file: String,
}

impl TreeSitterAdapter {
    /// Create a new adapter, load available language parsers, and compile queries.
    pub fn new() -> Self {
        let mut parsers: HashMap<String, Parser> = HashMap::new();
        let mut languages: HashMap<String, Language> = HashMap::new();

        // Convenience macro: try to create a parser for `$name` using the
        // `LanguageFn` constant `$const`.  Silently skips unavailable parsers.
        macro_rules! try_load {
            ($name:expr, $lang_fn:expr) => {{
                let lang: Language = $lang_fn.into();
                let mut p = Parser::new();
                if p.set_language(&lang).is_ok() {
                    let name: &str = $name;
                    parsers.insert(name.to_string(), p);
                    languages.insert(name.to_string(), lang);
                }
            }};
        }

        try_load!("rust", tree_sitter_rust::LANGUAGE);
        try_load!("python", tree_sitter_python::LANGUAGE);
        try_load!("javascript", tree_sitter_javascript::LANGUAGE);
        try_load!("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT);
        try_load!("go", tree_sitter_go::LANGUAGE);
        try_load!("html", tree_sitter_html::LANGUAGE);
        try_load!("css", tree_sitter_css::LANGUAGE);
        try_load!("json", tree_sitter_json::LANGUAGE);

        let mut adapter = Self {
            parsers,
            queries: HashMap::new(),
            languages,
            current_symbols: Vec::new(),
            current_imports: Vec::new(),
            current_calls: Vec::new(),
            current_import_bindings: Vec::new(),
            current_exports: Vec::new(),
            current_file: String::new(),
        };
        adapter.init_queries();
        adapter
    }

    // -----------------------------------------------------------------------
    // Query initialisation
    // -----------------------------------------------------------------------

    /// For every (lang, qtype, pattern) returned by [`queries()`], if a parser
    /// for that language is loaded, compile the pattern and store it.
    fn init_queries(&mut self) {
        let entries: Vec<(String, String, String)> = queries::queries()
            .into_iter()
            .filter(|(lang, _, _)| self.parsers.contains_key(*lang))
            .map(|(lang, qtype, pattern)| (lang.to_string(), qtype.to_string(), pattern.to_string()))
            .collect();

        for (lang, qtype, pattern) in entries {
            let language = self.languages.get(&lang).expect("language must exist");
            if let Ok(query) = Query::new(language, &pattern) {
                self.queries.entry(lang).or_default().insert(qtype, query);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Main parse entry-point
    // -----------------------------------------------------------------------

    /// Parse `content` (identified by `path` and `lang`), extracting symbols,
    /// imports, calls, and JS/TS import/export bindings into the per-file
    /// buffers.  Results are exposed via the accessor methods below.
    pub fn parse(&mut self, path: &Path, content: &str, lang: &str) -> Result<(), String> {
        // Size guard: 10 MB max.
        const MAX_BYTES: usize = 10 * 1024 * 1024;
        if content.len() > MAX_BYTES {
            return Err(format!(
                "file exceeds size limit: {} > {}",
                content.len(),
                MAX_BYTES
            ));
        }

        let parser = self
            .parsers
            .get_mut(lang)
            .ok_or_else(|| format!("no parser for language: {lang}"))?;

        let source_bytes = content.as_bytes();
        let tree: Tree = parser
            .parse(source_bytes, None)
            .ok_or_else(|| "parse returned None".to_string())?;
        let root = tree.root_node();

        // Depth guard: refuse to analyse excessively nested files.
        if has_excessive_nesting(root, 1000) {
            return Err("excessive nesting depth (>1000)".to_string());
        }

        // Reset per-file buffers.
        self.current_file = path.to_string_lossy().to_string();
        self.current_symbols = self.extract_symbols(root, source_bytes, lang);
        self.current_imports = self.extract_imports(root, source_bytes, lang);
        self.current_calls = self.extract_calls(root, source_bytes, lang);
        self.current_import_bindings.clear();
        self.current_exports.clear();

        if lang == "javascript" || lang == "typescript" {
            self.current_import_bindings =
                self.extract_js_ts_import_bindings(root, source_bytes);
            self.current_exports = self.extract_js_ts_export_bindings(root, source_bytes);

            // Extra symbol passes for JS/TS.
            let extra = self.extract_object_literal_methods(root, source_bytes);
            self.current_symbols.extend(extra);
            let extra = self.extract_anonymous_symbols(root, source_bytes);
            self.current_symbols.extend(extra);
            let extra = self.extract_exported_function_expressions(root, source_bytes);
            self.current_symbols.extend(extra);

            // Re-sort and deduplicate the extended symbol list.
            // Sort by span size ascending then col ascending.
            self.current_symbols.sort_by(|a, b| {
                let span_a = a.end_line.saturating_sub(a.line);
                let span_b = b.end_line.saturating_sub(b.line);
                span_a.cmp(&span_b).then(a.col.cmp(&b.col))
            });
            self.current_symbols.dedup_by(|a, b| a.name == b.name && a.line == b.line);
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Public accessors
    // -----------------------------------------------------------------------

    pub fn symbols(&self) -> Vec<Symbol> {
        self.current_symbols.clone()
    }

    pub fn imports(&self) -> Vec<String> {
        self.current_imports.clone()
    }

    pub fn calls(&self) -> Vec<(String, usize, String)> {
        self.current_calls.clone()
    }

    pub fn import_bindings(&self) -> Vec<JsImportBinding> {
        self.current_import_bindings.clone()
    }

    pub fn exports(&self) -> Vec<JsExportBinding> {
        self.current_exports.clone()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Execute a compiled query of type `qtype` for `lang` and collect
    /// captures that match the `@name` and `@def` anchors.
    fn run_query<'tree>(
        &self,
        lang: &str,
        qtype: &str,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
    ) -> Vec<RawCapture> {
        let lang_map = match self.queries.get(lang) {
            Some(m) => m,
            None => return Vec::new(),
        };
        let query = match lang_map.get(qtype) {
            Some(q) => q,
            None => return Vec::new(),
        };

        let name_idx = query.capture_index_for_name("name");
        let def_idx = query.capture_index_for_name("def");

        let mut cursor = QueryCursor::new();
        let mut qc = cursor.captures(query, root, source_bytes);

        let mut results: Vec<RawCapture> = Vec::new();

        while let Some(item) = qc.next() {
            let (match_, _idx) = item;

            let mut name_node: Option<Node<'tree>> = None;
            let mut def_node: Option<Node<'tree>> = None;

            for cap in match_.captures {
                if let Some(ni) = name_idx {
                    if cap.index == ni {
                        name_node = Some(cap.node);
                        continue;
                    }
                }
                if let Some(di) = def_idx {
                    if cap.index == di {
                        def_node = Some(cap.node);
                    }
                }
            }

            // Pick a representative node — prefer @def for span info,
            // fall back to any captured node when neither anchor exists.
            let span_node = def_node.or(name_node);
            let text_node = name_node.or(def_node);

            if let (Some(tn), Some(sn)) = (text_node, span_node) {
                let start_pos = sn.start_position();
                let end_pos = sn.end_position();
                results.push(RawCapture {
                    name: node_text(tn, source_bytes).to_string(),
                    kind: sn.kind().to_string(),
                    start_byte: sn.start_byte(),
                    end_byte: sn.end_byte(),
                    start_row: start_pos.row,
                    end_row: end_pos.row,
                    start_col: start_pos.column,
                });
            }
        }

        results
    }

    // -----------------------------------------------------------------------
    // Symbol extraction
    // -----------------------------------------------------------------------

    /// Extract top-level and nested symbols from `root` using the "function"
    /// and "class" queries.  HTML / CSS / JSON use specialised extractors.
    fn extract_symbols<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
        lang: &str,
    ) -> Vec<Symbol> {
        let file = &self.current_file;

        match lang {
            "html" => return self.extract_html_symbols(root, source_bytes, file),
            "css" => return self.extract_css_symbols(root, source_bytes, file),
            "json" => return self.extract_json_symbols(root, source_bytes, file),
            _ => {}
        }

        let mut captures = self.run_query(lang, "function", root, source_bytes);
        captures.extend(self.run_query(lang, "class", root, source_bytes));

        // Sort: span size ascending, then start_byte ascending.
        captures.sort_by(|a, b| {
            let span_a = a.end_byte.saturating_sub(a.start_byte);
            let span_b = b.end_byte.saturating_sub(b.start_byte);
            span_a
                .cmp(&span_b)
                .then(a.start_byte.cmp(&b.start_byte))
        });

        // Deduplicate by (name, start_row).
        captures.dedup_by(|a, b| a.name == b.name && a.start_row == b.start_row);

        captures
            .into_iter()
            .map(|rc| {
                let id = format!("{}:{}:{}", file, rc.name, rc.start_row);
                Symbol::new(
                    id,
                    rc.name,
                    rc.kind,
                    file.to_string(),
                    rc.start_row,
                    rc.end_row,
                    rc.start_col,
                    "public".to_string(),
                    String::new(),
                    String::new(),
                )
            })
            .collect()
    }

    /// Extract HTML tag names as symbols.
    fn extract_html_symbols<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
        file: &str,
    ) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        let nodes = walk_tree(root);
        for node in &nodes {
            if node.kind() == "element" {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "start_tag" {
                        if let Some(tag_name) = first_child_of_type(child, "tag_name") {
                            let name = node_text(tag_name, source_bytes).to_string();
                            let pos = tag_name.start_position();
                            let end = tag_name.end_position();
                            let id = format!("{}:{}:{}", file, name, pos.row);
                            symbols.push(Symbol::new(
                                id,
                                name,
                                "html_tag".to_string(),
                                file.to_string(),
                                pos.row,
                                end.row,
                                pos.column,
                                "public".to_string(),
                                String::new(),
                                String::new(),
                            ));
                        }
                        break;
                    }
                }
            }
        }
        symbols
    }

    /// Extract CSS class / id / tag selectors as symbols.
    fn extract_css_symbols<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
        file: &str,
    ) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        let nodes = walk_tree(root);
        for node in &nodes {
            let kind = node.kind();
            let (sym_kind, name) = match kind {
                "class_selector" => {
                    let inner = node_text(*node, source_bytes);
                    ("css_class".to_string(), inner.trim_start_matches('.').to_string())
                }
                "id_selector" => {
                    let inner = node_text(*node, source_bytes);
                    ("css_id".to_string(), inner.trim_start_matches('#').to_string())
                }
                "tag_name" => ("css_tag".to_string(), node_text(*node, source_bytes).to_string()),
                _ => continue,
            };
            let pos = node.start_position();
            let end = node.end_position();
            let id = format!("{}:{}:{}", file, name, pos.row);
            symbols.push(Symbol::new(
                id,
                name,
                sym_kind,
                file.to_string(),
                pos.row,
                end.row,
                pos.column,
                "public".to_string(),
                String::new(),
                String::new(),
            ));
        }
        symbols
    }

    /// Extract JSON property keys as symbols.
    fn extract_json_symbols<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
        file: &str,
    ) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        let nodes = walk_tree(root);
        for node in &nodes {
            if node.kind() == "pair" {
                if let Some(key_node) = node.child(0) {
                    let raw = node_text(key_node, source_bytes);
                    let name = raw.trim_matches('"').to_string();
                    let pos = key_node.start_position();
                    let end = key_node.end_position();
                    let id = format!("{}:{}:{}", file, name, pos.row);
                    symbols.push(Symbol::new(
                        id,
                        name,
                        "json_key".to_string(),
                        file.to_string(),
                        pos.row,
                        end.row,
                        pos.column,
                        "public".to_string(),
                        String::new(),
                        String::new(),
                    ));
                }
            }
        }
        symbols
    }

    // -----------------------------------------------------------------------
    // Import extraction (module paths)
    // -----------------------------------------------------------------------

    /// Extract imported module paths.  Strips language-specific decorations
    /// (quotes, angle brackets, `::` separators, etc.).
    fn extract_imports<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
        lang: &str,
    ) -> Vec<String> {
        // For Rust, run query directly to capture both @path and @name.
        if lang == "rust" {
            return self.extract_rust_imports(root, source_bytes);
        }
        let captures = self.run_query(lang, "import", root, source_bytes);
        let mut imports: Vec<String> = captures.into_iter().map(|rc| rc.name).collect();

        for imp in &mut imports {
            match lang {
                "javascript" | "typescript" => {
                    *imp = imp.trim_matches('"').trim_matches('\'').to_string();
                }
                "cpp" => {
                    *imp = imp.trim_matches('"').trim_matches('\'')
                        .trim_start_matches('<').trim_end_matches('>').to_string();
                }
                "php" => { *imp = imp.trim_matches('"').trim_matches('\'').to_string(); }
                _ => {}
            }
        }
        imports.retain(|i| !i.is_empty());
        imports.sort();
        imports.dedup();
        imports
    }

    fn extract_rust_imports<'tree>(&self, root: Node<'tree>, sb: &'tree [u8]) -> Vec<String> {
        let q = match self.queries.get("rust").and_then(|m| m.get("import")) {
            Some(q) => q, None => return vec![]
        };
        use std::collections::BTreeMap;
        let mut paths: BTreeMap<usize, String> = BTreeMap::new();
        let mut names: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        let mut cursor = QueryCursor::new();
        let cnames = q.capture_names().to_vec();
        let mut caps = cursor.captures(q, root, sb);
        while let Some((m, _)) = caps.next() {
            for c in m.captures {
                let cn = cnames[c.index as usize];
                let text = node_text(c.node, sb).to_string();
                let line = c.node.start_position().row + 1;
                if cn == "path" { paths.insert(line, text); }
                else if cn == "name" { names.entry(line).or_default().push(text); }
            }
        }
        let mut lines: Vec<usize> = paths.keys().chain(names.keys()).copied().collect();
        lines.sort(); lines.dedup();
        let mut results = Vec::new();
        for l in lines {
            if let Some(p) = paths.get(&l) { results.push(p.clone()); }
            else if let Some(ns) = names.get(&l) { for n in ns { results.push(n.clone()); } }
        }
        results
    }

    // -----------------------------------------------------------------------
    // Call extraction
    // -----------------------------------------------------------------------

    /// Extract call expressions and classify the reference kind.
    fn extract_calls<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
        lang: &str,
    ) -> Vec<(String, usize, String)> {
        let captures = self.run_query(lang, "call", root, source_bytes);

        let mut calls: Vec<(String, usize, String)> = captures
            .into_iter()
            .map(|rc| {
                let kind = call_reference_kind_by_name(&rc.name);
                (rc.name, rc.start_row, kind)
            })
            .collect();

        calls.sort();
        calls.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        calls
    }

    // -----------------------------------------------------------------------
    // JS/TS import bindings
    // -----------------------------------------------------------------------

    /// Walk the AST for `import_statement` (ES modules) and `call_expression`
    /// (CommonJS `require`) nodes, producing structured binding records.
    fn extract_js_ts_import_bindings<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
    ) -> Vec<JsImportBinding> {
        let mut bindings = Vec::new();
        let nodes = walk_tree(root);

        // --- ES module imports -------------------------------------------------
        for node in &nodes {
            if node.kind() != "import_statement" {
                continue;
            }
            let line = node.start_position().row;

            // Source module string (e.g. "react", "./foo").
            let source_node = node.child_by_field_name("source");
            let module = source_node
                .map(|n| {
                    let raw = node_text(n, source_bytes);
                    raw.trim_matches('"').trim_matches('\'').to_string()
                })
                .unwrap_or_default();

            // The import clause carries the identifiers.
            let mut clause_cursor = node.walk();
            for child in node.children(&mut clause_cursor) {
                if child.kind() != "import_clause" {
                    continue;
                }

                // 1) Default import  e.g. `import React from 'react'`
                if let Some(name_node) = child.child_by_field_name("name") {
                    let local_name = node_text(name_node, source_bytes).to_string();
                    bindings.push(JsImportBinding {
                        local_name,
                        imported_name: "default".to_string(),
                        module: module.clone(),
                        line,
                        kind: "es_default".to_string(),
                    });
                }

                // 2) Named imports & namespace import
                let mut inner = child.walk();
                for sub in child.children(&mut inner) {
                    match sub.kind() {
                        "named_imports" => {
                            Self::collect_named_imports(&sub, source_bytes, &module, line, &mut bindings);
                        }
                        "namespace_import" => {
                            if let Some(ns) = sub.child_by_field_name("name") {
                                let local_name = node_text(ns, source_bytes).to_string();
                                bindings.push(JsImportBinding {
                                    local_name,
                                    imported_name: "*".to_string(),
                                    module: module.clone(),
                                    line,
                                    kind: "es_namespace".to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // --- CommonJS require --------------------------------------------------
        for node in &nodes {
            if node.kind() != "call_expression" {
                continue;
            }
            let callee = match node.child(0) {
                Some(c) => c,
                None => continue,
            };
            let callee_text = node_text(callee, source_bytes);
            if callee_text != "require" {
                continue;
            }
            let module_arg = match node.child_by_field_name("arguments").or_else(|| node.child(1)) {
                Some(a) => a,
                None => continue,
            };
            let module = node_text(module_arg, source_bytes)
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            let line = node.start_position().row;

            // Walk up to the enclosing variable declarator so we can name the
            // local binding.
            let parent = node.parent();
            let grand = parent.and_then(|p| p.parent());
            match grand {
                Some(decl) if decl.kind() == "variable_declarator" => {
                    // Simple: `const x = require(...)`
                    if let Some(lhs) = decl.child_by_field_name("name") {
                        if lhs.kind() == "identifier" {
                            let local = node_text(lhs, source_bytes).to_string();
                            bindings.push(JsImportBinding {
                                local_name: local,
                                imported_name: "default".to_string(),
                                module: module.clone(),
                                line,
                                kind: "cjs_default".to_string(),
                            });
                        } else if lhs.kind() == "object_pattern" {
                            Self::collect_destructured_require(
                                &lhs, source_bytes, &module, line, &mut bindings,
                            );
                        }
                    }
                }
                Some(assign) if assign.kind() == "assignment_expression" => {
                    if let Some(lhs) = assign.child(0) {
                        let local = node_text(lhs, source_bytes).to_string();
                        bindings.push(JsImportBinding {
                            local_name: local,
                            imported_name: "default".to_string(),
                            module: module.clone(),
                            line,
                            kind: "cjs_default".to_string(),
                        });
                    }
                }
                _ => {}
            }
        }

        bindings
    }

    /// Collect named imports from a `named_imports` subtree.
    fn collect_named_imports<'tree>(
        node: &Node<'tree>,
        source_bytes: &'tree [u8],
        module: &str,
        line: usize,
        bindings: &mut Vec<JsImportBinding>,
    ) {
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            if child.kind() != "import_specifier" {
                continue;
            }
            let imported = child
                .child_by_field_name("name")
                .map(|n| node_text(n, source_bytes).to_string())
                .unwrap_or_default();
            let local = child
                .child_by_field_name("alias")
                .map(|n| node_text(n, source_bytes).to_string())
                .unwrap_or_else(|| imported.clone());

            bindings.push(JsImportBinding {
                local_name: local,
                imported_name: imported,
                module: module.to_string(),
                line,
                kind: "es_named".to_string(),
            });
        }
    }

    /// Collect destructured require bindings from an object pattern.
    fn collect_destructured_require<'tree>(
        node: &Node<'tree>,
        source_bytes: &'tree [u8],
        module: &str,
        line: usize,
        bindings: &mut Vec<JsImportBinding>,
    ) {
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            let local = match child.kind() {
                "shorthand_property_identifier_pattern"
                | "property_identifier_pattern" => {
                    node_text(child, source_bytes).to_string()
                }
                _ => continue,
            };
            bindings.push(JsImportBinding {
                local_name: local.clone(),
                imported_name: local,
                module: module.to_string(),
                line,
                kind: "cjs_named".to_string(),
            });
        }
    }

    // -----------------------------------------------------------------------
    // JS/TS export bindings
    // -----------------------------------------------------------------------

    /// Walk for `export_statement` (ES modules) and `assignment_expression`
    /// (CommonJS `module.exports` / `exports.xxx`).
    fn extract_js_ts_export_bindings<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
    ) -> Vec<JsExportBinding> {
        let mut exports = Vec::new();
        let nodes = walk_tree(root);
        let mut seen = std::collections::HashSet::new();

        // --- ES module exports -------------------------------------------------
        for node in &nodes {
            if node.kind() != "export_statement" {
                continue;
            }
            let line = node.start_position().row;

            // 1) Named declaration  e.g. `export const foo = 1`
            if let Some(decl) = node.child_by_field_name("declaration") {
                Self::collect_export_from_declaration(
                    &decl, source_bytes, None, line, &mut exports, &mut seen,
                );
                continue;
            }

            // 2) Default export  e.g. `export default function() {}`
            if let Some(default_node) = node.child_by_field_name("default") {
                let name = node_text(default_node, source_bytes).to_string();
                let is_named_decl = default_node.is_named()
                    && default_node.kind() != "function_declaration"
                    && default_node.kind() != "class_declaration";
                let binding = JsExportBinding {
                    exported_name: "default".to_string(),
                    source_name: if is_named_decl { Some(name) } else { None },
                    module: None,
                    line,
                    kind: "es_default".to_string(),
                };
                if seen.insert((binding.exported_name.clone(), binding.line)) {
                    exports.push(binding);
                }
                continue;
            }

            // 3) Export clause (named re-export or local re-export)
            let mut clause_cursor = node.walk();
            for child in node.children(&mut clause_cursor) {
                if child.kind() == "export_clause" {
                    let source_node = node.child_by_field_name("source");
                    let module = source_node.map(|n| {
                        node_text(n, source_bytes)
                            .trim_matches('"')
                            .trim_matches('\'')
                            .to_string()
                    });

                    Self::collect_export_specifiers(
                        &child, source_bytes, module, line, &mut exports, &mut seen,
                    );
                }
            }
        }

        // --- CommonJS exports --------------------------------------------------
        for node in &nodes {
            if node.kind() != "assignment_expression" {
                continue;
            }
            let lhs = match node.child(0) {
                Some(c) => c,
                None => continue,
            };
            let rhs = match node.child(1).or_else(|| node.child(2)) {
                Some(c) => c,
                None => continue,
            };
            let line = node.start_position().row;

            if lhs.kind() != "member_expression" {
                continue;
            }
            let lhs_text = node_text(lhs, source_bytes);

            // `module.exports = <value>`
            if lhs_text == "module.exports" {
                let source_name = node_text(rhs, source_bytes).to_string();
                let binding = JsExportBinding {
                    exported_name: "module.exports".to_string(),
                    source_name: Some(source_name),
                    module: None,
                    line,
                    kind: "cjs".to_string(),
                };
                if seen.insert((binding.exported_name.clone(), binding.line)) {
                    exports.push(binding);
                }
                continue;
            }

            // `exports.xxx = <value>`
            if let Some(rest) = lhs_text.strip_prefix("exports.") {
                let source_name = node_text(rhs, source_bytes).to_string();
                let binding = JsExportBinding {
                    exported_name: rest.to_string(),
                    source_name: Some(source_name),
                    module: None,
                    line,
                    kind: "cjs".to_string(),
                };
                if seen.insert((binding.exported_name.clone(), binding.line)) {
                    exports.push(binding);
                }
            }
        }

        exports
    }

    /// Extract exported names from a declaration node.
    fn collect_export_from_declaration<'tree>(
        node: &Node<'tree>,
        source_bytes: &'tree [u8],
        module: Option<String>,
        line: usize,
        exports: &mut Vec<JsExportBinding>,
        seen: &mut std::collections::HashSet<(String, usize)>,
    ) {
        let kind = node.kind();
        let name_node: Option<Node<'tree>> = match kind {
            "function_declaration"
            | "class_declaration"
            | "generator_function_declaration" => {
                node.child_by_field_name("name")
            }
            "lexical_declaration" | "variable_declaration" => {
                let mut cur = node.walk();
                node.children(&mut cur)
                    .find(|c| c.kind() == "variable_declarator")
                    .and_then(|decl| decl.child_by_field_name("name"))
            }
            _ => None,
        };

        if let Some(nn) = name_node {
            let name = node_text(nn, source_bytes).to_string();
            let binding = JsExportBinding {
                exported_name: name,
                source_name: None,
                module,
                line,
                kind: "es_named".to_string(),
            };
            if seen.insert((binding.exported_name.clone(), binding.line)) {
                exports.push(binding);
            }
        }
    }

    /// Collect export specifiers from an `export_clause`.
    ///
    /// Grammar: `export_specifier (name as alias?)` where `name` is the local
    /// identifier and `alias` is the externally visible name (if present).
    fn collect_export_specifiers<'tree>(
        node: &Node<'tree>,
        source_bytes: &'tree [u8],
        module: Option<String>,
        line: usize,
        exports: &mut Vec<JsExportBinding>,
        seen: &mut std::collections::HashSet<(String, usize)>,
    ) {
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            if child.kind() != "export_specifier" {
                continue;
            }
            // `name` is the local/source identifier, `alias` is the exported
            // name (after the `as` keyword).
            let name = child
                .child_by_field_name("name")
                .map(|n| node_text(n, source_bytes).to_string())
                .unwrap_or_default();
            let alias = child
                .child_by_field_name("alias")
                .map(|n| node_text(n, source_bytes).to_string());

            let exported_name = alias.clone().unwrap_or_else(|| name.clone());
            let binding = JsExportBinding {
                exported_name,
                source_name: Some(name),
                module: module.clone(),
                line,
                kind: "es_named".to_string(),
            };
            if seen.insert((binding.exported_name.clone(), binding.line)) {
                exports.push(binding);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Extra symbol passes for JS / TS
    // -----------------------------------------------------------------------

    /// Find method definitions inside object literals (e.g. `{ foo() {} }`).
    fn extract_object_literal_methods<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
    ) -> Vec<Symbol> {
        let file = &self.current_file;
        let mut symbols = Vec::new();
        let nodes = walk_tree(root);

        for node in &nodes {
            if node.kind() != "pair" {
                continue;
            }
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                if child.kind() != "method_definition" {
                    continue;
                }
                let key = node
                    .child_by_field_name("key")
                    .or_else(|| node.child(0));
                if let Some(k) = key {
                    let name = node_text(k, source_bytes)
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    let pos = k.start_position();
                    let end = k.end_position();
                    let id = format!("{}:{}:{}", file, name, pos.row);
                    symbols.push(Symbol::new(
                        id,
                        name,
                        "method".to_string(),
                        file.to_string(),
                        pos.row,
                        end.row,
                        pos.column,
                        "public".to_string(),
                        String::new(),
                        String::new(),
                    ));
                }
                break;
            }
        }

        symbols
    }

    /// Find anonymous functions / classes assigned to variables
    /// (e.g. `const foo = function() {}` or `let bar = () => {}`).
    fn extract_anonymous_symbols<'tree>(
        &self,
        root: Node<'tree>,
        source_bytes: &'tree [u8],
    ) -> Vec<Symbol> {
        let file = &self.current_file;
        let mut symbols = Vec::new();
        let nodes = walk_tree(root);

        for node in &nodes {
            if node.kind() != "variable_declarator" {
                continue;
            }
            let name_node = node.child_by_field_name("name");
            let value_node = node.child_by_field_name("value");

            if let (Some(name_n), Some(value_n)) = (name_node, value_node) {
                let sym_kind = match value_n.kind() {
                    "function" | "function_expression" => "function",
                    "arrow_function" => "arrow_function",
                    "class" | "class_expression" => "class",
                    _ => continue,
                };

                let name = node_text(name_n, source_bytes).to_string();
                let pos = value_n.start_position();
                let end = value_n.end_position();
                let id = format!("{}:{}:{}", file, name, pos.row);
                symbols.push(Symbol::new(
                    id,
                    name,
                    sym_kind.to_string(),
                    file.to_string(),
                    pos.row,
                    end.row,
                    pos.column,
                    "public".to_string(),
                    String::new(),
                    String::new(),
                ));
            }
        }

        symbols
    }

    /// Find exported function / class expressions (e.g. `export default function() {}`).
    fn extract_exported_function_expressions<'tree>(
        &self,
        root: Node<'tree>,
        _source_bytes: &'tree [u8],
    ) -> Vec<Symbol> {
        let file = &self.current_file;
        let mut symbols = Vec::new();
        let nodes = walk_tree(root);

        for node in &nodes {
            if node.kind() != "export_statement" {
                continue;
            }
            let default_child = node.child_by_field_name("default");
            let decl_child = node.child_by_field_name("declaration");

            if let Some(def) = default_child {
                let sym_kind = match def.kind() {
                    "function" | "function_expression" | "generator_function" => "function",
                    "arrow_function" => "arrow_function",
                    "class" | "class_expression" => "class",
                    _ => continue,
                };

                let name = format!("default_{}", sym_kind);
                let pos = def.start_position();
                let end = def.end_position();
                let id = format!("{}:{}:{}", file, name, pos.row);
                symbols.push(Symbol::new(
                    id,
                    name,
                    sym_kind.to_string(),
                    file.to_string(),
                    pos.row,
                    end.row,
                    pos.column,
                    "public".to_string(),
                    String::new(),
                    String::new(),
                ));
            } else if let Some(decl) = decl_child {
                if decl.child_by_field_name("name").is_some() {
                    continue;
                }
                let sym_kind = match decl.kind() {
                    "function_declaration" => "function",
                    "class_declaration" => "class",
                    _ => continue,
                };
                let name = format!("exported_{}", sym_kind);
                let pos = decl.start_position();
                let end = decl.end_position();
                let id = format!("{}:{}:{}", file, name, pos.row);
                symbols.push(Symbol::new(
                    id,
                    name,
                    sym_kind.to_string(),
                    file.to_string(),
                    pos.row,
                    end.row,
                    pos.column,
                    "public".to_string(),
                    String::new(),
                    String::new(),
                ));
            }
        }

        symbols
    }
}

// ===========================================================================
// Module-level helpers
// ===========================================================================

/// Get the UTF-8 text of a syntax node.  Returns an empty string on encoding
/// errors (should not happen for valid source code).
pub fn node_text<'a>(node: Node<'a>, source_bytes: &'a [u8]) -> &'a str {
    node.utf8_text(source_bytes).unwrap_or("")
}

/// Returns `true` if `child` is fully contained within `parent`.
pub fn within<'tree>(child: Node<'tree>, parent: Node<'tree>) -> bool {
    child.start_byte() >= parent.start_byte() && child.end_byte() <= parent.end_byte()
}

/// Classify a call-reference kind based on the name string.
fn call_reference_kind_by_name(name: &str) -> String {
    if name.contains('.') {
        "member".to_string()
    } else if name.contains("::") {
        "scoped".to_string()
    } else {
        "direct".to_string()
    }
}

/// Classify the reference kind of a call expression node by inspecting its
/// children.
pub fn call_reference_kind<'tree>(node: Node<'tree>) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "member_expression" => return "member".to_string(),
            "scoped_identifier" | "qualified_name" => return "scoped".to_string(),
            _ => {}
        }
    }
    "direct".to_string()
}

/// Walk the entire subtree rooted at `root` in pre-order and return all nodes.
pub fn walk_tree<'tree>(root: Node<'tree>) -> Vec<Node<'tree>> {
    let mut nodes = Vec::new();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        nodes.push(node);
        // Push children in reverse order so they are visited left-to-right
        // when popped.
        let mut cursor = node.walk();
        let mut children: Vec<Node<'tree>> = node.children(&mut cursor).collect();
        children.reverse();
        stack.extend(children);
    }

    nodes
}

/// Find the first child of `node` whose kind equals `kind`.
pub fn first_child_of_type<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

/// Build a one-line signature from a node: the first non-empty line, trimmed.
pub fn signature<'a>(node: Node<'a>, source_bytes: &'a [u8]) -> String {
    let text = node_text(node, source_bytes);
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

// ===========================================================================
// Private helpers
// ===========================================================================

/// Check whether the tree rooted at `root` exceeds `max_depth` levels of
/// nesting.  Uses iterative cursor traversal to avoid stack overflow.
fn has_excessive_nesting<'tree>(root: Node<'tree>, max_depth: u32) -> bool {
    let mut cursor = root.walk();
    loop {
        if cursor.depth() > max_depth {
            return true;
        }
        if cursor.goto_first_child() {
            continue;
        }
        if cursor.goto_next_sibling() {
            continue;
        }
        loop {
            if !cursor.goto_parent() {
                return false;
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
}
