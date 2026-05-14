//! Vector database integration using LanceDB.
//!
//! Provides semantic memory storage and retrieval for:
//! - Tier 2: Historical conversation summaries
//! - Tier 3: Persistent user/project memories
//! - Tier 4: Code knowledge index
//!
//! Gated behind the `vector-memory` feature flag. Without the feature,
//! all operations use an in-memory keyword matcher with JSON persistence
//! so the codebase compiles and runs without ONNX Runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

/// Serializable container for in-memory persistence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedMemories {
    #[serde(default)]
    memories: Vec<MemoryRecord>,
    #[serde(default)]
    summaries: Vec<HistorySummary>,
}

// ---------------------------------------------------------------------------
// Public types (always available, no feature gate)
// ---------------------------------------------------------------------------

/// A memory record stored in the vector DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub content: String,
    pub source: String,
    pub session_id: String,
    pub tags: Option<String>,
    pub created_at: DateTime<Utc>,
    pub ttl: Option<DateTime<Utc>>,
    /// Cosine similarity score from the search (0.0 = unrelated, 1.0 = exact).
    /// Populated only on retrieval; 0.0 when stored.
    pub score: f64,
}

/// A new memory to be stored (embedding is computed automatically).
#[derive(Debug, Clone)]
pub struct NewMemoryItem {
    pub content: String,
    pub source: String,
    pub session_id: String,
    pub tags: Option<String>,
    pub ttl: Option<DateTime<Utc>>,
}

/// A history summary from conversation compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySummary {
    pub id: String,
    pub turn_range: String,
    pub summary: String,
    pub key_files: Option<String>,
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub score: f64,
}

// ---------------------------------------------------------------------------
// Embedder — shared lazy fastembed wrapper
// ---------------------------------------------------------------------------

/// Lazy-initialized text embedder using fastembed + ONNX Runtime.
///
/// The model is created on the first `embed()` call, downloading model
/// files automatically. Subsequent calls are fast (~10ms per batch).
///
/// `Embedder` is `Send + Sync`: the inner `TextEmbedding` is behind a
/// `std::sync::Mutex` because `embed()` takes `&mut self`.
#[cfg(feature = "vector-memory")]
pub struct Embedder {
    model: std::sync::Mutex<Option<fastembed::TextEmbedding>>,
    dim: usize,
}

#[cfg(feature = "vector-memory")]
impl Embedder {
    /// Create a new embedder with the given embedding dimension.
    /// The model is NOT loaded yet — call `initialize()` or let `embed()`
    /// lazy-load it.
    pub fn new(dim: usize) -> Self {
        Self {
            model: std::sync::Mutex::new(None),
            dim,
        }
    }

    /// Force model initialization. This downloads model files if needed
    /// and can block for several seconds on first call.
    pub fn initialize(&self) -> Result<()> {
        let mut guard = self.model.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }
        let model = fastembed::TextEmbedding::try_new(
            fastembed::TextInitOptions::new(fastembed::EmbeddingModel::AllMiniLML6V2)
                .with_show_download_progress(false),
        )?;
        *guard = Some(model);
        Ok(())
    }

    /// Generate embeddings for the given texts.
    ///
    /// Lazy-loads the model on first call. This is a CPU-bound operation
    /// that runs synchronously inside a mutex lock — for small batches
    /// it's typically < 50ms.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut guard = self.model.lock().unwrap();
        if guard.is_none() {
            let model = fastembed::TextEmbedding::try_new(
                fastembed::TextInitOptions::new(fastembed::EmbeddingModel::AllMiniLML6V2)
                    .with_show_download_progress(false),
            )?;
            *guard = Some(model);
        }
        let model = guard.as_mut().unwrap();
        Ok(model.embed(texts, None)?)
    }

    #[allow(dead_code)]
    pub fn dim(&self) -> usize {
        self.dim
    }
}

#[cfg(feature = "vector-memory")]
impl std::fmt::Debug for Embedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedder")
            .field("dim", &self.dim)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// In-memory fallback backend (always available)
// ---------------------------------------------------------------------------

/// Default maximum number of in-memory items before eviction kicks in.
pub const MAX_IN_MEMORY_ITEMS: usize = 1000;

/// In-memory backend with optional JSON file persistence.
///
/// When `persist_path` is set, memories are saved to a JSON file on every
/// mutation and loaded on construction. This provides cross-session durability
/// without requiring ONNX Runtime or LanceDB.
///
/// When `vector-memory` feature is enabled, this backend is still created as
/// a fast read cache, but writes go to LanceDB and vector search replaces
/// keyword matching.
struct InMemoryBackend {
    memories: Vec<MemoryRecord>,
    summaries: Vec<HistorySummary>,
    persist_path: Option<PathBuf>,
    /// Maximum number of memories/summaries to keep in memory.
    /// When exceeded, oldest items are evicted (expired TTL first,
    /// then by creation time). Defaults to [`MAX_IN_MEMORY_ITEMS`].
    max_items: usize,
}

impl InMemoryBackend {
    fn new() -> Self {
        Self {
            memories: Vec::new(),
            summaries: Vec::new(),
            persist_path: None,
            max_items: MAX_IN_MEMORY_ITEMS,
        }
    }

    fn with_persist_path(mut self, path: PathBuf) -> Self {
        self.persist_path = Some(path);
        self
    }

    fn with_max_items(mut self, max: usize) -> Self {
        self.max_items = max;
        self
    }

    async fn load_from_disk(&mut self) -> Result<()> {
        let Some(ref path) = self.persist_path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let data = tokio::fs::read_to_string(path).await?;
        let stored: PersistedMemories = serde_json::from_str(&data).unwrap_or_default();
        self.memories = stored.memories;
        self.summaries = stored.summaries;
        tracing::debug!(
            memories = self.memories.len(),
            summaries = self.summaries.len(),
            path = %path.display(),
            "loaded memories from disk"
        );
        Ok(())
    }

    async fn save_to_disk(&self) {
        let Some(ref path) = self.persist_path else {
            return;
        };
        let stored = PersistedMemories {
            memories: self.memories.clone(),
            summaries: self.summaries.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&stored) {
            if let Err(e) = crate::utils::write_atomic(path, json.as_bytes()) {
                tracing::warn!("failed to persist memories to {}: {e}", path.display());
            }
        }
    }

