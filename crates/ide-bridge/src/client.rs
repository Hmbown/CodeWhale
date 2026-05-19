//! WebSocket client for the IDE bridge.
//!
//! Mirrors opencode's editor-context client: JSON-RPC 2.0 over a loopback
//! WebSocket, framed by the MCP `initialize` → `notifications/initialized`
//! handshake. Server-pushed `selection_changed` events are cached so the
//! TUI can include the latest editor selection in prompts without an
//! explicit round-trip.

use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::discovery::{BridgeAuth, BridgeTarget};
use crate::protocol::{Diagnostic, SelectionChange, WorkspaceFolders};
use crate::{Error, IDE_BRIDGE_AUTHORIZATION_HEADER, MCP_PROTOCOL_VERSION, Result};

/// Hard cap on the cached `SelectionChange.text` size. The IDE host can
/// publish arbitrarily large selections (e.g. user hits Ctrl+A on a big
/// log file). Truncating *at the cache boundary* — not just at render
/// time — bounds the memory footprint of the always-resident selection
/// snapshot, every cache deduplication compare, and any future feature
/// that reads `latest_selection()`.
///
/// The render-time limit (`MAX_SELECTED_TEXT_BYTES` in `tui/src/ide_bridge.rs`)
/// is much smaller — this is a defensive cap, not a presentation limit.
const MAX_CACHED_SELECTION_BYTES: usize = 64 * 1024;

/// Hard cap on inbound WebSocket message / frame size. The IDE bridge
/// is loopback-only and only sends modestly-sized JSON-RPC frames; any
/// frame larger than 8 MiB is either a buggy or malicious peer. Without
/// this, tungstenite defaults to 64 MiB / 16 MiB which is enough to
/// crash a TUI session before it can even disconnect.
const MAX_INBOUND_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub handshake_timeout: Duration,
    pub call_timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        // Match what the TUI actually uses so callers relying on
        // defaults don't surprise themselves with a longer timeout.
        Self {
            handshake_timeout: Duration::from_secs(3),
            call_timeout: Duration::from_secs(3),
        }
    }
}

fn ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(MAX_INBOUND_FRAME_BYTES),
        max_frame_size: Some(MAX_INBOUND_FRAME_BYTES),
        ..Default::default()
    }
}

type PendingMap = HashMap<u64, oneshot::Sender<Value>>;

struct Inner {
    selection: StdRwLock<Option<SelectionChange>>,
    /// Monotonic counter bumped every time `selection` is mutated. Used as a
    /// SolidJS-style "signal" — receivers can park on a `watch::Receiver`
    /// and react when the cached selection changes, without polling.
    selection_seq: tokio::sync::watch::Sender<u64>,
    pending: Mutex<PendingMap>,
    next_id: std::sync::atomic::AtomicU64,
    tx: mpsc::Sender<String>,
    shutdown: Notify,
    reader_handle: Mutex<Option<JoinHandle<()>>>,
    writer_handle: Mutex<Option<JoinHandle<()>>>,
    call_timeout: Duration,
    ide_name: Option<String>,
}

#[derive(Clone)]
pub struct IdeBridgeClient {
    inner: Arc<Inner>,
}

