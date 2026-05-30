use std::path::{Path, PathBuf};
use std::fs;

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

fn main() {
    let p = std::env::current_dir().unwrap();
    println!("Current dir: {:?}", p);
    println!("Git root: {:?}", find_git_root(&p));
}