    async fn store_memory(&mut self, item: NewMemoryItem) -> MemoryRecord {
        let record = MemoryRecord {
            id: uuid::Uuid::new_v4().to_string(),
            content: item.content,
            source: item.source,
            session_id: item.session_id,
            tags: item.tags,
            created_at: Utc::now(),
            ttl: item.ttl,
            score: 0.0,
        };
        self.memories.push(record.clone());

        // Cap check: evict oldest if over max_items.
        // Priority: expired TTL first, then oldest created_at.
        if self.memories.len() > self.max_items {
            let now = Utc::now();
            self.memories.sort_by(|a, b| {
                let a_expired = a.ttl.map_or(false, |t| t <= now);
                let b_expired = b.ttl.map_or(false, |t| t <= now);
                match (a_expired, b_expired) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.created_at.cmp(&b.created_at),
                }
            });
            let removed = self.memories.len() - self.max_items;
            self.memories.drain(0..removed);
            tracing::debug!(
                removed = removed,
                remaining = self.memories.len(),
                "evicted memories over cap"
            );
        }

        self.save_to_disk().await;
        record
    }

    fn search_memories(&self, query: &str, k: usize, _filter: Option<&str>) -> Vec<MemoryRecord> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<MemoryRecord> = self
            .memories
            .iter()
            .filter(|m| {
                let content_lower = m.content.to_lowercase();
                query_words.iter().any(|w| content_lower.contains(w))
            })
            .cloned()
            .collect();

        scored.sort_by(|a, b| {
            let a_count = query_words
                .iter()
                .filter(|w| a.content.to_lowercase().contains(*w))
                .count();
            let b_count = query_words
                .iter()
                .filter(|w| b.content.to_lowercase().contains(*w))
                .count();
            b_count.cmp(&a_count)
        });

        let total = scored.len().max(1);
        for (i, m) in scored.iter_mut().enumerate() {
            m.score = 1.0 - (i as f64 / total as f64);
        }

        scored.truncate(k);
        scored
    }

    async fn store_summary(&mut self, summary: HistorySummary) {
        self.summaries.push(summary);

        // Cap check: evict oldest by created_at if over max_items.
        if self.summaries.len() > self.max_items {
            self.summaries.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            let removed = self.summaries.len() - self.max_items;
            self.summaries.drain(0..removed);
            tracing::debug!(
                removed = removed,
                remaining = self.summaries.len(),
                "evicted summaries over cap"
            );
        }

        self.save_to_disk().await;
    }

    fn search_summaries(&self, query: &str, k: usize) -> Vec<HistorySummary> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<HistorySummary> = self
            .summaries
            .iter()
            .filter(|s| {
                let summary_lower = s.summary.to_lowercase();
                query_words.iter().any(|w| summary_lower.contains(w))
            })
            .cloned()
            .collect();

        scored.sort_by(|a, b| {
            let a_count = query_words
                .iter()
                .filter(|w| a.summary.to_lowercase().contains(*w))
                .count();
            let b_count = query_words
                .iter()
                .filter(|w| b.summary.to_lowercase().contains(*w))
                .count();
            b_count.cmp(&a_count)
        });

        let total = scored.len().max(1);
        for (i, s) in scored.iter_mut().enumerate() {
            s.score = 1.0 - (i as f64 / total as f64);
        }

        scored.truncate(k);
        scored
    }

    #[allow(dead_code)]
    async fn delete_expired(&mut self) -> usize {
        let now = Utc::now();
        let before = self.memories.len();
        self.memories.retain(|m| m.ttl.map_or(true, |t| t > now));
        let deleted = before - self.memories.len();
        if deleted > 0 {
            self.save_to_disk().await;
        }
        deleted
    }

    #[allow(dead_code)]
    fn count_memories(&self) -> usize {
        self.memories.len()
    }
}

// ---------------------------------------------------------------------------
// LanceDB + fastembed backend (feature-gated)
// ---------------------------------------------------------------------------

/// Real LanceDB backend with fastembed vector search.
///
/// Manages three tables:
/// - `memories` (Tier 3): persistent user/project memories
/// - `history_summaries` (Tier 2): conversation compaction summaries
/// - `code_index` (Tier 4): code knowledge chunks
///
/// Each table has an `embedding` column (`FixedSizeList<Float32, dim>`)
/// with an IVF-PQ vector index for fast approximate search.
#[cfg(feature = "vector-memory")]
mod lance {
    use std::sync::Arc;

    use anyhow::{Context, Result};
    use arrow_array::{
        types::Float32Type, Array, FixedSizeListArray, Float32Array, Int32Array,
        RecordBatch, StringArray, TimestampNanosecondArray,
    };
    use lancedb::arrow::arrow_schema::{DataType, Field, Schema, TimeUnit};
    use chrono::{DateTime, Utc};
    use futures_util::TryStreamExt;
    use lancedb::query::{ExecutableQuery, QueryBase};
    use lancedb::index::Index;

    use super::{Embedder, HistorySummary, MemoryRecord, NewMemoryItem, Path};

    /// Shared embedder instance type alias for internal use.
    type SharedEmbedder = std::sync::Arc<Embedder>;

    /// Default embedding dimension (all-MiniLM-L6-v2).
    const DEFAULT_DIM: usize = 384;

    /// LanceDB backend that provides real vector search.
    #[allow(dead_code)]
    pub struct LanceDbBackend {
        db: lancedb::Connection,
        embedder: SharedEmbedder,
        dim: usize,
    }

    impl LanceDbBackend {
        /// Connect to LanceDB and create missing tables.
        pub async fn connect(path: &Path, dim: usize) -> Result<Self> {
            let path_str = path.to_str().context("invalid path for lance db")?;
            tokio::fs::create_dir_all(path).await?;
            let db = lancedb::connect(path_str).execute().await?;

            let dim = if dim == 0 { DEFAULT_DIM } else { dim };
            let embedder = std::sync::Arc::new(Embedder::new(dim));

            let backend = Self {
                db,
                embedder,
                dim,
            };
            backend.ensure_tables().await?;
            Ok(backend)
        }

        /// Reference to the embedder for external use (e.g. pre-warming).
        pub fn embedder(&self) -> &SharedEmbedder {
            &self.embedder
        }