impl IdeBridgeClient {
    pub async fn connect(target: &BridgeTarget, opts: ConnectOptions) -> Result<Self> {
        let request = build_ws_request(&target.ws_url(), &target.auth)?;

        let (ws, _) = tokio::time::timeout(
            opts.handshake_timeout,
            tokio_tungstenite::connect_async_with_config(request, Some(ws_config()), false),
        )
        .await
        .map_err(|_| Error::Connect("handshake timed out".into()))?
        .map_err(|e| Error::Connect(e.to_string()))?;

        let (sink, stream) = ws.split();
        let (tx, rx) = mpsc::channel::<String>(64);
        let (selection_seq, _) = tokio::sync::watch::channel::<u64>(0);

        let inner = Arc::new(Inner {
            selection: StdRwLock::new(None),
            selection_seq,
            pending: Mutex::new(HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            tx,
            shutdown: Notify::new(),
            reader_handle: Mutex::new(None),
            writer_handle: Mutex::new(None),
            call_timeout: opts.call_timeout,
            ide_name: target.ide_name.clone(),
        });

        let w = Arc::clone(&inner);
        let writer_handle = tokio::spawn(run_writer(w, sink, rx));

        let r = Arc::clone(&inner);
        let reader_handle = tokio::spawn(run_reader(r, stream));

        *inner.writer_handle.lock().await = Some(writer_handle);
        *inner.reader_handle.lock().await = Some(reader_handle);

        let client = Self { inner };
        if let Err(err) = client.handshake(opts.handshake_timeout).await {
            client.shutdown().await;
            return Err(err);
        }
        Ok(client)
    }

    pub fn ide_name(&self) -> Option<&str> {
        self.inner.ide_name.as_deref()
    }

    pub fn latest_selection(&self) -> Option<SelectionChange> {
        self.inner.selection.read().ok()?.clone()
    }

    /// Borrow the cached selection without cloning. The closure runs
    /// while a read lock is held, so callers must keep the closure body
    /// fast (no awaits, no further `latest_selection` reentry). Returns
    /// `None` when the cache is empty.
    pub fn with_latest_selection<R>(&self, f: impl FnOnce(&SelectionChange) -> R) -> Option<R> {
        let guard = self.inner.selection.read().ok()?;
        guard.as_ref().map(f)
    }

    /// Subscribe to a SolidJS-style "selection changed" signal. The
    /// returned receiver advances by one every time the cached selection
    /// is mutated (server push, explicit `getCurrentSelection`, etc.).
    /// Callers should call `borrow_and_update()` / `changed().await` and
    /// then read `latest_selection()` to fetch the new value.
    pub fn subscribe_selection(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.selection_seq.subscribe()
    }

    pub async fn get_current_selection(&self) -> Result<SelectionChange> {
        let result = self.call_tool("getCurrentSelection", json!({})).await?;
        let sel: SelectionChange = decode_tool_json(&result)?;
        update_selection(&self.inner, Some(sel.clone()));
        Ok(sel)
    }

    pub async fn get_latest_selection(&self) -> Result<SelectionChange> {
        let result = self.call_tool("getLatestSelection", json!({})).await?;
        let sel: SelectionChange = decode_tool_json(&result)?;
        update_selection(&self.inner, Some(sel.clone()));
        Ok(sel)
    }

    pub async fn get_workspace_folders(&self) -> Result<WorkspaceFolders> {
        let result = self.call_tool("getWorkspaceFolders", json!({})).await?;
        decode_tool_json(&result)
    }

    pub async fn get_diagnostics(&self, uri: Option<&str>) -> Result<Vec<Diagnostic>> {
        let args = match uri {
            Some(u) => json!({ "uri": u }),
            None => json!({}),
        };
        let result = self.call_tool("getDiagnostics", args).await?;
        decode_tool_json(&result)
    }

    pub async fn close_all_diff_tabs(&self) -> Result<String> {
        let result = self.call_tool("closeAllDiffTabs", json!({})).await?;
        extract_text(&result)
    }

    pub async fn shutdown(&self) {
        self.inner.shutdown.notify_waiters();
        if let Some(h) = self.inner.writer_handle.lock().await.take() {
            h.abort();
        }
        if let Some(h) = self.inner.reader_handle.lock().await.take() {
            h.abort();
        }
    }

    // ── Private ──────────────────────────────────────────────────────────

    async fn handshake(&self, timeout: Duration) -> Result<()> {
        let result = self
            .send_request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "deepseek-tui",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
                Some(timeout),
            )
            .await
            .map_err(|e| match e {
                Error::Timeout(_) => Error::Handshake("initialize timed out".into()),
                other => Error::Handshake(other.to_string()),
            })?;

        if let Some(v) = result.get("protocolVersion").and_then(Value::as_str) {
            tracing::debug!(protocol_version = v, "IDE bridge initialized");
        }

        // Per MCP spec / opencode: the client follows up with
        // `notifications/initialized` and nothing else. The previous
        // `ide_connected` notification was a DeepSeek-only extension that
        // no third-party IDE host actually consumes — drop it.
        self.send_notification("notifications/initialized", json!({}))
            .await
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        let timeout = self.inner.call_timeout;
        let result = self
            .send_request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                Some(timeout),
            )
            .await?;
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(Error::ToolError(extract_text(&result)?));
        }
        Ok(result)
    }

    async fn send_request(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let id = self
            .inner
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        if self.inner.tx.send(payload.to_string()).await.is_err() {
            self.inner.pending.lock().await.remove(&id);
            return Err(Error::Closed);
        }

        let response = match timeout {
            Some(duration) => match tokio::time::timeout(duration, rx).await {
                Ok(Ok(response)) => response,
                Ok(Err(_)) => {
                    self.inner.pending.lock().await.remove(&id);
                    return Err(Error::Closed);
                }
                Err(_) => {
                    self.inner.pending.lock().await.remove(&id);
                    return Err(Error::Timeout(duration));
                }
            },
            None => rx.await.map_err(|_| Error::Closed)?,
        };
        if let Some(err) = response.get("error") {
            return Err(Error::Call(format_jsonrpc_error(err)));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.inner
            .tx
            .send(payload.to_string())
            .await
            .map_err(|_| Error::Closed)?;
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Background tasks
// ──────────────────────────────────────────────────────────────────────────────

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Message,
>;

type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

async fn run_writer(inner: Arc<Inner>, mut sink: WsSink, mut rx: mpsc::Receiver<String>) {
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(text) => {
                    if sink.send(tokio_tungstenite::tungstenite::Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            _ = inner.shutdown.notified() => break,
        }
    }
}

async fn run_reader(inner: Arc<Inner>, mut stream: WsStream) {
    loop {
        tokio::select! {
            frame = stream.next() => match frame {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                    dispatch_frame(&inner, &text).await;
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                _ => {}
            },
            _ = inner.shutdown.notified() => break,
        }
    }
    // Drain pending requests so callers don't hang.
    for (_, tx) in inner.pending.lock().await.drain() {
        let _ = tx.send(json!({ "error": { "code": -1, "message": "closed" } }));
    }
}

async fn dispatch_frame(inner: &Inner, text: &str) {
    // Single-pass deserialize: peek at `id` first because responses are
    // by far the hot path (every tools/call answer comes through here),
    // then fall back to the notification shape. This avoids parsing the
    // whole frame into a `Value` and cloning `params` for every frame.
    if let Ok(resp) = serde_json::from_str::<RawResponse>(text)
        && let Some(id) = resp.id
    {
        if let Some(tx) = inner.pending.lock().await.remove(&id) {
            // Re-parse the original frame as a Value only when we have
            // a waiter — there's no point materialising it otherwise.
            // The cost shows up only on requests we actually issued,
            // not on every server push.
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                let _ = tx.send(value);
            }
        }
        return;
    }

    if let Ok(notif) = serde_json::from_str::<SelectionChangedNotification>(text)
        && notif.method == "selection_changed"
    {
        update_selection(inner, Some(notif.params));
    }
}

#[derive(serde::Deserialize)]
struct RawResponse {
    #[serde(default)]
    id: Option<u64>,
}

#[derive(serde::Deserialize)]
struct SelectionChangedNotification {
    method: String,
    #[serde(default)]
    params: SelectionChange,
}

/// Write the new selection into the shared cache and bump the watch
/// signal so subscribers wake up.  Skips the wakeup when the cached
/// value is byte-identical to the incoming one — IDEs frequently re-emit
/// `selection_changed` with the same payload (e.g. on focus events).
///
/// The incoming `selection.text` is truncated at the cache boundary so
/// pathological IDE pushes (50 MiB selection on Ctrl+A) cannot inflate
/// resident memory or turn dedup compares into multi-megabyte memcmps.
fn update_selection(inner: &Inner, mut selection: Option<SelectionChange>) {
    if let Some(s) = selection.as_mut() {
        truncate_selection_text_in_place(&mut s.text, MAX_CACHED_SELECTION_BYTES);
    }

    if let Ok(mut guard) = inner.selection.write() {
        if !selection_changed(guard.as_ref(), selection.as_ref()) {
            return;
        }
        *guard = selection;
    } else {
        return;
    }
    let next = inner.selection_seq.borrow().wrapping_add(1);
    let _ = inner.selection_seq.send(next);
}

/// Cheap field-wise compare to skip the duplicate-push case (IDEs
/// frequently replay the same payload on focus changes). Falls through
/// to `String`'s `PartialEq`, which is itself O(1) on length mismatch
/// before the byte compare, so an explicit length check would be
/// redundant.
fn selection_changed(prev: Option<&SelectionChange>, next: Option<&SelectionChange>) -> bool {
    match (prev, next) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(a), Some(b)) => {
            a.file_path != b.file_path
                || a.file_url != b.file_url
                || a.selection != b.selection
                || a.text != b.text
        }
    }
}

