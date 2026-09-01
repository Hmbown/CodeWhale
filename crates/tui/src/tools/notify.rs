//! `notify` tool — model-callable desktop notification (#1322).
//!
//! Routes through the existing `tui::notifications` infrastructure (OSC 9
//! for known capable terminals, BEL fallback on macOS / Linux, `MessageBeep`
//! on Windows when explicitly opted in). The model decides when to fire —
//! the tool is intended for "long task done, come back" beats and
//! sub-agent-completion pings, not chatter.
//!
//! Auto-suppresses when `[notifications].method = "off"`. Output messages
//! are length-capped so a runaway model can't paint a paragraph into the
//! terminal title bar.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, required_str,
};
use crate::tui::notifications::{NotificationPayload, notify_done};

/// Maximum chars passed through for the title — keeps the OSC 9 escape
/// reasonable on terminals that wrap long titles awkwardly.
const NOTIFY_TITLE_CAP: usize = 80;
/// Maximum chars passed through for the body. Most receivers truncate
/// past ~120, so 200 leaves headroom while still bounded.
const NOTIFY_BODY_CAP: usize = 200;

/// Tool that fires a single desktop notification.
pub struct NotifyTool;

#[async_trait]
impl ToolSpec for NotifyTool {
    fn name(&self) -> &'static str {
        "notify"
    }

    fn description(&self) -> &'static str {
        "Send a desktop notification only when the user must act: a long task \
         completed, a blocking error needs a decision, or progress cannot \
         continue without an answer. Never notify for routine progress, \
         acknowledgements, or liveness. Pass a short `title` and optional \
         `body`. Users can silence everything with \
         `[notifications].method = \"off\"` or `[notifications].quiet = true`, \
         or this category with `[notifications.events].model-notify = false`; \
         disabled notifications are silent no-ops."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short notification title (≤ 80 chars after truncation). Required."
                },
                "body": {
                    "type": "string",
                    "description": "Optional longer body (≤ 200 chars after truncation)."
                }
            },
            "required": ["title"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        // No filesystem or shell side effects; the only output is a single
        // terminal-escape write to stdout. Mark as ReadOnly so the
        // approval-requirement default is `Auto` and the tool routes
        // through without prompting.
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let title_raw = required_str(&input, "title")?;
        let body_raw = optional_str(&input, "body")?.unwrap_or("");

        // Char-bounded truncation (not byte-bounded) so we don't slice
        // through a multi-byte sequence and emit invalid UTF-8 to the
        // terminal.
        let title: String = title_raw.chars().take(NOTIFY_TITLE_CAP).collect();
        let body: String = body_raw.chars().take(NOTIFY_BODY_CAP).collect();
        let title = title.trim();
        let body = body.trim();

        if title.is_empty() {
            return Err(ToolError::execution_failed("title must not be empty"));
        }

        // #4834: model-authored text is the least trusted input that can
        // reach Notification Center, so it goes through the typed payload
        // like every other event kind — bounded, control-byte-stripped,
        // and redacted for credentials, absolute paths, and raw tool JSON.
        let payload = NotificationPayload::model_notify(
            title,
            if body.is_empty() { None } else { Some(body) },
        );

        let in_tmux = std::env::var("TMUX")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

        // Threshold = 0 so the notification always fires; the model has
        // already decided this is the moment.
        let outcome = notify_done(
            crate::tui::notifications::configured_method(),
            in_tmux,
            &payload,
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(1),
        );

        Ok(ToolResult::success(format!(
            "{}: {title}",
            outcome.receipt()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ctx() -> ToolContext {
        ToolContext::new(Path::new("."))
    }

    #[tokio::test]
    async fn rejects_missing_title() {
        let err = NotifyTool.execute(json!({}), &ctx()).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("title"), "{err}");
    }

    #[tokio::test]
    async fn rejects_empty_title_after_trim() {
        let err = NotifyTool
            .execute(json!({"title": "   "}), &ctx())
            .await
            .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("must not be empty"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn truncates_title_to_cap() {
        let long = "x".repeat(500);
        let result = NotifyTool
            .execute(json!({"title": long}), &ctx())
            .await
            .expect("ok");
        // Confirmation message echoes the *truncated* title.
        let echo_x_count = result.content.matches('x').count();
        assert_eq!(echo_x_count, NOTIFY_TITLE_CAP);
    }

    #[tokio::test]
    async fn accepts_body_optional() {
        let result = NotifyTool
            .execute(json!({"title": "done", "body": "tests pass"}), &ctx())
            .await
            .expect("ok");
        assert!(result.success);
        assert!(result.content.contains("done"));
    }

    #[tokio::test]
    async fn safe_against_multibyte_truncation() {
        // Construct a title whose char-count is below the cap but whose
        // byte-count would be above a naive byte cap; assert no panic
        // and the success-content roundtrips the title intact.
        let title: String = "我".repeat(30); // 30 chars × 3 bytes = 90 bytes, < 80 chars cap (well, == 30 chars)
        let result = NotifyTool
            .execute(json!({"title": title.clone()}), &ctx())
            .await
            .expect("ok");
        assert!(result.content.contains(&title));
    }

    #[test]
    fn schema_exposes_title_and_body_fields() {
        let schema = NotifyTool.input_schema();
        let props = schema.get("properties").unwrap();
        assert!(props.get("title").is_some());
        assert!(props.get("body").is_some());
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("title")));
        assert!(!required.iter().any(|v| v.as_str() == Some("body")));
    }
}
