//! Runtime thread persistence: the thread/turn/item/event record
//! schema (serde shapes + schema version), the record-sort and usage
//! append helpers, and `RuntimeThreadStore` — the on-disk store with
//! its transactional event append and cursor reconciliation.
//!
//! Extracted verbatim from `runtime_threads.rs` (#5586). Free items and
//! impl methods the manager layer (still in the parent) calls are
//! `pub(super)`; the parent glob re-exports the public record types so
//! every `crate::runtime_threads::<Type>` path keeps resolving.

use super::*;

pub(super) fn sort_turn_items_by_start(items: &mut [TurnItemRecord]) {
    let fallback = Utc::now();
    items.sort_by(|a, b| {
        let left = a.started_at.unwrap_or(fallback);
        let right = b.started_at.unwrap_or(fallback);
        left.cmp(&right)
    });
}

/// Bumped to 2 for v0.6.6 after live engine semantics changed. The persisted
/// thread/turn/item records did not change shape, but a v1 reader on a v2
/// session should still fail closed rather than silently mis-replay.
pub(super) const CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 2;

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn serialize_route_label_option<S>(
    value: &Option<String>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    value
        .as_deref()
        .map(crate::cost_status::sanitize_persisted_route_label)
        .serialize(serializer)
}

fn serialize_endpoint_fingerprint_option<S>(
    value: &Option<String>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    value
        .as_deref()
        .filter(|fingerprint| {
            fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map(str::to_ascii_lowercase)
        .serialize(serializer)
}

fn serialize_routed_usage_source_ids<S>(
    values: &[String],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    values
        .iter()
        .map(|value| {
            if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                value.to_ascii_lowercase()
            } else {
                codewhale_config::catalog::base_url_fingerprint(value)
            }
        })
        .collect::<Vec<_>>()
        .serialize(serializer)
}
pub(super) const RUNTIME_RESTART_REASON: &str = "Interrupted by process restart";
pub(super) const EMPTY_TURN_REASON: &str = "Turn completed without engine output";
const APPROVAL_DECISION_TIMEOUT: Duration = Duration::from_secs(300);
const DYNAMIC_TOOL_RESULT_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(test)]
static TEST_APPROVAL_DECISION_TIMEOUT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
static TEST_DYNAMIC_TOOL_RESULT_TIMEOUT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(super) fn approval_decision_timeout() -> Duration {
    #[cfg(test)]
    {
        let ms = TEST_APPROVAL_DECISION_TIMEOUT_MS.load(std::sync::atomic::Ordering::SeqCst);
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    APPROVAL_DECISION_TIMEOUT
}

pub(super) fn dynamic_tool_result_timeout() -> Duration {
    #[cfg(test)]
    {
        let ms = TEST_DYNAMIC_TOOL_RESULT_TIMEOUT_MS.load(std::sync::atomic::Ordering::SeqCst);
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    DYNAMIC_TOOL_RESULT_TIMEOUT
}

#[cfg(test)]
pub(crate) fn set_test_approval_decision_timeout_ms(ms: u64) -> u64 {
    TEST_APPROVAL_DECISION_TIMEOUT_MS.swap(ms, std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
pub(crate) fn set_test_dynamic_tool_result_timeout_ms(ms: u64) -> u64 {
    TEST_DYNAMIC_TOOL_RESULT_TIMEOUT_MS.swap(ms, std::sync::atomic::Ordering::SeqCst)
}

const fn default_runtime_schema_version() -> u32 {
    CURRENT_RUNTIME_SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTurnStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Interrupted,
    Canceled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnItemKind {
    UserMessage,
    AgentMessage,
    AgentReasoning,
    ToolCall,
    FileChange,
    CommandExecution,
    ContextCompaction,
    Status,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnItemLifecycleStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Interrupted,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadRecord {
    #[serde(default = "default_runtime_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    /// Generic provider kind for this thread's model route. Named custom
    /// routes remain `custom` for compatibility with enum-only consumers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// Exact non-secret configured provider key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_id: Option<String>,
    /// Optional thread-level reasoning preference. A turn may override this;
    /// when absent, the Runtime falls back to the configured preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Optional thread-level model-visible tool allowlist. `None` keeps the
    /// normal configured tool catalog; `Some([])` deliberately exposes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    pub workspace: PathBuf,
    pub mode: String,
    /// Named default permission posture for new turns. Absent on legacy
    /// records, whose effective posture is derived from the old fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_posture: Option<String>,
    pub allow_shell: bool,
    pub trust_mode: bool,
    pub auto_approve: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_response_bookmark: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// User-set title for the thread. When `None`, consumers fall back to a
    /// derived title (typically the latest turn's input summary). Added in
    /// v0.8.10 (#562); old runtime records simply have no `title` and behave
    /// as before. Schema version is not bumped because this field is purely
    /// additive metadata — older readers ignore it without misinterpretation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The session ID associated with this thread. When set, `ensure_engine_loaded`
    /// loads the full message history (including thinking/tool blocks) from the
    /// session file instead of reconstructing from turns (which loses process info).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

pub(super) fn thread_execution_state_matches(left: &ThreadRecord, right: &ThreadRecord) -> bool {
    left.schema_version == right.schema_version
        && left.id == right.id
        && left.model == right.model
        && left.model_provider == right.model_provider
        && left.model_provider_id == right.model_provider_id
        && left.reasoning_effort == right.reasoning_effort
        && left.allowed_tools == right.allowed_tools
        && left.workspace == right.workspace
        && left.mode == right.mode
        && left.permission_posture == right.permission_posture
        && left.allow_shell == right.allow_shell
        && left.trust_mode == right.trust_mode
        && left.auto_approve == right.auto_approve
        && left.latest_turn_id == right.latest_turn_id
        && left.latest_response_bookmark == right.latest_response_bookmark
        && left.archived == right.archived
        && left.system_prompt == right.system_prompt
        && left.task_id == right.task_id
        && left.session_id == right.session_id
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    #[serde(default = "default_runtime_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub thread_id: String,
    pub status: RuntimeTurnStatus,
    pub input_summary: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Canonical posture that governed this turn. New records always carry
    /// this receipt; old records deserialize with no fabricated value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_posture: Option<String>,
    /// Concrete generic provider kind selected for this turn.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_route_label_option"
    )]
    pub effective_provider: Option<String>,
    /// Exact non-secret configured provider key selected for this turn.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_route_label_option"
    )]
    pub effective_provider_id: Option<String>,
    /// Non-secret discriminator for routes whose provider/model pair spans
    /// different billing systems (for example StepFun PAYG vs Step Plan).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_route_label_option"
    )]
    pub effective_billing_surface: Option<String>,
    /// SHA-256 fingerprint of the concrete dispatch endpoint. Raw URLs are
    /// intentionally never persisted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_endpoint_fingerprint_option"
    )]
    pub effective_endpoint_fingerprint: Option<String>,
    /// Immutable billing classification captured before dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_billing_mode: Option<RouteBillingMode>,
    /// Dispatch timestamp used for historical/live pricing lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_dispatched_at: Option<DateTime<Utc>>,
    /// Concrete wire model selected for this turn (especially important when
    /// the thread is configured as `auto`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_route_label_option"
    )]
    pub effective_model: Option<String>,
    /// Model calls made beneath this parent turn, each paired with its own
    /// immutable route. These are exclusive of `usage`, which is only the
    /// parent engine turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routed_usage: Vec<EffectiveRouteUsage>,
    /// Fingerprints of provider-call identities already appended to this turn.
    /// This durable ledger makes mailbox delivery, direct sinks, fallback
    /// recovery, and process restart idempotent without persisting raw ids.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_routed_usage_source_ids"
    )]
    pub routed_usage_source_ids: Vec<String>,
    /// Background provider calls discarded from the bounded fallback journal.
    /// Non-zero means token/cost aggregation is necessarily incomplete.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub routed_usage_dropped_records: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub item_ids: Vec<String>,
    #[serde(default)]
    pub steer_count: usize,
    /// Stable Agent Mail id that caused this turn. This is the durable
    /// idempotency bridge between a claimed mail envelope and the existing
    /// turn queue; ordinary external-user turns leave it unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_mail_message_id: Option<String>,
}

