//! Slop Ledger — durable tracking of unresolved architectural residue.
//!
//! AI agents often leave behind invisible "slop" after a task:
//! compatibility shims, unmigrated callers, duplicated concepts,
//! naming drift, stale docs/tests, suspected dead code, and tool gaps.
//!
//! The Slop Ledger makes this residue **visible and queryable** so the
//! next agent (or human) doesn't rediscover it, amplify it, or mistake
//! it for intended architecture.
//!
//! ## Design
//!
//! - **Storage**: `~/.codewhale/slop_ledger.json` (a JSON array of entries).
//! - **Schema**: each entry has a bucket, severity, confidence, owner,
//!   source links, status, cleanup recommendation, and timestamps.
//! - **Tools**: `slop_ledger_append`, `slop_ledger_query`,
//!   `slop_ledger_update`, `slop_ledger_export`.
//! - **Integration**: entries can link to durable tasks and threads;
//!   the export path produces a redacted Markdown handoff suitable for
//!   GitHub issues or compaction relays.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io;
use std::path::PathBuf;
use uuid::Uuid;

use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

// ── Enums ──────────────────────────────────────────────────────────────────

/// Classification bucket for a slop entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlopBucket {
    RetainedCompatibility,
    UnmigratedCallers,
    DuplicateConcepts,
    NamingDrift,
    StaleDocs,
    StaleTests,
    SuspectedDeadCode,
    UnverifiedPublicBehavior,
    ToolGaps,
    AcceptedDebt,
}

impl SlopBucket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetainedCompatibility => "retained_compatibility",
            Self::UnmigratedCallers => "unmigrated_callers",
            Self::DuplicateConcepts => "duplicate_concepts",
            Self::NamingDrift => "naming_drift",
            Self::StaleDocs => "stale_docs",
            Self::StaleTests => "stale_tests",
            Self::SuspectedDeadCode => "suspected_dead_code",
            Self::UnverifiedPublicBehavior => "unverified_public_behavior",
            Self::ToolGaps => "tool_gaps",
            Self::AcceptedDebt => "accepted_debt",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "retained_compatibility" => Some(Self::RetainedCompatibility),
            "unmigrated_callers" => Some(Self::UnmigratedCallers),
            "duplicate_concepts" => Some(Self::DuplicateConcepts),
            "naming_drift" => Some(Self::NamingDrift),
            "stale_docs" => Some(Self::StaleDocs),
            "stale_tests" => Some(Self::StaleTests),
            "suspected_dead_code" => Some(Self::SuspectedDeadCode),
            "unverified_public_behavior" => Some(Self::UnverifiedPublicBehavior),
            "tool_gaps" => Some(Self::ToolGaps),
            "accepted_debt" => Some(Self::AcceptedDebt),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn all_buckets() -> &'static [SlopBucket] {
        &[
            Self::RetainedCompatibility,
            Self::UnmigratedCallers,
            Self::DuplicateConcepts,
            Self::NamingDrift,
            Self::StaleDocs,
            Self::StaleTests,
            Self::SuspectedDeadCode,
            Self::UnverifiedPublicBehavior,
            Self::ToolGaps,
            Self::AcceptedDebt,
        ]
    }
}

/// Severity of the residue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlopSeverity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

impl SlopSeverity {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            "info" => Some(Self::Info),
            _ => None,
        }
    }
}

/// Confidence in the assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlopConfidence {
    Certain,
    High,
    Medium,
    Low,
}

impl SlopConfidence {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "certain" => Some(Self::Certain),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }
}

/// Lifecycle status of a slop entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlopEntryStatus {
    Open,
    InProgress,
    Resolved,
    Accepted,
    WontFix,
}

impl SlopEntryStatus {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "open" => Some(Self::Open),
            "in_progress" | "inprogress" => Some(Self::InProgress),
            "resolved" | "done" => Some(Self::Resolved),
            "accepted" => Some(Self::Accepted),
            "wontfix" | "wont_fix" => Some(Self::WontFix),
            _ => None,
        }
    }
}

// ── Core data structures ───────────────────────────────────────────────────