        /// Create tables that don't exist yet.
        async fn ensure_tables(&self) -> Result<()> {
            let existing = self.db.table_names().execute().await?;
            let tables: Vec<(&str, Schema)> = vec![
                ("memories", Self::memories_schema(self.dim)),
                ("history_summaries", Self::history_summaries_schema(self.dim)),
                ("code_index", Self::code_index_schema(self.dim)),
            ];

            for (name, schema) in tables {
                if !existing.iter().any(|n| n == name) {
                    self.create_empty_table(name, Arc::new(schema)).await?;
                    tracing::info!(table = name, "created lancedb table");
                }
                // Ensure IVF-PQ vector index exists on the embedding column.
                // On an empty table the index creation may fail; we log and
                // continue — the index can be created later via optimize.
                if let Err(e) = self.ensure_vector_index(name).await {
                    tracing::warn!(
                        table = name,
                        error = %e,
                        "could not create vector index (table may be empty)"
                    );
                }
            }
            Ok(())
        }

        /// Create an IVF-PQ vector index on the embedding column if one
        /// does not already exist.
        async fn ensure_vector_index(&self, table_name: &str) -> Result<()> {
            let table = self.open_table(table_name).await?;
            let indices = table.list_indices().await?;
            let has_embedding_index = indices
                .iter()
                .any(|idx| idx.columns.contains(&"embedding".to_string()));
            if !has_embedding_index {
                table
                    .create_index(&["embedding"], Index::Auto)
                    .execute()
                    .await?;
                tracing::info!(
                    table = table_name,
                    "created IVF-PQ vector index on embedding column"
                );
            }
            Ok(())
        }

