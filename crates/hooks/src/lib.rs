use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use codewhale_protocol::EventFrame;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

mod lifecycle_outbox;

pub use lifecycle_outbox::{
    LifecycleEvent, LifecycleOutbox, OUTBOX_DETAIL_MAX_CHARS, OUTBOX_HEADLINE_MAX_CHARS,
    OUTBOX_PREVIEW_MAX_CHARS, OUTBOX_TRUNCATION_MARKER, bounded_text,
};

/// All events that can be emitted through the hook system.
///
/// Each variant represents a distinct lifecycle or streaming event. The enum is
/// serialised with a `"type"` discriminator using `snake_case` naming (e.g.
/// `"response_start"`, `"tool_lifecycle"`), making it easy to consume from
/// JSON-based log files or webhook receivers.
#[allow(clippy::large_enum_variant)] // Keep the public HookEvent shape stable for 0.8.x.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookEvent {
    /// A new response stream has started.
    ResponseStart {
        /// Unique identifier for the response being streamed.
        response_id: String,
    },
    /// A chunk of text has been received for an in-progress response.
    ResponseDelta {
        /// Unique identifier for the response being streamed.
        response_id: String,
        /// The incremental text content of this chunk.
        delta: String,
    },
    /// A response stream has finished.
    ResponseEnd {
        /// Unique identifier for the response that completed.
        response_id: String,
    },
    /// A tool invocation has transitioned to a new phase (e.g. start, end, error).
    ToolLifecycle {
        /// Identifier of the response under which the tool was invoked.
        response_id: String,
        /// Name of the tool (e.g. `"shell"`, `"read_file"`).
        tool_name: String,
        /// Current phase of the tool execution (e.g. `"start"`, `"end"`).
        phase: String,
        /// Arbitrary structured payload associated with this phase.
        payload: Value,
    },
    /// A background job has transitioned to a new phase.
    JobLifecycle {
        /// Unique identifier of the job.
        job_id: String,
        /// Current phase of the job (e.g. `"queued"`, `"running"`, `"done"`).
        phase: String,
        /// Optional progress percentage (0-100).
        progress: Option<u8>,
        /// Optional human-readable detail about the current phase.
        detail: Option<String>,
    },
    /// An approval request has transitioned to a new phase.
    ApprovalLifecycle {
        /// Unique identifier of the approval request.
        approval_id: String,
        /// Current phase (e.g. `"requested"`, `"approved"`, `"denied"`).
        phase: String,
        /// Optional reason explaining the current phase.
        reason: Option<String>,
    },
    /// A catch-all variant that wraps an arbitrary [`EventFrame`].
    ///
    /// Use this when you need to forward a protocol-level event frame without
    /// mapping it to a more specific variant.
    GenericEventFrame {
        /// The raw event frame to forward.
        frame: Box<EventFrame>,
    },
}

impl HookEvent {
    /// Serialise this event into a [`serde_json::Value`].
    ///
    /// Returns a JSON object with the `"type"` discriminator and all variant
    /// fields. If serialisation fails (which should be extremely rare), a
    /// fallback `{"type":"serialization_error"}` value is returned instead of
    /// panicking.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({"type":"serialization_error"}))
    }
}

/// A destination that can receive [`HookEvent`]s.
///
/// Implementors handle the transport-specific details of delivering events
/// (writing to stdout, appending to a file, POSTing to a webhook, etc.).
/// The [`HookDispatcher`] fans out every event to all registered sinks, so a
/// single process can log to multiple destinations simultaneously.
///
/// Sinks are expected to be **best-effort**: implementations should avoid
/// panicking and should return an [`anyhow::Error`] only for truly unexpected
/// failures. [`HookDispatcher::emit`] discards individual sink errors so hook
/// delivery failures do not abort the application.
#[async_trait]
pub trait HookSink: Send + Sync {
    /// Deliver a single event to this sink.
    ///
    /// Implementations should be resilient to transient failures (e.g. a
    /// missing listener) and should not block the caller for extended periods.
    async fn emit(&self, event: &HookEvent) -> Result<()>;
}