impl TurnRecord {
    pub(crate) fn effective_provider_label(&self) -> Option<&str> {
        self.effective_provider_id
            .as_deref()
            .filter(|identity| !identity.trim().is_empty())
            .or_else(|| {
                self.effective_provider
                    .as_deref()
                    .filter(|provider| !provider.trim().is_empty())
            })
    }

    pub(super) fn persist_effective_route(&mut self, route: &EffectiveRouteEnvelope) {
        let route = route.sanitized_for_persistence();
        self.effective_provider = Some(route.provider.as_str().to_string());
        self.effective_provider_id = Some(route.provider_identity);
        self.effective_billing_surface = route.billing_surface;
        self.effective_endpoint_fingerprint = route.endpoint_fingerprint;
        self.effective_billing_mode = Some(route.billing_mode);
        self.effective_dispatched_at = Some(route.dispatched_at);
        self.effective_model = Some(route.model);
    }

    /// Rehydrate only a complete persisted dispatch record. Legacy rows must
    /// not borrow a provider identity or timestamp from the current thread.
    pub(super) fn effective_route_envelope(&self) -> Option<EffectiveRouteEnvelope> {
        let provider = self
            .effective_provider
            .as_deref()
            .and_then(ApiProvider::parse)?;
        let provider_identity = self
            .effective_provider_id
            .as_deref()
            .filter(|identity| !identity.trim().is_empty())?
            .to_string();
        let model = self
            .effective_model
            .as_deref()
            .filter(|model| !model.trim().is_empty())?
            .to_string();
        let dispatched_at = self.effective_dispatched_at?;
        Some(
            EffectiveRouteEnvelope {
                provider,
                provider_identity,
                model,
                billing_surface: self.effective_billing_surface.clone(),
                endpoint_fingerprint: self.effective_endpoint_fingerprint.clone(),
                billing_mode: self
                    .effective_billing_mode
                    .unwrap_or(RouteBillingMode::Unknown),
                dispatched_at,
            }
            .sanitized_for_persistence(),
        )
    }
}