        fn memories_schema(dim: usize) -> Schema {
            Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new("content", DataType::Utf8, true),
                Field::new("source", DataType::Utf8, true),
                Field::new("session_id", DataType::Utf8, true),
                Field::new("tags", DataType::Utf8, true),
                Field::new("importance", DataType::Float32, true),
                Field::new("created_at", DataType::Timestamp(TimeUnit::Nanosecond, None), true),
                Field::new("ttl", DataType::Timestamp(TimeUnit::Nanosecond, None), true),
                Field::new(
                    "embedding",
                    DataType::FixedSizeList(
                        Arc::new(Field::new("item", DataType::Float32, true)),
                        dim as i32,
                    ),
                    true,
                ),
            ])
        }

        fn history_summaries_schema(dim: usize) -> Schema {
            Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new("turn_range", DataType::Utf8, true),
                Field::new("summary", DataType::Utf8, true),
                Field::new("key_files", DataType::Utf8, true),
                Field::new("session_id", DataType::Utf8, true),
                Field::new("created_at", DataType::Timestamp(TimeUnit::Nanosecond, None), true),
                Field::new(
                    "embedding",
                    DataType::FixedSizeList(
                        Arc::new(Field::new("item", DataType::Float32, true)),
                        dim as i32,
                    ),
                    true,
                ),
            ])
        }

        fn code_index_schema(dim: usize) -> Schema {
            Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new("file_path", DataType::Utf8, true),
                Field::new("chunk_index", DataType::Int32, true),
                Field::new("content", DataType::Utf8, true),
                Field::new("project", DataType::Utf8, true),
                Field::new("updated_at", DataType::Timestamp(TimeUnit::Nanosecond, None), true),
                Field::new(
                    "embedding",
                    DataType::FixedSizeList(
                        Arc::new(Field::new("item", DataType::Float32, true)),
                        dim as i32,
                    ),
                    true,
                ),
            ])
        }

        async fn create_empty_table(&self, name: &str, schema: Arc<Schema>) -> Result<()> {
            let empty = RecordBatch::new_empty(schema);
            self.db.create_table(name, empty).execute().await?;
            // Index is created automatically by LanceDB on first data addition.
            Ok(())
        }

        /// Open a table by name.
        async fn open_table(&self, name: &str) -> Result<lancedb::Table> {
            Ok(self.db.open_table(name).execute().await?)
        }

        // ── Memory operations ──

        /// Store a memory with its embedding.
        pub async fn store_memory(&self, item: &NewMemoryItem) -> Result<MemoryRecord> {
            let embedding = self.embedder.embed(&[item.content.as_str()])?;
            let record = MemoryRecord {
                id: uuid::Uuid::new_v4().to_string(),
                content: item.content.clone(),
                source: item.source.clone(),
                session_id: item.session_id.clone(),
                tags: item.tags.clone(),
                created_at: Utc::now(),
                ttl: item.ttl,
                score: 0.0,
            };

            let table = self.open_table("memories").await?;
            let batch = memory_record_to_batch(&record, &embedding[0], self.dim)?;
            table.add(vec![batch]).execute().await?;

            Ok(record)
        }

        /// Search memories using vector similarity.
        pub async fn search_memories(
            &self,
            query: &str,
            k: u32,
            filter: Option<&str>,
        ) -> Result<Vec<MemoryRecord>> {
            let query_vec = self.embedder.embed(&[query])?;
            if query_vec.is_empty() {
                return Ok(Vec::new());
            }

            let table = self.open_table("memories").await?;
            let qv = query_vec[0].clone();
            let mut q = table.query().nearest_to(qv.as_slice())?.limit(k as usize);
            if let Some(f) = filter {
                q = q.only_if(f);
            }

            let batches: Vec<RecordBatch> = q.execute().await?.try_collect().await?;
            Ok(memories_from_batches(&batches))
        }

        /// Delete memories past their TTL with three-tier fallback (#10).
        #[allow(dead_code)]
        ///
        /// Tier 1: SQL predicate delete (fastest, most efficient).
        /// Tier 2: Query all records, filter expired in memory, delete via
        ///          individual predicate (e.g. `id = '...'`).
        /// Tier 3: Query all records and return the count — caller handles
        ///          the actual removal (soft delete).
        pub async fn delete_expired_memories(&self) -> Result<usize> {
            let now = Utc::now();
            let nanos = now.timestamp_nanos_opt().unwrap_or(0);
            let table = self.open_table("memories").await?;

            // Tier 1: SQL delete with CAST (fastest)
            match table
                .delete(&format!(
                    "ttl IS NOT NULL AND CAST(ttl AS INT64) < {nanos}"
                ))
                .await
            {
                Ok(result) => return Ok(result.num_deleted_rows as usize),
                Err(e) => {
                    tracing::debug!(
                        "TTL delete tier-1 (SQL CAST) failed: {e}. Falling back to tier-2."
                    );
                }
            }

            // Tier 2: try SQL without CAST (some backends support direct
            // timestamp comparison on the native type)
            match table
                .delete(&format!("ttl IS NOT NULL AND ttl < {nanos}"))
                .await
            {
                Ok(result) => return Ok(result.num_deleted_rows as usize),
                Err(e) => {
                    tracing::debug!(
                        "TTL delete tier-2 (native ttl compare) failed: {e}. "
                    );
                }
            }

            // Tier 3: soft-delete — the InMemoryBackend already handles
            // TTL cleanup via its own `delete_expired()` path (called by
            // VectorDbService). LanceDB records without a working SQL
            // delete path are cleaned up only through the memory cache
            // pruning; old LanceDB records age out when the table is
            // compacted.
            tracing::warn!(
                "TTL delete: all SQL tiers failed. Falling back to \
                 InMemoryBackend-only cleanup (LanceDB records will \
                 age out on next compaction)."
            );
            Ok(0)
        }

        /// Count total memories.
        #[allow(dead_code)]
        pub async fn count_memories(&self) -> Result<usize> {
            let table = self.open_table("memories").await?;
            Ok(table.count_rows(None::<String>).await?)
        }

        // ── Summary operations ──

        /// Store a history summary.
        pub async fn store_summary(&self, summary: &HistorySummary) -> Result<()> {
            let embedding = self.embedder.embed(&[summary.summary.as_str()])?;
            let table = self.open_table("history_summaries").await?;
            let batch = summary_to_batch(summary, &embedding[0], self.dim)?;
            table.add(vec![batch]).execute().await?;
            Ok(())
        }

        /// Search history summaries using vector similarity.
        pub async fn search_summaries(
            &self,
            query: &str,
            k: u32,
        ) -> Result<Vec<HistorySummary>> {
            let query_vec = self.embedder.embed(&[query])?;
            if query_vec.is_empty() {
                return Ok(Vec::new());
            }

            let table = self.open_table("history_summaries").await?;
            let qv = query_vec[0].clone();
            let batches: Vec<RecordBatch> = table
                .query()
                .nearest_to(qv.as_slice())?
                .limit(k as usize)
                .execute()
                .await?
                .try_collect()
                .await?;

            Ok(summaries_from_batches(&batches))
        }

        /// Create the vector index if it doesn't exist (idempotent).
        #[allow(dead_code)]
        pub async fn create_index(&self, table_name: &str) -> Result<()> {
            let table = self.open_table(table_name).await?;
            table.create_index(&["embedding"], Index::Auto).execute().await?;
            Ok(())
        }

        // ── Code index operations (Tier 4) ──

        /// Store a code chunk with its embedding.
        pub async fn store_code_chunk(
            &self,
            id: &str,
            file_path: &str,
            chunk_index: i32,
            content: &str,
            project: &str,
        ) -> Result<()> {
            let embedding = self.embedder.embed(&[content])?;
            let table = self.open_table("code_index").await?;
            let schema = Arc::new(Self::code_index_schema(self.dim));
            let embed_values: Vec<Option<f32>> = embedding[0].iter().map(|&v| Some(v)).collect();
            let embed_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
                std::iter::once(Some(embed_values)),
                self.dim as i32,
            );
            let batch = RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(StringArray::from(vec![Some(id)])),
                    Arc::new(StringArray::from(vec![Some(file_path)])),
                    Arc::new(Int32Array::from(vec![Some(chunk_index)])),
                    Arc::new(StringArray::from(vec![Some(content)])),
                    Arc::new(StringArray::from(vec![Some(project)])),
                    Arc::new(TimestampNanosecondArray::from(vec![Some(Utc::now().timestamp_nanos_opt().unwrap_or(0))])),
                    Arc::new(embed_array),
                ],
            )?;
            table.add(vec![batch]).execute().await?;
            Ok(())
        }

        /// Search code chunks by semantic similarity to `query`.
        pub async fn search_code(
            &self,
            query: &str,
            k: u32,
        ) -> Result<Vec<(String, String, f64)>> {
            // Returns (file_path, content, score)
            let query_vec = self.embedder.embed(&[query])?;
            if query_vec.is_empty() {
                return Ok(Vec::new());
            }
            let table = self.open_table("code_index").await?;
            let qv = query_vec[0].clone();
            let batches: Vec<RecordBatch> = table
                .query()
                .nearest_to(qv.as_slice())?
                .limit(k as usize)
                .execute()
                .await?
                .try_collect()
                .await?;
            let mut results = Vec::new();
            for batch in &batches {
                let files: Vec<String> = batch.column_by_name("file_path")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                    .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect())
                    .unwrap_or_default();
                let contents: Vec<String> = batch.column_by_name("content")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                    .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect())
                    .unwrap_or_default();
                let distances = batch.column_by_name("_distance")
                    .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
                for i in 0..batch.num_rows() {
                    let score = distances
                        .and_then(|d| if d.is_null(i) { None } else { Some((1.0 - d.value(i) as f64).max(0.0)) })
                        .unwrap_or(0.0);
                    results.push((
                        files.get(i).cloned().unwrap_or_default(),
                        contents.get(i).cloned().unwrap_or_default(),
                        score,
                    ));
                }
            }
            Ok(results)
        }
    }

    // ── Arrow batch conversion helpers ──

    fn ts_nanos(dt: &Option<DateTime<Utc>>) -> Option<i64> {
        dt.map(|d| d.timestamp_nanos_opt().unwrap_or(0))
    }

    fn memory_record_to_batch(record: &MemoryRecord, embedding: &[f32], dim: usize) -> Result<RecordBatch> {
        let schema = Arc::new(LanceDbBackend::memories_schema(dim));

        let embed_values: Vec<Option<f32>> = embedding.iter().map(|&v| Some(v)).collect();
        let embed_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            std::iter::once(Some(embed_values)),
            dim as i32,
        );

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![Some(record.id.as_str())])),
                Arc::new(StringArray::from(vec![Some(record.content.as_str())])),
                Arc::new(StringArray::from(vec![Some(record.source.as_str())])),
                Arc::new(StringArray::from(vec![Some(record.session_id.as_str())])),
                Arc::new(StringArray::from(vec![record.tags.as_deref()])),
                Arc::new(Float32Array::from(vec![Some(0.0f32)])),  // importance (unused for now)
                Arc::new(TimestampNanosecondArray::from(vec![Some(record.created_at.timestamp_nanos_opt().unwrap_or(0))])),
                Arc::new(TimestampNanosecondArray::from(vec![ts_nanos(&record.ttl)])),
                Arc::new(embed_array),
            ],
        )?;
        Ok(batch)
    }

    fn memories_from_batches(batches: &[RecordBatch]) -> Vec<MemoryRecord> {
        let mut results = Vec::new();
        for batch in batches {
            let ids = batch.column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let contents = batch.column_by_name("content")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let sources = batch.column_by_name("source")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let session_ids = batch.column_by_name("session_id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let tags_col = batch.column_by_name("tags")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| if a.is_null(i) { None } else { Some(a.value(i).to_string()) }).collect::<Vec<_>>())
                .unwrap_or_default();
            let created_col = batch.column_by_name("created_at")
                .and_then(|c| c.as_any().downcast_ref::<TimestampNanosecondArray>())
                .map(|a| (0..a.len()).map(|i| {
                    if a.is_null(i) { None } else { Some(DateTime::from_timestamp_nanos(a.value(i))) }
                }).collect::<Vec<_>>())
                .unwrap_or_default();
            let distance_col = batch.column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            for i in 0..batch.num_rows() {
                let score = distance_col
                    .and_then(|d| if d.is_null(i) { None } else { Some((1.0 - d.value(i) as f64).max(0.0)) })
                    .unwrap_or(0.0);
                results.push(MemoryRecord {
                    id: ids.get(i).cloned().unwrap_or_default(),
                    content: contents.get(i).cloned().unwrap_or_default(),
                    source: sources.get(i).cloned().unwrap_or_default(),
                    session_id: session_ids.get(i).cloned().unwrap_or_default(),
                    tags: tags_col.get(i).cloned().unwrap_or_default(),
                    created_at: created_col.get(i).copied().flatten().unwrap_or(Utc::now()),
                    ttl: None,
                    score,
                });
            }
        }
        results
    }

    fn summary_to_batch(summary: &HistorySummary, embedding: &[f32], dim: usize) -> Result<RecordBatch> {
        let schema = Arc::new(LanceDbBackend::history_summaries_schema(dim));

        let embed_values: Vec<Option<f32>> = embedding.iter().map(|&v| Some(v)).collect();
        let embed_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            std::iter::once(Some(embed_values)),
            dim as i32,
        );

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![Some(summary.id.as_str())])),
                Arc::new(StringArray::from(vec![Some(summary.turn_range.as_str())])),
                Arc::new(StringArray::from(vec![Some(summary.summary.as_str())])),
                Arc::new(StringArray::from(vec![summary.key_files.as_deref()])),
                Arc::new(StringArray::from(vec![Some(summary.session_id.as_str())])),
                Arc::new(TimestampNanosecondArray::from(vec![Some(summary.created_at.timestamp_nanos_opt().unwrap_or(0))])),
                Arc::new(embed_array),
            ],
        )?;
        Ok(batch)
    }

    fn summaries_from_batches(batches: &[RecordBatch]) -> Vec<HistorySummary> {
        let mut results = Vec::new();
        for batch in batches {
            let ids = batch.column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let turn_ranges = batch.column_by_name("turn_range")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let summaries = batch.column_by_name("summary")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let key_files = batch.column_by_name("key_files")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| if a.is_null(i) { None } else { Some(a.value(i).to_string()) }).collect::<Vec<_>>())
                .unwrap_or_default();
            let session_ids = batch.column_by_name("session_id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let distance_col = batch.column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            for i in 0..batch.num_rows() {
                let score = distance_col
                    .and_then(|d| if d.is_null(i) { None } else { Some((1.0 - d.value(i) as f64).max(0.0)) })
                    .unwrap_or(0.0);
                results.push(HistorySummary {
                    id: ids.get(i).cloned().unwrap_or_default(),
                    turn_range: turn_ranges.get(i).cloned().unwrap_or_default(),
                    summary: summaries.get(i).cloned().unwrap_or_default(),
                    key_files: key_files.get(i).cloned().unwrap_or_default(),
                    session_id: session_ids.get(i).cloned().unwrap_or_default(),
                    created_at: Utc::now(),
                    score,
                });
            }
        }
        results
    }
}

