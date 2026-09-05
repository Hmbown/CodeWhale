//! Git power tools: `git_status` and `git_diff`.
//!
//! These tools are read-only wrappers around common git inspection commands,
//! scoped to the workspace and optionally to a sub-path within it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::dependencies::ExternalTool;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, optional_u64,
};

const MAX_OUTPUT_CHARS: usize = 40_000;
const DEFAULT_UNIFIED: u64 = 3;
const MAX_UNIFIED: u64 = 50;

// === GitStatusTool ===

/// Tool for reading the concise git status of the workspace.
pub struct GitStatusTool;

#[async_trait]
impl ToolSpec for GitStatusTool {
    fn name(&self) -> &'static str {
        "git_status"
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Run `git status --porcelain=v1 -b` in the workspace (optionally scoped to a path)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional subdirectory or file to scope the status to (must be within the workspace)."
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let git_ctx = resolve_git_context(context, optional_str(&input, "path")?)?;

        let mut args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "-b".to_string(),
        ];
        if let Some(pathspec) = &git_ctx.pathspec {
            args.push("--".to_string());
            args.push(pathspec.display().to_string());
        }

        let command_str = format_command(&git_ctx.working_dir, &args);
        let output = run_git_command(&git_ctx.working_dir, &args)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = format!("git status failed: {}", stderr.trim());
            return Ok(ToolResult::error(message).with_metadata(json!({
                "command": command_str,
                "exit_code": output.status.code(),
                "stderr": stderr.trim(),
            })));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (content, truncated, omitted_chars) = truncate_with_note(&stdout, MAX_OUTPUT_CHARS);

        Ok(ToolResult::success(content).with_metadata(json!({
            "command": command_str,
            "working_dir": git_ctx.working_dir,
            "pathspec": git_ctx.pathspec,
            "truncated": truncated,
            "omitted_chars": omitted_chars,
        })))
    }
}

// === GitDiffTool ===

/// Tool for reading git diffs in the workspace.
pub struct GitDiffTool;

#[async_trait]
impl ToolSpec for GitDiffTool {
    fn name(&self) -> &'static str {
        "git_diff"
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Run `git diff` in the workspace with sensible defaults and safe truncation."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional subdirectory or file to scope the diff to (must be within the workspace)."
                },
                "cached": {
                    "type": "boolean",
                    "description": "When true, diff staged changes (`--cached`)."
                },
                "unified": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_UNIFIED,
                    "default": DEFAULT_UNIFIED,
                    "description": "Number of context lines to include around changes."
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let git_ctx = resolve_git_context(context, optional_str(&input, "path")?)?;
        let cached = optional_bool(&input, "cached", false)?;
        let unified = optional_u64(&input, "unified", DEFAULT_UNIFIED)?.min(MAX_UNIFIED);

        let mut args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "diff".to_string(),
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            format!("--unified={unified}"),
        ];
        if cached {
            args.push("--cached".to_string());
        }
        if let Some(pathspec) = &git_ctx.pathspec {
            args.push("--".to_string());
            args.push(pathspec.display().to_string());
        }

        let command_str = format_command(&git_ctx.working_dir, &args);
        let output = run_git_command(&git_ctx.working_dir, &args)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = format!("git diff failed: {}", stderr.trim());
            return Ok(ToolResult::error(message).with_metadata(json!({
                "command": command_str,
                "exit_code": output.status.code(),
                "stderr": stderr.trim(),
            })));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (content, truncated, omitted_chars) = truncate_with_note(&stdout, MAX_OUTPUT_CHARS);

        Ok(ToolResult::success(content).with_metadata(json!({
            "command": command_str,
            "working_dir": git_ctx.working_dir,
            "pathspec": git_ctx.pathspec,
            "cached": cached,
            "unified": unified,
            "truncated": truncated,
            "omitted_chars": omitted_chars,
        })))
    }
}

// === GitCommitSplitTool ===

/// Tool for splitting unstaged and staged changes into atomic commits.
pub struct GitCommitSplitTool;