/// The only mutation path for routed provider usage. Every source is recorded
/// once, route labels are sanitized at the boundary, and retained records are
/// bounded regardless of whether they arrived synchronously, by mailbox, or
/// from the fallback journal.
pub(super) fn append_routed_usage_record(
    turn: &mut TurnRecord,
    source_id: &str,
    usage: EffectiveRouteUsage,
) -> bool {
    let source_fingerprint = crate::cost_status::usage_source_fingerprint(source_id);
    if turn
        .routed_usage_source_ids
        .iter()
        .any(|persisted| persisted == &source_fingerprint)
    {
        return false;
    }
    turn.routed_usage_source_ids.push(source_fingerprint);
    if turn.routed_usage.len() == MAX_ROUTED_USAGE_RECORDS_PER_TURN {
        turn.routed_usage.remove(0);
        turn.routed_usage_dropped_records = turn.routed_usage_dropped_records.saturating_add(1);
    }
    turn.routed_usage.push(EffectiveRouteUsage {
        route: usage.route.sanitized_for_persistence(),
        usage: usage.usage,
    });
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnItemRecord {
    #[serde(default = "default_runtime_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub turn_id: String,
    pub kind: TurnItemKind,
    pub status: TurnItemLifecycleStatus,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub artifact_refs: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEventRecord {
    #[serde(default = "default_runtime_schema_version")]
    pub schema_version: u32,
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub event: String,
    pub payload: Value,
}

pub(crate) struct RuntimeEventReplay {
    /// Cursor immediately before the first replayed event. For a tail-limited
    /// replay this advances past omitted history so continuity remains exact.
    pub(crate) base_seq: u64,
    /// Filesystem parsing happens on the blocking pool and publishes bounded
    /// chunks through this small channel, applying backpressure instead of
    /// allocating an unbounded backlog on a Tokio worker.
    pub(crate) batches: mpsc::Receiver<std::result::Result<Vec<RuntimeEventRecord>, String>>,
}

type RuntimeEventReader = BufReader<std::io::Take<File>>;

pub(super) enum RuntimeEventMatch {
    TurnCompleted {
        turn_id: String,
    },
    DynamicTerminal {
        turn_id: String,
        call_id: String,
    },
    AgentMail {
        event_name: String,
        message_id: String,
        attempt_count: u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStoreState {
    #[serde(default = "default_runtime_schema_version")]
    pub(super) schema_version: u32,
    pub(super) next_seq: u64,
}

impl Default for RuntimeStoreState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            next_seq: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EventAppendFailureDisposition {
    RolledBack,
    Indeterminate,
}

#[derive(Debug)]
pub(super) struct RuntimeEventAppendError {
    disposition: EventAppendFailureDisposition,
    append_error: String,
    rollback_error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("Runtime event lock timed out after {0:?}")]
pub(super) struct RuntimeEventLockTimeout(pub(super) Duration);

impl RuntimeEventAppendError {
    pub(super) const fn retry_safe(&self) -> bool {
        matches!(self.disposition, EventAppendFailureDisposition::RolledBack)
    }
}

impl std::fmt::Display for RuntimeEventAppendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.rollback_error {
            Some(rollback_error) => write!(
                formatter,
                "Runtime event append is indeterminate after append error ({}) and rollback error ({})",
                self.append_error, rollback_error
            ),
            None => write!(
                formatter,
                "Runtime event append failed and was rolled back: {}",
                self.append_error
            ),
        }
    }
}

impl std::error::Error for RuntimeEventAppendError {}

pub(super) fn event_append_is_indeterminate(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        source
            .downcast_ref::<RuntimeEventAppendError>()
            .is_some_and(|append| !append.retry_safe())
    })
}

#[derive(Debug, Clone)]
pub struct RuntimeThreadStore {
    pub(super) threads_dir: PathBuf,
    pub(super) turns_dir: PathBuf,
    pub(super) items_dir: PathBuf,
    pub(super) events_dir: PathBuf,
    pub(super) goals_dir: PathBuf,
    pub(super) mail_dir: PathBuf,
    pub(super) turn_operations_dir: PathBuf,
    pub(super) owner_id: String,
    pub(super) state_path: PathBuf,
    pub(super) event_lock_path: PathBuf,
    /// Serializes load-modify-save operations on thread records. The guard is
    /// synchronous and must never cross an `.await`; JSON records are small,
    /// and one global guard avoids per-thread lock lifecycle races.
    pub(super) thread_mutation: Arc<parking_lot::Mutex<()>>,
    /// Serializes load-modify-save operations on turn records. Like the
    /// thread guard, it is synchronous and never crosses an `.await`.
    pub(super) turn_mutation: Arc<parking_lot::Mutex<()>>,
    /// Serializes envelope claim/state transitions. The durable envelope is
    /// the queue; this guard prevents concurrent replay/wake requests from
    /// starting more than one turn for the same message.
    pub(super) mail_mutation: Arc<parking_lot::Mutex<()>>,
    /// Files read by whole-directory turn scans (`list_all_turns`). Shared
    /// across store clones so a `spawn_blocking` snapshot still counts against
    /// the manager the test holds. Per-store so parallel tests do not collide.
    #[cfg(test)]
    pub(super) turn_dir_files_read: Arc<std::sync::atomic::AtomicU64>,
    /// Files read by whole-directory item scans (`list_items_for_turn` and
    /// `list_items_for_turns_map`).
    #[cfg(test)]
    pub(super) item_dir_files_read: Arc<std::sync::atomic::AtomicU64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RuntimeStoreOwner {
    pub(super) owner_id: String,
}

impl RuntimeThreadStore {
    pub fn open(root: PathBuf) -> Result<Self> {
        let root = checked_runtime_store_root(root)?;
        ensure_runtime_store_dir(&root)?;
        let threads_dir = root.join("threads");
        let turns_dir = root.join("turns");
        let items_dir = root.join("items");
        let events_dir = root.join("events");
        let goals_dir = root.join("goals");
        let mail_dir = root.join("agent-mail");
        let turn_operations_dir = root.join("turn-operations");
        ensure_runtime_store_dir(&threads_dir)?;
        ensure_runtime_store_dir(&turns_dir)?;
        ensure_runtime_store_dir(&items_dir)?;
        ensure_runtime_store_dir(&events_dir)?;
        ensure_runtime_store_dir(&goals_dir)?;
        ensure_runtime_store_dir(&mail_dir)?;
        ensure_runtime_store_dir(&turn_operations_dir)?;
        let state_path = root.join("state.json");
        let owner_path = root.join(AGENT_MAIL_OWNER_FILE);
        let event_lock_path = root.join(EVENT_TRANSACTION_LOCK_FILE);
        // The owner namespaces operation-key fingerprints. Creating it outside
        // a cross-process transaction lets two first-start processes mint
        // different owners, and therefore different operation locks, for the
        // same store. Reuse the root event lock before any owner-derived path
        // is computed so all processes load exactly one durable owner.
        let owner_id = load_or_create_runtime_store_owner(&owner_path, &event_lock_path)?;
        let store = Self {
            threads_dir,
            turns_dir,
            items_dir,
            events_dir,
            goals_dir,
            mail_dir,
            turn_operations_dir,
            owner_id,
            state_path,
            event_lock_path,
            thread_mutation: Arc::new(parking_lot::Mutex::new(())),
            turn_mutation: Arc::new(parking_lot::Mutex::new(())),
            mail_mutation: Arc::new(parking_lot::Mutex::new(())),
            #[cfg(test)]
            turn_dir_files_read: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(test)]
            item_dir_files_read: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        store.with_event_transaction(EVENT_TRANSACTION_LOCK_TIMEOUT, || {
            repair_torn_event_log_tails(&store.events_dir)?;
            if store.state_path.exists() {
                load_runtime_store_state(&store.state_path)?;
            } else {
                write_json_atomic(&store.state_path, &RuntimeStoreState::default())?;
            }
            Ok(())
        })?;
        store.recover_incomplete_turn_operations()?;
        store.recover_claimed_agent_mail()?;
        Ok(store)
    }