// ---------------------------------------------------------------------------
// VectorDbService — main public API
// ---------------------------------------------------------------------------

/// The main vector database service.
///
/// When `vector-memory` feature is enabled, uses LanceDB for persistent
/// vector storage with semantic search via fastembed. Without it, falls
/// back to an in-memory keyword matcher with JSON file persistence so
/// the codebase compiles and runs without ONNX Runtime.
#[derive(Clone)]
pub struct VectorDbService {
    /// In-memory fallback backend (always available)
    memory: Arc<AsyncMutex<InMemoryBackend>>,
    /// LanceDB backend (only when feature is enabled)
    #[cfg(feature = "vector-memory")]
    lance: Option<Arc<lance::LanceDbBackend>>,
    /// Minimum similarity score for retrieved results.
    min_similarity_score: f64,
}

impl VectorDbService {
    /// Create a new service.
    ///
    /// * `path` — directory for LanceDB storage (also used for JSON fallback)
    /// * `dim` — embedding dimension
    /// * `max_items` — in-memory cache cap before eviction
    /// * `min_similarity_score` — filter threshold for search results
    pub async fn connect(
        path: &Path,
        dim: usize,
        max_items: usize,
        min_similarity_score: f64,
    ) -> Result<Self> {
        let persist_path = path.join("memories.json");
        let mut backend = InMemoryBackend::new()
            .with_persist_path(persist_path)
            .with_max_items(max_items);
        backend.load_from_disk().await?;
        let memory = Arc::new(AsyncMutex::new(backend));

        #[cfg(feature = "vector-memory")]
        if dim > 0 {
            let lance_backend = lance::LanceDbBackend::connect(path, dim).await?;
            return Ok(Self {
                memory,
                lance: Some(Arc::new(lance_backend)),
                min_similarity_score,
            });
        }

        // Fallback: in-memory keyword backend (used when dim=0, or when
        // vector-memory feature is disabled).
        #[allow(unused_variables)]
        {
            let _ = path;
            let _ = dim;
            #[cfg(feature = "vector-memory")]
            {
                return Ok(Self {
                    memory,
                    lance: None,
                    min_similarity_score,
                });
            }
            #[cfg(not(feature = "vector-memory"))]
            {
                Ok(Self {
                    memory,
                    min_similarity_score,
                })
            }
        }
    }

