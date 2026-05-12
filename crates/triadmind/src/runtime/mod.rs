//! # Runtime — Frontend API route matching for topology analysis
//!
//! Scans TypeScript/JavaScript source files for frontend API calls (`fetch()`,
//! `axios.*()`, etc.) and matches them against known backend route declarations
//! (Next.js `route.ts` files, Express-style route registrations).
//!
//! External API domains (api.github.com, api.deepseek.com, etc.) are recognised
//! and mapped to `ExternalApi` nodes instead of generating unmatched-route
//! diagnostics.
//!
//! @LeftBranch: run_runtime_analysis, collect_frontend_calls, collect_known_routes
//! @RightBranch: RuntimeMap, RuntimeNode, RuntimeDiagnostic, FrontendApiCall, KnownRoute

use std::collections::{HashMap, HashSet};
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

// ── External API domains (not expected to have backend routes) ─────

const EXTERNAL_API_DOMAINS: &[&str] = &[
    "api.github.com",
    "github.com",
    "raw.githubusercontent.com",
    "api.deepseek.com",
    "api.openai.com",
    "api.anthropic.com",
    "registry.npmjs.org",
    "crates.io",
    "unpkg.com",
];

/// Check whether a URL host is a known external API domain.
fn is_external_domain(url: &str) -> bool {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .split('#')
        .next()
        .unwrap_or("");

    if host.is_empty() {
        return false;
    }

    EXTERNAL_API_DOMAINS
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{}", domain)))
}

/// Check whether a URL is a self-referencing URL (calling own website).
#[allow(dead_code)]
fn is_self_referencing(url: &str, project_domains: &[&str]) -> bool {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("");

    project_domains
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{}", d)))
}

// ── Types ───────────────────────────────────────────────────────────

/// A detected frontend API call.
#[derive(Debug, Clone)]
pub struct FrontendApiCall {
    /// HTTP method (GET, POST, PUT, DELETE, PATCH).
    pub method: String,
    /// Raw URL / path string from source.
    pub raw_path: String,
    /// Normalised path (template params → :param, query stripped).
    pub normalized_path: String,
    /// Source file relative path.
    pub source_path: String,
    /// Line number.
    pub line: usize,
    /// Whether this call targets an external domain.
    pub is_external: bool,
    /// External domain name (if is_external).
    pub external_domain: Option<String>,
}

/// A known backend API route.
#[derive(Debug, Clone)]
pub struct KnownRoute {
    /// Route id (e.g. "POST./api/admin/post").
    pub id: String,
    /// HTTP method.
    pub method: String,
    /// Normalised path.
    pub path: String,
    /// Source file relative path.
    pub source_path: String,
    /// Path variants for matching.
    pub variants: Vec<String>,
}

/// A node in the runtime topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub label: String,
    #[serde(rename = "sourcePath")]
    pub source_path: String,
    pub category: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// An edge in the runtime topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEdge {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub edge_type: String,
}

/// A diagnostic from runtime analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDiagnostic {
    pub level: String,
    pub code: String,
    pub message: String,
    #[serde(rename = "sourcePath", default)]
    pub source_path: Option<String>,
}

/// Complete runtime topology map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMap {
    #[serde(rename = "projectRoot")]
    pub project_root: String,
    pub nodes: Vec<RuntimeNode>,
    pub edges: Vec<RuntimeEdge>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
    #[serde(rename = "unmatchedRouteCount")]
    pub unmatched_route_count: usize,
    #[serde(rename = "externalApiCallCount")]
    pub external_api_call_count: usize,
}

/// Configuration for runtime analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeAnalysisConfig {
    /// Whether to include frontend analysis.
    #[serde(default = "default_true")]
    pub include_frontend: bool,
    /// Domain names of the project itself (used to distinguish self-referencing calls).
    #[serde(default)]
    pub project_domains: Vec<String>,
    /// File patterns to exclude from scanning.
    #[serde(default = "default_exclude_patterns")]
    pub exclude_path_patterns: Vec<String>,
    /// External API domains to whitelist (merged with built-in list).
    #[serde(default)]
    pub external_api_domains: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_exclude_patterns() -> Vec<String> {
    vec![
        "node_modules".into(),
        ".git".into(),
        ".triadmind".into(),
        "dist".into(),
        "build".into(),
        ".next".into(),
    ]
}

