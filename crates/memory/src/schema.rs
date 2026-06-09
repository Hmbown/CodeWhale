//! SQLite schema for the hippocampal memory store.
//!
//! Three core tables:
//!
//! - **`entities`**: A "thing" the model might need to remember — a file path,
//!   an issue number, a PR, a person, a concept, a decision.
//! - **`relations`**: A directed edge connecting two entities. The `kind` field
//!   says what the relationship means (e.g. `"fixes"`, `"part_of"`, `"depends_on"`).
//! - **`facts`**: A standalone statement about something the model learned. May
//!   reference an entity via `entity_id`.

use rusqlite::Connection;

/// Create all tables if they don't exist.
pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS entities (
            id          TEXT PRIMARY KEY,
            kind        TEXT NOT NULL,         -- 'file', 'issue', 'pr', 'concept', 'decision', 'person', 'config'
            name        TEXT NOT NULL,         -- human-readable label
            description TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_entities_kind   ON entities(kind);
        CREATE INDEX IF NOT EXISTS idx_entities_name   ON entities(name);

        CREATE TABLE IF NOT EXISTS relations (
            id            TEXT PRIMARY KEY,
            source_id     TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            target_id     TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            kind          TEXT NOT NULL,       -- 'fixes', 'part_of', 'depends_on', 'contains', 'references', 'implements'
            strength      REAL NOT NULL DEFAULT 1.0,  -- 0.0–1.0 confidence/importance
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            session_id    TEXT,                 -- which session created this relation
            UNIQUE(source_id, target_id, kind)
        );

        CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_id);
        CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_id);
        CREATE INDEX IF NOT EXISTS idx_relations_kind   ON relations(kind);

        CREATE TABLE IF NOT EXISTS facts (
            id          TEXT PRIMARY KEY,
            entity_id   TEXT REFERENCES entities(id) ON DELETE SET NULL,
            content     TEXT NOT NULL,          -- the factual statement
            source      TEXT NOT NULL DEFAULT '', -- where this fact came from (tool call, session, user)
            importance  REAL NOT NULL DEFAULT 0.5,  -- 0.0–1.0
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            session_id  TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_facts_entity   ON facts(entity_id);
        CREATE INDEX IF NOT EXISTS idx_facts_importance ON facts(importance DESC);

        -- Full-text search over facts (enables pattern-completion-like queries)
        CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
            content,
            content=facts,
            content_rowid=rowid
        );

        -- Triggers to keep FTS index in sync
        CREATE TRIGGER IF NOT EXISTS facts_ai AFTER INSERT ON facts BEGIN
            INSERT INTO facts_fts(rowid, content) VALUES (new.rowid, new.content);
        END;

        CREATE TRIGGER IF NOT EXISTS facts_ad AFTER DELETE ON facts BEGIN
            INSERT INTO facts_fts(facts_fts, rowid, content) VALUES('delete', old.rowid, old.content);
        END;

        CREATE TRIGGER IF NOT EXISTS facts_au AFTER UPDATE ON facts BEGIN
            INSERT INTO facts_fts(facts_fts, rowid, content) VALUES('delete', old.rowid, old.content);
            INSERT INTO facts_fts(rowid, content) VALUES (new.rowid, new.content);
        END;
        ",
    )
}
