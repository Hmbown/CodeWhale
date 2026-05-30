use std::path::{Path, PathBuf};
use std::fs;

fn paths_equivalent(lhs: &Path, rhs: &Path) -> bool {
    let lhs_canonical = fs::canonicalize(lhs).ok();
    let rhs_canonical = fs::canonicalize(rhs).ok();
    match (lhs_canonical, rhs_canonical) {
        (Some(lhs), Some(rhs)) => lhs == rhs,
        _ => lhs == rhs,
    }
}

fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
}

fn workspace_scope_matches(saved_workspace: &Path, current_workspace: &Path) -> bool {
    if paths_equivalent(saved_workspace, current_workspace) {
        return true;
    }

    match (
        find_git_root(saved_workspace),
        find_git_root(current_workspace),
    ) {
        (Some(saved_root), Some(current_root)) => paths_equivalent(&saved_root, &current_root),
        _ => false,
    }
}

fn main() {
    let p1 = Path::new("G:\\data\\code\\Codewhale\\crates\\tui");
    let p2 = Path::new("G:\\data\\code\\Codewhale");
    println!("match: {}", workspace_scope_matches(p1, p2));
}