/// A single slop ledger entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlopEntry {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Classification bucket.
    pub bucket: SlopBucket,
    /// How severe is this residue?
    pub severity: SlopSeverity,
    /// How confident is the assessment?
    pub confidence: SlopConfidence,
    /// Who owns cleaning this up (person, team, or "auto").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Source file paths, URLs, or line references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_links: Vec<String>,
    /// Short title (one line).
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Current lifecycle status.
    pub status: SlopEntryStatus,
    /// Suggested cleanup action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_recommendation: Option<String>,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    /// ISO 8601 last-updated timestamp.
    pub updated_at: String,
    /// Optional linked durable task id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Optional linked thread id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

impl SlopEntry {
    pub fn new(
        bucket: SlopBucket,
        severity: SlopSeverity,
        confidence: SlopConfidence,
        title: String,
        description: String,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            bucket,
            severity,
            confidence,
            owner: None,
            source_links: Vec::new(),
            title,
            description,
            status: SlopEntryStatus::Open,
            cleanup_recommendation: None,
            created_at: now.clone(),
            updated_at: now,
            task_id: None,
            thread_id: None,
        }
    }
}

// ── Query filter ───────────────────────────────────────────────────────────

/// Filter for querying ledger entries.
#[derive(Debug, Clone, Default)]
pub struct SlopLedgerFilter {
    pub bucket: Option<SlopBucket>,
    pub severity: Option<SlopSeverity>,
    pub status: Option<SlopEntryStatus>,
    pub search: Option<String>, // fuzzy match title + description
    pub limit: Option<usize>,
}

// ── Ledger (collection + persistence) ──────────────────────────────────────

/// The slop ledger — a collection of entries with JSON file persistence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlopLedger {
    entries: Vec<SlopEntry>,
    #[serde(skip)]
    ledger_path: PathBuf,
}

impl SlopLedger {
    /// Resolve the default ledger path.
    pub fn default_path() -> io::Result<PathBuf> {
        codewhale_config::resolve_state_dir("slop_ledger")
            .map(|p| p.join("slop_ledger.json"))
            .map_err(io::Error::other)
    }

    /// Load ledger from the default path, returning an empty ledger if the
    /// file doesn't exist.
    pub fn load() -> io::Result<Self> {
        let path = Self::default_path()?;
        Self::load_at(&path)
    }