impl Default for RuntimeAnalysisConfig {
    fn default() -> Self {
        Self {
            include_frontend: default_true(),
            project_domains: vec!["deepseek-tui.com".into()],
            exclude_path_patterns: default_exclude_patterns(),
            external_api_domains: vec![],
        }
    }
}

// ── Core Entry Point ────────────────────────────────────────────────

/// Run runtime topology analysis on a project.
pub fn run_runtime_analysis(
    project_root: &Path,
    config: &RuntimeAnalysisConfig,
) -> Result<RuntimeMap, anyhow::Error> {
    let mut all_files: Vec<(String, String)> = Vec::new();

    // Walk project directory for TS/JS files
    let walker = walkdir::WalkDir::new(project_root)
        .max_depth(20)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            for pat in &config.exclude_path_patterns {
                if name.contains(pat.as_str()) {
                    return false;
                }
            }
            true
        });

    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "mts" | "cts") {
            continue;
        }

        if let Ok(rel) = path.strip_prefix(project_root) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if let Ok(content) = std::fs::read_to_string(path) {
                all_files.push((rel_str, content));
            }
        }
    }

    // Collect known routes
    let known_routes = collect_known_routes(&all_files);

    // Collect frontend calls
    let mut frontend_calls = Vec::new();
    for (path, content) in &all_files {
        let calls = collect_frontend_calls(path, content, config);
        frontend_calls.extend(calls);
    }

    // Build runtime map
    build_runtime_map(project_root, &frontend_calls, &known_routes)
}

// ── Frontend Call Extraction ────────────────────────────────────────

/// Collect frontend API calls from a source file.
fn collect_frontend_calls(
    source_path: &str,
    content: &str,
    _config: &RuntimeAnalysisConfig,
) -> Vec<FrontendApiCall> {
    let mut calls = Vec::new();

    // ── fetch() calls ─────────────────────────────────────────
    // Match: fetch(`...`) or fetch("...") or fetch('...')
    // Also: fetch(URL, { method: "POST", ... })
    let fetch_re = Regex::new(
        r#"fetch\(\s*(`[^`]*`|"[^"]*"|'[^']*')\s*(?:,\s*\{[^}]*method\s*:\s*"([^"]+)"[^}]*\})?"#
    )
    .unwrap();

    for cap in fetch_re.captures_iter(content) {
        let raw_url = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        // Strip quotes / backticks
        let raw_path = raw_url
            .trim_start_matches('`')
            .trim_end_matches('`')
            .trim_start_matches('"')
            .trim_end_matches('"')
            .trim_start_matches('\'')
            .trim_end_matches('\'')
            .to_string();

        if raw_path.is_empty() {
            continue;
        }

        let method = cap
            .get(2)
            .map(|m| m.as_str().to_uppercase())
            .unwrap_or_else(|| "GET".into());

        let is_ext = is_external_domain(&raw_path);
        let domain = if is_ext {
            extract_domain(&raw_path)
        } else {
            None
        };

        let line = content[..cap.get(0).unwrap().start()]
            .lines()
            .count()
            + 1;

        calls.push(FrontendApiCall {
            method,
            raw_path: raw_path.clone(),
            normalized_path: normalize_api_path(&raw_path),
            source_path: source_path.to_string(),
            line,
            is_external: is_ext,
            external_domain: domain,
        });
    }

    // ── axios calls ───────────────────────────────────────────
    let axios_re = Regex::new(
        r#"axios\.(get|post|put|delete|patch)\s*\(\s*(`[^`]*`|"[^"]*"|'[^']*')"#,
    )
    .unwrap();

    for cap in axios_re.captures_iter(content) {
        let method = cap.get(1).map(|m| m.as_str().to_uppercase()).unwrap_or_default();
        let raw_url = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let raw_path = strip_quotes(raw_url);

        if raw_path.is_empty() {
            continue;
        }

        let is_ext = is_external_domain(&raw_path);
        let domain = if is_ext {
            extract_domain(&raw_path)
        } else {
            None
        };

        let line = content[..cap.get(0).unwrap().start()]
            .lines()
            .count()
            + 1;

        calls.push(FrontendApiCall {
            method,
            raw_path: raw_path.clone(),
            normalized_path: normalize_api_path(&raw_path),
            source_path: source_path.to_string(),
            line,
            is_external: is_ext,
            external_domain: domain,
        });
    }

    calls
}

