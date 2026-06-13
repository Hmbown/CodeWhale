//! Process-local cache for project context loading.
//!
//! The project-context loader sits on prompt/session hot paths and repeatedly
//! checks the same workspace, parent, global, constitution, and trust files.
//! This cache avoids rereading unchanged context while keeping the signature
//! broad enough for the loader's side effects and authority surfaces.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::project_context::ProjectContext;

const DEFAULT_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    workspace: PathBuf,
    signature: ContentSignature,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
struct ContentSignature {
    entries: Vec<ContentEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ContentEntry {
    path: PathBuf,
    fingerprint: Option<String>,
}

#[derive(Debug, Default)]
struct WorkspaceCache {
    by_key: HashMap<CacheKey, ProjectContext>,
    order: VecDeque<CacheKey>,
}

thread_local! {
    static CACHE: RefCell<WorkspaceCache> = RefCell::new(WorkspaceCache::default());
}

pub(crate) fn lookup(key: &CacheKey) -> Option<ProjectContext> {
    CACHE.with(|cache| cache.borrow().by_key.get(key).cloned())
}

pub(crate) fn store(key: CacheKey, value: ProjectContext) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.by_key.insert(key.clone(), value).is_none() {
            cache.order.push_back(key);
        }
        while cache.by_key.len() > DEFAULT_CAPACITY {
            let Some(oldest) = cache.order.pop_front() else {
                break;
            };
            cache.by_key.remove(&oldest);
        }
    });
}

#[cfg(test)]
pub(crate) fn clear() {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.by_key.clear();
        cache.order.clear();
    });
}

#[must_use]
pub(crate) fn compute_cache_key(workspace: &Path, home_dir: Option<&Path>) -> CacheKey {
    let workspace = canonicalize_or_keep(workspace);
    CacheKey {
        signature: ContentSignature::for_loader(&workspace, home_dir),
        workspace,
    }
}

impl ContentSignature {
    fn for_loader(workspace: &Path, home_dir: Option<&Path>) -> Self {
        let mut entries: Vec<ContentEntry> =
            crate::project_context::project_context_cache_candidate_paths(workspace, home_dir)
                .into_iter()
                .map(|path| ContentEntry {
                    fingerprint: file_fingerprint(&path),
                    path,
                })
                .collect();

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|a, b| a.path == b.path);

        Self { entries }
    }
}

fn file_fingerprint(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return Some("non-file".to_string());
    }

    match std::fs::read(path) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            Some(format!("sha256:{}", to_hex(&hasher.finalize())))
        }
        Err(error) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| format!("{}:{}", duration.as_secs(), duration.subsec_nanos()))
                .unwrap_or_else(|| "unknown".to_string());
            Some(format!(
                "unreadable:{}:{}:{error}",
                metadata.len(),
                modified
            ))
        }
    }
}

fn canonicalize_or_keep(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn cache_round_trip() {
        clear();
        let key = CacheKey {
            workspace: PathBuf::from("/tmp/context-cache-round-trip"),
            signature: ContentSignature::default(),
        };
        let ctx = ProjectContext::empty(PathBuf::from("/tmp/context-cache-round-trip"));

        store(key.clone(), ctx.clone());

        let got = lookup(&key).expect("cache hit");
        assert_eq!(got.project_root, ctx.project_root);
    }

    #[test]
    fn store_does_not_grow_unbounded() {
        clear();
        for i in 0..(DEFAULT_CAPACITY + 4) {
            let key = CacheKey {
                workspace: PathBuf::from(format!("/tmp/workspace-{i}")),
                signature: ContentSignature::default(),
            };
            store(key, ProjectContext::empty(PathBuf::from("/tmp")));
        }

        let count = CACHE.with(|cache| cache.borrow().by_key.len());
        assert!(count <= DEFAULT_CAPACITY, "cache held {count} entries");
    }

    #[test]
    fn cache_key_canonicalizes_equivalent_workspace_paths() {
        let workspace = tempdir().expect("workspace");
        let home = tempdir().expect("home");
        let plain = compute_cache_key(workspace.path(), Some(home.path()));
        let dotted = compute_cache_key(&workspace.path().join("."), Some(home.path()));

        assert_eq!(plain.workspace, dotted.workspace);
    }

    #[test]
    fn signature_changes_when_agents_md_is_overwritten_same_length() {
        let workspace = tempdir().expect("workspace");
        let home = tempdir().expect("home");
        fs::write(workspace.path().join("AGENTS.md"), "alpha").expect("write alpha");
        let before = compute_cache_key(workspace.path(), Some(home.path()));

        fs::write(workspace.path().join("AGENTS.md"), "bravo").expect("write bravo");
        let after = compute_cache_key(workspace.path(), Some(home.path()));

        assert_ne!(before, after);
    }

    #[test]
    fn signature_changes_when_constitution_json_changes() {
        let workspace = tempdir().expect("workspace");
        let home = tempdir().expect("home");
        fs::create_dir(workspace.path().join(".git")).expect("mkdir git");
        fs::create_dir(workspace.path().join(".codewhale")).expect("mkdir codewhale");
        let constitution = workspace
            .path()
            .join(".codewhale")
            .join("constitution.json");
        fs::write(&constitution, r#"{"schema_version":1,"authority":["a"]}"#)
            .expect("write constitution a");
        let before = compute_cache_key(workspace.path(), Some(home.path()));

        fs::write(&constitution, r#"{"schema_version":1,"authority":["b"]}"#)
            .expect("write constitution b");
        let after = compute_cache_key(workspace.path(), Some(home.path()));

        assert_ne!(before, after);
    }
}
