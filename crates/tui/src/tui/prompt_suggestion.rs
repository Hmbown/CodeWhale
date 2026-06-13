//! Ghost-text follow-up prompt suggestion.
//!
//! After each completed turn, a lightweight API call generates ONE short
//! follow-up question the user might want to ask next. The suggestion is
//! rendered as dimmed ghost text in the composer when the input is empty.

use std::sync::OnceLock;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use tracing::debug;

/// Reusable static client — avoids creating a new connection pool per request.
fn suggestion_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Generate a follow-up prompt suggestion based on recent messages.
///
/// Sends the conversation summary to the API with a system prompt that
/// asks for a single short follow-up question. Returns `None` on failure
/// or empty result — callers treat this as best-effort.
pub async fn generate_suggestion(
    api_key: &str,
    base_url: &str,
    model: &str,
    recent_messages: &str,
) -> Option<String> {
    let client = suggestion_client();
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "\
    You are a helpful assistant. Based on the recent conversation context, generate \
    ONE short follow-up question (under 60 characters) the user might want to ask \
    next. Reply with ONLY the question text, nothing else — no quotes, no explanations, \
    no prefixes."
            },
            {
                "role": "user",
                "content": format!(
                    "Recent conversation:\n{recent_messages}\n\n\
                     Generate ONE short follow-up question the user might ask next:"
                )
            }
        ],
        "max_tokens": 64,
        "temperature": 0.3,
        "stream": false
    });

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    debug!(%url, %model, "generating prompt suggestion");
    let response = match client
        .post(&url)
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .header(CONTENT_TYPE, "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return None,
    };

    let value: Value = match response.json().await {
        Ok(v) => v,
        Err(_) => return None,
    };

    let suggestion = value["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty() && s.len() <= 200)?;

    debug!(text = %suggestion, "prompt suggestion generated");
    Some(suggestion)
}

/// Extract the first text line from a single message.
fn message_summary(m: &crate::models::Message) -> Option<String> {
    let role = match m.role.as_str() {
        "user" => "User",
        "assistant" => "Assistant",
        _ => return None,
    };
    let text = m
        .content
        .iter()
        .filter_map(|block| match block {
            crate::models::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    let first_line = text.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return None;
    }
    let truncated: String = first_line
        .chars()
        .take(120)
        .chain(if first_line.chars().count() > 120 {
            Some('…')
        } else {
            None
        })
        .collect();
    Some(format!("{role}: {truncated}"))
}

/// Build a one-line-per-message summary of recent conversation context.
/// Takes the last N messages, skipping tool-only messages.
pub fn summarize_recent_messages(messages: &[crate::models::Message], limit: usize) -> String {
    let start = messages.len().saturating_sub(limit);
    messages[start..]
        .iter()
        .filter_map(message_summary)
        .collect::<Vec<_>>()
        .join("\n")
}