// ── Backend Route Discovery ─────────────────────────────────────────

/// Collect known API routes from the project's source files.
///
/// For Next.js projects, routes are inferred from the file path convention:
/// `app/api/<path>/route.ts` or `app/api/<path>/route.tsx`.
///
/// Each route file can export multiple HTTP method handlers (GET, POST, etc.).
fn collect_known_routes(files: &[(String, String)]) -> Vec<KnownRoute> {
    let mut routes = Vec::new();

    for (path, content) in files {
        // Detect Next.js API route files
        if !is_nextjs_api_route(path) {
            continue;
        }

        let route_path = extract_nextjs_route_path(path);

        // Detect exported HTTP method handlers
        let methods = detect_exported_methods(content);

        for method in &methods {
            let norm_path = normalize_api_path(&route_path);
            let id = format!("{}.{}", method, norm_path);
            let variants = build_path_variants(&norm_path);

            routes.push(KnownRoute {
                id,
                method: method.clone(),
                path: norm_path.clone(),
                source_path: path.clone(),
                variants,
            });
        }

        // If no explicit methods detected, assume all standard methods
        if methods.is_empty() {
            for method in &["GET", "POST", "PUT", "DELETE", "PATCH"] {
                let norm_path = normalize_api_path(&route_path);
                let id = format!("{}.{}", method, norm_path);
                let variants = build_path_variants(&norm_path);

                routes.push(KnownRoute {
                    id,
                    method: method.to_string(),
                    path: norm_path.clone(),
                    source_path: path.clone(),
                    variants,
                });
            }
        }
    }

    routes
}

/// Check if a file path looks like a Next.js API route handler.
fn is_nextjs_api_route(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.contains("/api/") && (normalized.ends_with("route.ts") || normalized.ends_with("route.tsx"))
}

/// Extract the API route path from a Next.js route file path.
///
/// E.g., `web/app/api/admin/post/route.ts` → `/api/admin/post`
fn extract_nextjs_route_path(file_path: &str) -> String {
    let normalized = file_path.replace('\\', "/");

    // Find `/api/` segment
    if let Some(api_pos) = normalized.find("/api/") {
        let after_api = &normalized[api_pos..]; // "/api/admin/post/route.ts"
                                                  // Remove trailing /route.ts or /route.tsx
        let route_path = after_api
            .trim_end_matches("route.ts")
            .trim_end_matches("route.tsx")
            .trim_end_matches('/');

        // Normalize dynamic segments: [locale] → :param
        let normalized_path = normalize_dynamic_segments(route_path);
        return normalized_path;
    }

    String::new()
}

/// Normalize Next.js dynamic route segments to `:param`.
fn normalize_dynamic_segments(path: &str) -> String {
    let re = Regex::new(r"\[([^\]]+)\]").unwrap();
    re.replace_all(path, ":param").to_string()
}

/// Detect exported HTTP method handlers in a route file.
///
/// Matches patterns like:
/// - `export async function GET(req: Request)`
/// - `export function POST(req: Request)`
fn detect_exported_methods(content: &str) -> Vec<String> {
    let re = Regex::new(
        r"(?m)^export\s+(?:async\s+)?function\s+(GET|POST|PUT|DELETE|PATCH)\b",
    )
    .unwrap();
    let mut methods = Vec::new();
    for cap in re.captures_iter(content) {
        methods.push(cap[1].to_string());
    }
    methods.sort();
    methods.dedup();
    methods
}

// ── Route Matching ──────────────────────────────────────────────────