#[async_trait]
impl ToolSpec for GitCommitSplitTool {
    fn name(&self) -> &'static str {
        "commit_split"
    }

    fn model_visible(&self) -> bool {
        true
    }

    fn description(&self) -> &'static str {
        "Analyze the working tree, group changes into logical commits, order them by dependency (rejecting cycles), and apply the split commits."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, only return the proposed split commits and dependency graph without writing."
                },
                "path": {
                    "type": "string",
                    "description": "Optional subdirectory or file to scope the split to (must be within the workspace)."
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Manual
    }

    fn supports_parallel(&self) -> bool {
        false
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let git_ctx = resolve_git_context(context, optional_str(&input, "path")?)?;
        let dry_run = optional_bool(&input, "dry_run", false)?;

        // 1. Prepare untracked files so they are included in the diff
        let status_args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "status".to_string(),
            "--porcelain=v1".to_string(),
        ];
        let output = run_git_command(&git_ctx.working_dir, &status_args)?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.starts_with("?? ") {
                    let path = &line[3..];
                    let add_args = vec![
                        "add".to_string(),
                        "-N".to_string(),
                        path.to_string(),
                    ];
                    let _ = run_git_command(&git_ctx.working_dir, &add_args);
                }
            }
        }

        // 2. Run git diff HEAD to get all staged and unstaged changes
        let mut diff_args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "diff".to_string(),
            "HEAD".to_string(),
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            "-U3".to_string(),
        ];
        if let Some(pathspec) = &git_ctx.pathspec {
            diff_args.push("--".to_string());
            diff_args.push(pathspec.display().to_string());
        }

        let output = run_git_command(&git_ctx.working_dir, &diff_args)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Ok(ToolResult::error(format!("git diff HEAD failed: {}", stderr.trim())));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let hunks = parse_diff(&stdout);
        if hunks.is_empty() {
            return Ok(ToolResult::success("No changes to commit."));
        }

        // 3. Group hunks into logical components
        let mut groups: Vec<CommitGroup> = Vec::new();
        let mut file_to_hunks: HashMap<String, Vec<Hunk>> = HashMap::new();
        let mut lock_files: Vec<(String, Vec<Hunk>)> = Vec::new();

        for hunk in hunks {
            if is_lock_file(&hunk.file_path) {
                let mut found = false;
                for (lf_path, lf_hunks) in &mut lock_files {
                    if lf_path == &hunk.file_path {
                        lf_hunks.push(hunk.clone());
                        found = true;
                        break;
                    }
                }
                if !found {
                    lock_files.push((hunk.file_path.clone(), vec![hunk]));
                }
            } else {
                file_to_hunks.entry(hunk.file_path.clone()).or_default().push(hunk);
            }
        }

        for (file_path, hunks) in file_to_hunks {
            let mut defined_symbols = HashSet::new();
            let mut referenced_symbols = HashSet::new();
            for h in &hunks {
                defined_symbols.extend(extract_defined_symbols(h));
                referenced_symbols.extend(extract_referenced_symbols(h));
            }
            for sym in &defined_symbols {
                referenced_symbols.remove(sym);
            }
            let mut files = HashMap::new();
            files.insert(file_path, hunks);
            groups.push(CommitGroup {
                files,
                defined_symbols,
                referenced_symbols,
            });
        }

        // Associate lock files with matching manifest groups, or their own group
        for (lf_path, lf_hunks) in lock_files {
            let mut assigned = false;
            for g in &mut groups {
                let mut match_found = false;
                for existing_file in g.files.keys() {
                    if matches_lock_file(existing_file, &lf_path) {
                        match_found = true;
                        break;
                    }
                }
                if match_found {
                    g.files.insert(lf_path.clone(), lf_hunks.clone());
                    assigned = true;
                    break;
                }
            }
            if !assigned {
                let mut files = HashMap::new();
                files.insert(lf_path, lf_hunks);
                groups.push(CommitGroup {
                    files,
                    defined_symbols: HashSet::new(),
                    referenced_symbols: HashSet::new(),
                });
            }
        }

        // Merge groups with closely related file names (e.g. tests, specs, docs)
        let mut merged_groups: Vec<CommitGroup> = Vec::new();
        'outer: for g in groups {
            for mg in &mut merged_groups {
                let mut should_merge = false;
                for f1 in g.files.keys() {
                    for f2 in mg.files.keys() {
                        if are_files_related(f1, f2) {
                            should_merge = true;
                            break;
                        }
                    }
                    if should_merge {
                        break;
                    }
                }
                if should_merge {
                    for (k, v) in g.files {
                        mg.files.insert(k, v);
                    }
                    mg.defined_symbols.extend(g.defined_symbols);
                    mg.referenced_symbols.extend(g.referenced_symbols);
                    for sym in &mg.defined_symbols {
                        mg.referenced_symbols.remove(sym);
                    }
                    continue 'outer;
                }
            }
            merged_groups.push(g);
        }
        let mut groups = merged_groups;

        // 4. Build the dependency graph
        let n = groups.len();
        let mut adj = vec![vec![]; n];
        let mut in_degree = vec![0; n];

        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let mut depends = false;
                for ref_sym in &groups[i].referenced_symbols {
                    if groups[j].defined_symbols.contains(ref_sym) {
                        depends = true;
                        break;
                    }
                }

                if !depends {
                    let i_is_source = groups[i].files.keys().any(|f| is_source_file(f));
                    let j_is_source = groups[j].files.keys().any(|f| is_source_file(f));
                    if !i_is_source && j_is_source {
                        for f1 in groups[i].files.keys() {
                            for f2 in groups[j].files.keys() {
                                if share_context(f1, f2) {
                                    depends = true;
                                    break;
                                }
                            }
                            if depends {
                                break;
                            }
                        }
                    }
                }

                if depends {
                    adj[j].push(i);
                    in_degree[i] += 1;
                }
            }
        }

        // 5. Order the commits using topological sort
        let mut ready = Vec::new();
        for i in 0..n {
            if in_degree[i] == 0 {
                ready.push(i);
            }
        }

        let mut sorted_order = Vec::new();
        while !ready.is_empty() {
            ready.sort_by(|&idx_a, &idx_b| {
                let a_is_source = groups[idx_a].files.keys().any(|f| is_source_file(f));
                let b_is_source = groups[idx_b].files.keys().any(|f| is_source_file(f));
                if a_is_source != b_is_source {
                    b_is_source.cmp(&a_is_source)
                } else {
                    let a_first_file = groups[idx_a].files.keys().next().unwrap();
                    let b_first_file = groups[idx_b].files.keys().next().unwrap();
                    a_first_file.cmp(b_first_file)
                }
            });

            let curr = ready.remove(0);
            sorted_order.push(curr);

            for &next in &adj[curr] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    ready.push(next);
                }
            }
        }

        // Cycle detection! Reject immediately if cycle is present
        if sorted_order.len() < n {
            let mut cyclic_files = Vec::new();
            for i in 0..n {
                if in_degree[i] > 0 {
                    cyclic_files.extend(groups[i].files.keys().cloned());
                }
            }
            let message = format!(
                "Dependency cycle detected among changes in the following files: {}. Atomic commit splitting rejected.",
                cyclic_files.join(", ")
            );
            return Ok(ToolResult::error(message).with_metadata(json!({
                "cycle_detected": true,
                "cyclic_files": cyclic_files,
            })));
        }

        // 6. Propose or Apply the split commits
        if dry_run {
            let mut proposed_commits = Vec::new();
            for (idx, &g_idx) in sorted_order.iter().enumerate() {
                let g = &groups[g_idx];
                let commit_message = generate_commit_message(g);
                let files_list: Vec<String> = g.files.keys().cloned().collect();
                proposed_commits.push(json!({
                    "order": idx + 1,
                    "message": commit_message,
                    "files": files_list,
                }));
            }
            return Ok(ToolResult::success(
                serde_json::to_string_pretty(&proposed_commits).unwrap_or_default()
            ).with_metadata(json!({
                "dry_run": true,
                "commits": proposed_commits,
            })));
        }

        let mut committed = Vec::new();
        for (idx, &g_idx) in sorted_order.iter().enumerate() {
            let g = &groups[g_idx];
            let commit_message = generate_commit_message(g);

            let mut hunks_to_apply = Vec::new();
            for file_hunks in g.files.values() {
                hunks_to_apply.extend(file_hunks.clone());
            }

            apply_hunks_and_commit(&git_ctx.working_dir, &hunks_to_apply, &commit_message)?;

            let files_list: Vec<String> = g.files.keys().cloned().collect();
            committed.push(json!({
                "order": idx + 1,
                "message": commit_message,
                "files": files_list,
            }));
        }

        Ok(ToolResult::success(format!(
            "Successfully split and applied {} commits.",
            committed.len()
        )).with_metadata(json!({
            "dry_run": false,
            "commits": committed,
        })))
    }
}

