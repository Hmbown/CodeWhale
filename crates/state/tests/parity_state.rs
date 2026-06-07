use std::path::PathBuf;

use codewhale_state::{SessionSource, StateStore, ThreadListFilters, ThreadMetadata, ThreadStatus};
use rusqlite::Connection;

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
        current_leaf_id: None,
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

#[test]
fn init_schema_migration() {
    let path = temp_state_path("init_schema_migration");
    let conn = Connection::open(&path).expect("open state db");
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS threads (
            id TEXT PRIMARY KEY,
            rollout_path TEXT,
            preview TEXT NOT NULL,
            ephemeral INTEGER NOT NULL,
            model_provider TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            status TEXT NOT NULL,
            path TEXT,
            cwd TEXT NOT NULL,
            cli_version TEXT NOT NULL,
            source TEXT NOT NULL,
            title TEXT,
            sandbox_policy TEXT,
            approval_mode TEXT,
            archived INTEGER NOT NULL DEFAULT 0,
            archived_at INTEGER,
            git_sha TEXT,
            git_branch TEXT,
            git_origin_url TEXT,
            memory_mode TEXT
        );
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            item_json TEXT,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
        );
        INSERT INTO threads (
            id, preview, ephemeral, model_provider, created_at, updated_at, status, cwd, cli_version, source, archived
        )
        VALUES (
            'thread-test-1', 'hello', false, 'deepseek', 0, 0, 'running', '/tmp/project', '0.0.0-test', 'interactive', false
        );
        INSERT INTO messages (thread_id, role, content, created_at) VALUES
        ('thread-test-1', 'foo0', 'bar0', 0),
        ('thread-test-1', 'foo1', 'bar1', 1),
        ('thread-test-1', 'foo2', 'bar2', 2);
        "#,
    )
    .expect("init schema migration");

    let store = StateStore::open(Some(path.clone())).expect("open state store");
    let thread = store
        .get_thread("thread-test-1")
        .expect("read thread")
        .unwrap();
    assert_eq!(thread.id, "thread-test-1");
    assert_eq!(thread.preview, "hello");
    assert!(!thread.ephemeral);
    assert_eq!(thread.model_provider, "deepseek");
    assert_eq!(thread.created_at, 0);
    assert_eq!(thread.updated_at, 0);
    assert_eq!(thread.status, ThreadStatus::Running);
    assert_eq!(thread.cwd, PathBuf::from("/tmp/project"));
    assert_eq!(thread.cli_version, "0.0.0-test");
    assert_eq!(thread.source, SessionSource::Interactive);
    assert!(thread.current_leaf_id.is_some());

    let messages = store
        .list_messages("thread-test-1", None)
        .expect("list messages");
    assert_eq!(messages.len(), 3);
    for (i, message) in messages.iter().enumerate() {
        assert_eq!(message.thread_id, "thread-test-1");
        assert_eq!(message.role, format!("foo{}", i));
        assert_eq!(message.content, format!("bar{}", i));
        assert_eq!(message.created_at, i as i64);
    }

    // Test idempotent
    StateStore::open(Some(path.clone())).expect("open state store");
}

#[test]
fn init_schema_migration_same_second_messages() {
    let path = temp_state_path("init_schema_migration_same_second_messages");
    let conn = Connection::open(&path).expect("open state db");
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS threads (
            id TEXT PRIMARY KEY,
            rollout_path TEXT,
            preview TEXT NOT NULL,
            ephemeral INTEGER NOT NULL,
            model_provider TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            status TEXT NOT NULL,
            path TEXT,
            cwd TEXT NOT NULL,
            cli_version TEXT NOT NULL,
            source TEXT NOT NULL,
            title TEXT,
            sandbox_policy TEXT,
            approval_mode TEXT,
            archived INTEGER NOT NULL DEFAULT 0,
            archived_at INTEGER,
            git_sha TEXT,
            git_branch TEXT,
            git_origin_url TEXT,
            memory_mode TEXT
        );
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            item_json TEXT,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
        );
        INSERT INTO threads (
            id, preview, ephemeral, model_provider, created_at, updated_at, status, cwd, cli_version, source, archived
        )
        VALUES (
            'thread-test-2', 'hello', false, 'deepseek', 0, 0, 'running', '/tmp/project', '0.0.0-test', 'interactive', false
        );
        INSERT INTO messages (thread_id, role, content, created_at) VALUES
            ('thread-test-2', 'foo0', 'bar0', 123),
            ('thread-test-2', 'foo1', 'bar1', 123),
            ('thread-test-2', 'foo2', 'bar2', 123),
            ('thread-test-2', 'foo3', 'bar3', 123);
        "#,
    )
    .expect("init schema migration");

    let store = StateStore::open(Some(path.clone())).expect("open state store");
    let messages = store
        .list_messages("thread-test-2", None)
        .expect("list messages");
    assert_eq!(messages.len(), 4);
    for (i, message) in messages.iter().enumerate() {
        assert_eq!(message.thread_id, "thread-test-2");
        assert_eq!(message.role, format!("foo{}", i));
        assert_eq!(message.content, format!("bar{}", i));
        assert_eq!(message.created_at, 123);
    }
    assert_eq!(messages[0].parent_entry_id, None);
    assert_eq!(messages[1].parent_entry_id, Some(messages[0].id));
    assert_eq!(messages[2].parent_entry_id, Some(messages[1].id));
    assert_eq!(messages[3].parent_entry_id, Some(messages[2].id));

    // Test idempotent reopen after same-second parent links are migrated.
    StateStore::open(Some(path.clone())).expect("open state store - idempotent");
}

