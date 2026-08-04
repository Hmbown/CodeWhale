//! Transport. One POST, or — by default — a local file.
//!
//! **No production endpoint is approved, and the shipped default is unset.**
//! Unset means the dry-run sink: batches are serialized with the same serializer
//! a real endpoint would see and appended to `dryrun.jsonl`, and no HTTP client
//! is ever constructed. That is the dogfood gate as the *default configuration*
//! rather than a separate mode — you read your own payloads by reading the file.

use std::path::Path;
use std::time::Duration;

use crate::buffer;
use crate::event::Batch;

/// Transport timeout, matching the release-metadata timeout.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// What happened to a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// Written to `dryrun.jsonl`.
    DryRun,
    /// Accepted by the endpoint.
    Accepted,
    /// Dropped. No retry, no backoff, no re-queue — a permanently offline
    /// machine attempts at most once per flush interval and never grows a
    /// queue.
    Dropped,
}

/// Serialize and deliver one batch.
///
/// The tombstone is re-checked immediately before delivery, so a wipe that
/// landed while the batch was being assembled still stops it.
pub fn send(root: &Path, endpoint: Option<&str>, batch: &Batch) -> SendOutcome {
    if buffer::tombstone_present(root) {
        return SendOutcome::Dropped;
    }
    let Ok(body) = serde_json::to_string(batch) else {
        return SendOutcome::Dropped;
    };
    match endpoint {
        None => {
            let path = buffer::dryrun_path(root);
            match buffer::append_locked(root, &path, &body) {
                Some(()) => SendOutcome::DryRun,
                None => SendOutcome::Dropped,
            }
        }
        Some(endpoint) => post(endpoint, &batch.app_version, body),
    }
}

/// A single first-party POST.
///
/// The client is built through `codewhale_release::platform_blocking_http_client_builder`,
/// never by hand: `reqwest` is pinned workspace-wide with `rustls-no-provider`,
/// so a construction that skips the provider install silently never connects on
/// some platforms — indistinguishable from fail-open, which means no test that
/// merely asserts "does not crash" would catch it. Android additionally needs
/// the webpki-roots swap, which that builder owns.
///
/// No cookies, no redirects, no auth header, no custom headers. The response
/// body is discarded and only the status class is read: this client must never
/// be made to depend on a server response.
fn post(endpoint: &str, app_version: &str, body: String) -> SendOutcome {
    let client = codewhale_release::platform_blocking_http_client_builder()
        .timeout(SEND_TIMEOUT)
        // No cookie store exists to disable: `reqwest` is pinned workspace-wide
        // without the `cookies` feature, so there is no jar to carry state
        // between batches even if a server tried to set one.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("codewhale-telemetry/{app_version}"))
        .build();
    let Ok(client) = client else {
        return SendOutcome::Dropped;
    };
    match client
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
    {
        Ok(response) if response.status().is_success() => SendOutcome::Accepted,
        _ => SendOutcome::Dropped,
    }
}