/// Truncate at a UTF-8 char boundary in place. Leaves a short marker so
/// downstream renderers know the payload was clamped.
fn truncate_selection_text_in_place(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = text.len() - end;
    text.truncate(end);
    text.push_str(&format!(
        "\n... [{dropped} bytes truncated by ide-bridge cache]"
    ));
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

fn build_ws_request(
    url: &str,
    auth: &BridgeAuth,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    let uri: tokio_tungstenite::tungstenite::http::Uri = url
        .parse()
        .map_err(|e| Error::Connect(format!("invalid url: {e}")))?;
    let host = uri.authority().map(|a| a.to_string()).unwrap_or_default();

    let mut builder = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(url)
        .header("Host", &host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Protocol", "mcp");

    if let BridgeAuth::Token(token) = auth {
        builder = builder.header(IDE_BRIDGE_AUTHORIZATION_HEADER, token.as_str());
    }

    builder
        .body(())
        .map_err(|e| Error::Connect(format!("build request: {e}")))
}

fn format_jsonrpc_error(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match error.get("code").and_then(Value::as_i64) {
        Some(code) => format!("{message} ({code})"),
        None => message.to_string(),
    }
}

fn extract_text(result: &Value) -> Result<String> {
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = block.get("text").and_then(Value::as_str)
            {
                return Ok(text.to_string());
            }
        }
    }
    if let Some(text) = result.as_str() {
        return Ok(text.to_string());
    }
    Ok(result.to_string())
}