// === Helpers ===

struct GitContext {
    working_dir: PathBuf,
    pathspec: Option<PathBuf>,
}

fn resolve_git_context(context: &ToolContext, path: Option<&str>) -> Result<GitContext, ToolError> {
    let workspace = canonical_or_workspace(&context.workspace);
    let mut working_dir = workspace.clone();
    let mut pathspec = None;

    if let Some(raw) = path {
        let resolved = context.resolve_path(raw)?;
        let metadata = fs::metadata(&resolved).map_err(|e| {
            ToolError::invalid_input(format!(
                "Path does not exist or is not accessible: {raw} ({e})"
            ))
        })?;

        if metadata.is_dir() {
            working_dir = resolved;
            pathspec = Some(PathBuf::from("."));
        } else {
            // For file paths, run from the parent and scope to the file name.
            let parent = resolved.parent().ok_or_else(|| {
                ToolError::invalid_input(format!("Path has no parent directory: {raw}"))
            })?;
            working_dir = parent.to_path_buf();
            pathspec = Some(pathspec_from(&working_dir, &resolved));
        }
    }

    if !working_dir.exists() {
        return Err(ToolError::invalid_input(format!(
            "Working directory does not exist: {}",
            working_dir.display()
        )));
    }

    Ok(GitContext {
        working_dir,
        pathspec,
    })
}