/// A [`HookSink`] that prints each event as a single JSON line to stdout.
///
/// Useful for local development and debugging. Events are printed via
/// [`println!`] so they appear interleaved with other program output.
#[derive(Default)]
pub struct StdoutHookSink;

#[async_trait]
impl HookSink for StdoutHookSink {
    async fn emit(&self, event: &HookEvent) -> Result<()> {
        println!("{}", event.to_json());
        Ok(())
    }
}

/// A [`HookSink`] that appends each event as a JSON line to a file.
///
/// The file is created (along with any missing parent directories) on the
/// first emitted event. Each line is a JSON object of the form
/// `{"at": "<ISO 8601 timestamp>", "event": {...}}`.
///
/// Concurrent [`emit`](HookSink::emit) calls serialize on an internal mutex so
/// that each JSON event is written and flushed as a complete line; without that
/// lock, overlapping `write_all` calls can interleave partial lines and corrupt
/// the JSONL log (see issue #4739).
pub struct JsonlHookSink {
    path: PathBuf,
    /// Serializes open+append+flush so concurrent tool-call emits cannot
    /// interleave bytes mid-line.
    write_lock: tokio::sync::Mutex<()>,
}

impl JsonlHookSink {
    /// Create a new sink that writes to the file at `path`.
    ///
    /// Parent directories are created lazily on the first [`HookSink::emit`]
    /// call.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: tokio::sync::Mutex::new(()),
        }
    }
}

#[async_trait]
impl HookSink for JsonlHookSink {
    async fn emit(&self, event: &HookEvent) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create hook log directory {}", parent.display())
            })?;
        }
        // Encode outside the lock so only I/O is serialized.
        let payload = json!({
            "at": Utc::now().to_rfc3339(),
            "event": event
        });
        let encoded = serde_json::to_string(&payload).context("failed to encode hook event")?;

        let _guard = self.write_lock.lock().await;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .with_context(|| format!("failed to open hook log {}", self.path.display()))?;
        file.write_all(encoded.as_bytes())
            .await
            .context("failed to write hook event")?;
        file.write_all(b"\n")
            .await
            .context("failed to write hook event newline")?;
        // Flush before drop so sequential emits (and tests that read the
        // file immediately after) observe every completed line. Holding the
        // mutex through flush guarantees concurrent writers never observe a
        // partial line at EOF.
        file.flush().await.context("failed to flush hook event")?;
        Ok(())
    }
}

/// A [`HookSink`] that POSTs each event as JSON to a remote HTTP endpoint.
///
/// The request body is `{"at": "<ISO 8601 timestamp>", "event": {...}}`.
/// Failed requests are retried up to 2 times with exponential back-off
/// (200 ms, 400 ms). After exhausting retries the error is propagated.
#[derive(Clone)]
pub struct WebhookHookSink {
    url: String,
    /// Optional bearer token sent as `Authorization: Bearer <token>`.
    bearer_token: Option<String>,
    client: reqwest::Client,
}

impl WebhookHookSink {
    /// Create a new sink that sends events to the given `url`.
    pub fn new(url: String) -> Self {
        Self::new_with_token(url, None)
    }

    /// Create a new sink that sends events to the given `url`, attaching
    /// `Authorization: Bearer <token>` when a token is provided.
    pub fn new_with_token(url: String, bearer_token: Option<String>) -> Self {
        Self {
            url,
            bearer_token,
            client: codewhale_release::platform_http_client_builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| {
                    codewhale_release::platform_http_client_builder()
                        .build()
                        .unwrap_or_else(|_| reqwest::Client::new())
                }),
        }
    }

    /// POST an arbitrary JSON payload to the configured endpoint.
    ///
    /// This is the shared delivery path behind both [`HookSink::emit`] and
    /// the lifecycle outbox fan-out. It is deliberately not part of the
    /// [`HookSink`] trait: outbox events are runtime event envelopes, not
    /// [`HookEvent`]s, and only the transport needs to be shared.
    pub async fn post_payload(&self, payload: serde_json::Value) -> Result<()> {
        let mut retries = 0usize;
        loop {
            let mut request = self.client.post(&self.url).json(&payload);
            if let Some(token) = self.bearer_token.as_deref().filter(|t| !t.is_empty()) {
                request = request.bearer_auth(token);
            }
            let resp = request.send().await;
            match resp {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => {
                    if retries >= 2 {
                        anyhow::bail!("webhook returned non-success status {}", response.status());
                    }
                }
                Err(err) => {
                    if retries >= 2 {
                        return Err(err).context("webhook request failed");
                    }
                }
            }
            retries += 1;
            tokio::time::sleep(std::time::Duration::from_millis(200 * retries as u64)).await;
        }
    }
}

