//! Workspace validation and first-class git worktree isolation for sub-agents.

use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::dependencies::{ExternalTool, Git};
use crate::tools::spec::ToolError;

use super::FleetRole;

const SUBAGENT_WORKTREE_ROOT_DIR: &str = ".codewhale-worktrees";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SubAgentWorktreeRequest {
    pub(super) branch: Option<String>,
    pub(super) path: Option<PathBuf>,
    pub(super) base_ref: Option<String>,
}

pub(super) fn prepare_child_workspace(
    parent_workspace: &Path,
    requested_cwd: Option<&Path>,
    worktree: Option<&SubAgentWorktreeRequest>,
    session_name: Option<&str>,
    agent_type: &FleetRole,
) -> Result<Option<PathBuf>, ToolError> {
    let discovery_anchor = if let Some(requested_cwd) = requested_cwd {
        validate_existing_child_cwd(parent_workspace, requested_cwd)?
    } else {
        parent_workspace
            .canonicalize()
            .unwrap_or_else(|_| parent_workspace.to_path_buf())
    };

    if let Some(worktree) = worktree {
        return create_isolated_worktree(&discovery_anchor, worktree, session_name, agent_type)
            .map(Some);
    }

    if requested_cwd.is_some() {
        return Ok(Some(discovery_anchor));
    }

    Ok(None)
}

fn validate_existing_child_cwd(
    parent_workspace: &Path,
    requested_cwd: &Path,
) -> Result<PathBuf, ToolError> {
    let resolved = if requested_cwd.is_absolute() {
        requested_cwd.to_path_buf()
    } else {
        parent_workspace.join(requested_cwd)
    };
    let canonical = resolved.canonicalize().map_err(|e| {
        ToolError::invalid_input(format!(
            "Invalid cwd '{}': {e} (path may not exist yet — use worktree=true to let Codewhale create an isolated checkout)",
            requested_cwd.display()
        ))
    })?;
    let workspace_canonical = parent_workspace
        .canonicalize()
        .unwrap_or_else(|_| parent_workspace.to_path_buf());
    if !canonical.starts_with(&workspace_canonical) {
        return Err(ToolError::invalid_input(format!(
            "cwd must be inside the parent workspace: {} is not under {}",
            canonical.display(),
            workspace_canonical.display()
        )));
    }
    Ok(canonical)
}

pub(super) fn create_isolated_worktree(
    parent_workspace: &Path,
    request: &SubAgentWorktreeRequest,
    session_name: Option<&str>,
    agent_type: &FleetRole,
) -> Result<PathBuf, ToolError> {
    let repo_root = git_repo_root(parent_workspace)?;
    let branch = request
        .branch
        .clone()
        .unwrap_or_else(|| default_worktree_branch(session_name, agent_type));
    validate_git_branch_name(&repo_root, &branch)?;

    let base_ref = request
        .base_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("HEAD")
        .to_string();
    let worktree_path = resolve_worktree_path(&repo_root, &branch, request.path.as_ref())?;
    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            ToolError::execution_failed(format!(
                "Failed to create worktree parent '{}': {err}",
                parent.display()
            ))
        })?;
    }

    let path_arg = worktree_path.to_string_lossy().to_string();
    let args = vec![
        "worktree".to_string(),
        "add".to_string(),
        "-b".to_string(),
        branch,
        path_arg,
        base_ref,
    ];
    run_git_checked(&repo_root, &args, "create sub-agent worktree")?;
    worktree_path.canonicalize().map_err(|err| {
        ToolError::execution_failed(format!(
            "Created worktree path '{}' could not be resolved: {err}",
            worktree_path.display()
        ))
    })
}