fn canonical_or_workspace(workspace: &Path) -> PathBuf {
    workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
}

fn pathspec_from(working_dir: &Path, resolved: &Path) -> PathBuf {
    match resolved.strip_prefix(working_dir) {
        Ok(rel) if rel.as_os_str().is_empty() => PathBuf::from("."),
        Ok(rel) => rel.to_path_buf(),
        Err(_) => PathBuf::from("."),
    }
}

fn run_git_command(working_dir: &Path, args: &[String]) -> Result<std::process::Output, ToolError> {
    let Some(mut cmd) = crate::dependencies::Git::command() else {
        return Err(ToolError::not_available(
            "git is not installed or not in PATH",
        ));
    };
    cmd.args(args).current_dir(working_dir);
    cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ToolError::not_available("git is not installed or not in PATH")
        } else {
            ToolError::execution_failed(format!("Failed to run git: {e}"))
        }
    })
}

fn format_command(working_dir: &Path, args: &[String]) -> String {
    // `[String]::join` produces the same string as collecting `&str` first, so
    // join the slice directly and skip the intermediate `Vec<&str>` allocation.
    format!("git -C {} {}", working_dir.display(), args.join(" "))
}

fn truncate_with_note(text: &str, max_chars: usize) -> (String, bool, usize) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false, 0);
    }
    let end = char_boundary_index(text, max_chars);
    let truncated = &text[..end];
    let omitted_chars = text
        .chars()
        .count()
        .saturating_sub(truncated.chars().count());
    let note = format!(
        "\n\n[output truncated to {max_chars} characters; {omitted_chars} characters omitted]"
    );
    (format!("{truncated}{note}"), true, omitted_chars)
}

fn char_boundary_index(text: &str, max_chars: usize) -> usize {
    if max_chars == 0 {
        return 0;
    }
    for (count, (idx, _)) in text.char_indices().enumerate() {
        if count == max_chars {
            return idx;
        }
    }
    text.len()
}

// === Commit Split Specific Types & Helpers ===

#[derive(Debug, Clone)]
pub struct Hunk {
    pub file_path: String,
    pub old_range: (usize, usize),
    pub new_range: (usize, usize),
    pub header: String,
    pub lines: Vec<String>,
}

struct CommitGroup {
    files: HashMap<String, Vec<Hunk>>,
    defined_symbols: HashSet<String>,
    referenced_symbols: HashSet<String>,
}