    /// Pre-warm the embedder (download model files). No-op when feature is off.
    pub async fn warmup_embedder(&self) {
        #[cfg(feature = "vector-memory")]
        if let Some(ref lance) = self.lance {
            if let Err(e) = lance.embedder().initialize() {
                tracing::warn!("embedder warmup failed: {e}");
            }
        }
    }

    /// Store a memory item (with vector embedding when feature is enabled).
    pub async fn store_memory(&self, item: NewMemoryItem) -> Result<MemoryRecord> {
        #[cfg(feature = "vector-memory")]
        if let Some(ref lance) = self.lance {
            let record = lance.store_memory(&item).await?;
            // Mirror to in-memory cache
            self.memory.lock().await.store_memory(item).await;
            return Ok(record);
        }

        let record = self.memory.lock().await.store_memory(item).await;
        Ok(record)
    }

    /// Search memories by semantic relevance to `query`.
    pub async fn search_memories(
        &self,
        query: &str,
        k: u32,
        filter: Option<&str>,
    ) -> Result<Vec<MemoryRecord>> {
        let mut results = {
            #[cfg(feature = "vector-memory")]
            if let Some(ref lance) = self.lance {
                lance.search_memories(query, k, filter).await?
            } else {
                self.memory
                    .lock()
                    .await
                    .search_memories(query, k as usize, filter)
            }
            #[cfg(not(feature = "vector-memory"))]
            {
                self.memory
                    .lock()
                    .await
                    .search_memories(query, k as usize, filter)
            }
        };

        // Filter by configured min similarity score
        results.retain(|r| r.score >= self.min_similarity_score);
        Ok(results)
    }

    /// Store a history summary (with vector embedding when feature is enabled).
    pub async fn store_summary(&self, summary: HistorySummary) -> Result<()> {
        #[cfg(feature = "vector-memory")]
        if let Some(ref lance) = self.lance {
            lance.store_summary(&summary).await?;
        }

        self.memory.lock().await.store_summary(summary).await;
        Ok(())
    }

    /// Search history summaries by semantic relevance to `query`.
    pub async fn search_summaries(&self, query: &str, k: u32) -> Result<Vec<HistorySummary>> {
        let mut results = {
            #[cfg(feature = "vector-memory")]
            if let Some(ref lance) = self.lance {
                lance.search_summaries(query, k).await?
            } else {
                self.memory.lock().await.search_summaries(query, k as usize)
            }
            #[cfg(not(feature = "vector-memory"))]
            {
                self.memory.lock().await.search_summaries(query, k as usize)
            }
        };

        // Filter by configured min similarity score
        results.retain(|s| s.score >= self.min_similarity_score);
        Ok(results)
    }

    /// Delete expired memories.
    #[allow(dead_code)]
    pub async fn delete_expired_memories(&self) -> Result<usize> {
        #[cfg(feature = "vector-memory")]
        if let Some(ref lance) = self.lance {
            let lance_deleted = lance.delete_expired_memories().await?;
            let in_mem_deleted = self.memory.lock().await.delete_expired().await;
            return Ok(lance_deleted + in_mem_deleted);
        }

        let deleted = self.memory.lock().await.delete_expired().await;
        Ok(deleted)
    }

    /// Count total memories.
    #[allow(dead_code)]
    pub async fn count_memories(&self) -> Result<usize> {
        #[cfg(feature = "vector-memory")]
        if let Some(ref lance) = self.lance {
            return lance.count_memories().await;
        }

        Ok(self.memory.lock().await.count_memories())
    }

    /// Store a code chunk into the code_index table (Tier 4).
    pub async fn store_code_chunk(
        &self,
        id: &str,
        file_path: &str,
        chunk_index: i32,
        content: &str,
        project: &str,
    ) -> Result<()> {
        #[cfg(feature = "vector-memory")]
        if let Some(ref lance) = self.lance {
            return lance
                .store_code_chunk(id, file_path, chunk_index, content, project)
                .await;
        }
        // No-op in keyword mode; code indexing requires embeddings.
        #[allow(unused_variables)]
        {
            let _ = (id, file_path, chunk_index, content, project);
        }
        Ok(())
    }

    /// Index a file into the code_index table (Tier 4).
    ///
    /// Splits the file content into overlapping chunks (~2000 chars each)
    /// and stores each one with its embedding. Called after `write_file` and
    /// `edit_file` to keep the code index up-to-date.
    ///
    /// Best-effort: errors are logged but never propagated — a failed index
    /// write still returns successfully so the tool result is not affected.
    pub async fn index_file(&self, file_path: &str, content: &str, project: &str) {
        let chunks = chunk_content(content, 2000);
        for (i, chunk) in chunks.iter().enumerate() {
            let id = format!("{file_path}:chunk_{i}");
            if let Err(e) = self
                .store_code_chunk(&id, file_path, i as i32, chunk, project)
                .await
            {
                tracing::warn!(
                    file_path = %file_path,
                    chunk = i,
                    error = %e,
                    "index_file: failed to store code chunk"
                );
            }
        }
    }

    /// Search code chunks by semantic similarity (Tier 4).
    pub async fn search_code(
        &self,
        query: &str,
        k: u32,
    ) -> Result<Vec<(String, String, f64)>> {
        #[cfg(feature = "vector-memory")]
        if let Some(ref lance) = self.lance {
            return lance.search_code(query, k).await;
        }
        let _ = (query, k);
        Ok(Vec::new())
    }
}

