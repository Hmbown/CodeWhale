//! SQLite schema for the hippocampal memory store.
//!
//! Uses a versioned migration system so the schema can evolve without
//! breaking existing databases.
//!
//! ## Current Schema (v2)
//!
//! **Core tables** (from v1):
//! - `entities`: A "thing" the model might need to remember
//! - `relations`: Directed edges connecting two entities
//! - `facts`: Standalone factual statements, optionally bound to an entity
//! - `facts_fts`: FTS5 full-text index over facts
//!
//! **Added in v2**:
//! - `namespaces`: Workspace/project-level isolation
//! - `glossary`: Keyword/tag definitions
//! - `fact_glossary` / `entity_glossary`: Many-to-many relationship links
//! - `fact_versions`: Version history for rollback support
//! - `schema_version`: Migration tracking

use rusqlite::Connection;

/// Run all pending migrations on `conn`.
/// Safe to call repeatedly — each migration runs exactly once.
pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    // Create schema version tracker
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version    INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if current < 1 {
        tracing::info!("memory: running migration v1 (initial schema)");
        migration_v1(conn)?;
    }
    if current < 2 {
        tracing::info!("memory: running migration v2 (namespaces + glossary + versions)");
        migration_v2(conn)?;
    }

    Ok(())
}

// ── Migration v1: initial schema ────────────────────────────────────────

fn migration_v1(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS entities (
            id          TEXT PRIMARY KEY,
            kind        TEXT NOT NULL,
            name        TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_entities_kind ON entities(kind);
        CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);

        CREATE TABLE IF NOT EXISTS relations (
            id            TEXT PRIMARY KEY,
            source_id     TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            target_id     TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            kind          TEXT NOT NULL,
            strength      REAL NOT NULL DEFAULT 1.0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            session_id    TEXT,
            UNIQUE(source_id, target_id, kind)
        );

        CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_id);
        CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_id);
        CREATE INDEX IF NOT EXISTS idx_relations_kind   ON relations(kind);

        CREATE TABLE IF NOT EXISTS facts (
            id          TEXT PRIMARY KEY,
            entity_id   TEXT REFERENCES entities(id) ON DELETE SET NULL,
            content     TEXT NOT NULL,
            source      TEXT NOT NULL DEFAULT '',
            importance  REAL NOT NULL DEFAULT 0.5,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            session_id  TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_facts_entity     ON facts(entity_id);
        CREATE INDEX IF NOT EXISTS idx_facts_importance ON facts(importance DESC);

        CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
            content,
            content=facts,
            content_rowid=rowid
        );

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

        INSERT INTO schema_version (version) VALUES (1);
        ",
    )
}

// ── Migration v2: namespaces + glossary + fact versions ─────────────────

fn migration_v2(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        -- Namespace table: workspace/project-level isolation
        CREATE TABLE IF NOT EXISTS namespaces (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            description TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Glossary/tags: keyword system for labeling memories
        CREATE TABLE IF NOT EXISTS glossary (
            id           TEXT PRIMARY KEY,
            term         TEXT NOT NULL,
            definition   TEXT NOT NULL DEFAULT '',
            category     TEXT NOT NULL DEFAULT 'general',
            namespace_id TEXT REFERENCES namespaces(id) ON DELETE SET NULL,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(term, namespace_id)
        );

        CREATE INDEX IF NOT EXISTS idx_glossary_term      ON glossary(term);
        CREATE INDEX IF NOT EXISTS idx_glossary_category  ON glossary(category);
        CREATE INDEX IF NOT EXISTS idx_glossary_namespace ON glossary(namespace_id);

        -- Fact-to-glossary mapping (many-to-many)
        CREATE TABLE IF NOT EXISTS fact_glossary (
            fact_id     TEXT NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
            glossary_id TEXT NOT NULL REFERENCES glossary(id) ON DELETE CASCADE,
            PRIMARY KEY (fact_id, glossary_id)
        );

        -- Entity-to-glossary mapping (many-to-many)
        CREATE TABLE IF NOT EXISTS entity_glossary (
            entity_id   TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            glossary_id TEXT NOT NULL REFERENCES glossary(id) ON DELETE CASCADE,
            PRIMARY KEY (entity_id, glossary_id)
        );

        -- Fact versions: provides rollback support
        CREATE TABLE IF NOT EXISTS fact_versions (
            id          TEXT PRIMARY KEY,
            fact_id     TEXT NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
            content     TEXT NOT NULL,
            source      TEXT NOT NULL DEFAULT '',
            importance  REAL NOT NULL DEFAULT 0.5,
            version     INTEGER NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            session_id  TEXT,
            UNIQUE(fact_id, version)
        );

        CREATE INDEX IF NOT EXISTS idx_fact_versions_fact_id ON fact_versions(fact_id);
        CREATE INDEX IF NOT EXISTS idx_fact_versions_version ON fact_versions(fact_id, version DESC);

        -- Add namespace_id columns to existing tables via ALTER TABLE.
        -- SQLite ignores the IF NOT EXISTS clause silently on older versions,
        -- but rusqlite's bundled SQLite (3.45+) supports it.
        ALTER TABLE entities  ADD COLUMN namespace_id TEXT REFERENCES namespaces(id);
        ALTER TABLE relations ADD COLUMN namespace_id TEXT REFERENCES namespaces(id);
        ALTER TABLE facts     ADD COLUMN namespace_id TEXT REFERENCES namespaces(id);
        ALTER TABLE facts     ADD COLUMN version      INTEGER NOT NULL DEFAULT 1;

        INSERT INTO schema_version (version) VALUES (2);
        ",
    )
}