pub(super) fn git_repo_root(workspace: &Path) -> Result<PathBuf, ToolError> {
    const MAX_PARENT_LEVELS: usize = 4;
    let start = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut paths_tried = Vec::new();
    let mut current = Some(start.as_path());
    let mut levels = 0usize;

    while let Some(dir) = current {
        paths_tried.push(dir.display().to_string());

        if let Some(root) = try_git_toplevel(dir) {
            return Ok(root);
        }

        if let Ok(entries) = fs::read_dir(dir) {
            let mut nested_roots = Vec::new();
            for entry in entries.flatten() {
                let child = entry.path();
                if !child.is_dir() || !path_looks_like_git_checkout(&child) {
                    continue;
                }
                if child
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
                {
                    continue;
                }
                if let Some(root) = try_git_toplevel(&child) {
                    nested_roots.push(root);
                }
            }
            match nested_roots.len() {
                0 => {}
                1 => return Ok(nested_roots.into_iter().next().expect("single nested root")),
                _ => {
                    let repos = nested_roots
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(ToolError::invalid_input(format!(
                        "Multiple git repositories found under {}. Specify cwd to disambiguate: {repos}",
                        dir.display()
                    )));
                }
            }
        }

        levels += 1;
        if levels > MAX_PARENT_LEVELS {
            break;
        }
        current = dir.parent();
    }

    Err(ToolError::invalid_input(format!(
        "worktree=true requires a git repository. Tried: {}",
        paths_tried.join(", ")
    )))
}

fn path_looks_like_git_checkout(path: &Path) -> bool {
    let git_path = path.join(".git");
    git_path.is_dir() || git_path.is_file()
}

fn try_git_toplevel(path: &Path) -> Option<PathBuf> {
    let output = Git::output(&["rev-parse", "--show-toplevel"], path).ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
}

fn validate_git_branch_name(repo_root: &Path, branch: &str) -> Result<(), ToolError> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(ToolError::invalid_input(
            "worktree_branch cannot be blank".to_string(),
        ));
    }
    run_git_checked(
        repo_root,
        &[
            "check-ref-format".to_string(),
            "--branch".to_string(),
            branch.to_string(),
        ],
        "validate sub-agent worktree branch",
    )
    .map(|_| ())
    .map_err(|err| ToolError::invalid_input(format!("Invalid worktree_branch '{branch}': {err}")))
}

fn default_worktree_branch(session_name: Option<&str>, agent_type: &FleetRole) -> String {
    let seed = session_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| agent_type.as_str());
    format!(
        "codex/agent-{}-{}",
        sanitize_worktree_slug(seed),
        &Uuid::new_v4().to_string()[..8]
    )
}

fn resolve_worktree_path(
    repo_root: &Path,
    branch: &str,
    requested_path: Option<&PathBuf>,
) -> Result<PathBuf, ToolError> {
    let default_root = default_worktree_root(repo_root);
    let path = match requested_path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => {
            let resolved = normalize_path_lexically(&default_root.join(path));
            if !resolved.starts_with(&default_root) {
                return Err(ToolError::invalid_input(format!(
                    "relative worktree_path '{}' must stay under {}",
                    path.display(),
                    default_root.display()
                )));
            }
            resolved
        }
        None => default_root.join(sanitize_worktree_slug(branch)),
    };
    let normalized = normalize_path_lexically(&path);
    let repo_canonical = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    if normalized.starts_with(&repo_canonical) {
        return Err(ToolError::invalid_input(format!(
            "worktree_path must not be inside the parent checkout: {} is under {}",
            normalized.display(),
            repo_canonical.display()
        )));
    }
    Ok(normalized)
}

fn default_worktree_root(repo_root: &Path) -> PathBuf {
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_worktree_slug)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repo".to_string());
    let parent = repo_root.parent().unwrap_or(repo_root);
    normalize_path_lexically(&parent.join(SUBAGENT_WORKTREE_ROOT_DIR).join(repo_name))
}

fn sanitize_worktree_slug(input: &str) -> String {
    let mut slug = String::new();
    for ch in input.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if matches!(ch, '-' | '_' | '.') {
            ch
        } else {
            '-'
        };
        if normalized == '-' && slug.ends_with('-') {
            continue;
        }
        slug.push(normalized);
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches(['-', '.', '_']).to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn run_git_checked(workspace: &Path, args: &[String], action: &str) -> Result<String, ToolError> {
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = Git::output(&arg_refs, workspace).map_err(|err| {
        ToolError::execution_failed(format!("Failed to {action}: could not run git: {err}"))
    })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("git exited with status {}", output.status)
    };
    Err(ToolError::execution_failed(format!(
        "Failed to {action}: {detail}"
    )))
}