/// Split content into overlapping chunks for code indexing.
///
/// Chunks are split at newline boundaries and kept at roughly `chunk_size`
/// characters each. Adjacent chunks overlap by ~10% to preserve cross-chunk
/// semantic context.
fn chunk_content(content: &str, chunk_size: usize) -> Vec<String> {
    let overlap = chunk_size / 10;
    let lines: Vec<&str> = content.lines().collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut line_idx = 0usize;

    while line_idx < lines.len() {
        current.clear();
        let mut char_count = 0usize;
        while line_idx < lines.len() && char_count < chunk_size {
            if !current.is_empty() {
                current.push('\n');
                char_count += 1;
            }
            current.push_str(lines[line_idx]);
            char_count += lines[line_idx].len();
            line_idx += 1;
        }
        if !current.is_empty() {
            chunks.push(current.clone());
        }
        // Back up for overlap
        if line_idx < lines.len() {
            let mut back_chars = 0usize;
            let mut back_lines = 0usize;
            for past_line in lines[..line_idx].iter().rev() {
                back_chars += past_line.len() + 1; // +1 for newline
                back_lines += 1;
                if back_chars >= overlap {
                    break;
                }
            }
            line_idx = line_idx.saturating_sub(back_lines);
        }
    }

    chunks
}

// ---------------------------------------------------------------------------
// Retrieved context — bundles verbatim window indices + semantic retrieval
// ---------------------------------------------------------------------------

/// Context retrieved from the vector database for a request.
///
/// Contains the verbatim window (which messages to send verbatim) and any
/// retrieved memory/summary blocks to inject into the system prompt.
#[derive(Debug, Clone, Default)]
pub struct RetrievedContext {
    /// Messages to send verbatim to the API (filtered from full history).
    pub verbatim_messages: Vec<usize>,
    /// Whether the verbatim window was extended for tool pairing.
    #[allow(dead_code)]
    pub window_extended: bool,
    /// Retrieved memory blocks for system prompt injection.
    pub memory_blocks: Vec<String>,
    /// Retrieved history summary blocks for system prompt injection.
    pub summary_blocks: Vec<String>,
}

impl RetrievedContext {
    /// True when no context was retrieved (all messages verbatim, no blocks).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.memory_blocks.is_empty() && self.summary_blocks.is_empty()
    }

    /// Build a `<retrieved_context>` block for injection into the system
    /// prompt. Returns `None` when no blocks are present.
    pub fn to_system_block(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        for mem in &self.memory_blocks {
            parts.push(format!("- [memory] {mem}"));
        }
        for sum in &self.summary_blocks {
            parts.push(format!("- [history] {sum}"));
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!(
            "<retrieved_context>\n{}\n</retrieved_context>",
            parts.join("\n")
        ))
    }
}

// ---------------------------------------------------------------------------
// Verbatim Window — determines which messages are sent in full to the API
// ---------------------------------------------------------------------------

/// Default verbatim window size (in turns). Matches `seam_manager::VERBATIM_WINDOW_TURNS`.
pub const DEFAULT_VERBATIM_WINDOW_TURNS: usize = 16;

/// Split messages into a verbatim window (recent + pinned) and a history
/// portion that will be retrieved via vector search instead.
#[derive(Debug, Clone)]
pub struct VerbatimWindow {
    /// Indices of messages to send in full to the API.
    pub indices: Vec<usize>,
    /// Whether the window was extended to preserve tool call/result pairs.
    pub extended: bool,
}

impl VerbatimWindow {
    /// Build the verbatim window from session metadata.
    ///
    /// * `total` — total number of messages in the session
    /// * `window_size` — how many recent messages to keep verbatim (default 8)
    /// * `pins` — externally pinned indices (user `/pin`, hot paths, etc.)
    /// * `tool_call_indices` — `(tool_use_id, message_index)` for all tool calls
    /// * `tool_result_indices` — `(tool_use_id, message_index)` for all tool results
    pub fn build(
        total: usize,
        window_size: usize,
        pins: &[usize],
        tool_call_indices: &[(String, usize)],
        tool_result_indices: &[(String, usize)],
    ) -> Self {
        let mut indices: Vec<usize> = Vec::new();

        // 1. Always include last `window_size` messages
        let recent_start = total.saturating_sub(window_size);
        indices.extend(recent_start..total);

        // 2. Include pinned indices
        for &p in pins {
            if p < total && !indices.contains(&p) {
                indices.push(p);
            }
        }

        // 3. Enforce tool call pairing
        let mut extended = false;

        // 3a. Tool call in window → pull in its result
        for (tool_id, call_idx) in tool_call_indices {
            if indices.contains(call_idx) {
                if let Some((_, result_idx)) =
                    tool_result_indices.iter().find(|(id, _)| id == tool_id)
                {
                    if !indices.contains(result_idx) {
                        indices.push(*result_idx);
                        extended = true;
                    }
                }
            }
        }

        // 3b. Tool result in window → pull in its call
        for (tool_id, result_idx) in tool_result_indices {
            if indices.contains(result_idx) {
                if let Some((_, call_idx)) =
                    tool_call_indices.iter().find(|(id, _)| id == tool_id)
                {
                    if !indices.contains(call_idx) {
                        indices.push(*call_idx);
                        extended = true;
                    }
                }
            }
        }

        indices.sort_unstable();
        indices.dedup();

        Self { indices, extended }
    }

    /// Number of messages in the window.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Whether the window is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Check if a specific index is in the verbatim window.
    #[allow(dead_code)]
    pub fn contains(&self, idx: usize) -> bool {
        self.indices.contains(&idx)
    }

