//! UI-side approval disposition and durable denial receipts.

use crate::audit::log_sensitive_event;
use crate::core::engine::EngineHandle;
use crate::localization::MessageId;
use crate::tui::app::{App, AppMode, StatusToastLevel};
use crate::tui::approval::ApprovalMode;
use crate::tui::history::HistoryCell;

pub(super) fn is_session_approved_for_tool(
    app: &App,
    _tool_name: &str,
    grouping_key: &str,
) -> bool {
    // Session grants match the grouping key only (command family / host /
    // patch paths). A bare tool name is never session-wide: approving one
    // shell command used to auto-approve the entire shell tool for the
    // session. The `contains(tool_name)` clause was the escalation (ops R2).
    app.approval_session_approved.contains(grouping_key)
}

pub(super) fn is_session_denied_for_key(app: &App, approval_key: &str) -> bool {
    app.approval_session_denied.contains(approval_key)
}

pub(super) fn session_denied_notice(app: &App, tool_name: &str) -> String {
    app.tr(MessageId::ApprovalAutoDeniedSession)
        .replace("{tool}", tool_name)
}

pub(super) fn surface_session_denied_notice(app: &mut App, tool_name: &str) {
    let notice = session_denied_notice(app, tool_name);
    app.status_message = Some(notice.clone());
    app.push_status_toast(notice.clone(), StatusToastLevel::Warning, Some(12_000));

    // Tool completion and turn completion can replace the one-line status
    // before the next frame is painted. Keep the recovery path in the
    // transcript as a settled receipt as well, where it survives that event
    // ordering and remains available to screen readers and scrollback.
    let latest_transcript_cell = app
        .active_cell
        .as_ref()
        .and_then(|cell| cell.entries().last())
        .or_else(|| app.history.last());
    let already_latest_receipt = matches!(
        latest_transcript_cell,
        Some(HistoryCell::System { content }) if content == &notice
    );
    if !already_latest_receipt {
        let receipt = HistoryCell::System { content: notice };
        if let Some(active_cell) = app.active_cell.as_mut() {
            // Never grow committed history underneath an active cell: tool
            // lookup indices address `history ++ active_cell`, so changing
            // history.len() mid-turn would retarget the pending completion.
            active_cell.push_untracked(receipt);
            app.bump_active_cell_revision();
        } else {
            app.add_message(receipt);
        }
    }
}

pub(super) async fn auto_deny_session_approval(
    app: &mut App,
    engine_handle: &EngineHandle,
    id: &str,
    tool_name: &str,
    approval_key: &str,
) {
    log_sensitive_event(
        "tool.approval.auto_deny_session",
        serde_json::json!({
            "tool_name": tool_name,
            "approval_key": approval_key,
            "session_id": app.current_session_id,
        }),
    );
    let _ = engine_handle.deny_tool_call(id.to_string()).await;
    surface_session_denied_notice(app, tool_name);
}

pub(super) fn app_auto_approve_enabled(app: &App) -> bool {
    app.mode == AppMode::Yolo || app.approval_mode == ApprovalMode::Bypass
}

/// Build the UI-side TurnAuthority for approval disposition (#4412).
///
/// Shell/trust bits do not affect disposition; mode + approval_mode + the
/// full-access shape (Yolo/Bypass) are what the shared resolver consults.
fn app_turn_authority_for_approvals(app: &App) -> crate::core::authority::TurnAuthority {
    crate::core::authority::TurnAuthority::from_effective_fields(
        app.mode,
        true,
        false,
        app_auto_approve_enabled(app),
        app.approval_mode,
    )
}

pub(super) fn resolve_ui_approval_disposition(
    app: &App,
    tool_name: &str,
    grouping_key: &str,
    approval_key: &str,
    approval_force_prompt: bool,
) -> crate::core::authority::ApprovalRequestDisposition {
    crate::core::authority::resolve_approval_request_disposition(
        &app_turn_authority_for_approvals(app),
        is_session_approved_for_tool(app, tool_name, grouping_key),
        is_session_denied_for_key(app, approval_key),
        approval_force_prompt,
    )
}

pub(super) fn should_suppress_user_input_prompt(app: &App) -> bool {
    // Legacy hosts may still report Yolo/auto-approve with a stale `Auto`
    // enum. Canonicalize that shape to Full Access before applying the one
    // posture that suppresses questions: genuine Auto-Review.
    let effective_posture = if app_auto_approve_enabled(app) {
        ApprovalMode::Bypass
    } else {
        app.approval_mode
    };
    !crate::core::authority::permission_posture_allows_questions(effective_posture)
}