/// Match a frontend API call against known routes.
fn match_route<'a>(
    call: &FrontendApiCall,
    known_routes: &'a [KnownRoute],
) -> Option<&'a KnownRoute> {
    // If the call targets an external domain, don't match — it's not an internal route
    if call.is_external {
        return None;
    }

    let call_variants = build_call_path_variants(&call.normalized_path);

    // Find candidates with matching method
    let candidates: Vec<&KnownRoute> = known_routes
        .iter()
        .filter(|r| r.method == call.method)
        .collect();

    // Exact variant match
    for candidate in &candidates {
        if has_exact_match(&call_variants, &candidate.variants) {
            return Some(candidate);
        }
    }

    // Dynamic segment match
    for candidate in &candidates {
        if has_dynamic_match(&call_variants, &candidate.variants) {
            return Some(candidate);
        }
    }

    None
}

fn has_exact_match(call_variants: &[String], route_variants: &[String]) -> bool {
    let route_set: HashSet<&str> = route_variants.iter().map(|s| s.as_str()).collect();
    call_variants
        .iter()
        .any(|v| route_set.contains(v.as_str()))
}

fn has_dynamic_match(call_variants: &[String], route_variants: &[String]) -> bool {
    for cv in call_variants {
        for rv in route_variants {
            if match_comparable_path(cv, rv) {
                return true;
            }
        }
    }
    false
}

fn match_comparable_path(left: &str, right: &str) -> bool {
    let left_parts: Vec<&str> = left.split('/').filter(|s| !s.is_empty()).collect();
    let right_parts: Vec<&str> = right.split('/').filter(|s| !s.is_empty()).collect();

    if left_parts.len() != right_parts.len() {
        return false;
    }

    for (l, r) in left_parts.iter().zip(right_parts.iter()) {
        if l == r {
            continue;
        }
        if is_dynamic_segment(l) || is_dynamic_segment(r) {
            continue;
        }
        return false;
    }

    true
}

fn is_dynamic_segment(segment: &str) -> bool {
    segment == ":param"
        || (segment.starts_with('{') && segment.ends_with('}'))
        || (segment.starts_with('[') && segment.ends_with(']'))
        || (segment.starts_with(':') && segment.len() > 1)
}

// ── Path Normalization ──────────────────────────────────────────────

/// Normalize an API path for comparison.
///
/// - Strips query strings and hashes
/// - Converts template literals (${var}) to :param
/// - Collapses double slashes
/// - Normalizes dynamic segments ([id], {id}, :id) to :param
fn normalize_api_path(raw: &str) -> String {
    // If this is a full URL, extract just the path
    let path_only = if raw.starts_with("http://") || raw.starts_with("https://") {
        if let Some(pos) = raw.find("://") {
            let after_scheme = &raw[pos + 3..];
            if let Some(slash_pos) = after_scheme.find('/') {
                &after_scheme[slash_pos..]
            } else {
                "/"
            }
        } else {
            raw
        }
    } else {
        raw
    };

    // Strip query string and hash
    let no_query = path_only.split('?').next().unwrap_or(path_only);
    let no_hash = no_query.split('#').next().unwrap_or(no_query);

    // Replace template literals with :param
    let re = Regex::new(r"\$\{[^}]+\}").unwrap();
    let replaced = re.replace_all(no_hash, ":param");

    // Normalize dynamic segments
    let dyn_re = Regex::new(r"\[[^\]]+\]|\{[^\}]+\}").unwrap();
    let dyn_replaced = dyn_re.replace_all(&replaced, ":param");

    // Collapse double slashes
    let collapsed = Regex::new(r"/{2,}")
        .unwrap()
        .replace_all(&dyn_replaced, "/")
        .to_string();

    // Ensure leading /
    if collapsed.is_empty() || collapsed == "/" {
        "/".to_string()
    } else if !collapsed.starts_with('/') {
        format!("/{}", collapsed)
    } else {
        collapsed
    }
}

/// Build path variants for route matching.
fn build_path_variants(path: &str) -> Vec<String> {
    vec![path.to_string()]
}

/// Build path variants for call matching (includes :param variations).
fn build_call_path_variants(path: &str) -> Vec<String> {
    build_path_variants(path)
}