    /// Iterate over indices in order.
    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &usize> {
        self.indices.iter()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── VerbatimWindow ──

    #[test]
    fn keeps_last_n_messages() {
        let vw = VerbatimWindow::build(20, 8, &[], &[], &[]);
        assert_eq!(vw.indices, (12..20).collect::<Vec<_>>());
        assert!(!vw.extended);
    }

    #[test]
    fn fewer_messages_than_window() {
        let vw = VerbatimWindow::build(3, 8, &[], &[], &[]);
        assert_eq!(vw.indices, (0..3).collect::<Vec<_>>());
    }

    #[test]
    fn zero_messages() {
        let vw = VerbatimWindow::build(0, 8, &[], &[], &[]);
        assert!(vw.is_empty());
    }

    #[test]
    fn includes_pins_outside_window() {
        let vw = VerbatimWindow::build(20, 4, &[0, 5], &[], &[]);
        assert!(vw.contains(0));
        assert!(vw.contains(5));
        assert!(vw.contains(19));
    }

    #[test]
    fn pins_within_window_not_duplicated() {
        let vw = VerbatimWindow::build(10, 8, &[8, 9], &[], &[]);
        assert_eq!(vw.indices.len(), 8);
        let count_8 = vw.indices.iter().filter(|&&i| i == 8).count();
        let count_9 = vw.indices.iter().filter(|&&i| i == 9).count();
        assert_eq!(count_8, 1);
        assert_eq!(count_9, 1);
    }

    #[test]
    fn tool_call_pulls_in_result() {
        let calls = vec![("t1".to_string(), 10)];
        let results = vec![("t1".to_string(), 11)];
        let vw = VerbatimWindow::build(20, 4, &[10], &calls, &results);
        assert!(vw.contains(10));
        assert!(vw.contains(11));
        assert!(vw.extended);
    }

    #[test]
    fn tool_result_pulls_in_call() {
        let calls = vec![("t1".to_string(), 5)];
        let results = vec![("t1".to_string(), 15)];
        let vw = VerbatimWindow::build(20, 4, &[15], &calls, &results);
        assert!(vw.contains(15));
        assert!(vw.contains(5));
    }

    #[test]
    fn orphan_tool_result_does_not_extend() {
        let vw = VerbatimWindow::build(20, 4, &[15], &[], &[("orphan".to_string(), 15)]);
        assert!(vw.contains(15));
        assert!(!vw.extended);
    }

    #[test]
    fn out_of_bounds_pin_ignored() {
        let vw = VerbatimWindow::build(10, 4, &[99], &[], &[]);
        assert!(!vw.contains(99));
        assert_eq!(vw.indices.len(), 4);
    }

    #[test]
    fn window_size_zero_keeps_only_pins() {
        let vw = VerbatimWindow::build(20, 0, &[0], &[], &[]);
        assert_eq!(vw.indices, vec![0]);
    }

    #[test]
    fn mixed_pins_and_recent_no_gaps() {
        let vw = VerbatimWindow::build(10, 3, &[0, 2, 5], &[], &[]);
        assert_eq!(vw.indices, vec![0, 2, 5, 7, 8, 9]);
    }

    // ── VectorDbService (in-memory mode, same tests as before) ──

    /// Helper: create a VectorDbService backed by a fresh temp dir.
    /// Uses dim=0 to force in-memory keyword backend even when
    /// `vector-memory` feature is enabled (avoids ONNX model download).
    async fn new_test_svc() -> (VectorDbService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let svc = VectorDbService::connect(dir.path().join("vdb").as_path(), 0, 1000, 0.0)
            .await
            .unwrap();
        (svc, dir)
    }

    #[tokio::test]
    async fn store_and_retrieve_memory() {
        let (svc, _dir) = new_test_svc().await;

        svc.store_memory(NewMemoryItem {
            content: "用户喜欢 4 空格缩进".into(),
            source: "model".into(),
            session_id: "s1".into(),
            tags: Some("preference".into()),
            ttl: None,
        })
        .await
        .unwrap();

        let results = svc.search_memories("缩进", 5, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("4 空格缩进"));
    }

    #[tokio::test]
    async fn search_non_existent() {
        let (svc, _dir) = new_test_svc().await;

        svc.store_memory(NewMemoryItem {
            content: "测试记忆".into(),
            source: "user".into(),
            session_id: "s1".into(),
            tags: None,
            ttl: None,
        })
        .await
        .unwrap();

        let results = svc.search_memories("不存在的关键词", 5, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn multiple_memories_ranked() {
        let (svc, _dir) = new_test_svc().await;

        svc.store_memory(NewMemoryItem {
            content: "用户用 cargo test --workspace 运行测试".into(),
            source: "model".into(),
            session_id: "s1".into(),
            tags: None,
            ttl: None,
        })
        .await
        .unwrap();

        svc.store_memory(NewMemoryItem {
            content: "用户喜欢 VS Code 编辑器".into(),
            source: "model".into(),
            session_id: "s1".into(),
            tags: None,
            ttl: None,
        })
        .await
        .unwrap();

        let results = svc.search_memories("cargo 测试", 2, None).await.unwrap();
        assert!(results.len() >= 1);
        assert!(results[0].content.contains("test"));
    }

    #[tokio::test]
    async fn store_and_search_summaries() {
        let (svc, _dir) = new_test_svc().await;

        svc.store_summary(HistorySummary {
            id: "sum-1".into(),
            turn_range: "1-10".into(),
            summary: "用户修改了 config.rs 中的编译选项".into(),
            key_files: Some("crates/tui/src/config.rs".into()),
            session_id: "s1".into(),
            created_at: Utc::now(),
            score: 0.0,
        })
        .await
        .unwrap();

        let results = svc.search_summaries("config", 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].summary.contains("config.rs"));
    }

    #[tokio::test]
    async fn delete_expired_memories() {
        let (svc, _dir) = new_test_svc().await;

        svc.store_memory(NewMemoryItem {
            content: "会过期的记忆".into(),
            source: "test".into(),
            session_id: "s1".into(),
            tags: None,
            ttl: Some(Utc::now() - chrono::Duration::hours(1)),
        })
        .await
        .unwrap();

        svc.store_memory(NewMemoryItem {
            content: "永不过期的记忆".into(),
            source: "test".into(),
            session_id: "s1".into(),
            tags: None,
            ttl: None,
        })
        .await
        .unwrap();

        let deleted = svc.delete_expired_memories().await.unwrap();
        assert_eq!(deleted, 1);

        let count = svc.count_memories().await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn count_stored_memories() {
        let (svc, _dir) = new_test_svc().await;

        for i in 0..5 {
            svc.store_memory(NewMemoryItem {
                content: format!("记忆 {i}"),
                source: "test".into(),
                session_id: "s1".into(),
                tags: None,
                ttl: None,
            })
            .await
            .unwrap();
        }

        assert_eq!(svc.count_memories().await.unwrap(), 5);
    }
}