    /// Load ledger from a specific path.
    pub fn load_at(path: &std::path::Path) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self {
                entries: Vec::new(),
                ledger_path: path.to_path_buf(),
            });
        }
        let data = fs::read_to_string(path)?;
        let mut ledger: SlopLedger = serde_json::from_str(&data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse slop ledger JSON: {e}"),
            )
        })?;
        ledger.ledger_path = path.to_path_buf();
        Ok(ledger)
    }

    /// Persist the ledger to disk.
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.ledger_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::other(format!("serialization error: {e}")))?;
        crate::utils::write_atomic(&self.ledger_path, data.as_bytes())
    }

    /// Append one or more entries. Returns the new entry count and
    /// the short ids of the appended entries.
    pub fn append(&mut self, entries: Vec<SlopEntry>) -> (usize, Vec<String>) {
        let ids: Vec<String> = entries.iter().map(|e| short_id(&e.id)).collect();
        self.entries.extend(entries);
        (self.entries.len(), ids)
    }

    /// Return the total number of entries.
    #[must_use]
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Query entries matching the filter.
    pub fn query(&self, filter: &SlopLedgerFilter) -> Vec<&SlopEntry> {
        let mut results: Vec<&SlopEntry> = self
            .entries
            .iter()
            .filter(|e| {
                if let Some(bucket) = &filter.bucket
                    && e.bucket != *bucket
                {
                    return false;
                }
                if let Some(severity) = &filter.severity
                    && e.severity != *severity
                {
                    return false;
                }
                if let Some(status) = &filter.status
                    && e.status != *status
                {
                    return false;
                }
                if let Some(search) = &filter.search {
                    let q = search.to_lowercase();
                    if !e.title.to_lowercase().contains(&q)
                        && !e.description.to_lowercase().contains(&q)
                    {
                        return false;
                    }
                }
                true
            })
            .collect();

        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }
        results
    }

    /// Find an entry by id.
    pub fn find_mut(&mut self, id: &str) -> Option<&mut SlopEntry> {
        self.entries.iter_mut().find(|e| e.id.starts_with(id))
    }

    /// Update an entry's status (and optionally other fields) and save.
    pub fn update_status(
        &mut self,
        id: &str,
        status: SlopEntryStatus,
        cleanup_recommendation: Option<String>,
    ) -> io::Result<Option<&SlopEntry>> {
        let full_id = {
            let entry = match self.find_mut(id) {
                Some(e) => e,
                None => return Ok(None),
            };
            entry.status = status;
            entry.updated_at = chrono::Utc::now().to_rfc3339();
            if let Some(rec) = cleanup_recommendation {
                entry.cleanup_recommendation = Some(rec);
            }
            entry.id.clone()
        };
        self.save()?;
        // Return a shared ref to the updated entry.
        Ok(self.entries.iter().find(|e| e.id == full_id))
    }

    /// Export all entries as a Markdown string suitable for handoff or
    /// GitHub issue body.
    pub fn export_markdown(
        &self,
        title: Option<&str>,
        filter: Option<&SlopLedgerFilter>,
    ) -> String {
        let entries: Vec<&SlopEntry> = match filter {
            Some(f) => self.query(f),
            None => self.entries.iter().collect(),
        };

        let heading = title.unwrap_or("Slop Ledger Export");
        let mut out = format!("# {heading}\n\n");
        out.push_str(&format!(
            "_Generated at {} — {} entries_\n\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"),
            entries.len()
        ));

        if entries.is_empty() {
            out.push_str("_(no entries)_\n");
            return out;
        }

        // Group by bucket
        use std::collections::BTreeMap;
        let mut by_bucket: BTreeMap<&str, Vec<&&SlopEntry>> = BTreeMap::new();
        for e in &entries {
            by_bucket.entry(e.bucket.as_str()).or_default().push(e);
        }

        for (bucket_name, bucket_entries) in &by_bucket {
            out.push_str(&format!("## {bucket_name}\n\n"));
            out.push_str("| ID | Severity | Confidence | Status | Title | Source |\n");
            out.push_str("|---|---|---|---|---|---|\n");
            for e in bucket_entries {
                let source = e.source_links.first().map(|s| s.as_str()).unwrap_or("-");
                let title = truncate_str(&e.title, 60);
                out.push_str(&format!(
                    "| {} | {:?} | {:?} | {:?} | {title} | {source} |\n",
                    short_id(&e.id),
                    e.severity,
                    e.confidence,
                    e.status
                ));
            }
            out.push('\n');

            // Detailed entries
            for e in bucket_entries {
                out.push_str(&format!("### {} — {}\n\n", short_id(&e.id), e.title));
                out.push_str(&format!("- **Severity**: {:?}\n", e.severity));
                out.push_str(&format!("- **Confidence**: {:?}\n", e.confidence));
                out.push_str(&format!("- **Status**: {:?}\n", e.status));
                if let Some(ref owner) = e.owner {
                    out.push_str(&format!("- **Owner**: {owner}\n"));
                }
                if !e.source_links.is_empty() {
                    out.push_str("- **Sources**:\n");
                    for link in &e.source_links {
                        out.push_str(&format!("  - {link}\n"));
                    }
                }
                out.push_str(&format!("\n{}\n", e.description));
                if let Some(ref rec) = e.cleanup_recommendation {
                    out.push_str(&format!("\n**Cleanup**: {rec}\n"));
                }
                out.push_str("\n---\n\n");
            }
        }

        redact_exported_text(&mut out);
        out
    }

    /// Summary counts by bucket and status — useful for quick display.
    pub fn summary(&self) -> String {
        use std::collections::BTreeMap;
        let mut by_bucket: BTreeMap<&str, usize> = BTreeMap::new();
        let mut open_count = 0usize;
        let mut resolved_count = 0usize;
        let mut accepted_count = 0usize;

        for e in &self.entries {
            *by_bucket.entry(e.bucket.as_str()).or_default() += 1;
            match e.status {
                SlopEntryStatus::Resolved => resolved_count += 1,
                SlopEntryStatus::Accepted | SlopEntryStatus::WontFix => accepted_count += 1,
                _ => open_count += 1,
            }
        }

        let mut out = format!(
            "Slop Ledger: {} total | {} open | {} resolved | {} accepted\n",
            self.entries.len(),
            open_count,
            resolved_count,
            accepted_count
        );
        for (bucket, count) in &by_bucket {
            out.push_str(&format!("  {bucket}: {count}\n"));
        }
        redact_exported_text(&mut out);
        out
    }
}

