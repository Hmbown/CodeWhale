use std::path::PathBuf;

use codewhale_state::{SessionSource, StateStore, ThreadListFilters, ThreadMetadata, ThreadStatus};

fn temp_state_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "deepseek_state_test_{}_{}_{}.db",
        label,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

#[test]
fn upsert_and_resume_thread_metadata() {
    let path = temp_state_path("upsert_resume");
    let store = StateStore::open(Some(path.clone())).expect("open state store");
    let now = chrono::Utc::now().timestamp();
    let thread = ThreadMetadata {
        id: "thread-test-1".to_string(),
        rollout_path: Some(PathBuf::from("/tmp/rollout.jsonl")),
        preview: "hello".to_string(),
        ephemeral: false,
        model_provider: "deepseek".to_string(),
        created_at: now,
        updated_at: now,
        status: ThreadStatus::Running,
        path: Some(PathBuf::from("/tmp/project")),
        cwd: PathBuf::from("/tmp/project"),
        cli_version: "0.0.0-test".to_string(),
        source: SessionSource::Interactive,
        name: Some("Test Thread".to_string()),
        sandbox_policy: Some("workspace-write".to_string()),
        approval_mode: Some("on-request".to_string()),
        archived: false,
        archived_at: None,
        git_sha: None,
        git_branch: None,
        git_origin_url: None,
        memory_mode: Some("extended".to_string()),
    };
    store.upsert_thread(&thread).expect("upsert thread");

    let loaded = store
        .get_thread("thread-test-1")
        .expect("read thread")
        .expect("thread must exist");
    assert_eq!(loaded.id, "thread-test-1");
    assert_eq!(loaded.name.as_deref(), Some("Test Thread"));
    assert_eq!(loaded.memory_mode.as_deref(), Some("extended"));
    assert_eq!(
        loaded.rollout_path,
        Some(PathBuf::from("/tmp/rollout.jsonl"))
    );

    store
        .mark_archived("thread-test-1")
        .expect("archive thread");
    let archived = store
        .get_thread("thread-test-1")
        .expect("read archived thread")
        .expect("thread exists after archive");
    assert!(archived.archived);

    let listed = store
        .list_threads(ThreadListFilters {
            include_archived: true,
            limit: Some(10),
        })
        .expect("list threads");
    assert!(!listed.is_empty());
}
