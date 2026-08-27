//! Non-blocking first-prompt session titles.
//!
//! After the first user prompt of a new thread, a cheap side completion may
//! replace the truncation title. The main turn never waits. A user rename
//! always wins. Later turns do not re-name. Failure leaves the default title.

use std::time::Duration;

use tokio::sync::mpsc;
use tracing::debug;

use crate::client::DeepSeekClient;
use crate::core::events::Event;
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, Role, SystemPrompt};
use crate::session_manager::{
    DEFAULT_SESSION_TITLE, SessionTitleSource, normalize_session_title, sanitize_session_title,
};

/// Tight output budget for the namer. Hygiene, not customer copy.
pub const NAMING_MAX_OUTPUT_TOKENS: u32 = 48;
/// Truncate the first user prompt before sending it to the namer only.
pub const NAMING_PROMPT_CHAR_LIMIT: usize = 400;
pub const NAMING_TIMEOUT_SECS: u64 = 12;

pub const NAMER_SYSTEM_PROMPT: &str = "Write a 3-6 word title for this conversation. \
Output only the title text: no quotes, no surrounding punctuation, no trailing period, \
no model name, no 'help me with'. Use the conversation's own language. \
Title Case or sentence case is fine.";

/// Whether this first prompt should start a namer job.
#[must_use]
pub fn should_auto_name(
    source: SessionTitleSource,
    first_user_prompt: &str,
    already_named: bool,
) -> bool {
    if already_named || source == SessionTitleSource::User {
        return false;
    }
    !first_user_prompt.trim().is_empty()
}

/// Truncate the first prompt for the namer only. The session record keeps
/// the full user message.
#[must_use]
pub fn namer_excerpt(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let mut excerpt: String = trimmed.chars().take(NAMING_PROMPT_CHAR_LIMIT).collect();
    if trimmed.chars().count() > NAMING_PROMPT_CHAR_LIMIT {
        excerpt.push('…');
    }
    excerpt
}

/// Strip wrapping quotes / labels models like to add. Empty after cleanup
/// means "keep the truncation title".
#[must_use]
pub fn sanitize_generated_title(raw: &str) -> Option<String> {
    let mut normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized = normalized
        .trim_matches(['"', '\'', '`', '“', '”', '‘', '’', ' ', '\n'])
        .to_string();
    if let Some(rest) = normalized.strip_prefix('#') {
        normalized = rest.trim().to_string();
    }
    for prefix in ["Title:", "title:", "Summary:", "summary:"] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            normalized = rest.trim().to_string();
        }
    }
    normalized = normalized
        .trim_end_matches(['.', '。', '!', '?'])
        .trim()
        .to_string();
    let sanitized = sanitize_session_title(&normalized);
    let candidate = sanitized.trim();
    if candidate.is_empty() || candidate.eq_ignore_ascii_case(DEFAULT_SESSION_TITLE) {
        return None;
    }
    let word_count = candidate.split_whitespace().count();
    if word_count == 0 || word_count > 8 {
        return None;
    }
    normalize_session_title(candidate).ok()
}

/// Apply a generated title only when the user has not renamed.
#[must_use]
pub fn apply_generated_title(
    current: &str,
    source: SessionTitleSource,
    generated: &str,
) -> Option<String> {
    if source == SessionTitleSource::User {
        return None;
    }
    let title = sanitize_generated_title(generated)?;
    if title == current {
        return None;
    }
    Some(title)
}

/// Fire-and-forget namer job. Failure is logged and swallowed.
pub async fn run_namer_for_session(
    session_id: String,
    spec: NamerCompletionSpec,
    client: DeepSeekClient,
    model: String,
    tx_event: mpsc::Sender<Event>,
) {
    let request = MessageRequest {
        model,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: spec.user,
                cache_control: None,
            }],
        }],
        max_tokens: spec.max_output_tokens,
        system: Some(SystemPrompt::Text(spec.system.to_string())),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: Some("off".to_string()),
        stream: Some(false),
        temperature: None,
        top_p: None,
    };

    let response = match tokio::time::timeout(
        Duration::from_secs(spec.timeout_secs),
        client.create_message(request),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => {
            tracing::warn!(target: "session_namer", "namer LLM call failed for {session_id}: {err}");
            return;
        }
        Err(_) => {
            tracing::warn!(target: "session_namer", "namer timed out for {session_id}");
            return;
        }
    };

    if crate::models::is_incomplete_stop_reason(response.stop_reason.as_deref()) {
        debug!(target: "session_namer", "incomplete namer response for {session_id}; keeping truncation title");
        return;
    }

    let raw: String = response
        .content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text { text, .. } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();

    let Some(title) = sanitize_generated_title(&raw) else {
        debug!(target: "session_namer", "namer produced no usable title for {session_id}");
        return;
    };

    let _ = tx_event
        .send(Event::SessionTitleGenerated { session_id, title })
        .await;
}