#[test]
fn test_fork() {
    let path = temp_state_path("test_fork");
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
        current_leaf_id: None,
    };

    store.upsert_thread(&thread).expect("upsert thread");
    store
        .append_message("thread-test-1", "foo0", "bar0", None)
        .expect("append message");
    store
        .append_message("thread-test-1", "foo1", "bar1", None)
        .expect("append message");
    store
        .append_message("thread-test-1", "foo2", "bar2", None)
        .expect("append message");
    store
        .append_message("thread-test-1", "foo3", "bar3", None)
        .expect("append message");
    store
        .append_message("thread-test-1", "foo4", "bar4", None)
        .expect("append message");

    let messages = store
        .list_messages("thread-test-1", None)
        .expect("list messages");
    assert_eq!(messages.len(), 5);
    let ids = messages
        .iter()
        .enumerate()
        .map(|(i, message)| {
            assert_eq!(message.thread_id, "thread-test-1");
            assert_eq!(message.role, format!("foo{}", i));
            assert_eq!(message.content, format!("bar{}", i));
            message.id.to_string()
        })
        .collect::<Vec<_>>();

    store.upsert_thread(&thread).expect("upsert thread");

    store
        .fork_at_message(&ids[2], "foo5", "bar5", None)
        .expect("fork at message");
    let messages = store
        .list_messages("thread-test-1", None)
        .expect("list messages");
    assert_eq!(messages.len(), 4);
    const LIST_1: [i64; 4] = [0, 1, 2, 5];
    messages
        .iter()
        .zip(LIST_1.iter())
        .for_each(|(message, &i)| {
            assert_eq!(message.thread_id, "thread-test-1");
            assert_eq!(message.role, format!("foo{}", i));
            assert_eq!(message.content, format!("bar{}", i));
        });
    let leaves = store
        .list_leaf_messages("thread-test-1")
        .expect("list leaf messages");
    assert_eq!(leaves.len(), 2);

    store
        .set_current_leaf_id("thread-test-1", &ids[4])
        .expect("set current leaf id");
    store
        .append_message("thread-test-1", "foo6", "bar6", None)
        .expect("append message");
    let messages = store
        .list_messages("thread-test-1", None)
        .expect("list messages");
    assert_eq!(messages.len(), 6);
    const LIST_2: [i64; 6] = [0, 1, 2, 3, 4, 6];
    messages
        .iter()
        .zip(LIST_2.iter())
        .for_each(|(message, &i)| {
            assert_eq!(message.thread_id, "thread-test-1");
            assert_eq!(message.role, format!("foo{}", i));
            assert_eq!(message.content, format!("bar{}", i));
        });

    let leaves = store
        .list_leaf_messages("thread-test-1")
        .expect("list leaf messages");
    assert_eq!(leaves.len(), 2);

    store
        .clear_messages("thread-test-1")
        .expect("clear messages");
    let leaves = store
        .list_leaf_messages("thread-test-1")
        .expect("list leaf messages");
    assert_eq!(leaves.len(), 0);
    let thread = store
        .get_thread("thread-test-1")
        .expect("get thread")
        .unwrap();
    assert!(thread.current_leaf_id.is_none());
}
