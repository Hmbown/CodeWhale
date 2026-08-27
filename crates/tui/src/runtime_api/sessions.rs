use std::collections::HashMap;
use std::path::PathBuf;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::models::{ContentBlock, Message};
use crate::runtime_threads::{
    CreateThreadRequest, RuntimeTurnStatus, ThreadDetail, ThreadListFilter, TurnItemKind,
    TurnItemLifecycleStatus,
};
use crate::session_manager::{
    SavedSession, SessionListFilter, SessionManager, SessionMetadata, SessionMutator,
    create_saved_session_with_id_and_mode,
};
use crate::session_peek::{MAX_PEEK_ENTRIES, SessionPeek, build_peek};
use crate::session_projection::{SessionQuery, SessionSortMode, SessionSummary, project_sessions};

use super::{ApiError, RuntimeApiState, map_thread_err, truncate_text};
use crate::models::Role;

#[derive(Debug, Serialize)]
pub(super) struct SessionsResponse {
    sessions: Vec<SessionMetadata>,
}

#[derive(Debug, Serialize)]
pub(super) struct SessionDetailResponse {
    pub(super) metadata: SessionMetadata,
    pub(super) messages: Vec<Value>,
    pub(super) system_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateSessionRequest {
    thread_id: String,
    title: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct CreateSessionResponse {
    session_id: String,
    thread_id: String,
    message_count: usize,
    title: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResumeSessionRequest {
    model: Option<String>,
    mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ResumeSessionResponse {
    thread_id: String,
    session_id: String,
    message_count: usize,
    summary: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct SessionsQuery {
    limit: Option<usize>,
    search: Option<String>,
    /// Include archived sessions. Same name and meaning as the `/v1/threads`
    /// query pair, so a client does not need two mental models (#4397).
    #[serde(default)]
    include_archived: Option<bool>,
    /// Return archived sessions only. Overrides `include_archived`.
    #[serde(default)]
    archived_only: Option<bool>,
    /// Restrict to sessions recorded against this workspace. Absent means
    /// every workspace, matching the historical behaviour of this route.
    #[serde(default)]
    workspace: Option<PathBuf>,
    /// `recent` (default), `name`, or `size`.
    #[serde(default)]
    sort: Option<String>,
}

/// `PATCH /v1/sessions/{id}` body. Both fields are optional; omitting one
/// leaves it untouched.
#[derive(Debug, Deserialize)]
pub(super) struct PatchSessionRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
}

/// Lifecycle receipt for a session mutation.
///
/// Deliberately shaped like the thread patch receipt: the caller gets the
/// resulting record plus an explicit `changes` map of what actually moved, so
/// a no-op patch is distinguishable from an applied one without diffing.
#[derive(Debug, Serialize)]
pub(super) struct PatchSessionResponse {
    session: SessionMetadata,
    changes: HashMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SaveSessionRequest {
    /// Thread ID to save as a session. If omitted, saves the most recently
    /// active thread.
    #[serde(default)]
    thread_id: Option<String>,
    /// If provided, update the existing session with this ID instead of
    /// creating a new one. This matches TUI's `build_session_snapshot`
    /// behavior where it updates the current session in-place.
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct SaveSessionResponse {
    session_id: String,
    session: SessionDetailResponse,
}

/// Turn a `SessionsQuery` into the shared projection query.
///
/// The whole point of routing through [`SessionQuery`] is that the API's
/// filter/sort/search semantics are the *same code* the TUI picker and the
/// sidebar rail run, not a parallel reimplementation that drifts.
fn projection_query(query: &SessionsQuery) -> SessionQuery {
    let mut projected = SessionQuery::default()
        .with_filter(SessionListFilter::from_query(
            query.include_archived,
            query.archived_only,
        ))
        .with_sort(
            query
                .sort
                .as_deref()
                .map_or(SessionSortMode::Recent, SessionSortMode::from_str_or_recent),
        )
        .with_search(query.search.clone().unwrap_or_default())
        .with_limit(query.limit.unwrap_or(50).clamp(1, 500));
    if let Some(workspace) = query.workspace.as_deref() {
        projected = projected.scoped_to(workspace);
    }
    projected
}

pub(super) async fn list_sessions(
    State(state): State<RuntimeApiState>,
    Query(query): Query<SessionsQuery>,
) -> Result<Json<SessionsResponse>, ApiError> {
    let manager = SessionManager::new(state.sessions_dir.clone())
        .map_err(|e| ApiError::internal(format!("Failed to open sessions dir: {e}")))?;
    let all = manager
        .list_sessions()
        .map_err(|e| ApiError::internal(format!("Failed to list sessions: {e}")))?;
    // This route keeps returning full `SessionMetadata` for compatibility;
    // `/v1/sessions/summary` is the projected shape. Membership *and* order
    // come from the shared projection so the two routes never disagree.
    let sessions: Vec<SessionMetadata> = project_sessions(&all, &projection_query(&query), None)
        .into_iter()
        .filter_map(|summary| all.iter().find(|m| m.id == summary.id).cloned())
        .collect();
    Ok(Json(SessionsResponse { sessions }))
}

/// `GET /v1/sessions/summary` — the projected row shape.
///
/// Field-compatible with `/v1/threads/summary` so the embedded dashboard can
/// render a saved session and a live thread with one row renderer, which is
/// what "one projection" means in practice rather than as an aspiration.
pub(super) async fn list_sessions_summary(
    State(state): State<RuntimeApiState>,
    Query(query): Query<SessionsQuery>,
) -> Result<Json<Vec<SessionSummary>>, ApiError> {
    let manager = SessionManager::new(state.sessions_dir.clone())
        .map_err(|e| ApiError::internal(format!("Failed to open sessions dir: {e}")))?;
    let all = manager
        .list_sessions()
        .map_err(|e| ApiError::internal(format!("Failed to list sessions: {e}")))?;
    Ok(Json(project_sessions(
        &all,
        &projection_query(&query),
        None,
    )))
}

/// `PATCH /v1/sessions/{id}` — rename and/or archive a saved session.
///
/// Both mutations go through the manager's single writers
/// (`rename_session`, `set_session_archived`), which is what keeps the web
/// dashboard, the TUI picker, and `/sessions archive` from producing three
/// different notions of the same lifecycle state.
pub(super) async fn patch_session(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<PatchSessionRequest>,
) -> Result<Json<PatchSessionResponse>, ApiError> {
    if req.title.is_none() && req.archived.is_none() {
        return Err(ApiError::bad_request(
            "PATCH /v1/sessions/{id} requires at least one of `title` or `archived`",
        ));
    }
    let manager = SessionManager::new(state.sessions_dir.clone())
        .map_err(|e| ApiError::internal(format!("Failed to open sessions dir: {e}")))?;

    let before = manager
        .load_session(&id)
        .map_err(|e| map_session_err(&id, e, "read"))?
        .metadata;
    let mut metadata = before.clone();
    let mut changes: HashMap<String, Value> = HashMap::new();

    if let Some(title) = req.title.as_deref() {
        // Validate the title before touching the store so a rejected title
        // reports *why* it was rejected rather than the generic "invalid
        // session id" that `map_session_err` produces for `InvalidInput`.
        crate::session_manager::normalize_session_title(title)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        metadata = manager
            .rename_session(&id, title, SessionMutator::External)
            .map_err(|e| map_session_err(&id, e, "rename"))?;
        if metadata.title != before.title {
            changes.insert("title".to_string(), json!(metadata.title));
        }
    }
    if let Some(archived) = req.archived {
        metadata = manager
            .set_session_archived(&id, archived, SessionMutator::External)
            .map_err(|e| map_session_err(&id, e, "archive"))?;
        if metadata.archived != before.archived {
            changes.insert("archived".to_string(), json!(metadata.archived));
        }
    }

    Ok(Json(PatchSessionResponse {
        session: metadata,
        changes,
    }))
}

/// `GET /v1/sessions/{id}` query options.
#[derive(Debug, Deserialize, Default)]
pub(super) struct SessionDetailQuery {
    /// When true, return a bounded, redacted [`SessionPeek`] instead of the
    /// full transcript. The dashboard always asks for this: shipping a
    /// multi-megabyte transcript to a browser in order to show twelve lines is
    /// both wasteful and a needless place to re-emit secrets.
    #[serde(default)]
    peek: Option<bool>,
    /// Entry budget for the peek, clamped to [`MAX_PEEK_ENTRIES`].
    #[serde(default)]
    entries: Option<usize>,
}

/// Either the full session or a bounded peek, chosen by `?peek=true`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(super) enum SessionDetailOrPeek {
    Peek(Box<SessionPeek>),
    Detail(Box<SessionDetailResponse>),
}

pub(super) async fn get_session(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Query(query): Query<SessionDetailQuery>,
) -> Result<Json<SessionDetailOrPeek>, ApiError> {
    let manager = SessionManager::new(state.sessions_dir.clone())
        .map_err(|e| ApiError::internal(format!("Failed to open sessions dir: {e}")))?;
    let session = manager
        .load_session(&id)
        .map_err(|e| map_session_err(&id, e, "read"))?;

    if query.peek.unwrap_or(false) {
        let entries = query.entries.unwrap_or(MAX_PEEK_ENTRIES);
        return Ok(Json(SessionDetailOrPeek::Peek(Box::new(build_peek(
            &session, entries,
        )))));
    }
    Ok(Json(SessionDetailOrPeek::Detail(Box::new(
        session_to_detail(session),
    ))))
}

pub(super) async fn resume_session_thread(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
    Json(req): Json<ResumeSessionRequest>,
) -> Result<(StatusCode, Json<ResumeSessionResponse>), ApiError> {
    let manager = SessionManager::new(state.sessions_dir.clone())
        .map_err(|e| ApiError::internal(format!("Failed to open sessions dir: {e}")))?;
    let session = manager
        .load_session(&id)
        .map_err(|e| map_session_err(&id, e, "read"))?;

    let model = req.model.unwrap_or_else(|| session.metadata.model.clone());
    let mode = req.mode.unwrap_or_else(|| {
        session
            .metadata
            .mode
            .clone()
            .unwrap_or_else(|| "agent".to_string())
    });

    let thread = state
        .runtime_threads
        .create_thread(CreateThreadRequest {
            model: Some(model),
            model_provider: Some(session.metadata.model_provider.clone()),
            model_provider_id: session.metadata.model_provider_id.clone(),
            workspace: Some(session.metadata.workspace.clone()),
            mode: Some(mode),
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: session.system_prompt.clone(),
            task_id: None,
            ..Default::default()
        })
        .await
        .map_err(map_resume_thread_create_err)?;

    let msg_count = session.messages.len();
    state
        .runtime_threads
        .seed_thread_from_messages(&thread.id, &session.messages)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to seed thread history: {e}")))?;

    // Link the session to the new thread so that `ensure_engine_loaded`
    // can restore the full message history from the session file.
    if let Err(e) = state
        .runtime_threads
        .set_thread_session_id(&thread.id, &id)
        .await
    {
        let session_ref = crate::utils::redacted_identifier_for_log(&id);
        tracing::warn!(
            session = %session_ref,
            thread_id = %thread.id,
            error = %e,
            "Failed to link session to thread"
        );
    }

    let summary = format!(
        "Resumed session '{}' ({} messages) into thread {}",
        session.metadata.title, msg_count, thread.id
    );

    Ok((
        StatusCode::CREATED,
        Json(ResumeSessionResponse {
            thread_id: thread.id,
            session_id: id,
            message_count: msg_count,
            summary,
        }),
    ))
}

pub(super) async fn create_session_from_thread(
    State(state): State<RuntimeApiState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), ApiError> {
    let thread_id = req.thread_id.trim();
    if thread_id.is_empty() {
        return Err(ApiError::bad_request("thread_id is required"));
    }

    let detail = state
        .runtime_threads
        .get_thread_detail(thread_id)
        .await
        .map_err(map_thread_err)?;

    if thread_detail_has_live_work(&detail) {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            message: format!(
                "Thread {thread_id} has a queued or active turn; wait for completion before saving as a session"
            ),
        });
    }

    let messages = messages_from_thread_detail(&detail);
    if messages.is_empty() {
        return Err(ApiError::bad_request(format!(
            "Thread {thread_id} has no user or assistant messages to save"
        )));
    }

    let manager = SessionManager::new(state.sessions_dir.clone())
        .map_err(|e| ApiError::internal(format!("Failed to open sessions dir: {e}")))?;
    let total_tokens = total_tokens_from_thread_detail(&detail);
    let session_handle = uuid::Uuid::new_v4().to_string();
    let mut session = create_saved_session_with_id_and_mode(
        session_handle.clone(),
        &messages,
        &detail.thread.model,
        &detail.thread.workspace,
        total_tokens,
        None,
        Some(&detail.thread.mode),
    );
    {
        let config = state.runtime_threads.read_config();
        stamp_session_provider_from_thread(&config, &detail, &mut session.metadata).map_err(
            |reason| {
            ApiError::bad_request(format!(
                    "Thread {thread_id} provider route is unavailable; session export will not fall back: {reason}"
            ))
            },
        )?;
    }
    session.system_prompt = detail.thread.system_prompt.clone();

    if let Some(title) =
        session_title_override(req.title.as_deref(), detail.thread.title.as_deref())
    {
        session.metadata.title = title;
    }
    let title = session.metadata.title.clone();
    let message_count = session.metadata.message_count;

    persist_thread_cost(&state, thread_id, &mut session).await?;

    manager
        .save_session(&session)
        .map_err(|e| ApiError::internal(format!("Failed to save session: {e}")))?;

    // Link the session to the thread so that `ensure_engine_loaded` can
    // restore the full message history from the session file.
    if let Err(e) = state
        .runtime_threads
        .set_thread_session_id(&detail.thread.id, &session_handle)
        .await
    {
        let session_ref = crate::utils::redacted_identifier_for_log(&session_handle);
        tracing::warn!(
            session = %session_ref,
            thread_id = %detail.thread.id,
            error = %e,
            "Failed to link session to thread"
        );
    }

    Ok((
        StatusCode::CREATED,
        Json(CreateSessionResponse {
            session_id: session_handle,
            thread_id: detail.thread.id,
            message_count,
            title,
        }),
    ))
}

pub(super) fn stamp_session_provider_from_thread(
    config: &crate::config::Config,
    detail: &ThreadDetail,
    metadata: &mut crate::session_manager::SessionMetadata,
) -> Result<(), String> {
    let thread_has_route = detail
        .thread
        .model_provider
        .as_deref()
        .is_some_and(|provider| !provider.trim().is_empty())
        || detail.thread.model_provider_id.is_some();
    let provider_identity = if thread_has_route {
        config.resolve_persisted_provider_identity(
            detail.thread.model_provider.as_deref(),
            detail.thread.model_provider_id.as_deref(),
        )?
    } else if let Some(turn) = detail.turns.iter().rev().find(|turn| {
        turn.effective_provider
            .as_deref()
            .is_some_and(|provider| !provider.trim().is_empty())
            || turn.effective_provider_id.is_some()
    }) {
        config.resolve_persisted_provider_identity(
            turn.effective_provider.as_deref(),
            turn.effective_provider_id.as_deref(),
        )?
    } else {
        let key = config
            .provider
            .as_deref()
            .unwrap_or(crate::config::ApiProvider::Deepseek.as_str());
        config.resolve_provider_identity(key)?
    };
    metadata.set_model_provider_route(
        provider_identity.provider.as_str(),
        provider_identity.persisted_id(),
    );
    Ok(())
}

fn thread_detail_has_live_work(detail: &ThreadDetail) -> bool {
    detail.turns.iter().any(|turn| {
        matches!(
            turn.status,
            RuntimeTurnStatus::Queued | RuntimeTurnStatus::InProgress
        )
    }) || detail.items.iter().any(|item| {
        matches!(
            item.status,
            TurnItemLifecycleStatus::Queued | TurnItemLifecycleStatus::InProgress
        )
    })
}

pub(super) fn messages_from_thread_detail(detail: &ThreadDetail) -> Vec<Message> {
    let items_by_id: HashMap<&str, _> = detail
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let mut messages = Vec::new();

    for turn in &detail.turns {
        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
        let mut user_blocks: Vec<ContentBlock> = Vec::new();
        let flush_assistant = |blocks: &mut Vec<ContentBlock>, msgs: &mut Vec<Message>| {
            if !blocks.is_empty() {
                msgs.push(Message {
                    role: Role::Assistant,
                    content: std::mem::take(blocks),
                });
            }
        };
        let flush_user = |blocks: &mut Vec<ContentBlock>, msgs: &mut Vec<Message>| {
            if !blocks.is_empty() {
                msgs.push(Message {
                    role: Role::User,
                    content: std::mem::take(blocks),
                });
            }
        };

        for item_id in &turn.item_ids {
            let Some(item) = items_by_id.get(item_id.as_str()) else {
                continue;
            };
            match item.kind {
                TurnItemKind::UserMessage => {
                    flush_assistant(&mut assistant_blocks, &mut messages);

                    let text = item.detail.as_deref().map(str::trim).unwrap_or("");
                    if !text.is_empty() {
                        user_blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                            cache_control: None,
                        });
                    }
                }
                TurnItemKind::AgentMessage => {
                    flush_user(&mut user_blocks, &mut messages);
                    let text = item.detail.as_deref().map(str::trim).unwrap_or("");
                    if !text.is_empty() {
                        assistant_blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                            cache_control: None,
                        });
                    }
                }
                TurnItemKind::AgentReasoning => {
                    flush_user(&mut user_blocks, &mut messages);
                    let thinking = item.detail.as_deref().map(str::trim).unwrap_or("");
                    if !thinking.is_empty() {
                        assistant_blocks.push(ContentBlock::Thinking {
                            thinking: thinking.to_string(),
                            signature: None,
                            state: None,
                        });
                    }
                }
                TurnItemKind::ToolCall => {
                    // Check metadata to distinguish tool_use from tool_result.
                    let meta = item.metadata.as_ref();
                    let is_tool_result = meta.and_then(|m| m.get("tool_result_for")).is_some();
                    if is_tool_result {
                        flush_assistant(&mut assistant_blocks, &mut messages);

                        let tool_use_id = meta
                            .and_then(|m| m.get("tool_result_for"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let content = item.detail.as_deref().unwrap_or("").to_string();
                        let is_error = meta
                            .and_then(|m| m.get("is_error"))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let content_blocks = meta
                            .and_then(|m| m.get("content_blocks"))
                            .and_then(|v| v.as_array())
                            .cloned();
                        user_blocks.push(ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error: if is_error { Some(true) } else { None },
                            content_blocks,
                        });
                    } else {
                        flush_user(&mut user_blocks, &mut messages);
                        let tool_use_id = meta
                            .and_then(|m| m.get("tool_use_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let tool_name = meta
                            .and_then(|m| m.get("tool_name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let input_str = item.detail.as_deref().unwrap_or("{}");
                        let input: Value = serde_json::from_str(input_str).unwrap_or(Value::Null);
                        assistant_blocks.push(ContentBlock::ToolUse {
                            id: tool_use_id,
                            name: tool_name,
                            input,
                            caller: None,
                            thought_signature: None,
                        });
                    }
                }
                // Skip other item kinds (file_change, command_execution, etc.)
                _ => {}
            }
        }
        flush_assistant(&mut assistant_blocks, &mut messages);
        flush_user(&mut user_blocks, &mut messages);
    }

    messages
}

/// Merge the thread's authoritative cost into a session about to be saved.
///
/// The engine snapshot carries messages/tokens but no cost — cost lives in
/// the turn records' route-audited usage — so derive it from the same
/// accumulation that powers `/v1/usage` (recorded-time pricing, both
/// published currencies). The parent/child split mirrors the TUI writer's
/// field semantics (`sync_cost_to_metadata`): `session_cost_*` carries
/// parent-turn spend and `subagent_cost_*` routed-child spend, so a session
/// previously saved by the TUI never gets child spend counted twice in
/// `total_estimate()`. Merging each side with max keeps a session resumed
/// across threads from losing previously persisted spend, and extends the
/// monotonic display guarantee (#244) to the persisted shape. Coverage
/// travels with the money it qualifies (#4318): the counters are
/// parent-turn coverage (the TUI's own session-level accounting), CNY
/// included, and `coverage_recorded` marks that this writer computed them
/// from audited turn records rather than deserializing a legacy default.
async fn persist_thread_cost(
    state: &RuntimeApiState,
    thread_id: &str,
    session: &mut crate::session_manager::SavedSession,
) -> Result<(), ApiError> {
    let usage = state
        .runtime_threads
        .aggregate_usage_for_thread(thread_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to aggregate thread usage: {e}")))?;
    let combined = usage.combined();
    let cost = &mut session.metadata.cost;
    cost.session_cost_usd = cost.session_cost_usd.max(usage.parent.cost_usd);
    cost.session_cost_cny = cost.session_cost_cny.max(usage.parent.cost_cny);
    cost.subagent_cost_usd = cost.subagent_cost_usd.max(usage.routed_children.cost_usd);
    cost.subagent_cost_cny = cost.subagent_cost_cny.max(usage.routed_children.cost_cny);
    // The display total is session + subagent, so the high-water mark rides
    // the combined figure in both currencies.
    cost.displayed_cost_high_water_usd = cost.displayed_cost_high_water_usd.max(combined.cost_usd);
    cost.displayed_cost_high_water_cny = cost.displayed_cost_high_water_cny.max(combined.cost_cny);
    cost.priced_turns = cost
        .priced_turns
        .max(u32::try_from(usage.parent.priced_turns).unwrap_or(u32::MAX));
    cost.unpriced_turns = cost
        .unpriced_turns
        .max(u32::try_from(usage.parent.unpriced_turns).unwrap_or(u32::MAX));
    cost.cny_priced_turns = cost
        .cny_priced_turns
        .max(u32::try_from(usage.parent.cny_priced_turns).unwrap_or(u32::MAX));
    cost.cny_unpriced_turns = cost
        .cny_unpriced_turns
        .max(u32::try_from(usage.parent.cny_unpriced_turns).unwrap_or(u32::MAX));
    // Coverage travels with the money (#4318): reasons and classes are the
    // qualifiers a reload needs to treat these totals as known, not a
    // legacy-unknown complete zero. Parent-turn coverage only — the same
    // field the TUI writer uses; routed-child spend lives in subagent_cost_*.
    cost.unpriced_reasons
        .extend(usage.parent.unpriced_reasons.iter().cloned());
    cost.cny_unpriced_reasons
        .extend(usage.parent.cny_unpriced_reasons.iter().cloned());
    cost.unpriced_classes
        .extend(usage.parent.unpriced_classes.iter().cloned());
    cost.pricing_provenances
        .extend(usage.parent.pricing_provenances.iter().cloned());
    cost.live_pricing_defects
        .extend(usage.parent.live_pricing_defects.iter().cloned());
    cost.live_pricing_unusable_defects
        .extend(usage.parent.live_pricing_unusable_defects.iter().cloned());
    cost.route_receipts
        .extend(usage.parent.route_receipts.iter().cloned());
    cost.coverage_recorded = true;
    Ok(())
}

/// `PUT /v1/sessions` — save a thread's current engine state as a session.
///
/// Unlike `POST /v1/sessions` (which reconstructs messages from stored turn
/// items), this endpoint asks the engine for its live session snapshot so
/// token counts and message ordering are authoritative.
pub(super) async fn save_current_session(
    State(state): State<RuntimeApiState>,
    Json(req): Json<SaveSessionRequest>,
) -> Result<Json<SaveSessionResponse>, ApiError> {
    // Find the thread to save.
    let thread_id = match req.thread_id {
        Some(id) => id,
        None => {
            // Find the most recently updated thread.
            let threads = state
                .runtime_threads
                .list_threads(ThreadListFilter::IncludeArchived, Some(100))
                .await
                .map_err(map_thread_err)?;
            threads
                .into_iter()
                .max_by_key(|t| t.updated_at)
                .map(|t| t.id)
                .ok_or_else(|| ApiError::bad_request("No threads to save"))?
        }
    };

    // Get the engine handle (loads the thread into an engine if needed),
    // then request a session snapshot. This reuses the same code path as
    // TUI's `build_session_snapshot`: the engine holds the authoritative
    // messages and token usage, so we don't need to reconstruct from turns.
    let engine = state
        .runtime_threads
        .get_engine(&thread_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get engine for thread: {e}")))?;

    let snapshot = engine
        .get_session_snapshot()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get session snapshot: {e}")))?;

    let manager = SessionManager::new(state.sessions_dir.clone())
        .map_err(|e| ApiError::internal(format!("Failed to open sessions dir: {e}")))?;

    // Build or update the session, mirroring TUI's `build_session_snapshot`.
    // Only `io::ErrorKind::NotFound` falls back to creating a new session;
    // other I/O errors (e.g. PermissionDenied) are propagated so callers
    // don't silently overwrite a corrupt or inaccessible session file.
    let mut session = if let Some(ref existing_id) = req.session_id {
        match manager.load_session(existing_id) {
            Ok(existing) => {
                let mut updated = crate::session_manager::update_session(
                    existing,
                    &snapshot.messages,
                    snapshot.total_tokens,
                    snapshot.system_prompt.as_ref(),
                );
                updated.metadata.model = snapshot.model.clone();
                updated.metadata.set_model_provider_route(
                    &snapshot.model_provider,
                    snapshot.model_provider_id.as_deref(),
                );
                updated.metadata.mode = Some(snapshot.mode.clone());
                updated
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    let mut session = crate::session_manager::create_saved_session_with_id_and_mode(
                        existing_id.clone(),
                        &snapshot.messages,
                        &snapshot.model,
                        &snapshot.workspace,
                        snapshot.total_tokens,
                        snapshot.system_prompt.as_ref(),
                        Some(snapshot.mode.as_str()),
                    );
                    session.metadata.set_model_provider_route(
                        &snapshot.model_provider,
                        snapshot.model_provider_id.as_deref(),
                    );
                    session
                } else {
                    return Err(ApiError::internal(format!(
                        "Failed to load session {existing_id}: {e}"
                    )));
                }
            }
        }
    } else {
        let mut session = crate::session_manager::create_saved_session_with_mode(
            &snapshot.messages,
            &snapshot.model,
            &snapshot.workspace,
            snapshot.total_tokens,
            snapshot.system_prompt.as_ref(),
            Some(snapshot.mode.as_str()),
        );
        session.metadata.set_model_provider_route(
            &snapshot.model_provider,
            snapshot.model_provider_id.as_deref(),
        );
        session
    };

    persist_thread_cost(&state, &thread_id, &mut session).await?;

    // Save the session.
    manager
        .save_session(&session)
        .map_err(|e| ApiError::internal(format!("Failed to save session: {e}")))?;

    // Link the session to the thread so that `ensure_engine_loaded` can
    // restore the full message history (including thinking/tool blocks)
    // from the session file instead of reconstructing from turns.
    let session_handle = session.metadata.id.clone();
    if let Err(e) = state
        .runtime_threads
        .set_thread_session_id(&thread_id, &session_handle)
        .await
    {
        let session_ref = crate::utils::redacted_identifier_for_log(&session_handle);
        tracing::warn!(
            session = %session_ref,
            thread_id = %thread_id,
            error = %e,
            "Failed to link session to thread"
        );
    }

    Ok(Json(SaveSessionResponse {
        session_id: session_handle,
        session: session_to_detail(session),
    }))
}

fn total_tokens_from_thread_detail(detail: &ThreadDetail) -> u64 {
    detail
        .turns
        .iter()
        .filter_map(|turn| turn.usage.as_ref())
        .map(|usage| u64::from(usage.input_tokens) + u64::from(usage.output_tokens))
        .sum()
}

fn session_title_override(requested: Option<&str>, thread_title: Option<&str>) -> Option<String> {
    requested
        .and_then(nonempty_title)
        .or_else(|| thread_title.and_then(nonempty_title))
}

fn nonempty_title(title: &str) -> Option<String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(truncate_text(trimmed, 50))
    }
}

pub(super) async fn delete_session(
    State(state): State<RuntimeApiState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let manager = SessionManager::new(state.sessions_dir.clone())
        .map_err(|e| ApiError::internal(format!("Failed to open sessions dir: {e}")))?;
    manager
        .delete_session(&id)
        .map_err(|e| map_session_err(&id, e, "delete"))?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn session_to_detail(session: SavedSession) -> SessionDetailResponse {
    let messages: Vec<Value> = session
        .messages
        .iter()
        .map(|msg| {
            let content_blocks: Vec<Value> = msg
                .content
                .iter()
                .map(|block| match block {
                    crate::models::ContentBlock::Text { text, .. } => {
                        json!({ "type": "text", "text": text })
                    }
                    crate::models::ContentBlock::Thinking { thinking, .. } => {
                        json!({ "type": "thinking", "text": thinking })
                    }
                    crate::models::ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        caller, ..} => {
                        let mut obj =
                            json!({ "type": "tool_use", "id": id, "name": name, "input": input });
                        if let Some(caller) = caller {
                            obj["caller"] = json!(caller);
                        }
                        obj
                    }
                    crate::models::ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        content_blocks,
                        ..
                    } => {
                        let mut obj = json!({ "type": "tool_result", "tool_use_id": tool_use_id });
                        if let Some(cbs) = content_blocks {
                            obj["content_blocks"] = json!(cbs);
                            if !content.is_empty() {
                                obj["content"] = json!(content);
                            }
                        } else {
                            obj["content"] = json!(content);
                        }
                        if let Some(e) = is_error {
                            obj["is_error"] = json!(e);
                        }
                        obj
                    }
                    crate::models::ContentBlock::ServerToolUse { id, name, input } => {
                        json!({ "type": "tool_use", "id": id, "name": name, "input": input })
                    }
                    crate::models::ContentBlock::ToolSearchToolResult {
                        tool_use_id,
                        content,
                    } => {
                        json!({ "type": "tool_result", "tool_use_id": tool_use_id, "content": content })
                    }
                    crate::models::ContentBlock::CodeExecutionToolResult {
                        tool_use_id,
                        content,
                    } => {
                        json!({ "type": "tool_result", "tool_use_id": tool_use_id, "content": content })
                    }
                    crate::models::ContentBlock::ImageUrl { .. } => Value::Null,
                })
                .collect();
            json!({
                "role": msg.role,
                "content": content_blocks,
            })
        })
        .collect();
    SessionDetailResponse {
        metadata: session.metadata,
        messages,
        system_prompt: session.system_prompt,
    }
}

fn map_session_err(id: &str, err: std::io::Error, action: &str) -> ApiError {
    match err.kind() {
        std::io::ErrorKind::NotFound => ApiError::not_found(format!("Session '{id}' not found")),
        std::io::ErrorKind::InvalidData => {
            ApiError::bad_request(format!("Failed to parse session '{id}': {err}"))
        }
        std::io::ErrorKind::InvalidInput => {
            ApiError::bad_request(format!("Invalid session id '{id}'"))
        }
        // The session is open in an interactive Codewhale session, which holds
        // the authoritative copy in memory. Fail closed with a typed conflict
        // rather than write something its next autosave would revert.
        std::io::ErrorKind::ResourceBusy => ApiError {
            status: StatusCode::CONFLICT,
            message: err.to_string(),
        },
        _ => ApiError::internal(format!("Failed to {action} session '{id}': {err}")),
    }
}

fn map_resume_thread_create_err(err: anyhow::Error) -> ApiError {
    let reason = err.to_string();
    let message = format!("Failed to create thread: {reason}");
    if reason.starts_with("saved session has an empty provider identity")
        || reason.starts_with("saved session requires custom provider")
        || reason.starts_with("legacy session records only the generic `custom` provider kind")
        || reason.starts_with("legacy `provider = \"custom\"`")
    {
        ApiError::bad_request(message)
    } else {
        // Thread-store writes, event persistence, and other runtime failures
        // are server-side faults; never disguise them as a client config error.
        ApiError::internal(message)
    }
}

#[cfg(test)]
mod session_query_tests {
    use super::*;

    fn query(
        include_archived: Option<bool>,
        archived_only: Option<bool>,
        sort: Option<&str>,
        workspace: Option<&str>,
        limit: Option<usize>,
    ) -> SessionsQuery {
        SessionsQuery {
            limit,
            search: Some("whale".to_string()),
            include_archived,
            archived_only,
            workspace: workspace.map(PathBuf::from),
            sort: sort.map(str::to_string),
        }
    }

    #[test]
    fn archive_params_resolve_like_the_threads_routes() {
        assert_eq!(
            projection_query(&query(None, None, None, None, None)).filter,
            SessionListFilter::ActiveOnly
        );
        assert_eq!(
            projection_query(&query(Some(true), None, None, None, None)).filter,
            SessionListFilter::IncludeArchived
        );
        assert_eq!(
            projection_query(&query(Some(true), Some(true), None, None, None)).filter,
            SessionListFilter::ArchivedOnly
        );
    }

    #[test]
    fn sort_and_workspace_scope_flow_through_and_bad_sorts_fall_back() {
        let projected = projection_query(&query(None, None, Some("name"), Some("/repo"), Some(9)));
        assert_eq!(projected.sort, SessionSortMode::Name);
        // `Path` in this module is `axum::extract::Path`; spell out the std one.
        assert_eq!(
            projected.workspace_scope.as_deref(),
            Some(std::path::Path::new("/repo"))
        );
        assert_eq!(projected.limit, 9);
        assert_eq!(projected.search, "whale");

        // An unknown sort must not fail the request — a stale client should
        // still get a listing, just in the default order.
        assert_eq!(
            projection_query(&query(None, None, Some("nonsense"), None, None)).sort,
            SessionSortMode::Recent
        );
    }

    #[test]
    fn limit_is_clamped_at_both_ends() {
        assert_eq!(
            projection_query(&query(None, None, None, None, Some(0))).limit,
            1
        );
        assert_eq!(
            projection_query(&query(None, None, None, None, Some(10_000))).limit,
            500
        );
        // Absent limit keeps the historical page size.
        assert_eq!(
            projection_query(&query(None, None, None, None, None)).limit,
            50
        );
    }

    #[test]
    fn absent_workspace_means_every_workspace() {
        assert!(
            projection_query(&query(None, None, None, None, None))
                .workspace_scope
                .is_none(),
            "the API must not silently scope to the runtime's own CWD"
        );
    }
}

#[cfg(test)]
mod resume_thread_error_tests {
    use super::*;

    #[test]
    fn provider_config_errors_are_client_errors_but_storage_errors_stay_internal() {
        let provider = map_resume_thread_create_err(anyhow::anyhow!(
            "saved session requires custom provider 'lm-studio', but `[providers.lm-studio]` is missing"
        ));
        assert_eq!(provider.status, StatusCode::BAD_REQUEST);

        let storage = map_resume_thread_create_err(anyhow::anyhow!(
            "Failed to save runtime thread: permission denied"
        ));
        assert_eq!(storage.status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