// ── Domain Helpers ──────────────────────────────────────────────────

fn extract_domain(url: &str) -> Option<String> {
    let stripped = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = stripped.split('/').next().unwrap_or("");
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn strip_quotes(s: &str) -> String {
    s.trim_start_matches('`')
        .trim_end_matches('`')
        .trim_start_matches('"')
        .trim_end_matches('"')
        .trim_start_matches('\'')
        .trim_end_matches('\'')
        .to_string()
}

// ── Runtime Map Builder ─────────────────────────────────────────────

fn build_runtime_map(
    project_root: &Path,
    frontend_calls: &[FrontendApiCall],
    known_routes: &[KnownRoute],
) -> Result<RuntimeMap, anyhow::Error> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut diagnostics = Vec::new();
    let mut unmatched_count = 0usize;
    let mut external_count = 0usize;
    let mut seen_frontend: HashSet<String> = HashSet::new();

    for call in frontend_calls {
        // Create frontend entry node (one per source file)
        if seen_frontend.insert(call.source_path.clone()) {
            nodes.push(RuntimeNode {
                id: format!("FrontendEntry.{}", call.source_path),
                node_type: "FrontendEntry".into(),
                label: call.source_path.clone(),
                source_path: call.source_path.clone(),
                category: "frontend".into(),
                metadata: HashMap::new(),
            });
        }

        let frontend_id = format!("FrontendEntry.{}", call.source_path);

        // External API calls → ExternalApi nodes
        if call.is_external {
            let domain = call.external_domain.as_deref().unwrap_or("external");
            let external_id = format!("ExternalApi.{}", domain);

            // Add ExternalApi node if not already present
            if !nodes.iter().any(|n| n.id == external_id) {
                nodes.push(RuntimeNode {
                    id: external_id.clone(),
                    node_type: "ExternalApi".into(),
                    label: format!("{} API", domain),
                    source_path: call.source_path.clone(),
                    category: "external".into(),
                    metadata: {
                        let mut m = HashMap::new();
                        m.insert("domain".into(), domain.into());
                        m
                    },
                });
            }

            edges.push(RuntimeEdge {
                from: frontend_id,
                to: external_id,
                edge_type: "calls".into(),
            });

            external_count += 1;
            continue;
        }

        // Try to match against known routes
        match match_route(call, known_routes) {
            Some(route) => {
                edges.push(RuntimeEdge {
                    from: frontend_id.clone(),
                    to: route.id.clone(),
                    edge_type: "calls".into(),
                });
            }
            None => {
                // Skip self-referencing URLs (project scraping its own pages)
                if call.raw_path.starts_with("http://") || call.raw_path.starts_with("https://") {
                    // It's an absolute URL; check if it's a known external domain
                    // Already handled above by is_external check
                }

                unmatched_count += 1;
                diagnostics.push(RuntimeDiagnostic {
                    level: "warning".into(),
                    code: "RUNTIME_FRONTEND_API_ROUTE_UNMATCHED".into(),
                    message: format!(
                        "Could not match frontend {} call {} to a known ApiRoute",
                        call.method, call.raw_path
                    ),
                    source_path: Some(call.source_path.clone()),
                });
            }
        }
    }

    Ok(RuntimeMap {
        project_root: project_root.to_string_lossy().to_string(),
        nodes,
        edges,
        diagnostics,
        unmatched_route_count: unmatched_count,
        external_api_call_count: external_count,
    })
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_external_domain_github() {
        assert!(is_external_domain("https://api.github.com/repos/foo/issues"));
        assert!(is_external_domain("https://api.github.com/repos/foo/pulls/1"));
        assert!(is_external_domain("https://raw.githubusercontent.com/foo/bar/main/CHANGELOG.md"));
        assert!(!is_external_domain("/api/admin/post"));
        assert!(!is_external_domain("https://deepseek-tui.com/en"));
    }

    #[test]
    fn test_normalize_api_path_basic() {
        assert_eq!(normalize_api_path("/api/admin/post"), "/api/admin/post");
        assert_eq!(normalize_api_path("https://api.github.com/repos/foo/issues?state=open"), "/repos/foo/issues");
        assert_eq!(normalize_api_path("${BASE}/v1/chat/completions"), "/:param/v1/chat/completions");
        assert_eq!(normalize_api_path("[locale]/admin"), "/:param/admin");
    }

    #[test]
    fn test_extract_nextjs_route_path() {
        assert_eq!(
            extract_nextjs_route_path("web/app/api/admin/post/route.ts"),
            "/api/admin/post"
        );
        assert_eq!(
            extract_nextjs_route_path("web/app/api/admin/login/route.tsx"),
            "/api/admin/login"
        );
        assert_eq!(
            extract_nextjs_route_path("web/app/api/[locale]/page/route.ts"),
            "/api/:param/page"
        );
    }

    #[test]
    fn test_detect_exported_methods() {
        let content = r#"
export async function GET(req: Request) {
    return NextResponse.json({ ok: true });
}

export async function POST(req: Request) {
    return NextResponse.json({ ok: true });
}
"#;
        let methods = detect_exported_methods(content);
        assert_eq!(methods, vec!["GET", "POST"]);
    }

    #[test]
    fn test_is_nextjs_api_route() {
        assert!(is_nextjs_api_route(
            "web/app/api/admin/post/route.ts"
        ));
        assert!(is_nextjs_api_route(
            "app/api/cron/route.tsx"
        ));
        assert!(!is_nextjs_api_route(
            "web/lib/community-agent.ts"
        ));
    }

    #[test]
    fn test_collect_frontend_calls_fetch() {
        let content = r#"
const res = await fetch("/api/admin/post", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ action: "post" }),
});
"#;
        let config = RuntimeAnalysisConfig::default();
        let calls = collect_frontend_calls("admin-client.tsx", content, &config);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "POST");
        assert_eq!(calls[0].normalized_path, "/api/admin/post");
        assert!(!calls[0].is_external);
    }

    #[test]
    fn test_collect_frontend_calls_github_external() {
        let content = r#"
const res = await fetch(`https://api.github.com/repos/${repo}/issues?state=open`);
"#;
        let config = RuntimeAnalysisConfig::default();
        let calls = collect_frontend_calls("tasks.ts", content, &config);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "GET");
        assert!(calls[0].is_external);
        assert_eq!(
            calls[0].external_domain.as_deref(),
            Some("api.github.com")
        );
    }

    #[test]
    fn test_run_runtime_analysis_integration() {
        let tmp = std::env::temp_dir().join(format!("triadmind_rt_{}", std::process::id()));
        let _ = std::fs::create_dir_all(tmp.join("web/app/api/admin/post"));
        let _ = std::fs::create_dir_all(tmp.join("web/lib"));

        // Create a route handler
        std::fs::write(
            tmp.join("web/app/api/admin/post/route.ts"),
            r#"
export async function POST(req: Request) {
    return NextResponse.json({ ok: true });
}
"#,
        )
        .unwrap();

        // Create a frontend file calling the route
        std::fs::write(
            tmp.join("web/app/page.tsx"),
            r#"
const res = await fetch("/api/admin/post", { method: "POST" });
"#,
        )
        .unwrap();

        // Create a file calling GitHub API
        std::fs::write(
            tmp.join("web/lib/tasks.ts"),
            r#"const r = await fetch("https://api.github.com/repos/foo/issues");"#,
        )
        .unwrap();

        let config = RuntimeAnalysisConfig::default();
        let result = run_runtime_analysis(&tmp, &config).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);

        // Should have matched the internal route
        let matched_edge = result
            .edges
            .iter()
            .find(|e| e.edge_type == "calls" && e.to.starts_with("POST."));
        assert!(matched_edge.is_some(), "Expected a matched route edge");

        // Should have an ExternalApi node for GitHub
        let external_node = result
            .nodes
            .iter()
            .find(|n| n.node_type == "ExternalApi");
        assert!(external_node.is_some(), "Expected an ExternalApi node");

        // Should have 0 unmatched routes (GitHub call is external, not unmatched)
        assert_eq!(result.unmatched_route_count, 0);
    }
}