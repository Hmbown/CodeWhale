//! SQLite-backed CRUD for the hippocampal memory store.
//!
//! Provides structured storage for entities, relations, facts (with FTS5),
//! glossary terms, fact version history, and namespace-level isolation.

use std::path::Path;

use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::schema;

// ── Data types ─────────────────────────────────────────────────────────

/// A "thing" the model remembers — file, issue, PR, concept, decision, etc.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Entity {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
    pub namespace_id: Option<String>,
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
    pub namespace_id: Option<String>,
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
    pub namespace_id: Option<String>,
    pub version: i64,
}

/// A workspace/project-level namespace for memory isolation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Namespace {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A keyword/tag that can be attached to facts or entities.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlossaryTerm {
    pub id: String,
    pub term: String,
    pub definition: String,
    pub category: String,
    pub namespace_id: Option<String>,
    pub created_at: String,
}

/// A historical version of a fact (for rollback support).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FactVersion {
    pub id: String,
    pub fact_id: String,
    pub content: String,
    pub source: String,
    pub importance: f64,
    pub version: i64,
    pub created_at: String,
    pub session_id: Option<String>,
}

/// Aggregate statistics about the memory store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct MemoryStats {
    pub total_entities: i64,
    pub total_relations: i64,
    pub total_facts: i64,
    pub total_glossary_terms: i64,
    pub total_namespaces: i64,
    pub avg_importance: f64,
    pub oldest_fact: Option<String>,
    pub newest_fact: Option<String>,
}

// ── MemoryStore ────────────────────────────────────────────────────────

/// The central memory store — backed by a single SQLite file.
///
/// Thread-safe via `std::sync::Mutex` so the store can be shared
/// across `Arc` boundaries (engine → tool context).
pub struct MemoryStore {
    conn: std::sync::Mutex<Connection>,
}

impl MemoryStore {
    /// Open (or create + migrate) the memory database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        schema::migrate(&conn)?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    // ── Namespaces ──────────────────────────────────────────────────

    /// Create or update a namespace.
    pub fn upsert_namespace(&self, name: &str, description: &str) -> Result<Namespace> {
        let conn = self.conn.lock().unwrap();
        let id = namespace_id(name);
        conn.execute(
            "INSERT INTO namespaces (id, name, description) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET
               description = CASE WHEN ?3 != '' THEN ?3 ELSE description END,
               updated_at = datetime('now')",
            params![id, name, description],
        )?;
        drop(conn);
        self.get_namespace(&id)?.ok_or_else(|| anyhow::anyhow!("namespace not found after upsert"))
    }

