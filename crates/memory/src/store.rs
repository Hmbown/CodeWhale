//! SQLite-backed CRUD for the hippocampal memory store.

use std::path::Path;

use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::schema;

/// A "thing" the model remembers — file, issue, PR, concept, decision, etc.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Entity {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A directed relationship between two entities.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Relation {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub kind: String,
    pub strength: f64,
    pub created_at: String,
    pub session_id: Option<String>,
}

/// A standalone fact the model learned.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Fact {
    pub id: String,
    pub entity_id: Option<String>,
    pub content: String,
    pub source: String,
    pub importance: f64,
    pub created_at: String,
    pub session_id: Option<String>,
}

/// The central memory store — backed by a single SQLite file.
pub struct MemoryStore {
    conn: Connection,
}

impl MemoryStore {
    /// Open (or create + migrate) the memory database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    // ── Entities ──────────────────────────────────────────────────────

    /// Ensure an entity exists. If it does, update the description/updated_at.
    pub fn upsert_entity(&self, kind: &str, name: &str, description: &str) -> Result<Entity> {
        let id = entity_id(kind, name);
        self.conn.execute(
            "INSERT INTO entities (id, kind, name, description) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               description = CASE WHEN ?4 != '' THEN ?4 ELSE description END,
               updated_at = datetime('now')",
            params![id, kind, name, description],
        )?;
        Ok(self.get_entity(&id)?.expect("just upserted"))
    }