fn decode_tool_json<T: serde::de::DeserializeOwned>(result: &Value) -> Result<T> {
    if let Some(structured) = result.get("structuredContent") {
        return serde_json::from_value(structured.clone())
            .map_err(|e| Error::Decode(e.to_string()));
    }
    if result.is_object()
        && result.get("content").is_none()
        && let Ok(value) = serde_json::from_value(result.clone())
    {
        return Ok(value);
    }
    let text = extract_text(result)?;
    serde_json::from_str(&text).map_err(|e| Error::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::RwLock as StdRwLock;

    fn test_client(call_timeout: Duration) -> IdeBridgeClient {
        let (client, mut rx) = test_client_with_rx(call_timeout);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        client
    }

    fn test_client_with_rx(call_timeout: Duration) -> (IdeBridgeClient, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel::<String>(8);
        let (selection_seq, _) = tokio::sync::watch::channel::<u64>(0);
        let client = IdeBridgeClient {
            inner: Arc::new(Inner {
                selection: StdRwLock::new(None),
                selection_seq,
                pending: Mutex::new(HashMap::new()),
                next_id: std::sync::atomic::AtomicU64::new(1),
                tx,
                shutdown: Notify::new(),
                reader_handle: Mutex::new(None),
                writer_handle: Mutex::new(None),
                call_timeout,
                ide_name: None,
            }),
        };
        (client, rx)
    }

    #[tokio::test]
    async fn handshake_uses_mcp_version_and_only_emits_initialized() {
        let (client, mut rx) = test_client_with_rx(Duration::from_secs(1));
        let responder = client.clone();
        let capture = tokio::spawn(async move {
            let init = rx.recv().await.expect("initialize request");
            let init_json: Value = serde_json::from_str(&init).unwrap();
            assert_eq!(init_json["method"], "initialize");
            assert_eq!(
                init_json["params"]["protocolVersion"],
                crate::MCP_PROTOCOL_VERSION
            );

            let id = init_json["id"].as_u64().unwrap();
            let tx = responder.inner.pending.lock().await.remove(&id).unwrap();
            tx.send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocolVersion": crate::MCP_PROTOCOL_VERSION }
            }))
            .unwrap();

            let initialized: Value =
                serde_json::from_str(&rx.recv().await.expect("initialized notification")).unwrap();

            // Confirm there is no follow-up notification (e.g. legacy ide_connected).
            let lingering = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
            assert!(lingering.is_err(), "no extra notifications expected");

            initialized
        });

        client.handshake(Duration::from_secs(1)).await.unwrap();
        let initialized = capture.await.unwrap();
        assert_eq!(initialized["method"], "notifications/initialized");
    }

    #[tokio::test]
    async fn timed_out_request_is_removed_from_pending() {
        let client = test_client(Duration::from_millis(1));
        let err = client
            .send_request("tools/call", json!({}), Some(Duration::from_millis(1)))
            .await
            .unwrap_err();

        assert!(matches!(err, Error::Timeout(_)));
        assert!(client.inner.pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn selection_changed_push_advances_subscriber() {
        let client = test_client(Duration::from_secs(1));
        let mut rx = client.subscribe_selection();
        let initial = *rx.borrow_and_update();

        // Simulate a server push.
        let frame = json!({
            "jsonrpc": "2.0",
            "method": "selection_changed",
            "params": {
                "filePath": "/tmp/a.rs",
                "text": "foo",
                "selection": {
                    "start": { "line": 1, "character": 2 },
                    "end":   { "line": 1, "character": 7 }
                }
            }
        })
        .to_string();
        super::dispatch_frame(&client.inner, &frame).await;

        // Watch advanced exactly once and the cache reflects the new value.
        rx.changed().await.unwrap();
        assert_ne!(*rx.borrow(), initial);
        let cached = client.latest_selection().unwrap();
        assert_eq!(cached.file_path.as_deref(), Some("/tmp/a.rs"));

        // Replaying the same frame must NOT bump the signal again — IDEs
        // frequently re-emit identical selections on focus changes.
        super::dispatch_frame(&client.inner, &frame).await;
        let lingering = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
        assert!(
            lingering.is_err(),
            "duplicate push must not wake subscribers"
        );
    }

    #[test]
    fn decode_tool_json_accepts_structured_content() {
        let result = json!({
            "structuredContent": {
                "filePath": "/tmp/a.rs",
                "text": "selected"
            }
        });
        let selection: SelectionChange = decode_tool_json(&result).unwrap();
        assert_eq!(selection.file_path.as_deref(), Some("/tmp/a.rs"));
        assert_eq!(selection.text, "selected");
    }

    #[test]
    fn decode_tool_json_accepts_direct_json() {
        let result = json!({
            "filePath": "/tmp/a.rs",
            "text": "selected"
        });
        let selection: SelectionChange = decode_tool_json(&result).unwrap();
        assert_eq!(selection.file_path.as_deref(), Some("/tmp/a.rs"));
        assert_eq!(selection.text, "selected");
    }

    #[test]
    fn decode_tool_json_accepts_mcp_text_content() {
        let result = json!({
            "content": [
                {
                    "type": "text",
                    "text": "{\"filePath\":\"/tmp/a.rs\",\"text\":\"selected\"}"
                }
            ]
        });
        let selection: SelectionChange = decode_tool_json(&result).unwrap();
        assert_eq!(selection.file_path.as_deref(), Some("/tmp/a.rs"));
        assert_eq!(selection.text, "selected");
    }

    #[test]
    fn jsonrpc_errors_include_code_when_available() {
        let error = json!({ "code": -32601, "message": "method not found" });
        assert_eq!(format_jsonrpc_error(&error), "method not found (-32601)");
    }

    #[test]
    fn ws_request_uses_ide_auth_header_and_mcp_protocol() {
        let request =
            build_ws_request("ws://127.0.0.1:4242/", &BridgeAuth::Token("secret".into())).unwrap();
        let headers = request.headers();

        assert_eq!(
            headers
                .get(IDE_BRIDGE_AUTHORIZATION_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("secret")
        );
        assert_eq!(
            headers
                .get("Sec-WebSocket-Protocol")
                .and_then(|value| value.to_str().ok()),
            Some("mcp")
        );
    }

    #[test]
    fn cached_selection_text_is_truncated_at_boundary() {
        let mut text = "a".repeat(MAX_CACHED_SELECTION_BYTES + 1024);
        truncate_selection_text_in_place(&mut text, MAX_CACHED_SELECTION_BYTES);
        assert!(text.len() < MAX_CACHED_SELECTION_BYTES + 256);
        assert!(text.contains("bytes truncated by ide-bridge cache"));
    }

    #[test]
    fn truncate_preserves_utf8_boundary() {
        // Each "深" is 3 bytes in UTF-8. Limit chosen to land mid-codepoint.
        let mut text = "深".repeat(10);
        truncate_selection_text_in_place(&mut text, 4);
        // Must not panic and must remain valid UTF-8 (String invariant).
        assert!(text.starts_with('\u{6df1}'));
        assert!(text.contains("bytes truncated"));
    }

    #[test]
    fn cache_truncates_giant_inbound_selection() {
        let (selection_seq, _) = tokio::sync::watch::channel::<u64>(0);
        let (tx, _rx) = mpsc::channel::<String>(1);
        let inner = Inner {
            selection: StdRwLock::new(None),
            selection_seq,
            pending: Mutex::new(HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            tx,
            shutdown: Notify::new(),
            reader_handle: Mutex::new(None),
            writer_handle: Mutex::new(None),
            call_timeout: Duration::from_secs(1),
            ide_name: None,
        };

        let huge = SelectionChange {
            file_path: Some("/tmp/big.log".into()),
            file_url: None,
            text: "x".repeat(MAX_CACHED_SELECTION_BYTES * 4),
            selection: None,
        };
        update_selection(&inner, Some(huge));

        let cached = inner.selection.read().unwrap().clone().unwrap();
        assert!(cached.text.len() < MAX_CACHED_SELECTION_BYTES + 256);
    }
}