// ── Tools ──────────────────────────────────────────────────────────────────

/// `slop_ledger_append` — append one or more entries to the slop ledger.
pub struct SlopLedgerAppendTool;

#[async_trait]
impl ToolSpec for SlopLedgerAppendTool {
    fn name(&self) -> &'static str {
        "slop_ledger_append"
    }

    fn description(&self) -> &'static str {
        "Append one or more entries to the slop ledger — a durable record of \
         unresolved architectural residue (compatibility shims, unmigrated \
         callers, duplicate concepts, stale docs/tests, suspected dead code, \
         tool gaps, etc.). Use this when you complete a task and notice \
         residue that should be tracked for future cleanup. Each entry needs \
         a bucket, severity, confidence, title, and description."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "entries": {
                    "type": "array",
                    "description": "One or more slop entries to append.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "bucket": {
                                "type": "string",
                                "description": "One of: retained_compatibility, unmigrated_callers, duplicate_concepts, naming_drift, stale_docs, stale_tests, suspected_dead_code, unverified_public_behavior, tool_gaps, accepted_debt"
                            },
                            "severity": {
                                "type": "string",
                                "description": "critical | high | medium | low | info"
                            },
                            "confidence": {
                                "type": "string",
                                "description": "certain | high | medium | low"
                            },
                            "title": {
                                "type": "string",
                                "description": "Short title (one line)"
                            },
                            "description": {
                                "type": "string",
                                "description": "Detailed description of the residue"
                            },
                            "owner": {
                                "type": "string",
                                "description": "Optional: who should clean this up?"
                            },
                            "source_links": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Optional: file paths or URLs"
                            }
                        },
                        "required": ["bucket", "severity", "confidence", "title", "description"]
                    }
                }
            },
            "required": ["entries"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let entries_val = input
            .get("entries")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::invalid_input("'entries' must be a non-empty array"))?;

        let mut ledger = SlopLedger::load()
            .map_err(|e| ToolError::execution_failed(format!("failed to load slop ledger: {e}")))?;

        let mut appended = Vec::new();
        for entry_val in entries_val {
            let bucket_str = required_str(entry_val, "bucket")?;
            let bucket = SlopBucket::from_str(bucket_str).ok_or_else(|| {
                ToolError::invalid_input(format!("unknown bucket: '{bucket_str}'"))
            })?;

            let severity = SlopSeverity::from_str(required_str(entry_val, "severity")?)
                .ok_or_else(|| {
                    ToolError::invalid_input("invalid severity (use critical|high|medium|low|info)")
                })?;

            let confidence = SlopConfidence::from_str(required_str(entry_val, "confidence")?)
                .ok_or_else(|| {
                    ToolError::invalid_input("invalid confidence (use certain|high|medium|low)")
                })?;

            let title = required_str(entry_val, "title")?.to_string();
            let description = required_str(entry_val, "description")?.to_string();

            let mut entry = SlopEntry::new(bucket, severity, confidence, title, description);

            if let Some(owner) = entry_val.get("owner").and_then(|v| v.as_str()) {
                entry.owner = Some(owner.to_string());
            }
            if let Some(links) = entry_val.get("source_links").and_then(|v| v.as_array()) {
                entry.source_links = links
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }

            // Attach active task/thread context if available
            if let Some(ref task_id) = context.runtime.active_task_id {
                entry.task_id = Some(task_id.clone());
            }
            if let Some(ref thread_id) = context.runtime.active_thread_id {
                entry.thread_id = Some(thread_id.clone());
            }

            appended.push(entry);
        }

        let (total, ids) = ledger.append(appended);
        let appended_count = ids.len();

        ledger
            .save()
            .map_err(|e| ToolError::execution_failed(format!("failed to save slop ledger: {e}")))?;

        Ok(ToolResult::success(format!(
            "Appended {} slop ledger entr{} ({} total): {}",
            appended_count,
            if appended_count == 1 { "y" } else { "ies" },
            total,
            ids.join(", ")
        )))
    }
}