#[async_trait]
impl HookSink for WebhookHookSink {
    async fn emit(&self, event: &HookEvent) -> Result<()> {
        self.post_payload(json!({
            "at": Utc::now().to_rfc3339(),
            "event": event,
        }))
        .await
    }
}

/// A [`HookSink`] that sends events over a Unix domain socket.
///
/// Each event is serialized as a single JSON line (`{"at": "...", "event": {...}}\n`)
/// and written to the socket. If the socket is not available (listener not running),
/// the event is silently dropped - hook sinks are best-effort observability, not
/// control flow.
///
/// On non-Unix platforms this struct exists but its [`HookSink::emit`] is a no-op.
#[derive(Debug, Clone)]
pub struct UnixSocketHookSink {
    #[cfg(unix)]
    path: PathBuf,
}

impl UnixSocketHookSink {
    /// Create a sink that connects to the Unix domain socket at `path`.
    pub fn new(path: PathBuf) -> Self {
        #[cfg(unix)]
        {
            Self { path }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Self {}
        }
    }
}

#[async_trait]
impl HookSink for UnixSocketHookSink {
    #[cfg(unix)]
    async fn emit(&self, event: &HookEvent) -> Result<()> {
        let mut stream = match tokio::net::UnixStream::connect(&self.path).await {
            Ok(s) => s,
            Err(_) => return Ok(()), // listener not running, skip silently
        };
        let payload = json!({
            "at": Utc::now().to_rfc3339(),
            "event": event
        });
        let mut line = serde_json::to_string(&payload).context("failed to encode hook event")?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .await
            .context("failed to write to unix socket")?;
        Ok(())
    }

    #[cfg(not(unix))]
    async fn emit(&self, _event: &HookEvent) -> Result<()> {
        // Unix sockets are not available on this platform.
        Ok(())
    }
}

/// Fans out [`HookEvent`]s to a collection of [`HookSink`]s.
///
/// Register one or more sinks via [`add_sink`](HookDispatcher::add_sink),
/// then call [`emit`](HookDispatcher::emit) to broadcast an event to all of
/// them. If a sink returns an error it is silently ignored so that a failing
/// sink does not prevent remaining sinks from receiving the event.
#[derive(Default, Clone)]
pub struct HookDispatcher {
    sinks: Vec<Arc<dyn HookSink>>,
}

impl HookDispatcher {
    /// Register a new sink that will receive all subsequently emitted events.
    pub fn add_sink(&mut self, sink: Arc<dyn HookSink>) {
        self.sinks.push(sink);
    }

    /// Number of registered sinks. Exposed so transport setup can assert
    /// exactly which sinks were wired (e.g. no stdout sink in stdio mode).
    #[must_use]
    pub fn sink_count(&self) -> usize {
        self.sinks.len()
    }