    pub fn get_entity(&self, id: &str) -> Result<Option<Entity>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, name, description, created_at, updated_at FROM entities WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        Ok(rows.next()?.map(|r| Entity {
            id: r.get(0).unwrap(),
            kind: r.get(1).unwrap(),
            name: r.get(2).unwrap(),
            description: r.get(3).unwrap(),
            created_at: r.get(4).unwrap(),
            updated_at: r.get(5).unwrap(),
        }))
    }

    pub fn search_entities(&self, query: &str, limit: usize) -> Result<Vec<Entity>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, name, description, created_at, updated_at
             FROM entities
             WHERE name LIKE ?1 OR description LIKE ?1
             ORDER BY updated_at DESC
             LIMIT ?2",
        )?;
        let pattern = format!("%{query}%");
        let rows = stmt.query_map(params![pattern, limit as i64], |r| {
            Ok(Entity {
                id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                description: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Relations ─────────────────────────────────────────────────────

    pub fn upsert_relation(
        &self,
        source_id: &str,
        target_id: &str,
        kind: &str,
        strength: f64,
        session_id: Option<&str>,
    ) -> Result<Relation> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO relations (id, source_id, target_id, kind, strength, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(source_id, target_id, kind) DO UPDATE SET
               strength = ?5,
               session_id = COALESCE(?6, session_id),
               created_at = datetime('now')",
            params![id, source_id, target_id, kind, strength, session_id],
        )?;
        Ok(self
            .conn
            .query_row(
                "SELECT id, source_id, target_id, kind, strength, created_at, session_id
                 FROM relations WHERE id = ?1",
                params![id],
                |r| {
                    Ok(Relation {
                        id: r.get(0)?,
                        source_id: r.get(1)?,
                        target_id: r.get(2)?,
                        kind: r.get(3)?,
                        strength: r.get(4)?,
                        created_at: r.get(5)?,
                        session_id: r.get(6)?,
                    })
                },
            )?)
    }

    /// Find all relations connected to an entity (either as source or target).
    pub fn relations_for_entity(&self, entity_id: &str, limit: usize) -> Result<Vec<Relation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_id, target_id, kind, strength, created_at, session_id
             FROM relations
             WHERE source_id = ?1 OR target_id = ?1
             ORDER BY strength DESC, created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![entity_id, limit as i64], |r| {
            Ok(Relation {
                id: r.get(0)?,
                source_id: r.get(1)?,
                target_id: r.get(2)?,
                kind: r.get(3)?,
                strength: r.get(4)?,
                created_at: r.get(5)?,
                session_id: r.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Walk the graph: given an entity, find entities reachable via relations of `kind`.
    pub fn graph_walk(&self, start_id: &str, relation_kind: &str, depth: usize) -> Result<Vec<Entity>> {
        if depth == 0 || depth > 5 {
            bail!("graph walk depth must be 1–5");
        }

        // Simple 1-hop walker — we expand this to n-hops with a recursive CTE later.
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.kind, e.name, e.description, e.created_at, e.updated_at
             FROM relations r
             JOIN entities e ON e.id = r.target_id
             WHERE r.source_id = ?1 AND r.kind = ?2
             UNION
             SELECT e.id, e.kind, e.name, e.description, e.created_at, e.updated_at
             FROM relations r
             JOIN entities e ON e.id = r.source_id
             WHERE r.target_id = ?1 AND r.kind = ?2
             LIMIT 30",
        )?;
        let rows = stmt.query_map(params![start_id, relation_kind], |r| {
            Ok(Entity {
                id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                description: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Facts ──────────────────────────────────────────────────────────

    pub fn insert_fact(
        &self,
        entity_id: Option<&str>,
        content: &str,
        source: &str,
        importance: f64,
        session_id: Option<&str>,
    ) -> Result<Fact> {
        let id = Uuid::new_v4().to_string();
        let importance = importance.clamp(0.0, 1.0);
        self.conn.execute(
            "INSERT INTO facts (id, entity_id, content, source, importance, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, entity_id, content, source, importance, session_id],
        )?;
        Ok(self
            .conn
            .query_row(
                "SELECT id, entity_id, content, source, importance, created_at, session_id
                 FROM facts WHERE id = ?1",
                params![id],
                |r| {
                    Ok(Fact {
                        id: r.get(0)?,
                        entity_id: r.get(1)?,
                        content: r.get(2)?,
                        source: r.get(3)?,
                        importance: r.get(4)?,
                        created_at: r.get(5)?,
                        session_id: r.get(6)?,
                    })
                },
            )?)
    }

    /// Full-text search over facts (uses FTS5 for pattern-completion-like queries).
    pub fn search_facts(&self, query: &str, limit: usize) -> Result<Vec<Fact>> {
        // Escape FTS5 special characters and use prefix matching
        let safe = query.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>();
        let fts_query = safe
            .split_whitespace()
            .map(|w| format!("{w}*"))
            .collect::<Vec<_>>()
            .join(" AND ");

        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.entity_id, f.content, f.source, f.importance, f.created_at, f.session_id
             FROM facts f
             JOIN facts_fts ON facts_fts.rowid = f.rowid
             WHERE facts_fts MATCH ?1
             ORDER BY f.importance DESC, f.created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], |r| {
            Ok(Fact {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                content: r.get(2)?,
                source: r.get(3)?,
                importance: r.get(4)?,
                created_at: r.get(5)?,
                session_id: r.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get the most important facts (no query = general overview).
    pub fn important_facts(&self, limit: usize) -> Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, entity_id, content, source, importance, created_at, session_id
             FROM facts
             ORDER BY importance DESC, created_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(Fact {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                content: r.get(2)?,
                source: r.get(3)?,
                importance: r.get(4)?,
                created_at: r.get(5)?,
                session_id: r.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Delete old/low-importance facts (active forgetting).
    pub fn prune_low_importance_facts(&self, threshold: f64, older_than_days: i64) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM facts WHERE importance < ?1
             AND datetime(created_at) < datetime('now', ?2)",
            params![threshold, format!("-{older_than_days} days")],
        )?;
        Ok(count)
    }
}

/// Deterministic entity ID based on kind + name (same kind+name → same ID).
fn entity_id(kind: &str, name: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(format!("{kind}\0{name}").as_bytes());
    hash.iter().take(8).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_store() -> MemoryStore {
        let dir = tempdir().unwrap();
        MemoryStore::open(&dir.path().join("memory.db")).unwrap()
    }

    #[test]
    fn test_entity_upsert_and_search() {
        let store = test_store();
        let e = store.upsert_entity("file", "dispatch.rs", "Tool error formatting").unwrap();
        assert_eq!(e.kind, "file");
        assert_eq!(e.name, "dispatch.rs");

        let found = store.search_entities("dispatch", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, e.id);
    }

    #[test]
    fn test_relation_and_graph_walk() {
        let store = test_store();
        let file = store.upsert_entity("file", "dispatch.rs", "").unwrap();
        let pr = store.upsert_entity("pr", "PR #2933", "fix: tool error messages").unwrap();

        store.upsert_relation(&file.id, &pr.id, "part_of", 1.0, None).unwrap();

        let connected = store.graph_walk(&file.id, "part_of", 1).unwrap();
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].id, pr.id);
    }

    #[test]
    fn test_fact_insert_and_search() {
        let store = test_store();
        let e = store.upsert_entity("file", "dispatch.rs", "").unwrap();

        store.insert_fact(
            Some(&e.id),
            "format_tool_error had misleading generic suffix. Fixed by removing it.",
            "code review",
            0.9,
            None,
        ).unwrap();

        // FTS5 search via pattern completion
        let results = store.search_facts("format tool error", 10).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].content.contains("format_tool_error"));
    }

    #[test]
    fn test_prune_low_importance() {
        let store = test_store();
        store.insert_fact(None, "transient debug note", "debug", 0.1, None).unwrap();
        store.insert_fact(None, "important architecture decision", "design", 0.9, None).unwrap();

        let pruned = store.prune_low_importance_facts(0.3, 0).unwrap();
        assert_eq!(pruned, 1);

        let remaining = store.important_facts(10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].content.contains("architecture"));
    }
}