/// `slop_ledger_query` — query the slop ledger.
pub struct SlopLedgerQueryTool;

#[async_trait]
impl ToolSpec for SlopLedgerQueryTool {
    fn name(&self) -> &'static str {
        "slop_ledger_query"
    }

    fn description(&self) -> &'static str {
        "Query the slop ledger for unresolved architectural residue. \
         Filter by bucket, severity, status, or text search."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bucket": {
                    "type": "string",
                    "description": "Optional: filter by bucket"
                },
                "severity": {
                    "type": "string",
                    "description": "Optional: filter by severity"
                },
                "status": {
                    "type": "string",
                    "description": "Optional: filter by status"
                },
                "search": {
                    "type": "string",
                    "description": "Optional: fuzzy text search in title and description"
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional: max results (default 50)"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let filter = SlopLedgerFilter {
            bucket: input
                .get("bucket")
                .and_then(|v| v.as_str())
                .and_then(SlopBucket::from_str),
            severity: input
                .get("severity")
                .and_then(|v| v.as_str())
                .and_then(SlopSeverity::from_str),
            status: input
                .get("status")
                .and_then(|v| v.as_str())
                .and_then(SlopEntryStatus::from_str),
            search: input
                .get("search")
                .and_then(|v| v.as_str())
                .map(String::from),
            limit: input
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .or(Some(50)),
        };

        let ledger = SlopLedger::load()
            .map_err(|e| ToolError::execution_failed(format!("failed to load slop ledger: {e}")))?;

        if ledger.is_empty() {
            return Ok(ToolResult::success("Slop ledger is empty."));
        }

        let results = ledger.query(&filter);
        let mut out = format!("Found {} matching slop ledger entries:\n\n", results.len());
        for entry in &results {
            out.push_str(&format!(
                "- [{}] **{}** ({:?} | {:?} | {:?}) — {}\n",
                short_id(&entry.id),
                entry.bucket.as_str(),
                entry.severity,
                entry.confidence,
                entry.status,
                entry.title
            ));
            if let Some(ref desc) = entry.description.lines().next() {
                out.push_str(&format!("  {desc}\n"));
            }
        }
        Ok(ToolResult::success(out))
    }
}

/// `slop_ledger_update` — update an entry's status.
pub struct SlopLedgerUpdateTool;

#[async_trait]
impl ToolSpec for SlopLedgerUpdateTool {
    fn name(&self) -> &'static str {
        "slop_ledger_update"
    }

    fn description(&self) -> &'static str {
        "Update a slop ledger entry's status (e.g., mark as resolved, accepted, or in-progress)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The entry ID (or prefix) to update"
                },
                "status": {
                    "type": "string",
                    "description": "New status: open | in_progress | resolved | accepted | wontfix"
                },
                "cleanup_recommendation": {
                    "type": "string",
                    "description": "Optional: cleanup notes when resolving or accepting"
                }
            },
            "required": ["id", "status"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let id = required_str(&input, "id")?;
        let status =
            SlopEntryStatus::from_str(required_str(&input, "status")?).ok_or_else(|| {
                ToolError::invalid_input(
                    "invalid status (use open|in_progress|resolved|accepted|wontfix)",
                )
            })?;

        let cleanup = input
            .get("cleanup_recommendation")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut ledger = SlopLedger::load()
            .map_err(|e| ToolError::execution_failed(format!("failed to load slop ledger: {e}")))?;

        match ledger.update_status(id, status, cleanup) {
            Ok(Some(entry)) => Ok(ToolResult::success(format!(
                "Updated slop ledger entry {} ({}) → {:?}",
                short_id(&entry.id),
                entry.title,
                entry.status
            ))),
            Ok(None) => Ok(ToolResult::success(format!(
                "No slop ledger entry found matching '{id}'. Use slop_ledger_query to list entries."
            ))),
            Err(e) => Err(ToolError::execution_failed(format!(
                "failed to update slop ledger: {e}"
            ))),
        }
    }
}