    /// Broadcast an event to every registered sink.
    ///
    /// Errors from individual sinks are silently discarded so that one failing
    /// sink does not block the others.
    pub async fn emit(&self, event: HookEvent) {
        for sink in &self.sinks {
            let _ = sink.emit(&event).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[cfg(unix)]
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(unix)]
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    static SOCKET_PATH_NONCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn hook_event_serializes_with_snake_case_type_and_payload() {
        let event = HookEvent::ToolLifecycle {
            response_id: "resp-1".to_string(),
            tool_name: "shell".to_string(),
            phase: "end".to_string(),
            payload: json!({ "exit_code": 0 }),
        };

        let encoded = event.to_json();

        assert_eq!(encoded["type"], "tool_lifecycle");
        assert_eq!(encoded["response_id"], "resp-1");
        assert_eq!(encoded["tool_name"], "shell");
        assert_eq!(encoded["phase"], "end");
        assert_eq!(encoded["payload"]["exit_code"], 0);
    }

    #[test]
    fn generic_event_frame_serialization_is_unchanged_by_boxing() {
        let event = HookEvent::GenericEventFrame {
            frame: Box::new(EventFrame::ResponseStart {
                response_id: "resp-1".to_string(),
            }),
        };

        let encoded = event.to_json();

        assert_eq!(encoded["type"], "generic_event_frame");
        assert_eq!(encoded["frame"]["event"], "response_start");
        assert_eq!(encoded["frame"]["response_id"], "resp-1");
    }

    #[tokio::test]
    async fn jsonl_sink_creates_parent_dir_and_appends_events() {
        let root = unique_temp_dir("jsonl_sink");
        let path = root.join("nested").join("hooks.jsonl");
        let sink = JsonlHookSink::new(path.clone());

        sink.emit(&HookEvent::ResponseStart {
            response_id: "resp-1".to_string(),
        })
        .await
        .unwrap();
        sink.emit(&HookEvent::ResponseEnd {
            response_id: "resp-1".to_string(),
        })
        .await
        .unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);

        let first: Value = serde_json::from_str(lines[0]).unwrap();
        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert!(first["at"].as_str().is_some());
        assert_eq!(first["event"]["type"], "response_start");
        assert_eq!(first["event"]["response_id"], "resp-1");
        assert_eq!(second["event"]["type"], "response_end");
        assert_eq!(second["event"]["response_id"], "resp-1");

        let _ = std::fs::remove_dir_all(root);
    }

    /// Concurrent emits must not interleave partial lines (#4739).
    ///
    /// Spawns many tasks writing through one shared [`JsonlHookSink`] and
    /// asserts every line in the resulting file is complete, parseable JSON.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn jsonl_sink_concurrent_emits_write_atomic_json_lines() {
        let root = unique_temp_dir("jsonl_sink_concurrent");
        let path = root.join("hooks.jsonl");
        let sink = Arc::new(JsonlHookSink::new(path.clone()));

        const TASKS: usize = 32;
        const EVENTS_PER_TASK: usize = 20;
        let expected = TASKS * EVENTS_PER_TASK;

        let mut handles = Vec::with_capacity(TASKS);
        for task_id in 0..TASKS {
            let sink = Arc::clone(&sink);
            handles.push(tokio::spawn(async move {
                for n in 0..EVENTS_PER_TASK {
                    let event = HookEvent::ToolLifecycle {
                        response_id: format!("resp-{task_id}"),
                        tool_name: "shell".to_string(),
                        phase: "end".to_string(),
                        payload: json!({ "n": n, "task": task_id }),
                    };
                    sink.emit(&event).await.expect("concurrent emit");
                }
            }));
        }
        for handle in handles {
            handle.await.expect("join concurrent writer");
        }

        let raw = std::fs::read_to_string(&path).expect("read concurrent jsonl");
        // Trailing newline yields an empty final split; keep non-empty lines only.
        let lines: Vec<&str> = raw.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(
            lines.len(),
            expected,
            "expected {expected} complete lines, got {}; sample: {:?}",
            lines.len(),
            lines
                .first()
                .map(|s| s.chars().take(80).collect::<String>())
        );