/// Fire-and-forget namer budget + prompt. The turn never waits on this.
#[must_use]
pub fn namer_completion_spec(first_user_prompt: &str) -> NamerCompletionSpec {
    NamerCompletionSpec {
        system: NAMER_SYSTEM_PROMPT,
        user: namer_excerpt(first_user_prompt),
        max_output_tokens: NAMING_MAX_OUTPUT_TOKENS,
        timeout_secs: NAMING_TIMEOUT_SECS,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamerCompletionSpec {
    pub system: &'static str,
    pub user: String,
    pub max_output_tokens: u32,
    pub timeout_secs: u64,
}

/// Catalog-driven cheap route for naming. Flash/cheap family defaults only;
/// suffix variants are never treated as a family default here.
#[must_use]
pub fn naming_model_hint(flash_or_cheap: Option<&str>, session_model: &str) -> String {
    flash_or_cheap
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(session_model)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_empty_and_user_titles() {
        assert!(!should_auto_name(
            SessionTitleSource::Truncation,
            "   ",
            false
        ));
        assert!(!should_auto_name(
            SessionTitleSource::User,
            "Fix the login",
            false
        ));
        assert!(!should_auto_name(
            SessionTitleSource::Truncation,
            "Fix the login",
            true
        ));
        assert!(should_auto_name(
            SessionTitleSource::Truncation,
            "Fix the login",
            false
        ));
    }

    #[test]
    fn first_prompt_only() {
        assert!(should_auto_name(
            SessionTitleSource::Generated,
            "second question",
            false
        ));
        assert!(!should_auto_name(
            SessionTitleSource::Generated,
            "second question",
            true
        ));
    }

    #[test]
    fn user_rename_wins() {
        assert_eq!(
            apply_generated_title("My Title", SessionTitleSource::User, "Auto Login Fix"),
            None
        );
    }

    #[test]
    fn generated_title_replaces_truncation() {
        assert_eq!(
            apply_generated_title(
                "help me with the login bug please",
                SessionTitleSource::Truncation,
                "\"Login Bug Fix.\""
            )
            .as_deref(),
            Some("Login Bug Fix")
        );
    }

    #[test]
    fn namer_failure_keeps_default() {
        assert_eq!(
            apply_generated_title("New Session", SessionTitleSource::Truncation, ""),
            None
        );
        assert_eq!(
            apply_generated_title("New Session", SessionTitleSource::Truncation, "   "),
            None
        );
    }

    #[test]
    fn long_prompt_is_truncated_for_namer_only() {
        let long = "word ".repeat(200);
        let excerpt = namer_excerpt(&long);
        assert!(excerpt.chars().count() <= NAMING_PROMPT_CHAR_LIMIT + 1);
        assert!(excerpt.ends_with('…'));
    }

    #[test]
    fn namer_job_uses_tight_budget() {
        let spec = namer_completion_spec("Fix the login bug in auth.rs");
        assert_eq!(spec.max_output_tokens, 48);
        assert_eq!(spec.timeout_secs, 12);
        assert!(spec.system.contains("3-6 word title"));
        assert!(spec.user.contains("Fix the login"));
    }

    #[test]
    fn naming_model_uses_catalog_cheap_route() {
        assert_eq!(
            naming_model_hint(Some("provider-flash"), "expensive-model"),
            "provider-flash"
        );
        assert_eq!(naming_model_hint(None, "session-model"), "session-model");
        assert_eq!(
            naming_model_hint(Some("  "), "session-model"),
            "session-model"
        );
    }
}