/// `slop_ledger_export` — export ledger as Markdown.
pub struct SlopLedgerExportTool;

#[async_trait]
impl ToolSpec for SlopLedgerExportTool {
    fn name(&self) -> &'static str {
        "slop_ledger_export"
    }

    fn description(&self) -> &'static str {
        "Export the slop ledger as a Markdown report. Use this for handoffs, \
         compaction relays, or GitHub issue creation. The output is suitable \
         for pasting directly into a GitHub issue body."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Optional: report title (default 'Slop Ledger Export')"
                },
                "bucket": {
                    "type": "string",
                    "description": "Optional: filter by bucket"
                },
                "severity": {
                    "type": "string",
                    "description": "Optional: filter by severity"
                },
                "status": {
                    "type": "string",
                    "description": "Optional: filter by status"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let title = input.get("title").and_then(|v| v.as_str());

        let filter = if input.get("bucket").is_some()
            || input.get("severity").is_some()
            || input.get("status").is_some()
        {
            Some(SlopLedgerFilter {
                bucket: input
                    .get("bucket")
                    .and_then(|v| v.as_str())
                    .and_then(SlopBucket::from_str),
                severity: input
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .and_then(SlopSeverity::from_str),
                status: input
                    .get("status")
                    .and_then(|v| v.as_str())
                    .and_then(SlopEntryStatus::from_str),
                ..Default::default()
            })
        } else {
            None
        };

        let ledger = SlopLedger::load()
            .map_err(|e| ToolError::execution_failed(format!("failed to load slop ledger: {e}")))?;

        let markdown = ledger.export_markdown(title, filter.as_ref());
        Ok(ToolResult::success(markdown))
    }
}

/// Truncate a UTF-8 string to at most `max_chars` characters, appending '…'
/// when truncation occurs. Operates on char boundaries — never panics on
/// multi-byte characters.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

/// Return a display-safe short id without assuming byte offsets are char
/// boundaries. Ledger ids are normally UUIDs, but imported or hand-edited
/// ledgers may contain shorter or non-ASCII ids.
#[must_use]
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Redact sensitive patterns from exported text: API keys and secrets
/// paths. Scan the output for known key prefixes (`sk-`, `Bearer `, `dsk-`)
/// and replace the token until a whitespace / punctuation boundary with
/// `[REDACTED]`. Also normalises fully-qualified secrets directory paths
/// to the portable `~/.codewhale/secrets` form.
fn redact_exported_text(text: &mut String) {
    let prefixes: &[&[u8]] = &[b"sk-", b"Bearer ", b"dsk-", b"deepseek-"];
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let mut matched = false;
        for prefix in prefixes {
            if bytes[i..].len() >= prefix.len()
                && bytes[i..i + prefix.len()].eq_ignore_ascii_case(prefix)
            {
                // Scan forward to first whitespace or delimiter.
                let end = bytes[i + prefix.len()..]
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || *b == b',' || *b == b';')
                    .map(|p| i + prefix.len() + p)
                    .unwrap_or(bytes.len());
                result.push_str("[REDACTED]");
                i = end;
                matched = true;
                break;
            }
        }
        if !matched {
            // Advance by one char (preserving multi-byte UTF-8 safety).
            let ch = text[i..].chars().next().unwrap();
            result.push(ch);
            i += ch.len_utf8();
        }
    }

    // Normalise secrets directory paths.
    if let Some(home) = dirs::home_dir() {
        for leaf in [".codewhale/secrets", ".deepseek/secrets"] {
            let dir = home.join(leaf);
            let prefix = dir.to_string_lossy().to_string();
            result = result.replace(&prefix, "~/.codewhale/secrets");
        }
    }
    *text = result;
}

impl SlopLedger {
    /// Completion-gate / verifier hook: returns `true` when there are
    /// unresolved slop entries (status `Open` or `InProgress`) that the
    /// agent should review before claiming the task is done.
    ///
    /// Tools and engine hooks can call this on claim-of-done to surface
    /// architectural residue the agent may have overlooked.
    #[allow(dead_code)]
    #[must_use]
    pub fn has_open_entries(&self) -> bool {
        self.entries.iter().any(|e| {
            matches!(
                e.status,
                SlopEntryStatus::Open | SlopEntryStatus::InProgress
            )
        })
    }