        for (idx, line) in lines.iter().enumerate() {
            let parsed: Value = serde_json::from_str(line).unwrap_or_else(|err| {
                panic!("line {idx} is not complete JSON ({err}): {line:?}");
            });
            assert!(
                parsed.get("at").and_then(|v| v.as_str()).is_some(),
                "line {idx} missing at: {parsed}"
            );
            assert_eq!(
                parsed["event"]["type"], "tool_lifecycle",
                "line {idx} unexpected event type"
            );
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatcher_continues_after_sink_error() {
        let mut dispatcher = HookDispatcher::default();
        let first = Arc::new(RecordingSink::default());
        let second = Arc::new(RecordingSink::default());

        dispatcher.add_sink(first.clone());
        dispatcher.add_sink(Arc::new(FailingSink));
        dispatcher.add_sink(second.clone());

        dispatcher
            .emit(HookEvent::ApprovalLifecycle {
                approval_id: "approval-1".to_string(),
                phase: "requested".to_string(),
                reason: Some("needs review".to_string()),
            })
            .await;

        assert_eq!(
            first.events(),
            vec![json!({
                "type": "approval_lifecycle",
                "approval_id": "approval-1",
                "phase": "requested",
                "reason": "needs review",
            })]
        );
        assert_eq!(second.events(), first.events());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_sink_skips_when_listener_absent() {
        let (_root, socket_path) = unique_short_socket_path("missing");
        let sink = UnixSocketHookSink::new(socket_path);
        let result = sink
            .emit(&HookEvent::ResponseStart {
                response_id: "resp-1".to_string(),
            })
            .await;
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_sink_sends_event_to_listener() {
        use tokio::io::AsyncBufReadExt;
        use tokio::net::UnixListener;

        let (root, socket_path) = unique_short_socket_path("send");
        std::fs::create_dir_all(&root).expect("mkdir");
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path).expect("bind");
        let sink = UnixSocketHookSink::new(socket_path.clone());
        let cleanup = || {
            let _ = std::fs::remove_file(&socket_path);
            let _ = std::fs::remove_dir_all(&root);
        };

        let mut handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut reader = tokio::io::BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read_line");
            line
        });

        let event = HookEvent::ResponseStart {
            response_id: "resp-42".to_string(),
        };
        let emit = sink.emit(&event);
        match tokio::time::timeout(Duration::from_secs(5), emit).await {
            Ok(result) => result.expect("emit"),
            Err(_) => {
                handle.abort();
                let _ = (&mut handle).await;
                cleanup();
                panic!("unix socket emit timed out");
            }
        }

        let received = match tokio::time::timeout(Duration::from_secs(5), &mut handle).await {
            Ok(result) => result.expect("join"),
            Err(_) => {
                handle.abort();
                let _ = (&mut handle).await;
                cleanup();
                panic!("unix socket exchange timed out");
            }
        };
        let parsed: Value = serde_json::from_str(&received).expect("parse");
        assert_eq!(parsed["event"]["type"], "response_start");
        assert_eq!(parsed["event"]["response_id"], "resp-42");
        assert!(parsed["at"].as_str().is_some());

        cleanup();
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<Value>>,
    }

    impl RecordingSink {
        fn events(&self) -> Vec<Value> {
            self.events.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl HookSink for RecordingSink {
        async fn emit(&self, event: &HookEvent) -> Result<()> {
            self.events.lock().unwrap().push(event.to_json());
            Ok(())
        }
    }

    struct FailingSink;

    #[async_trait::async_trait]
    impl HookSink for FailingSink {
        async fn emit(&self, _event: &HookEvent) -> Result<()> {
            anyhow::bail!("sink failed")
        }
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "deepseek-hooks-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[cfg(unix)]
    fn unique_short_socket_path(label: &str) -> (PathBuf, PathBuf) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let nonce = SOCKET_PATH_NONCE.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from("/tmp").join(format!(
            "cw-hk-{label}-{}-{nanos}-{nonce}",
            std::process::id()
        ));
        let path = root.join("hook.sock");
        (root, path)
    }

    #[test]
    fn webhook_sink_new_does_not_panic() {
        // Construction must never panic: if the configured client builder
        // fails, the code falls back to a default `reqwest::Client` instead of
        // calling `.expect(...)`.
        let sink = WebhookHookSink::new("https://example.invalid/webhook".to_string());
        let _ = sink;
    }
}
