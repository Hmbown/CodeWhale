//! Tree-sitter S-expression queries for multi-language code analysis.

/// Return all registered queries as `(language, query_type, s-expression)` tuples.
pub fn queries() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        // ── Python ──
        ("python", "function", "(function_definition name: (identifier) @name) @definition.function\n(decorated_definition (function_definition name: (identifier) @name)) @definition.function\n(class_definition body: (block (function_definition name: (identifier) @name))) @definition.method\n(assignment left: (identifier) @name right: (lambda)) @definition.lambda"),
        ("python", "class", "(class_definition name: (identifier) @name) @definition.class\n(decorated_definition (class_definition name: (identifier) @name)) @definition.class"),
        ("python", "import", "(import_statement name: (dotted_name) @name)\n(import_statement name: (aliased_import name: (dotted_name) @name))\n(import_from_statement module_name: (dotted_name) @name)\n(import_from_statement module_name: (relative_import) @name)"),
        ("python", "call", "(call function: (identifier) @name) @reference.call\n(call function: (attribute attribute: (identifier) @name)) @reference.call"),

        // ── JavaScript ──
        ("javascript", "function", "(function_declaration name: (identifier) @name) @definition.function\n(variable_declarator name: (identifier) @name value: (arrow_function)) @definition.function\n(variable_declarator name: (identifier) @name value: (function_expression)) @definition.function\n(method_definition name: (property_identifier) @name) @definition.method"),
        ("javascript", "anonymous_function", "(arrow_function) @definition.anonymous_function\n(function_expression) @definition.anonymous_function"),
        ("javascript", "class", "(class_declaration name: (identifier) @name) @definition.class"),
        ("javascript", "import", "(import_statement source: (string) @source)\n(import_specifier name: (identifier) @name)\n(import_clause (identifier) @name)"),
        ("javascript", "call", "(call_expression function: (identifier) @name) @reference.call\n(call_expression function: (member_expression property: (property_identifier) @name)) @reference.call"),

        // ── TypeScript ──
        ("typescript", "function", "(function_declaration name: (identifier) @name) @definition.function\n(variable_declarator name: (identifier) @name value: (arrow_function)) @definition.function\n(method_definition name: (property_identifier) @name) @definition.method"),
        ("typescript", "anonymous_function", "(arrow_function) @definition.anonymous_function\n(function_expression) @definition.anonymous_function"),
        ("typescript", "class", "(class_declaration name: (_) @name) @definition.class"),
        ("typescript", "import", "(import_statement source: (string) @source)\n(import_specifier name: (identifier) @name)\n(import_clause (identifier) @name)"),
        ("typescript", "call", "(call_expression function: (identifier) @name) @reference.call\n(call_expression function: (member_expression property: (property_identifier) @name)) @reference.call"),

        // ── Go ──
        ("go", "function", "(function_declaration name: (identifier) @name) @definition.function\n(method_declaration name: (field_identifier) @name) @definition.method"),
        ("go", "class", "(type_spec name: (type_identifier) @name type: (struct_type)) @definition.struct\n(type_spec name: (type_identifier) @name type: (interface_type)) @definition.interface"),
        ("go", "import", "(import_spec path: (interpreted_string_literal) @path)"),
        ("go", "call", "(call_expression function: (identifier) @name) @reference.call\n(call_expression function: (selector_expression field: (field_identifier) @name)) @reference.call"),

        // ── Rust ──
        ("rust", "function", "(function_item name: (identifier) @name) @definition.function\n(function_signature_item name: (identifier) @name) @definition.trait_method"),
        ("rust", "class", "(struct_item name: (type_identifier) @name) @definition.struct\n(enum_item name: (type_identifier) @name) @definition.enum\n(trait_item name: (type_identifier) @name) @definition.trait\n(impl_item type: (type_identifier) @name) @definition.impl\n(type_item name: (type_identifier) @name) @definition.type\n(mod_item name: (identifier) @name) @definition.module"),
        ("rust", "import", "(use_declaration argument: (scoped_identifier path: (identifier) @path name: (identifier) @name))\n(use_declaration argument: (scoped_use_list path: (identifier) @path))\n(extern_crate_declaration name: (identifier) @name)\n(use_declaration argument: (identifier) @name)"),
        ("rust", "call", "(call_expression function: (identifier) @name) @reference.call\n(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call\n(call_expression function: (scoped_identifier name: (identifier) @name)) @reference.call"),

        // ── HTML ──
        ("html", "function", ""),
        ("html", "class", ""),
        ("html", "import", ""),
        ("html", "call", ""),

        // ── CSS ──
        ("css", "function", ""),
        ("css", "class", ""),
        ("css", "import", ""),
        ("css", "call", ""),

        // ── JSON ──
        ("json", "function", ""),
        ("json", "class", ""),
        ("json", "import", ""),
        ("json", "call", ""),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_query_count() {
        assert_eq!(queries().len(), 34);
    }

    #[test]
    fn test_all_sexp_balanced() {
        for (lang, qtype, sexp) in &queries() {
            if sexp.is_empty() { continue; }
            let open = sexp.matches('(').count();
            let close = sexp.matches(')').count();
            assert_eq!(open, close, "unbalanced: ({}, {}) o={} c={}", lang, qtype, open, close);
        }
    }

    #[test]
    fn test_eight_languages_present() {
        let langs: std::collections::HashSet<&str> = queries().iter().map(|(l, _, _)| *l).collect();
        for l in &["python","javascript","typescript","go","rust","html","css","json"] {
            assert!(langs.contains(l), "missing language: {}", l);
        }
    }
}