    /// Return a concise completion-gate summary suitable for a verifier
    /// sub-agent or the claim-of-done prompt. Returns `None` when all
    /// entries are resolved — the caller can then treat the gate as "pass".
    #[allow(dead_code)]
    #[must_use]
    pub fn completion_gate_summary(&self) -> Option<String> {
        let open: Vec<&SlopEntry> = self
            .entries
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    SlopEntryStatus::Open | SlopEntryStatus::InProgress
                )
            })
            .collect();
        if open.is_empty() {
            return None;
        }
        let mut out = format!(
            "## ⚠️ SlopLedger gate — {} open slop entries\n\n",
            open.len()
        );
        out.push_str("Review these before claiming completion:\n\n");
        for e in open {
            out.push_str(&format!(
                "- **{}** `{}` ({:?}/{:?}): {}\n",
                e.bucket.as_str(),
                short_id(&e.id),
                e.severity,
                e.confidence,
                truncate_str(&e.title, 80),
            ));
        }
        Some(out)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_ledger() -> (TempDir, SlopLedger) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("slop_ledger.json");
        let ledger = SlopLedger {
            entries: Vec::new(),
            ledger_path: path,
        };
        (tmp, ledger)
    }

    #[test]
    fn bucket_roundtrip() {
        for bucket in SlopBucket::all_buckets() {
            let s = bucket.as_str();
            let parsed = SlopBucket::from_str(s);
            assert_eq!(parsed, Some(*bucket), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn append_and_save_load() {
        let (_tmp, mut ledger) = temp_ledger();

        let entry = SlopEntry::new(
            SlopBucket::StaleDocs,
            SlopSeverity::Medium,
            SlopConfidence::High,
            "README is outdated".into(),
            "The README still references v0.7 APIs.".into(),
        );

        let _ = ledger.append(vec![entry]);
        assert_eq!(ledger.len(), 1);
        ledger.save().unwrap();

        let loaded = SlopLedger::load_at(&ledger.ledger_path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.entries[0].title, "README is outdated");
    }

    #[test]
    fn short_id_handles_short_and_non_ascii_ids() {
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("abcdefghi"), "abcdefgh");
        assert_eq!(short_id("残渣-ledger-entry"), "残渣-ledge");
    }

    #[test]
    fn display_paths_do_not_panic_on_short_or_non_ascii_ids() {
        let (_tmp, mut ledger) = temp_ledger();

        let mut short = SlopEntry::new(
            SlopBucket::StaleDocs,
            SlopSeverity::Low,
            SlopConfidence::High,
            "short id".into(),
            "desc".into(),
        );
        short.id = "abc".into();

        let mut unicode = SlopEntry::new(
            SlopBucket::ToolGaps,
            SlopSeverity::Medium,
            SlopConfidence::Medium,
            "unicode id".into(),
            "desc".into(),
        );
        unicode.id = "残渣-ledger-entry".into();

        let (_total, ids) = ledger.append(vec![short, unicode]);
        assert_eq!(ids, vec!["abc", "残渣-ledge"]);

        let md = ledger.export_markdown(None, None);
        assert!(md.contains("| abc |"));
        assert!(md.contains("| 残渣-ledge |"));
        assert!(ledger.completion_gate_summary().is_some());
    }

    #[test]
    fn query_by_bucket() {
        let (_tmp, mut ledger) = temp_ledger();

        let _ = ledger.append(vec![
            SlopEntry::new(
                SlopBucket::StaleDocs,
                SlopSeverity::Low,
                SlopConfidence::Certain,
                "doc A".into(),
                "desc A".into(),
            ),
            SlopEntry::new(
                SlopBucket::ToolGaps,
                SlopSeverity::High,
                SlopConfidence::Medium,
                "gap B".into(),
                "desc B".into(),
            ),
        ]);

        let filter = SlopLedgerFilter {
            bucket: Some(SlopBucket::StaleDocs),
            ..Default::default()
        };
        let results = ledger.query(&filter);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "doc A");
    }

    #[test]
    fn query_by_search() {
        let (_tmp, mut ledger) = temp_ledger();

        let _ = ledger.append(vec![SlopEntry::new(
            SlopBucket::SuspectedDeadCode,
            SlopSeverity::Medium,
            SlopConfidence::Low,
            "dead legacy handler".into(),
            "The legacy handler in src/old.rs appears unused.".into(),
        )]);

        let filter = SlopLedgerFilter {
            search: Some("legacy".into()),
            ..Default::default()
        };
        let results = ledger.query(&filter);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn update_status() {
        let (_tmp, mut ledger) = temp_ledger();

        let entry = SlopEntry::new(
            SlopBucket::NamingDrift,
            SlopSeverity::Low,
            SlopConfidence::High,
            "naming issue".into(),
            "desc".into(),
        );
        let id = entry.id.clone();
        let _ = ledger.append(vec![entry]);
        ledger.save().unwrap();

        let result = ledger
            .update_status(
                &id,
                SlopEntryStatus::Resolved,
                Some("Renamed in #1234".into()),
            )
            .unwrap();
        assert!(result.is_some());

        let loaded = SlopLedger::load_at(&ledger.ledger_path).unwrap();
        assert_eq!(loaded.entries[0].status, SlopEntryStatus::Resolved);
        assert_eq!(
            loaded.entries[0].cleanup_recommendation,
            Some("Renamed in #1234".into())
        );
    }

    #[test]
    fn update_status_returns_entry_for_prefix_match() {
        let (_tmp, mut ledger) = temp_ledger();

        let entry = SlopEntry::new(
            SlopBucket::NamingDrift,
            SlopSeverity::Low,
            SlopConfidence::High,
            "naming issue".into(),
            "desc".into(),
        );
        let id = entry.id.clone();
        let prefix = short_id(&id);
        let _ = ledger.append(vec![entry]);
        ledger.save().unwrap();

        let result = ledger
            .update_status(&prefix, SlopEntryStatus::Resolved, None)
            .unwrap();

        assert_eq!(result.map(|entry| entry.id.as_str()), Some(id.as_str()));
    }

    #[test]
    fn export_markdown() {
        let (_tmp, mut ledger) = temp_ledger();

        let mut entry = SlopEntry::new(
            SlopBucket::StaleDocs,
            SlopSeverity::Medium,
            SlopConfidence::High,
            "Outdated README".into(),
            "The README references removed flags.".into(),
        );
        entry.source_links = vec!["README.md:42".into()];
        let _ = ledger.append(vec![entry]);

        let md = ledger.export_markdown(Some("Test Export"), None);
        assert!(md.contains("Test Export"));
        assert!(md.contains("stale_docs"));
        assert!(md.contains("Outdated README"));
        assert!(md.contains("README.md:42"));
    }

    #[test]
    fn empty_ledger_loads() {
        let (_tmp, ledger) = temp_ledger();
        assert!(ledger.is_empty());
        assert_eq!(ledger.len(), 0);
    }

    #[test]
    fn summary_counts() {
        let (_tmp, mut ledger) = temp_ledger();

        let mut e1 = SlopEntry::new(
            SlopBucket::StaleDocs,
            SlopSeverity::Medium,
            SlopConfidence::High,
            "doc".into(),
            "desc".into(),
        );
        e1.status = SlopEntryStatus::Open;

        let mut e2 = SlopEntry::new(
            SlopBucket::ToolGaps,
            SlopSeverity::High,
            SlopConfidence::Certain,
            "gap".into(),
            "desc".into(),
        );
        e2.status = SlopEntryStatus::Resolved;

        let mut e3 = SlopEntry::new(
            SlopBucket::AcceptedDebt,
            SlopSeverity::Low,
            SlopConfidence::Medium,
            "debt".into(),
            "desc".into(),
        );
        e3.status = SlopEntryStatus::Accepted;

        let _ = ledger.append(vec![e1, e2, e3]);

        let summary = ledger.summary();
        assert!(summary.contains("3 total"));
        assert!(summary.contains("stale_docs: 1"));
        assert!(summary.contains("tool_gaps: 1"));
        assert!(summary.contains("accepted_debt: 1"));
    }
}