fn parse_diff(diff_output: &str) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut current_file = String::new();
    let mut current_hunk_header = String::new();
    let mut current_hunk_lines = Vec::new();
    let mut in_hunk = false;
    let mut old_range = (0, 0);
    let mut new_range = (0, 0);

    for line in diff_output.lines() {
        if line.starts_with("diff --git ") {
            if in_hunk {
                hunks.push(Hunk {
                    file_path: current_file.clone(),
                    old_range,
                    new_range,
                    header: current_hunk_header.clone(),
                    lines: current_hunk_lines.clone(),
                });
                in_hunk = false;
                current_hunk_lines.clear();
            }
            if let Some(pos) = line.rfind(" b/") {
                let path = &line[pos + 3..];
                current_file = path.trim_matches('"').to_string();
            } else {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    current_file = parts[3].strip_prefix("b/").unwrap_or(parts[3]).to_string();
                }
            }
        } else if line.starts_with("@@ ") {
            if in_hunk {
                hunks.push(Hunk {
                    file_path: current_file.clone(),
                    old_range,
                    new_range,
                    header: current_hunk_header.clone(),
                    lines: current_hunk_lines.clone(),
                });
                current_hunk_lines.clear();
            }
            in_hunk = true;
            current_hunk_header = line.to_string();
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                old_range = parse_range(parts[1].strip_prefix('-').unwrap_or(parts[1]));
                new_range = parse_range(parts[2].strip_prefix('+').unwrap_or(parts[2]));
            }
        } else if in_hunk {
            current_hunk_lines.push(line.to_string());
        }
    }

    if in_hunk {
        hunks.push(Hunk {
            file_path: current_file,
            old_range,
            new_range,
            header: current_hunk_header,
            lines: current_hunk_lines,
        });
    }

    hunks
}

fn parse_range(s: &str) -> (usize, usize) {
    let parts: Vec<&str> = s.split(',').collect();
    let start = parts.get(0).and_then(|x| x.parse().ok()).unwrap_or(0);
    let count = parts.get(1).and_then(|x| x.parse().ok()).unwrap_or(1);
    (start, count)
}

fn is_lock_file(path: &str) -> bool {
    let name = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.ends_with(".lock") || name == "go.sum" || name == "package-lock.json" || name == "pnpm-lock.yaml" || name == "yarn.lock"
}

fn matches_lock_file(manifest: &str, lock: &str) -> bool {
    let m_path = Path::new(manifest);
    let l_path = Path::new(lock);
    let m_dir = m_path.parent().unwrap_or(Path::new(""));
    let l_dir = l_path.parent().unwrap_or(Path::new(""));
    if m_dir != l_dir {
        return false;
    }
    let m_name = m_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let l_name = l_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match (m_name, l_name) {
        ("Cargo.toml", "Cargo.lock") => true,
        ("package.json", "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml") => true,
        ("go.mod", "go.sum") => true,
        _ => {
            let m_stem = m_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let l_stem = l_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            m_stem == l_stem || (m_name.ends_with(".json") && l_name.ends_with(".json"))
        }
    }
}

fn are_files_related(f1: &str, f2: &str) -> bool {
    let p1 = Path::new(f1);
    let p2 = Path::new(f2);
    let stem1 = p1.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let stem2 = p2.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    if stem1 == stem2 {
        return true;
    }
    let clean_stem = |s: &str| {
        s.replace("_test", "")
            .replace("test_", "")
            .replace("_spec", "")
            .replace("spec_", "")
            .replace("test", "")
    };
    clean_stem(&stem1) == clean_stem(&stem2) && !clean_stem(&stem1).is_empty()
}