    /// Get a namespace by ID.
    pub fn get_namespace(&self, id: &str) -> Result<Option<Namespace>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, created_at, updated_at FROM namespaces WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        Ok(rows.next()?.map(|r| Namespace {
            id: r.get(0).unwrap(),
            name: r.get(1).unwrap(),
            description: r.get(2).unwrap(),
            created_at: r.get(3).unwrap(),
            updated_at: r.get(4).unwrap(),
        }))
    }

    /// Get or create a namespace for a workspace path.
    /// Uses the workspace path as the namespace name.
    pub fn get_or_create_workspace_namespace(&self, workspace_path: &str) -> Result<Namespace> {
        let name = format!("workspace:{workspace_path}");
        // Try to find existing
        let conn = self.conn.lock().unwrap();
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM namespaces WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .ok();
        drop(conn);

        if let Some(id) = existing {
            return self.get_namespace(&id)?.ok_or_else(|| anyhow::anyhow!("namespace disappeared"));
        }
        self.upsert_namespace(&name, "Auto-created workspace namespace")
    }

    /// List all namespaces.
    pub fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, created_at, updated_at FROM namespaces ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Namespace {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Entities ────────────────────────────────────────────────────

    /// Ensure an entity exists. If it does, update the description/updated_at.
    pub fn upsert_entity(
        &self,
        kind: &str,
        name: &str,
        description: &str,
    ) -> Result<Entity> {
        let conn = self.conn.lock().unwrap();
        let id = entity_id(kind, name);
        conn.execute(
            "INSERT INTO entities (id, kind, name, description) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               description = CASE WHEN ?4 != '' THEN ?4 ELSE description END,
               updated_at = datetime('now')",
            params![id, kind, name, description],
        )?;
        drop(conn);
        Ok(self.get_entity(&id)?.expect("just upserted"))
    }

    /// Upsert entity with namespace support.
    pub fn upsert_entity_in_namespace(
        &self,
        kind: &str,
        name: &str,
        description: &str,
        namespace_id: Option<&str>,
    ) -> Result<Entity> {
        let conn = self.conn.lock().unwrap();
        let id = entity_id(kind, name);
        conn.execute(
            "INSERT INTO entities (id, kind, name, description, namespace_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               description = CASE WHEN ?4 != '' THEN ?4 ELSE description END,
               namespace_id = COALESCE(?5, namespace_id),
               updated_at = datetime('now')",
            params![id, kind, name, description, namespace_id],
        )?;
        drop(conn);
        Ok(self.get_entity(&id)?.expect("just upserted"))
    }

    pub fn get_entity(&self, id: &str) -> Result<Option<Entity>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, description, created_at, updated_at, namespace_id
             FROM entities WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        Ok(rows.next()?.map(|r| Entity {
            id: r.get(0).unwrap(),
            kind: r.get(1).unwrap(),
            name: r.get(2).unwrap(),
            description: r.get(3).unwrap(),
            created_at: r.get(4).unwrap(),
            updated_at: r.get(5).unwrap(),
            namespace_id: r.get(6).ok(),
        }))
    }

    pub fn search_entities(&self, query: &str, limit: usize) -> Result<Vec<Entity>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, description, created_at, updated_at, namespace_id
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
                namespace_id: r.get(6).ok(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Search entities within a specific namespace.
    pub fn search_entities_in_namespace(
        &self,
        query: &str,
        namespace_id: &str,
        limit: usize,
    ) -> Result<Vec<Entity>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, description, created_at, updated_at, namespace_id
             FROM entities
             WHERE (name LIKE ?1 OR description LIKE ?1) AND namespace_id = ?2
             ORDER BY updated_at DESC
             LIMIT ?3",
        )?;
        let pattern = format!("%{query}%");
        let rows = stmt.query_map(params![pattern, namespace_id, limit as i64], |r| {
            Ok(Entity {
                id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                description: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
                namespace_id: r.get(6).ok(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Delete an entity by ID.
    pub fn delete_entity(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute("DELETE FROM entities WHERE id = ?1", params![id])?;
        Ok(count > 0)
    }

    // ── Relations ───────────────────────────────────────────────────

    pub fn upsert_relation(
        &self,
        source_id: &str,
        target_id: &str,
        kind: &str,
        strength: f64,
        session_id: Option<&str>,
    ) -> Result<Relation> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO relations (id, source_id, target_id, kind, strength, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(source_id, target_id, kind) DO UPDATE SET
               strength = ?5,
               session_id = COALESCE(?6, session_id),
               created_at = datetime('now')",
            params![id, source_id, target_id, kind, strength, session_id],
        )?;
        let rel = conn.query_row(
            "SELECT id, source_id, target_id, kind, strength, created_at, session_id, namespace_id
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
                    namespace_id: r.get(7).ok(),
                })
            },
        )?;
        Ok(rel)
    }

    /// Find all relations connected to an entity (either as source or target).
    pub fn relations_for_entity(&self, entity_id: &str, limit: usize) -> Result<Vec<Relation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, source_id, target_id, kind, strength, created_at, session_id, namespace_id
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
                namespace_id: r.get(7).ok(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Walk the graph: given an entity, find entities reachable via relations of `kind`.
    pub fn graph_walk(
        &self,
        start_id: &str,
        relation_kind: &str,
        _depth: usize,
    ) -> Result<Vec<Entity>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.kind, e.name, e.description, e.created_at, e.updated_at, e.namespace_id
             FROM relations r
             JOIN entities e ON e.id = r.target_id
             WHERE r.source_id = ?1 AND r.kind = ?2
             UNION
             SELECT e.id, e.kind, e.name, e.description, e.created_at, e.updated_at, e.namespace_id
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
                namespace_id: r.get(6).ok(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Facts ───────────────────────────────────────────────────────

    pub fn insert_fact(
        &self,
        entity_id: Option<&str>,
        content: &str,
        source: &str,
        importance: f64,
        session_id: Option<&str>,
    ) -> Result<Fact> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        let importance = importance.clamp(0.0, 1.0);
        conn.execute(
            "INSERT INTO facts (id, entity_id, content, source, importance, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, entity_id, content, source, importance, session_id],
        )?;
        drop(conn);
        self.get_fact(&id)?.ok_or_else(|| anyhow::anyhow!("fact not found after insert"))
    }

    /// Insert a fact with namespace support. Also saves version 1 automatically.
    pub fn insert_fact_in_namespace(
        &self,
        entity_id: Option<&str>,
        content: &str,
        source: &str,
        importance: f64,
        session_id: Option<&str>,
        namespace_id: Option<&str>,
    ) -> Result<Fact> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        let importance = importance.clamp(0.0, 1.0);
        conn.execute(
            "INSERT INTO facts (id, entity_id, content, source, importance, session_id, namespace_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, entity_id, content, source, importance, session_id, namespace_id],
        )?;

        // Save initial version
        conn.execute(
            "INSERT INTO fact_versions (id, fact_id, content, source, importance, version, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params![Uuid::new_v4().to_string(), id, content, source, importance, session_id],
        )?;

        drop(conn);
        self.get_fact(&id)?.ok_or_else(|| anyhow::anyhow!("fact not found after insert"))
    }

    /// Get a fact by ID.
    pub fn get_fact(&self, id: &str) -> Result<Option<Fact>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, entity_id, content, source, importance, created_at, session_id, namespace_id, version
             FROM facts WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        Ok(rows.next()?.map(|r| Fact {
            id: r.get(0).unwrap(),
            entity_id: r.get(1).ok(),
            content: r.get(2).unwrap(),
            source: r.get(3).unwrap(),
            importance: r.get(4).unwrap(),
            created_at: r.get(5).unwrap(),
            session_id: r.get(6).ok(),
            namespace_id: r.get(7).ok(),
            version: r.get(8).unwrap(),
        }))
    }

    /// Update an existing fact, saving the previous version for rollback.
    pub fn update_fact(
        &self,
        fact_id: &str,
        new_content: &str,
        new_importance: f64,
        session_id: Option<&str>,
    ) -> Result<Fact> {
        let conn = self.conn.lock().unwrap();
        let importance = new_importance.clamp(0.0, 1.0);

        // Get current state to save as old version
        let (old_content, old_source, old_importance, old_version): (String, String, f64, i64) = conn
            .query_row(
                "SELECT content, source, importance, version FROM facts WHERE id = ?1",
                params![fact_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|_| anyhow::anyhow!("fact not found: {fact_id}"))?;

        // Save old version
        conn.execute(
            "INSERT INTO fact_versions (id, fact_id, content, source, importance, version, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                Uuid::new_v4().to_string(),
                fact_id,
                old_content,
                old_source,
                old_importance,
                old_version,
                session_id,
            ],
        )?;

        // Update the fact with new content and bumped version
        conn.execute(
            "UPDATE facts SET content = ?1, importance = ?2, version = ?3, session_id = ?4
             WHERE id = ?5",
            params![new_content, importance, old_version + 1, session_id, fact_id],
        )?;

        drop(conn);
        self.get_fact(fact_id)?.ok_or_else(|| anyhow::anyhow!("fact disappeared after update"))
    }

    /// Rollback a fact to a specific version.
    /// Returns the restored fact.
    pub fn rollback_fact(&self, fact_id: &str, target_version: i64) -> Result<Fact> {
        let conn = self.conn.lock().unwrap();

        // Find the target version
        let (content, source, importance): (String, String, f64) = conn
            .query_row(
                "SELECT content, source, importance FROM fact_versions
                 WHERE fact_id = ?1 AND version = ?2
                 ORDER BY version DESC LIMIT 1",
                params![fact_id, target_version],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| {
                anyhow::anyhow!("version {target_version} not found for fact {fact_id}")
            })?;

        // Get current version to save as old
        let (old_content, old_source, old_importance, old_version): (String, String, f64, i64) = conn
            .query_row(
                "SELECT content, source, importance, version FROM facts WHERE id = ?1",
                params![fact_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;

        // Save current as version before rollback
        conn.execute(
            "INSERT INTO fact_versions (id, fact_id, content, source, importance, version, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                Uuid::new_v4().to_string(),
                fact_id,
                old_content,
                old_source,
                old_importance,
                old_version,
                Option::<&str>::None,
            ],
        )?;

        // Restore the target version
        conn.execute(
            "UPDATE facts SET content = ?1, source = ?2, importance = ?3, version = ?4
             WHERE id = ?5",
            params![content, source, importance, old_version + 1, fact_id],
        )?;

        drop(conn);
        self.get_fact(fact_id)?.ok_or_else(|| anyhow::anyhow!("fact disappeared after rollback"))
    }

    /// Get the version history of a fact.
    pub fn get_fact_versions(&self, fact_id: &str) -> Result<Vec<FactVersion>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, fact_id, content, source, importance, version, created_at, session_id
             FROM fact_versions
             WHERE fact_id = ?1
             ORDER BY version ASC",
        )?;
        let rows = stmt.query_map(params![fact_id], |r| {
            Ok(FactVersion {
                id: r.get(0)?,
                fact_id: r.get(1)?,
                content: r.get(2)?,
                source: r.get(3)?,
                importance: r.get(4)?,
                version: r.get(5)?,
                created_at: r.get(6)?,
                session_id: r.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Full-text search over facts (uses FTS5).
    pub fn search_facts(&self, query: &str, limit: usize) -> Result<Vec<Fact>> {
        let safe = query
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>();
        let fts_query = safe
            .split_whitespace()
            .map(|w| format!("{w}*"))
            .collect::<Vec<_>>()
            .join(" AND ");

        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT f.id, f.entity_id, f.content, f.source, f.importance,
                    f.created_at, f.session_id, f.namespace_id, f.version
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
                namespace_id: r.get(7)?,
                version: r.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Search facts within a specific namespace.
    pub fn search_facts_in_namespace(
        &self,
        query: &str,
        namespace_id: &str,
        limit: usize,
    ) -> Result<Vec<Fact>> {
        let safe = query
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>();
        let fts_query = safe
            .split_whitespace()
            .map(|w| format!("{w}*"))
            .collect::<Vec<_>>()
            .join(" AND ");

        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT f.id, f.entity_id, f.content, f.source, f.importance,
                    f.created_at, f.session_id, f.namespace_id, f.version
             FROM facts f
             JOIN facts_fts ON facts_fts.rowid = f.rowid
             WHERE facts_fts MATCH ?1 AND f.namespace_id = ?2
             ORDER BY f.importance DESC, f.created_at DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![fts_query, namespace_id, limit as i64], |r| {
            Ok(Fact {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                content: r.get(2)?,
                source: r.get(3)?,
                importance: r.get(4)?,
                created_at: r.get(5)?,
                session_id: r.get(6)?,
                namespace_id: r.get(7)?,
                version: r.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get the most important facts (no query = general overview).
    pub fn important_facts(&self, limit: usize) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, entity_id, content, source, importance, created_at,
                    session_id, namespace_id, version
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
                namespace_id: r.get(7)?,
                version: r.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get important facts within a namespace.
    pub fn important_facts_in_namespace(
        &self,
        namespace_id: &str,
        limit: usize,
    ) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, entity_id, content, source, importance, created_at,
                    session_id, namespace_id, version
             FROM facts
             WHERE namespace_id = ?1
             ORDER BY importance DESC, created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![namespace_id, limit as i64], |r| {
            Ok(Fact {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                content: r.get(2)?,
                source: r.get(3)?,
                importance: r.get(4)?,
                created_at: r.get(5)?,
                session_id: r.get(6)?,
                namespace_id: r.get(7)?,
                version: r.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Delete low-importance facts (active forgetting).
    pub fn prune_low_importance_facts(
        &self,
        threshold: f64,
        older_than_days: i64,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = if older_than_days > 0 {
            conn.execute(
                "DELETE FROM facts WHERE importance < ?1
                 AND datetime(created_at) < datetime('now', ?2)",
                params![threshold, format!("-{older_than_days} days")],
            )?
        } else {
            conn.execute(
                "DELETE FROM facts WHERE importance < ?1",
                params![threshold],
            )?
        };
        Ok(count)
    }

    /// Delete a specific fact by ID.
    pub fn delete_fact(&self, fact_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute("DELETE FROM facts WHERE id = ?1", params![fact_id])?;
        Ok(count > 0)
    }

    // ── Glossary / Keywords ─────────────────────────────────────────

    /// Add a glossary term (keyword/tag).
    pub fn add_glossary_term(
        &self,
        term: &str,
        definition: &str,
        category: &str,
        namespace_id: Option<&str>,
    ) -> Result<GlossaryTerm> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO glossary (id, term, definition, category, namespace_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(term, namespace_id) DO UPDATE SET
               definition = CASE WHEN ?3 != '' THEN ?3 ELSE definition END,
               category = CASE WHEN ?4 != 'general' THEN ?4 ELSE category END",
            params![id, term, definition, category, namespace_id],
        )?;
        drop(conn);
        self.get_glossary_term(&id)?
            .ok_or_else(|| anyhow::anyhow!("glossary term not found after insert"))
    }

    /// Get a glossary term by ID.
    pub fn get_glossary_term(&self, id: &str) -> Result<Option<GlossaryTerm>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, term, definition, category, namespace_id, created_at
             FROM glossary WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        Ok(rows.next()?.map(|r| GlossaryTerm {
            id: r.get(0).unwrap(),
            term: r.get(1).unwrap(),
            definition: r.get(2).unwrap(),
            category: r.get(3).unwrap(),
            namespace_id: r.get(4).ok(),
            created_at: r.get(5).unwrap(),
        }))
    }

    /// Search glossary terms by keyword.
    pub fn search_glossary(&self, query: &str, limit: usize) -> Result<Vec<GlossaryTerm>> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{query}%");
        let mut stmt = conn.prepare(
            "SELECT id, term, definition, category, namespace_id, created_at
             FROM glossary
             WHERE term LIKE ?1 OR definition LIKE ?1
             ORDER BY term ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |r| {
            Ok(GlossaryTerm {
                id: r.get(0)?,
                term: r.get(1)?,
                definition: r.get(2)?,
                category: r.get(3)?,
                namespace_id: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Link a fact to a glossary term.
    pub fn link_fact_glossary(&self, fact_id: &str, glossary_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO fact_glossary (fact_id, glossary_id) VALUES (?1, ?2)",
            params![fact_id, glossary_id],
        )?;
        // 0 rows affected means already existed — still return true
        Ok(true)
    }

    /// Unlink a fact from a glossary term.
    pub fn unlink_fact_glossary(&self, fact_id: &str, glossary_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "DELETE FROM fact_glossary WHERE fact_id = ?1 AND glossary_id = ?2",
            params![fact_id, glossary_id],
        )?;
        Ok(count > 0)
    }

    /// Get all glossary terms linked to a fact.
    pub fn get_fact_glossary_terms(&self, fact_id: &str) -> Result<Vec<GlossaryTerm>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT g.id, g.term, g.definition, g.category, g.namespace_id, g.created_at
             FROM glossary g
             JOIN fact_glossary fg ON fg.glossary_id = g.id
             WHERE fg.fact_id = ?1
             ORDER BY g.term",
        )?;
        let rows = stmt.query_map(params![fact_id], |r| {
            Ok(GlossaryTerm {
                id: r.get(0)?,
                term: r.get(1)?,
                definition: r.get(2)?,
                category: r.get(3)?,
                namespace_id: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Link an entity to a glossary term.
    pub fn link_entity_glossary(&self, entity_id: &str, glossary_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO entity_glossary (entity_id, glossary_id) VALUES (?1, ?2)",
            params![entity_id, glossary_id],
        )?;
        Ok(true)
    }

    /// Search facts by glossary term (find all facts tagged with a term).
    pub fn search_facts_by_glossary(
        &self,
        glossary_id: &str,
        limit: usize,
    ) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT f.id, f.entity_id, f.content, f.source, f.importance,
                    f.created_at, f.session_id, f.namespace_id, f.version
             FROM facts f
             JOIN fact_glossary fg ON fg.fact_id = f.id
             WHERE fg.glossary_id = ?1
             ORDER BY f.importance DESC, f.created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![glossary_id, limit as i64], |r| {
            Ok(Fact {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                content: r.get(2)?,
                source: r.get(3)?,
                importance: r.get(4)?,
                created_at: r.get(5)?,
                session_id: r.get(6)?,
                namespace_id: r.get(7)?,
                version: r.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ── Statistics ──────────────────────────────────────────────────

    /// Get aggregate statistics about the memory store.
    pub fn get_memory_stats(&self) -> Result<MemoryStats> {
        let conn = self.conn.lock().unwrap();

        let total_entities: i64 =
            conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))?;
        let total_relations: i64 =
            conn.query_row("SELECT COUNT(*) FROM relations", [], |r| r.get(0))?;
        let total_facts: i64 =
            conn.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))?;
        let total_glossary_terms: i64 =
            conn.query_row("SELECT COUNT(*) FROM glossary", [], |r| r.get(0))?;
        let total_namespaces: i64 =
            conn.query_row("SELECT COUNT(*) FROM namespaces", [], |r| r.get(0))?;
        let avg_importance: f64 = conn
            .query_row("SELECT COALESCE(AVG(importance), 0.0) FROM facts", [], |r| {
                r.get(0)
            })?;
        let oldest_fact: Option<String> = conn
            .query_row(
                "SELECT MIN(created_at) FROM facts",
                [],
                |r| r.get(0),
            )
            .ok();
        let newest_fact: Option<String> = conn
            .query_row(
                "SELECT MAX(created_at) FROM facts",
                [],
                |r| r.get(0),
            )
            .ok();

        Ok(MemoryStats {
            total_entities,
            total_relations,
            total_facts,
            total_glossary_terms,
            total_namespaces,
            avg_importance,
            oldest_fact,
            newest_fact,
        })
    }
}

// ── ID helpers ─────────────────────────────────────────────────────────

/// Deterministic entity ID based on kind + name (same kind+name → same ID).
fn entity_id(kind: &str, name: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(format!("{kind}\0{name}").as_bytes());
    hash.iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

/// Deterministic namespace ID based on name.
fn namespace_id(name: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(format!("namespace\0{name}").as_bytes());
    hash.iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

// ── Tests ──────────────────────────────────────────────────────────────

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
        let pr = store
            .upsert_entity("pr", "PR #2933", "fix: tool error messages")
            .unwrap();

        store
            .upsert_relation(&file.id, &pr.id, "part_of", 1.0, None)
            .unwrap();

        let connected = store.graph_walk(&file.id, "part_of", 1).unwrap();
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].id, pr.id);
    }

    #[test]
    fn test_fact_insert_and_search() {
        let store = test_store();
        let e = store.upsert_entity("file", "dispatch.rs", "").unwrap();

        store
            .insert_fact(
                Some(&e.id),
                "format_tool_error had misleading generic suffix. Fixed by removing it.",
                "code review",
                0.9,
                None,
            )
            .unwrap();

        let results = store.search_facts("format tool error", 10).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].content.contains("format_tool_error"));
    }

    #[test]
    fn test_prune_low_importance() {
        let store = test_store();
        store
            .insert_fact(None, "transient debug note", "debug", 0.1, None)
            .unwrap();
        store
            .insert_fact(
                None,
                "important architecture decision",
                "design",
                0.9,
                None,
            )
            .unwrap();

        let pruned = store.prune_low_importance_facts(0.3, 0).unwrap();
        assert_eq!(pruned, 1);

        let remaining = store.important_facts(10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].content.contains("architecture"));
    }

    // ── New v2 tests ────────────────────────────────────────────────

    #[test]
    fn test_namespace_creation_and_isolation() {
        let store = test_store();
        let ns = store.upsert_namespace("workspace:/project/alpha", "Alpha project").unwrap();
        assert_eq!(ns.name, "workspace:/project/alpha");

        // Facts in namespace should be isolated
        let fact1 = store
            .insert_fact_in_namespace(
                None,
                "API rate limit is 100 req/min",
                "config",
                0.7,
                None,
                Some(&ns.id),
            )
            .unwrap();
        assert_eq!(fact1.namespace_id.as_deref(), Some(&ns.id));
        assert_eq!(fact1.version, 1);

        // Search in namespace should find it
        let found = store.search_facts_in_namespace("API rate", &ns.id, 10).unwrap();
        assert_eq!(found.len(), 1);

        // Search globally should also find it (global search doesn't filter)
        let global = store.search_facts("API rate", 10).unwrap();
        assert_eq!(global.len(), 1);
    }

    #[test]
    fn test_glossary_system() {
        let store = test_store();
        let term = store
            .add_glossary_term("rate-limit", "API request cap per time window", "tech", None)
            .unwrap();
        assert_eq!(term.term, "rate-limit");

        // Link a fact to the glossary term
        let fact = store
            .insert_fact(None, "API rate limit is 100 req/min", "config", 0.7, None)
            .unwrap();
        store.link_fact_glossary(&fact.id, &term.id).unwrap();

        // Find fact by glossary term
        let tagged = store.search_facts_by_glossary(&term.id, 10).unwrap();
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].id, fact.id);

        // Get glossary terms for fact
        let terms = store.get_fact_glossary_terms(&fact.id).unwrap();
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].term, "rate-limit");
    }

    #[test]
    fn test_fact_versioning_and_rollback() {
        let store = test_store();
        let ns = store.upsert_namespace("workspace:/test", "test").unwrap();

        // Insert version 1
        let fact = store
            .insert_fact_in_namespace(
                None,
                "original content",
                "test",
                0.5,
                None,
                Some(&ns.id),
            )
            .unwrap();
        assert_eq!(fact.version, 1);

        // Update to version 2
        let updated = store.update_fact(&fact.id, "updated content", 0.8, None).unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.content, "updated content");

        // Check version history
        let versions = store.get_fact_versions(&fact.id).unwrap();
        assert_eq!(versions.len(), 1); // one old version saved
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[0].content, "original content");

        // Rollback to version 1
        let rolled_back = store.rollback_fact(&fact.id, 1).unwrap();
        assert_eq!(rolled_back.version, 3); // version bumped
        assert_eq!(rolled_back.content, "original content");
    }

    #[test]
    fn test_memory_stats() {
        let store = test_store();
        let stats = store.get_memory_stats().unwrap();
        assert_eq!(stats.total_entities, 0);
        assert_eq!(stats.total_facts, 0);

        store
            .insert_fact(None, "test fact", "test", 0.5, None)
            .unwrap();
        store
            .upsert_entity("test", "test-file", "test entity")
            .unwrap();
        store
            .add_glossary_term("test-tag", "a test tag", "general", None)
            .unwrap();

        let stats = store.get_memory_stats().unwrap();
        assert_eq!(stats.total_entities, 1);
        assert_eq!(stats.total_facts, 1);
        assert_eq!(stats.total_glossary_terms, 1);
    }
}