    pub(super) fn open_event_lock(&self) -> Result<File> {
        let file =
            open_runtime_store_file(&self.event_lock_path, "Runtime event lock", |options| {
                options.create(true).truncate(false).read(true).write(true);
            })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .context("Failed to secure Runtime event lock")?;
        }
        Ok(file)
    }

    pub(super) fn with_event_transaction<T>(
        &self,
        timeout: Duration,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let mut lock = fd_lock::RwLock::new(self.open_event_lock()?);
        let started = Instant::now();
        let mut operation = Some(operation);
        loop {
            match lock
                .try_write()
                .map(|_guard| operation.take().expect("event transaction runs once")())
            {
                Ok(result) => return result,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    wait_for_event_lock(started, timeout)?;
                }
                Err(error) => return Err(error).context("Failed to lock Runtime events"),
            }
        }
    }

    pub(super) fn record_path(
        base: &Path,
        id: &str,
        extension: &str,
        label: &str,
    ) -> Result<PathBuf> {
        let id = validated_record_id(id, label)?;
        Ok(base.join(format!("{id}.{extension}")))
    }

    pub(super) fn thread_path(&self, thread_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.threads_dir, thread_id, "json", "thread id")
    }

    pub(super) fn turn_path(&self, turn_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.turns_dir, turn_id, "json", "turn id")
    }

    pub(super) fn item_path(&self, item_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.items_dir, item_id, "json", "item id")
    }

    pub(super) fn events_path(&self, thread_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.events_dir, thread_id, "jsonl", "thread id")
    }

    pub(super) fn goal_path(&self, thread_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.goals_dir, thread_id, "json", "thread id")
    }

    pub(super) fn mail_path(&self, message_id: &AgentMailMessageId) -> Result<PathBuf> {
        Self::record_path(
            &self.mail_dir,
            message_id.as_str(),
            "json",
            "Agent Mail message id",
        )
    }

    pub(super) fn turn_operation_path(&self, operation_key_fingerprint: &str) -> Result<PathBuf> {
        validate_sha256_fingerprint(operation_key_fingerprint, "operation key fingerprint")?;
        Self::record_path(
            &self.turn_operations_dir,
            &format!("op_{operation_key_fingerprint}"),
            "json",
            "turn operation binding id",
        )
    }

    pub(super) fn turn_operation_lock_path(
        &self,
        operation_key_fingerprint: &str,
    ) -> Result<PathBuf> {
        validate_sha256_fingerprint(operation_key_fingerprint, "operation key fingerprint")?;
        Self::record_path(
            &self.turn_operations_dir,
            &format!("op_{operation_key_fingerprint}"),
            "lock",
            "turn operation claim lock id",
        )
    }

    pub(super) fn open_turn_operation_claim_lock(
        &self,
        operation_key_fingerprint: &str,
    ) -> Result<File> {
        let path = self.turn_operation_lock_path(operation_key_fingerprint)?;
        open_runtime_store_file(&path, "Runtime turn operation claim lock", |options| {
            options.create(true).truncate(false).read(true).write(true);
        })
    }

    pub(super) fn with_turn_operation_claim<T>(
        &self,
        operation_key_fingerprint: Option<&str>,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let Some(operation_key_fingerprint) = operation_key_fingerprint else {
            return operation();
        };
        let mut claim =
            fd_lock::RwLock::new(self.open_turn_operation_claim_lock(operation_key_fingerprint)?);
        let _guard = self.acquire_turn_operation_claim(&mut claim)?;
        operation()
    }

    pub(super) fn acquire_turn_operation_claim<'a>(
        &self,
        claim: &'a mut fd_lock::RwLock<File>,
    ) -> Result<fd_lock::RwLockWriteGuard<'a, File>> {
        match claim.try_write() {
            Ok(guard) => Ok(guard),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                bail!("Runtime turn operation is already being claimed; retry")
            }
            Err(error) => Err(error).context("Failed to claim Runtime turn operation"),
        }
    }

    /// Remove a binding left before its turn record by a process crash.
    ///
    /// Bindings are committed before turns, while engine submission happens
    /// only after both are durable. A binding with no turn therefore never
    /// reached the engine and is safe to discard during startup recovery.
    pub(super) fn recover_incomplete_turn_operations(&self) -> Result<()> {
        let operations_dir = checked_existing_runtime_store_dir(&self.turn_operations_dir)?;
        for entry in fs::read_dir(&operations_dir)
            .with_context(|| format!("Failed to read {}", operations_dir.display()))?
        {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let raw = read_store_file(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let observed: RuntimeTurnOperationBinding = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            observed.validate()?;
            self.with_turn_operation_claim(Some(&observed.operation_key_fingerprint), || {
                // A live writer may have replaced the file between the
                // directory scan and this claim. Re-read under the same
                // cross-process lock used by `start_turn` before deciding
                // that the binding is torn.
                let Some(binding) =
                    self.load_turn_operation_binding(&observed.operation_key_fingerprint)?
                else {
                    return Ok(());
                };
                if !self.turn_path(&binding.turn_id)?.exists() {
                    // Persistence is binding -> item -> turn. A process can
                    // stop after the item write but before the turn commit;
                    // that item was never submitted to an engine and has no
                    // authoritative parent. Remove it under the same operation
                    // claim before making the key retryable.
                    for item in self.list_items_for_turn(&binding.turn_id)? {
                        self.remove_item(&item.id)?;
                    }
                    remove_file_if_exists(&path)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    pub(super) fn save_turn_operation_binding(
        &self,
        binding: &RuntimeTurnOperationBinding,
    ) -> Result<()> {
        binding.validate()?;
        write_json_atomic(
            &self.turn_operation_path(&binding.operation_key_fingerprint)?,
            binding,
        )
    }

    pub(super) fn load_turn_operation_binding(
        &self,
        operation_key_fingerprint: &str,
    ) -> Result<Option<RuntimeTurnOperationBinding>> {
        let path = self.turn_operation_path(operation_key_fingerprint)?;
        if !path.exists() {
            return Ok(None);
        }
        let raw = read_store_file(&path)
            .with_context(|| format!("Failed to read Runtime turn operation {}", path.display()))?;
        let binding: RuntimeTurnOperationBinding =
            serde_json::from_str(&raw).with_context(|| {
                format!("Failed to parse Runtime turn operation {}", path.display())
            })?;
        binding.validate()?;
        Ok(Some(binding))
    }

    pub(super) fn remove_turn_operation_binding(
        &self,
        operation_key_fingerprint: &str,
    ) -> Result<()> {
        remove_file_if_exists(&self.turn_operation_path(operation_key_fingerprint)?)
    }

    pub(super) fn recover_claimed_agent_mail(&self) -> Result<()> {
        let _mail_mutation = self.mail_mutation.lock();
        for mut mail in self.list_agent_mail()? {
            if mail.status != AgentMailStatus::Delivering {
                continue;
            }
            mail.status = AgentMailStatus::Failed;
            mail.failure = Some(AgentMailFailureReceipt {
                code: AgentMailFailureCode::DeliveryRejected,
                message: "Delivery claim recovered after runtime restart".to_string(),
                retryable: true,
                failed_at: Utc::now(),
            });
            self.save_agent_mail(&mail)?;
        }
        Ok(())
    }

    pub(super) fn save_agent_mail(&self, mail: &AgentMailEnvelope) -> Result<()> {
        mail.validate().map_err(|error| anyhow!(error))?;
        write_json_atomic(&self.mail_path(&mail.message_id)?, mail)
    }

    pub(super) fn load_agent_mail(
        &self,
        message_id: &AgentMailMessageId,
    ) -> Result<AgentMailEnvelope> {
        let path = self.mail_path(message_id)?;
        let raw = read_store_file(&path)
            .with_context(|| format!("Failed to read Agent Mail envelope {}", path.display()))?;
        let mail: AgentMailEnvelope = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse Agent Mail envelope {}", path.display()))?;
        mail.validate().map_err(|error| anyhow!(error))?;
        Ok(mail)
    }

    pub(super) fn list_agent_mail(&self) -> Result<Vec<AgentMailEnvelope>> {
        let mut out = Vec::new();
        let mail_dir = checked_existing_runtime_store_dir(&self.mail_dir)?;
        for entry in fs::read_dir(&mail_dir)
            .with_context(|| format!("Failed to read {}", mail_dir.display()))?
        {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let raw = read_store_file(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let mail: AgentMailEnvelope = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            mail.validate().map_err(|error| anyhow!(error))?;
            out.push(mail);
        }
        out.sort_by_key(|mail| mail.created_at);
        Ok(out)
    }

    /// Persist a goal record for a thread. The goal is stored as a JSON file
    /// in the `goals/` subdirectory; it is independent of the TUI state store
    /// and requires only that the runtime thread exists.
    pub fn save_goal(&self, goal: &codewhale_protocol::ThreadGoal) -> Result<()> {
        write_json_atomic(&self.goal_path(&goal.thread_id)?, goal)
    }

    /// Load the goal for a thread, returning `None` if no goal has been set.
    pub fn load_goal(&self, thread_id: &str) -> Result<Option<codewhale_protocol::ThreadGoal>> {
        let path = self.goal_path(thread_id)?;
        if !path.exists() {
            return Ok(None);
        }
        let raw = read_store_file(&path)
            .with_context(|| format!("Failed to read goal {}", path.display()))?;
        let goal: codewhale_protocol::ThreadGoal = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse goal {}", path.display()))?;
        Ok(Some(goal))
    }

    /// Remove the goal for a thread, returning `true` if one existed.
    pub fn delete_goal(&self, thread_id: &str) -> Result<bool> {
        let path = self.goal_path(thread_id)?;
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path)
            .with_context(|| format!("Failed to delete goal {}", path.display()))?;
        Ok(true)
    }

    pub fn save_thread(&self, thread: &ThreadRecord) -> Result<()> {
        write_json_atomic(&self.thread_path(&thread.id)?, thread)
    }

    pub fn save_turn(&self, turn: &TurnRecord) -> Result<()> {
        validated_record_id(&turn.thread_id, "thread id")?;
        write_json_atomic(&self.turn_path(&turn.id)?, turn)
    }

    pub fn save_item(&self, item: &TurnItemRecord) -> Result<()> {
        validated_record_id(&item.turn_id, "turn id")?;
        write_json_atomic(&self.item_path(&item.id)?, item)
    }

    pub(super) fn remove_turn(&self, turn_id: &str) -> Result<()> {
        remove_file_if_exists(&self.turn_path(turn_id)?)
    }

    pub(super) fn remove_thread(&self, thread_id: &str) -> Result<()> {
        remove_file_if_exists(&self.thread_path(thread_id)?)
    }

    pub(super) fn remove_item(&self, item_id: &str) -> Result<()> {
        remove_file_if_exists(&self.item_path(item_id)?)
    }

    pub fn load_thread(&self, thread_id: &str) -> Result<ThreadRecord> {
        let path = self.thread_path(thread_id)?;
        let raw = read_store_file(&path)
            .with_context(|| format!("Failed to read thread {}", path.display()))?;
        let record: ThreadRecord = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse thread {}", path.display()))?;
        if record.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
            bail!(
                "Thread schema v{} is newer than supported v{}",
                record.schema_version,
                CURRENT_RUNTIME_SCHEMA_VERSION
            );
        }
        Ok(record)
    }

    pub fn load_turn(&self, turn_id: &str) -> Result<TurnRecord> {
        let path = self.turn_path(turn_id)?;
        let raw = read_store_file(&path)
            .with_context(|| format!("Failed to read turn {}", path.display()))?;
        let record: TurnRecord = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse turn {}", path.display()))?;
        if record.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
            bail!(
                "Turn schema v{} is newer than supported v{}",
                record.schema_version,
                CURRENT_RUNTIME_SCHEMA_VERSION
            );
        }
        Ok(record)
    }

    pub fn load_item(&self, item_id: &str) -> Result<TurnItemRecord> {
        let path = self.item_path(item_id)?;
        let raw = read_store_file(&path)
            .with_context(|| format!("Failed to read item {}", path.display()))?;
        let record: TurnItemRecord = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse item {}", path.display()))?;
        if record.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
            bail!(
                "Item schema v{} is newer than supported v{}",
                record.schema_version,
                CURRENT_RUNTIME_SCHEMA_VERSION
            );
        }
        Ok(record)
    }

    pub fn list_threads(&self) -> Result<Vec<ThreadRecord>> {
        let mut out = Vec::new();
        let threads_dir = checked_existing_runtime_store_dir(&self.threads_dir)?;
        for entry in fs::read_dir(&threads_dir)
            .with_context(|| format!("Failed to read {}", threads_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = read_store_file(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let thread: ThreadRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if thread.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
                bail!(
                    "Thread schema v{} is newer than supported v{}",
                    thread.schema_version,
                    CURRENT_RUNTIME_SCHEMA_VERSION
                );
            }
            out.push(thread);
        }
        out.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        Ok(out)
    }

    pub fn list_turns_for_thread(&self, thread_id: &str) -> Result<Vec<TurnRecord>> {
        validated_record_id(thread_id, "thread id")?;
        let mut out = self.list_all_turns()?;
        out.retain(|turn| turn.thread_id == thread_id);
        Ok(out)
    }

    /// Every turn in the store, sorted by creation time. One directory scan;
    /// callers that need multiple threads' turns (boot recovery) use this
    /// instead of paying a full scan per thread (#3757).
    pub fn list_all_turns(&self) -> Result<Vec<TurnRecord>> {
        let mut out = Vec::new();
        let turns_dir = checked_existing_runtime_store_dir(&self.turns_dir)?;
        for entry in fs::read_dir(&turns_dir)
            .with_context(|| format!("Failed to read {}", turns_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = read_store_file(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            #[cfg(test)]
            self.turn_dir_files_read
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let turn: TurnRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if turn.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
                bail!(
                    "Turn schema v{} is newer than supported v{}",
                    turn.schema_version,
                    CURRENT_RUNTIME_SCHEMA_VERSION
                );
            }
            out.push(turn);
        }
        out.sort_by_key(|a| a.created_at);
        Ok(out)
    }

    pub fn list_items_for_turn(&self, turn_id: &str) -> Result<Vec<TurnItemRecord>> {
        validated_record_id(turn_id, "turn id")?;
        let mut out = Vec::new();
        let items_dir = checked_existing_runtime_store_dir(&self.items_dir)?;
        for entry in fs::read_dir(&items_dir)
            .with_context(|| format!("Failed to read {}", items_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = read_store_file(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            #[cfg(test)]
            self.item_dir_files_read
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let item: TurnItemRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if item.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
                bail!(
                    "Item schema v{} is newer than supported v{}",
                    item.schema_version,
                    CURRENT_RUNTIME_SCHEMA_VERSION
                );
            }
            if item.turn_id == turn_id {
                out.push(item);
            }
        }
        sort_turn_items_by_start(&mut out);
        Ok(out)
    }

    pub fn list_items_for_turns_map(
        &self,
        turn_ids: &[String],
    ) -> Result<HashMap<String, Vec<TurnItemRecord>>> {
        if turn_ids.is_empty() {
            return Ok(HashMap::new());
        }

        for turn_id in turn_ids {
            validated_record_id(turn_id, "turn id")?;
        }

        let wanted: HashSet<&str> = turn_ids.iter().map(String::as_str).collect();
        let mut out: HashMap<String, Vec<TurnItemRecord>> = HashMap::new();
        let items_dir = checked_existing_runtime_store_dir(&self.items_dir)?;
        for entry in fs::read_dir(&items_dir)
            .with_context(|| format!("Failed to read {}", items_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = read_store_file(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            #[cfg(test)]
            self.item_dir_files_read
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let item: TurnItemRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if item.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
                bail!(
                    "Item schema v{} is newer than supported v{}",
                    item.schema_version,
                    CURRENT_RUNTIME_SCHEMA_VERSION
                );
            }
            if wanted.contains(item.turn_id.as_str()) {
                out.entry(item.turn_id.clone()).or_default().push(item);
            }
        }

        for items in out.values_mut() {
            sort_turn_items_by_start(items);
        }
        Ok(out)
    }

    pub async fn append_event(
        &self,
        thread_id: &str,
        turn_id: Option<&str>,
        item_id: Option<&str>,
        event: impl Into<String>,
        payload: Value,
    ) -> Result<RuntimeEventRecord> {
        validated_record_id(thread_id, "thread id")?;
        if let Some(turn_id) = turn_id {
            validated_record_id(turn_id, "turn id")?;
        }
        if let Some(item_id) = item_id {
            validated_record_id(item_id, "item id")?;
        }
        let store = self.clone();
        let thread_id = thread_id.to_string();
        let turn_id = turn_id.map(ToString::to_string);
        let item_id = item_id.map(ToString::to_string);
        let event = event.into();
        tokio::task::spawn_blocking(move || {
            store.append_event_transaction(
                thread_id,
                turn_id,
                item_id,
                event,
                payload,
                EVENT_TRANSACTION_LOCK_TIMEOUT,
            )
        })
        .await
        .context("Runtime event transaction worker failed")?
    }

    pub(super) fn append_event_transaction(
        &self,
        thread_id: String,
        turn_id: Option<String>,
        item_id: Option<String>,
        event: String,
        payload: Value,
        lock_timeout: Duration,
    ) -> Result<RuntimeEventRecord> {
        let path = self.events_path(&thread_id)?;
        self.with_event_transaction(lock_timeout, || {
            reject_symlinked_store_dir(&self.events_dir)?;
            repair_torn_event_log_tail(&path)?;
            let mut state = load_runtime_store_state(&self.state_path)?;
            let seq = state.next_seq;
            state.next_seq = seq
                .checked_add(1)
                .context("Runtime event sequence exhausted")?;
            write_json_atomic(&self.state_path, &state)?;

            let record = RuntimeEventRecord {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                seq,
                timestamp: Utc::now(),
                thread_id,
                turn_id,
                item_id,
                event,
                payload,
            };

            let mut file = open_runtime_store_file(&path, "event append", |options| {
                options.create(true).append(true);
            })?;
            let rollback_file =
                open_runtime_store_file(&path, "Runtime event rollback", |options| {
                    options.write(true);
                })?;
            validate_same_runtime_store_file_handles(&file, &rollback_file, &path)?;
            let original_len = file
                .metadata()
                .with_context(|| format!("Failed to inspect {}", path.display()))?
                .len();
            let mut line = serde_json::to_vec(&record)?;
            // A trailing newline is the commit marker. Startup removes a
            // parseable but unterminated tail without reusing its sequence.
            line.push(b'\n');
            let append_result = (|| -> std::io::Result<()> {
                file.write_all(&line)?;
                file.flush()?;
                #[cfg(test)]
                if take_test_event_append_fault(&record.thread_id, EventAppendTestFault::AfterFlush)
                {
                    return Err(std::io::Error::other(
                        "injected Runtime event failure after flush",
                    ));
                }
                file.sync_all()?;
                #[cfg(test)]
                if take_test_event_append_fault(&record.thread_id, EventAppendTestFault::AfterSync)
                {
                    return Err(std::io::Error::other(
                        "injected Runtime event failure after fsync",
                    ));
                }
                Ok(())
            })();
            if let Err(append_error) = append_result {
                // A failed flush/fsync can still leave the complete JSONL record
                // visible (or even durable). Roll back to the exact pre-append
                // offset and fsync that truncation before reporting a retryable
                // error. If rollback itself fails, classify the write as
                // indeterminate so callers never restore/retry and duplicate a
                // possibly committed terminal receipt.
                // The pre-opened rollback handle was identity-checked before
                // any bytes were written and stays live across this transaction.
                drop(file);
                let rollback_result =
                    rollback_failed_event_append_handle(&rollback_file, original_len);
                let error = match rollback_result {
                    Ok(()) => RuntimeEventAppendError {
                        disposition: EventAppendFailureDisposition::RolledBack,
                        append_error: append_error.to_string(),
                        rollback_error: None,
                    },
                    Err(rollback_error) => RuntimeEventAppendError {
                        disposition: EventAppendFailureDisposition::Indeterminate,
                        append_error: append_error.to_string(),
                        rollback_error: Some(rollback_error.to_string()),
                    },
                };
                return Err(anyhow!(error));
            }
            Ok(record)
        })
    }

    pub fn events_since(
        &self,
        thread_id: &str,
        since_seq: Option<u64>,
    ) -> Result<Vec<RuntimeEventRecord>> {
        let path = self.events_path(thread_id)?;
        let Some(mut reader) = self.open_event_reader(thread_id)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        while let Some(event) = read_complete_event(&mut reader, &path)? {
            if let Some(since) = since_seq
                && event.seq <= since
            {
                continue;
            }
            out.push(event);
        }
        Ok(out)
    }

    /// Incremental JSONL replay from a byte cursor. The returned cursor only
    /// advances past complete newline-terminated records so a live tail can
    /// be retried without rereading earlier history.
    pub fn events_from_offset(
        &self,
        thread_id: &str,
        offset: u64,
        limit: Option<usize>,
    ) -> Result<(Vec<RuntimeEventRecord>, u64)> {
        let path = self.events_path(thread_id)?;
        self.with_event_transaction(EVENT_TRANSACTION_LOCK_TIMEOUT, || {
            reject_symlinked_store_dir(&self.events_dir)?;
            if !path.exists() {
                return Ok((Vec::new(), offset));
            }
            let mut file =
                open_runtime_store_file(&path, "Runtime event cursor replay", |options| {
                    options.read(true);
                })?;
            let committed_len = file
                .metadata()
                .with_context(|| format!("Failed to inspect {}", path.display()))?
                .len();
            let start = offset.min(committed_len);
            file.seek(SeekFrom::Start(start))?;
            let mut reader = BufReader::new(file.take(committed_len.saturating_sub(start)));
            let mut out = Vec::new();
            let mut cursor = start;
            while let Some((event, consumed)) = read_complete_event_bytes(&mut reader, &path)? {
                cursor += consumed;
                out.push(event);
                if limit.is_some_and(|limit| out.len() >= limit) {
                    break;
                }
            }
            Ok((out, cursor))
        })
    }

    pub(super) fn publish_event_replay(
        &self,
        thread_id: &str,
        since_seq: Option<u64>,
        tail_limit: Option<usize>,
        base_tx: oneshot::Sender<std::result::Result<u64, String>>,
        batch_tx: mpsc::Sender<std::result::Result<Vec<RuntimeEventRecord>, String>>,
    ) {
        let mut base_tx = Some(base_tx);
        let result = match tail_limit {
            Some(limit) => {
                self.publish_tail_event_replay(thread_id, since_seq, limit, &mut base_tx, &batch_tx)
            }
            None => self.publish_full_event_replay(thread_id, since_seq, &mut base_tx, &batch_tx),
        };
        if let Err(error) = result {
            let message = format!("{error:#}");
            if let Some(base_tx) = base_tx.take() {
                let _ = base_tx.send(Err(message));
            } else {
                let _ = batch_tx.blocking_send(Err(message));
            }
        }
    }

    pub(super) fn open_event_reader(&self, thread_id: &str) -> Result<Option<RuntimeEventReader>> {
        let path = self.events_path(thread_id)?;
        self.with_event_transaction(EVENT_TRANSACTION_LOCK_TIMEOUT, || {
            reject_symlinked_store_dir(&self.events_dir)?;
            if !path.exists() {
                return Ok(None);
            }
            let file = open_runtime_store_file(&path, "Runtime event replay", |options| {
                options.read(true);
            })?;
            let committed_len = file
                .metadata()
                .with_context(|| format!("Failed to inspect {}", path.display()))?
                .len();
            Ok(Some(BufReader::new(file.take(committed_len))))
        })
    }

    pub(super) fn contains_event(
        &self,
        thread_id: &str,
        expected: &RuntimeEventMatch,
    ) -> Result<bool> {
        let Some(mut reader) = self.open_event_reader(thread_id)? else {
            return Ok(false);
        };
        let path = self.events_path(thread_id)?;
        while let Some(event) = read_complete_event(&mut reader, &path)? {
            let matches = match expected {
                RuntimeEventMatch::TurnCompleted { turn_id } => {
                    event.event == "turn.completed"
                        && event.turn_id.as_deref() == Some(turn_id.as_str())
                }
                RuntimeEventMatch::DynamicTerminal { turn_id, call_id } => {
                    matches!(
                        event.event.as_str(),
                        "tool_call.resolved" | "tool_call.canceled" | "tool_call.timeout"
                    ) && event.turn_id.as_deref() == Some(turn_id.as_str())
                        && event.payload.get("call_id").and_then(Value::as_str)
                            == Some(call_id.as_str())
                }
                RuntimeEventMatch::AgentMail {
                    event_name,
                    message_id,
                    attempt_count,
                } => {
                    event.event == *event_name
                        && event
                            .payload
                            .get("mail")
                            .and_then(|mail| mail.get("message_id"))
                            .and_then(Value::as_str)
                            == Some(message_id.as_str())
                        && event
                            .payload
                            .get("mail")
                            .and_then(|mail| mail.get("attempt_count"))
                            .and_then(Value::as_u64)
                            == Some(*attempt_count as u64)
                }
            };
            if matches {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn publish_full_event_replay(
        &self,
        thread_id: &str,
        since_seq: Option<u64>,
        base_tx: &mut Option<oneshot::Sender<std::result::Result<u64, String>>>,
        batch_tx: &mpsc::Sender<std::result::Result<Vec<RuntimeEventRecord>, String>>,
    ) -> Result<()> {
        let Some(mut reader) = self.open_event_reader(thread_id)? else {
            if let Some(base_tx) = base_tx.take() {
                let _ = base_tx.send(Ok(since_seq.unwrap_or(0)));
            }
            return Ok(());
        };
        if base_tx
            .take()
            .is_some_and(|base_tx| base_tx.send(Ok(since_seq.unwrap_or(0))).is_err())
        {
            return Ok(());
        }

        let path = self.events_path(thread_id)?;
        let mut batch = Vec::with_capacity(RUNTIME_EVENT_REPLAY_BATCH_SIZE);
        while let Some(event) = read_complete_event(&mut reader, &path)? {
            if since_seq.is_some_and(|since| event.seq <= since) {
                continue;
            }
            batch.push(event);
            if batch.len() == RUNTIME_EVENT_REPLAY_BATCH_SIZE {
                if batch_tx.blocking_send(Ok(batch)).is_err() {
                    return Ok(());
                }
                batch = Vec::with_capacity(RUNTIME_EVENT_REPLAY_BATCH_SIZE);
            }
        }
        if !batch.is_empty() {
            let _ = batch_tx.blocking_send(Ok(batch));
        }
        Ok(())
    }

    pub(super) fn publish_tail_event_replay(
        &self,
        thread_id: &str,
        since_seq: Option<u64>,
        tail_limit: usize,
        base_tx: &mut Option<oneshot::Sender<std::result::Result<u64, String>>>,
        batch_tx: &mpsc::Sender<std::result::Result<Vec<RuntimeEventRecord>, String>>,
    ) -> Result<()> {
        let Some(mut reader) = self.open_event_reader(thread_id)? else {
            if let Some(base_tx) = base_tx.take() {
                let _ = base_tx.send(Ok(since_seq.unwrap_or(0)));
            }
            return Ok(());
        };
        let path = self.events_path(thread_id)?;
        let mut base_seq = since_seq.unwrap_or(0);
        let mut tail = VecDeque::with_capacity(tail_limit.min(RUNTIME_EVENT_REPLAY_BATCH_SIZE));
        while let Some(event) = read_complete_event(&mut reader, &path)? {
            if since_seq.is_some_and(|since| event.seq <= since) {
                continue;
            }
            if tail_limit == 0 {
                base_seq = event.seq;
                continue;
            }
            tail.push_back(event);
            if tail.len() > tail_limit
                && let Some(omitted) = tail.pop_front()
            {
                base_seq = omitted.seq;
            }
        }
        if base_tx
            .take()
            .is_some_and(|base_tx| base_tx.send(Ok(base_seq)).is_err())
        {
            return Ok(());
        }
        while !tail.is_empty() {
            let take = tail.len().min(RUNTIME_EVENT_REPLAY_BATCH_SIZE);
            let batch = tail.drain(..take).collect::<Vec<_>>();
            if batch_tx.blocking_send(Ok(batch)).is_err() {
                return Ok(());
            }
        }
        Ok(())
    }

    pub async fn current_seq(&self) -> Result<u64> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.with_event_transaction(EVENT_TRANSACTION_LOCK_TIMEOUT, || {
                Ok(load_runtime_store_state(&store.state_path)?
                    .next_seq
                    .saturating_sub(1))
            })
        })
        .await
        .context("Runtime event cursor worker failed")?
    }
}
