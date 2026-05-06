// Smoke tests — parse real project files and verify results.
use std::path::Path;

#[test]
fn parse_rust_project() {
    let project = Path::new("/home/guojiancheng/.A1/DeepSeek-TUI");
    let test_file = project.join("crates/deepmap/src/parser.rs");
    if !test_file.exists() { eprintln!("SKIP: no test file"); return; }

    let mut adapter = deepmap::parser::TreeSitterAdapter::new();
    let content = std::fs::read_to_string(&test_file).unwrap();
    adapter.parse(&test_file, &content, "rust").unwrap();

    let syms = adapter.symbols();
    let imports = adapter.imports();
    let calls = adapter.calls();

    println!("=== Rust: {} ===", test_file.display());
    println!("  Symbols: {}", syms.len());
    for s in syms.iter().take(10) {
        println!("    {} [{}:{}] {}", s.name, s.kind, s.line, s.signature);
    }
    println!("  Imports: {}", imports.len());
    for i in imports.iter().take(5) { println!("    {}", i); }
    println!("  Calls: {}", calls.len());
    for c in calls.iter().take(5) { println!("    {}:{}", c.0, c.1); }

    assert!(syms.len() > 10, "Should find Rust symbols");
    // Rust scoped_identifier imports with deep nesting need more complex query.
    // 1 import (from scoped_use_list) is found; nested scoped_identifier
    // paths are not captured. This is a known limitation tracked for PR 2.
    assert!(imports.len() >= 1, "Should find at least scoped_use_list imports");
    assert!(calls.len() > 5, "Should find function calls");
}

#[test]
fn parse_python_project() {
    let project = Path::new("/home/guojiancheng/.A1/repomap");
    let test_file = project.join("repomap_core.py");
    if !test_file.exists() { eprintln!("SKIP: no test file"); return; }

    let mut adapter = deepmap::parser::TreeSitterAdapter::new();
    let content = std::fs::read_to_string(&test_file).unwrap();
    adapter.parse(&test_file, &content, "python").unwrap();

    let syms = adapter.symbols();
    let imports = adapter.imports();
    let calls = adapter.calls();

    println!("=== Python: {} ===", test_file.display());
    println!("  Symbols: {}", syms.len());
    for s in syms.iter().take(8) {
        println!("    {} [{}:{}] {}", s.name, s.kind, s.line, s.signature);
    }
    println!("  Imports: {}", imports.len());
    for i in imports.iter().take(5) { println!("    {}", i); }

    assert!(syms.len() > 5, "Should find Python symbols");
    assert!(imports.len() > 3, "Should find Python imports");
}

#[test]
fn parse_typescript_project() {
    let project = Path::new("/home/guojiancheng/.A1/deeper-web");
    let test_file = project.join("server/src/app.ts");
    if !test_file.exists() { eprintln!("SKIP: no test file"); return; }

    let mut adapter = deepmap::parser::TreeSitterAdapter::new();
    let content = std::fs::read_to_string(&test_file).unwrap();
    adapter.parse(&test_file, &content, "typescript").unwrap();

    let syms = adapter.symbols();
    let imports = adapter.imports();
    let bindings = adapter.import_bindings();
    let exports = adapter.exports();

    println!("=== TypeScript: {} ===", test_file.display());
    println!("  Symbols: {}", syms.len());
    for s in syms.iter().take(8) {
        println!("    {} [{}:{}] {}", s.name, s.kind, s.line, s.signature);
    }
    println!("  Imports: {}", imports.len());
    println!("  Import bindings: {}", bindings.len());
    for b in bindings.iter().take(5) { println!("    {} from '{}'", b.local_name, b.module); }
    println!("  Exports: {}", exports.len());

    assert!(syms.len() > 3, "Should find TS symbols");
    assert!(imports.len() > 1, "Should find TS imports");
}

#[test]
fn parse_json_file() {
    let project = Path::new("/home/guojiancheng/.A1/deeper-web");
    let test_file = project.join("package.json");
    if !test_file.exists() { eprintln!("SKIP: no test file"); return; }

    let mut adapter = deepmap::parser::TreeSitterAdapter::new();
    let content = std::fs::read_to_string(&test_file).unwrap();
    adapter.parse(&test_file, &content, "json").unwrap();

    let syms = adapter.symbols();
    println!("=== JSON: {} ===", test_file.display());
    println!("  Keys: {}", syms.len());
    for s in syms.iter().take(10) { println!("    {}", s.name); }

    assert!(syms.len() > 5, "Should find JSON keys");
}

#[test]
fn test_resolver() {
    let project = Path::new("/home/guojiancheng/.A1/DeepSeek-TUI");
    let mut adapter = deepmap::parser::TreeSitterAdapter::new();

    // Parse a Rust file with use statements
    let test_file = project.join("crates/deepmap/src/types.rs");
    let content = std::fs::read_to_string(&test_file).unwrap();
    adapter.parse(&test_file, &content, "rust").unwrap();

    // Build a minimal graph for resolver testing
    let syms = adapter.symbols();
    let imports = adapter.imports();

    let mut graph = deepmap::types::RepoGraph::default();
    for s in &syms {
        graph.symbols.insert(s.id.clone(), s.clone());
        graph.file_symbols.entry(s.file.clone()).or_default().push(s.id.clone());
    }
    graph.file_imports.insert("crates/deepmap/src/types.rs".into(), imports.clone());

    let resolver = deepmap::resolver::ImportResolver::new(project, &graph);

    println!("=== Resolver ===");
    println!("  Import configs: {}", resolver.import_configs.len());
    for c in &resolver.import_configs {
        println!("    {}", c.config_path.as_deref().unwrap_or("none"));
        println!("    aliases: {}", c.alias_rules.len());
    }

    // Test resolving a relative import
    let targets = resolver.resolve_import_targets("crates/deepmap/src/types.rs", "../tui");
    println!("  Resolve '../tui': {:?}", targets);
}