fn is_source_file(path: &str) -> bool {
    let p = Path::new(path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    if name.contains("test") || name.contains("spec") || name.contains("mock") {
        return false;
    }
    matches!(ext, "rs" | "py" | "go" | "js" | "ts" | "cpp" | "h" | "c" | "java" | "cs" | "rb" | "php")
}

fn share_context(f1: &str, f2: &str) -> bool {
    let p1 = Path::new(f1);
    let p2 = Path::new(f2);
    p1.parent() == p2.parent()
}

fn extract_defined_symbols(hunk: &Hunk) -> HashSet<String> {
    let mut symbols = HashSet::new();
    for line in &hunk.lines {
        if line.starts_with('+') && !line.starts_with("+++") {
            let content = &line[1..];
            let tokens = tokenize(content);
            for i in 0..tokens.len() {
                let tok = &tokens[i];
                if tok == "fn" || tok == "func" || tok == "def" || tok == "function" || tok == "struct" || tok == "enum" || tok == "trait" || tok == "class" || tok == "interface" || tok == "type" || tok == "const" || tok == "let" || tok == "mod" {
                    if i + 1 < tokens.len() {
                        let sym = &tokens[i + 1];
                        if is_valid_identifier(sym) {
                            symbols.insert(sym.clone());
                        }
                    }
                }
            }
        }
    }
    symbols
}

fn extract_referenced_symbols(hunk: &Hunk) -> HashSet<String> {
    let mut symbols = HashSet::new();
    for line in &hunk.lines {
        if line.starts_with('+') && !line.starts_with("+++") {
            let content = &line[1..];
            let tokens = tokenize(content);
            for tok in tokens {
                if is_valid_identifier(&tok) && !is_keyword(&tok) {
                    symbols.insert(tok);
                }
            }
        }
    }
    symbols
}

fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() || c == '_' {
            current.push(c);
        } else {
            if !current.is_empty() {
                tokens.push(current.clone());
                current.clear();
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let first = s.chars().next().unwrap();
    (first.is_alphabetic() || first == '_') && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "if" | "else" | "while" | "for" | "return" | "import" | "use" | "pub" | "impl" | "crate"
            | "self" | "true" | "false" | "let" | "mut" | "match" | "var" | "void" | "int"
            | "string" | "bool" | "float" | "double" | "public" | "private" | "protected"
            | "static" | "final" | "class" | "fn" | "struct" | "enum" | "trait" | "interface"
            | "type" | "const" | "mod" | "def" | "func" | "function" | "and" | "or" | "not"
            | "in" | "as" | "break" | "continue" | "new" | "this" | "super"
    )
}

fn generate_commit_message(group: &CommitGroup) -> String {
    let files: Vec<&String> = group.files.keys().collect();
    if files.len() == 1 {
        let file = files[0];
        let p = Path::new(file);
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or(file);
        if !group.defined_symbols.is_empty() {
            let syms: Vec<String> = group.defined_symbols.iter().take(3).cloned().collect();
            format!("refactor({}): define {}", name, syms.join(", "))
        } else {
            format!("style/update: changes in {}", name)
        }
    } else {
        if !group.defined_symbols.is_empty() {
            let syms: Vec<String> = group.defined_symbols.iter().take(3).cloned().collect();
            format!("feat: implement {}", syms.join(", "))
        } else {
            format!("chore: update multiple files including {}", files[0])
        }
    }
}

fn apply_hunks_and_commit(
    working_dir: &Path,
    hunks: &[Hunk],
    commit_message: &str,
) -> Result<(), ToolError> {
    let mut patch = String::new();
    let mut files_map: HashMap<String, Vec<&Hunk>> = HashMap::new();
    for hunk in hunks {
        files_map.entry(hunk.file_path.clone()).or_default().push(hunk);
    }

    for (file_path, file_hunks) in files_map {
        patch.push_str(&format!("diff --git a/{file_path} b/{file_path}\n"));
        patch.push_str(&format!("--- a/{file_path}\n"));
        patch.push_str(&format!("+++ b/{file_path}\n"));
        for hunk in file_hunks {
            patch.push_str(&hunk.header);
            patch.push('\n');
            for line in &hunk.lines {
                patch.push_str(line);
                patch.push('\n');
            }
        }
    }

    let mut child = {
        let mut cmd = crate::dependencies::Git::command().ok_or_else(|| {
            ToolError::not_available("git is not installed or not in PATH")
        })?;
        cmd.args(&["-c", "core.quotepath=false", "apply", "--cached", "-"])
            .current_dir(working_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        cmd.spawn().map_err(|e| {
            ToolError::execution_failed(format!("Failed to spawn git apply: {e}"))
        })?
    };

    {
        use std::io::Write;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            ToolError::execution_failed("Failed to open stdin for git apply")
        })?;
        stdin.write_all(patch.as_bytes()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to write to git apply: {e}"))
        })?;
    }

    let output = child.wait_with_output().map_err(|e| {
        ToolError::execution_failed(format!("Failed to wait for git apply: {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::execution_failed(format!(
            "git apply --cached failed: {}",
            stderr.trim()
        )));
    }

    let commit_args = vec![
        "-c".to_string(),
        "core.quotepath=false".to_string(),
        "commit".to_string(),
        "-m".to_string(),
        commit_message.to_string(),
    ];
    let output = run_git_command(working_dir, &commit_args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::execution_failed(format!(
            "git commit failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn git_available() -> bool {
        crate::dependencies::Git::available()
    }

    fn init_git_repo(root: &Path) {
        let run = |args: &[&str]| {
            let status = crate::dependencies::Git::status(args, root).expect("git should spawn");
            assert!(status.success(), "git {args:?} failed");
        };

        run(&["init", "-q"]);
        run(&["config", "core.autocrlf", "false"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test User"]);
    }

    fn commit_all(root: &Path, message: &str) {
        let run = |args: &[&str]| {
            let status = crate::dependencies::Git::status(args, root).expect("git should spawn");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["add", "."]);
        run(&["commit", "-q", "-m", message]);
    }

    #[tokio::test]
    async fn git_status_reports_branch_and_changes() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let file = tmp.path().join("file.txt");
        fs::write(&file, "hello\n").expect("write");
        commit_all(tmp.path(), "init");

        fs::write(&file, "hello\nworld\n").expect("modify");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitStatusTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(result.success);
        assert!(result.content.contains("##"));
        assert!(result.content.contains("file.txt"));
    }

    #[tokio::test]
    async fn git_status_reports_unquoted_unicode_paths() {
        if !git_available() {
            return;
        }

        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let file = tmp.path().join("中文-данные.txt");
        fs::write(&file, "hello\n").expect("write");
        commit_all(tmp.path(), "init");

        fs::write(&file, "hello\nworld\n").expect("modify");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitStatusTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(result.success);
        assert!(
            result
                .metadata
                .as_ref()
                .and_then(|m| m.get("command"))
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains("-c core.quotepath=false"))
        );
        assert!(result.content.contains("中文-данные.txt"));
        assert!(!result.content.contains("\\344"));
        assert!(!result.content.contains("\\320"));
    }

    #[tokio::test]
    async fn git_diff_supports_cached_and_path_scoping() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let subdir = tmp.path().join("src");
        fs::create_dir_all(&subdir).expect("mkdir");
        let file = subdir.join("lib.rs");
        fs::write(&file, "pub fn one() -> i32 { 1 }\n").expect("write");
        commit_all(tmp.path(), "init");

        fs::write(&file, "pub fn one() -> i32 { 2 }\n").expect("modify");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitDiffTool;

        let uncached = tool
            .execute(json!({ "path": "src" }), &ctx)
            .await
            .expect("diff");
        assert!(uncached.success);
        assert!(uncached.content.contains("diff --git"));
        assert!(uncached.content.contains("lib.rs"));

        let _ =
            crate::dependencies::Git::status(&["add", "src/lib.rs"], tmp.path()).expect("git add");

        let cached = tool
            .execute(json!({ "path": "src", "cached": true }), &ctx)
            .await
            .expect("diff cached");
        assert!(cached.success);
        assert!(cached.content.contains("diff --git"));
        assert!(
            cached
                .metadata
                .as_ref()
                .and_then(|m| m.get("cached"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn git_diff_reports_unquoted_unicode_paths() {
        if !git_available() {
            return;
        }

        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let unicode_name = "\u{4e2d}\u{6587}-\u{0434}\u{0430}\u{043d}\u{043d}\u{044b}\u{0435}.txt";
        let file = tmp.path().join(unicode_name);
        fs::write(&file, "hello\n").expect("write");
        commit_all(tmp.path(), "init");

        fs::write(&file, "hello\nworld\n").expect("modify");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitDiffTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");

        assert!(result.success);
        assert!(
            result
                .metadata
                .as_ref()
                .and_then(|m| m.get("command"))
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains("-c core.quotepath=false"))
        );
        assert!(result.content.contains(unicode_name));
        assert!(!result.content.contains("\\344"));
        assert!(!result.content.contains("\\320"));
    }

    #[test]
    fn format_command_joins_args_without_intermediate_vec() {
        let args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "-b".to_string(),
        ];
        let rendered = format_command(Path::new("/tmp/repo"), &args);
        assert_eq!(
            rendered,
            "git -C /tmp/repo -c core.quotepath=false status --porcelain=v1 -b"
        );

        assert_eq!(
            format_command(Path::new("/tmp/repo"), &[]),
            "git -C /tmp/repo "
        );
    }

    #[test]
    fn truncation_adds_note() {
        let long = "a".repeat(MAX_OUTPUT_CHARS + 100);
        let (truncated, did_truncate, omitted) = truncate_with_note(&long, MAX_OUTPUT_CHARS);
        assert!(did_truncate);
        assert!(omitted > 0);
        assert!(truncated.contains("output truncated"));
    }

    #[test]
    fn test_parse_diff() {
        let diff = r#"diff --git a/src/lib.rs b/src/lib.rs
index e69de29..4b2a8d3 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 line1
-line2
+line2 modified
 line3
+line4 added
"#;
        let hunks = parse_diff(diff);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, "src/lib.rs");
        assert_eq!(hunks[0].old_range, (1, 3));
        assert_eq!(hunks[0].new_range, (1, 4));
        assert_eq!(hunks[0].header, "@@ -1,3 +1,4 @@");
        assert_eq!(hunks[0].lines.len(), 5);
    }

    #[test]
    fn test_dependency_extraction() {
        let hunk = Hunk {
            file_path: "src/lib.rs".to_string(),
            old_range: (1, 1),
            new_range: (1, 2),
            header: "@@ -1 +1,2 @@".to_string(),
            lines: vec![
                " pub fn add(a: i32, b: i32) -> i32 {".to_string(),
                "+    let sum = a + b;".to_string(),
                "+    struct Answer;".to_string(),
                "     sum".to_string(),
            ],
        };
        let defined = extract_defined_symbols(&hunk);
        let referenced = extract_referenced_symbols(&hunk);

        assert!(defined.contains("Answer"));
        assert!(defined.contains("sum"));
        assert!(referenced.contains("sum"));
        assert!(referenced.contains("Answer"));
    }

    #[tokio::test]
    async fn test_git_commit_split_success() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let math_file = tmp.path().join("math.rs");
        let main_file = tmp.path().join("main.rs");

        fs::write(&math_file, "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").expect("write math");
        fs::write(&main_file, "fn main() { let x = math::add(1, 2); }\n").expect("write main");

        commit_all(tmp.path(), "init");

        fs::write(&math_file, "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n").expect("modify math");
        fs::write(&main_file, "fn main() { let x = math::add(1, 2); let y = math::sub(3, 4); }\n").expect("modify main");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitCommitSplitTool;

        let result = tool.execute(json!({ "dry_run": true }), &ctx).await.expect("execute");
        assert!(result.success);
        let val: Value = serde_json::from_str(&result.content).expect("parse response");
        let commits = val.as_array().expect("array of commits");
        assert_eq!(commits.len(), 2);

        let first_commit = &commits[0];
        let first_files = first_commit.get("files").unwrap().as_array().unwrap();
        assert!(first_files.iter().any(|f| f.as_str().unwrap().contains("math.rs")));

        let second_commit = &commits[1];
        let second_files = second_commit.get("files").unwrap().as_array().unwrap();
        assert!(second_files.iter().any(|f| f.as_str().unwrap().contains("main.rs")));

        let run_result = tool.execute(json!({ "dry_run": false }), &ctx).await.expect("execute");
        assert!(run_result.success);

        let log_output = run_git_command(tmp.path(), &["log".to_string(), "--oneline".to_string()]).expect("git log");
        let log_stdout = String::from_utf8_lossy(&log_output.stdout);
        let lines: Vec<&str> = log_stdout.lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[tokio::test]
    async fn test_git_commit_split_cycle() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let a_file = tmp.path().join("a.rs");
        let b_file = tmp.path().join("b.rs");

        fs::write(&a_file, "pub fn func_a() {}\n").expect("write a");
        fs::write(&b_file, "pub fn func_b() {}\n").expect("write b");

        commit_all(tmp.path(), "init");

        fs::write(&a_file, "pub fn func_a() {}\npub fn func_a2() { b::func_b2(); }\n").expect("modify a");
        fs::write(&b_file, "pub fn func_b() {}\npub fn func_b2() { a::func_a2(); }\n").expect("modify b");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitCommitSplitTool;

        let result = tool.execute(json!({ "dry_run": true }), &ctx).await.expect("execute");
        assert!(!result.success);
        assert!(result.content.contains("Dependency cycle detected"));
    }
}