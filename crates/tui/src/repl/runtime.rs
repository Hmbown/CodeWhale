//! Long-lived Python REPL runtime.
//!
//! One `python3 -u` subprocess lives for the duration of an RLM turn (or an
//! inline `repl` block sequence in the agent loop). Code blocks are sent
//! over stdin framed by `__RLM_RUN__`/`__RLM_END__` sentinels; the bootstrap
//! `exec()`s them into the same global namespace so variables, imports,
//! and even open file handles persist naturally across rounds.
//!
//! Sub-LLM helpers (`llm_query`, `llm_query_batched`, `rlm_query`,
//! `rlm_query_batched`) are wired through a stdin/stdout RPC protocol:
//! Python emits `__RLM_REQ_<sid>__::{json}` on stdout, Rust dispatches the
//! request and writes `__RLM_RESP_<sid>__::{json}` back on stdin. No HTTP
//! sidecar, no temp ports — the same pipes carry both control and data.
//!
//! The session id (`<sid>`) is a UUID generated per spawn, so user output
//! that happens to contain "REQ" or "FINAL" can't be confused with control
//! messages.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use uuid::Uuid;

use crate::child_env;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of executing one code block.
#[derive(Debug, Clone)]
pub struct ReplRound {
    /// Stdout shown to the model as metadata next round.
    pub stdout: String,
    /// Full stdout (with sentinels stripped, but otherwise raw).
    pub full_stdout: String,
    /// Stderr from this round (if any).
    pub stderr: String,
    /// `True` if the user code raised an unhandled Python exception.
    pub has_error: bool,
    /// Captured `FINAL(value)` payload, if any.
    pub final_value: Option<String>,
    /// Number of `llm_query`/`rlm_query` RPCs the round issued.
    pub rpc_count: u32,
    /// Wall-clock duration of the round.
    pub elapsed: Duration,
}

/// One RPC request emitted by Python during a round.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcRequest {
    /// `llm_query(prompt, model=None, max_tokens=None, system=None)`
    Llm {
        prompt: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        max_tokens: Option<u32>,
        #[serde(default)]
        system: Option<String>,
    },
    /// `llm_query_batched(prompts, model=None)`
    LlmBatch {
        prompts: Vec<String>,
        #[serde(default)]
        model: Option<String>,
    },
    /// `rlm_query(prompt, model=None)` — recursive sub-RLM (paper's `sub_RLM`).
    Rlm {
        prompt: String,
        #[serde(default)]
        model: Option<String>,
    },
    /// `rlm_query_batched(prompts, model=None)`
    RlmBatch {
        prompts: Vec<String>,
        #[serde(default)]
        model: Option<String>,
    },
}

/// Response for one RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcResponse {
    /// Single-text reply (Llm / Rlm).
    Single(SingleResp),
    /// Batch reply (LlmBatch / RlmBatch).
    Batch(BatchResp),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleResp {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResp {
    pub results: Vec<SingleResp>,
}

/// Trait-object handle for dispatching Python RPCs back into Rust.
///
/// Each RLM turn supplies one. Implementations forward to the LLM client
/// (and recursively into `run_rlm_turn_inner` for `Rlm` / `RlmBatch`).
pub trait RpcDispatcher: Send + Sync {
    fn dispatch<'a>(
        &'a self,
        req: RpcRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RpcResponse> + Send + 'a>>;
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_STDOUT_LIMIT: usize = 8_192;
const ROUND_TIMEOUT: Duration = Duration::from_secs(180);
#[cfg(not(windows))]
const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(windows)]
const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// PythonRuntime
// ---------------------------------------------------------------------------

/// Long-lived Python REPL.
#[derive(Debug)]
pub struct PythonRuntime {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Per-spawn session id used in protocol sentinels.
    session_id: String,
    /// Path to the file holding `context` (kept around for cleanup).
    context_path: Option<PathBuf>,
    stdout_limit: usize,
    round_count: u64,
    started: Instant,
    round_timeout: Option<Duration>,
}

impl PythonRuntime {
    /// Spawn a REPL with no `context` variable and no LLM helpers wired up.
    /// Used by the agent loop for inline `repl` blocks the model emits in
    /// regular conversation.
    pub async fn new() -> Result<Self, String> {
        Self::spawn_inner(None, Some(ROUND_TIMEOUT)).await
    }

    /// Compatibility shim — older RLM code path used to pass a state file.
    /// The state file is no longer used, but the path doubles as an extra
    /// scratch location callers can rely on for cleanup symmetry.
    pub fn with_state_path(_path: PathBuf) -> Self {
        // Synchronous constructor is no longer meaningful: spawning Python
        // is async. Callers in turn.rs already use `spawn_with_context` —
        // this stub is kept only so the public surface compiles for any
        // out-of-tree user. It returns a deliberately broken runtime that
        // panics on first use, which is preferable to silently lying.
        unreachable!(
            "PythonRuntime::with_state_path is deprecated — \
             use PythonRuntime::new() or PythonRuntime::spawn_with_context()"
        )
    }

    /// Spawn a REPL with `context` (and `ctx`) preloaded from a file. Used
    /// by the RLM turn loop.
    pub async fn spawn_with_context(context_path: &Path) -> Result<Self, String> {
        Self::spawn_inner(Some(context_path), None).await
    }

    async fn spawn_inner(
        context_path: Option<&Path>,
        round_timeout: Option<Duration>,
    ) -> Result<Self, String> {
        let session_id = Uuid::new_v4().simple().to_string();
        let bootstrap = render_bootstrap(&session_id);

        let mut cmd = Command::new("python3");
        cmd.arg("-u")
            .arg("-c")
            .arg(&bootstrap)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let context_env = context_path
            .map(|path| {
                vec![(
                    OsString::from("RLM_CONTEXT_FILE"),
                    path.as_os_str().to_os_string(),
                )]
            })
            .unwrap_or_default();
        child_env::apply_to_tokio_command(&mut cmd, context_env);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn python3: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "python3 stdin pipe missing".to_string())?;
        let raw_stdout = child
            .stdout
            .take()
            .ok_or_else(|| "python3 stdout pipe missing".to_string())?;
        let stdout = BufReader::new(raw_stdout);

        let mut rt = Self {
            child,
            stdin,
            stdout,
            session_id: session_id.clone(),
            context_path: context_path.map(Path::to_path_buf),
            stdout_limit: DEFAULT_STDOUT_LIMIT,
            round_count: 0,
            started: Instant::now(),
            round_timeout,
        };

        // Wait for `__RLM_READY_<sid>__` before handing control back. If
        // Python failed to start (missing module, syntax error in the
        // bootstrap, etc.), this is where we'll find out.
        let ready_sentinel = format!("__RLM_READY_{session_id}__");
        match tokio::time::timeout(SPAWN_READY_TIMEOUT, rt.read_until_ready(&ready_sentinel)).await
        {
            Ok(Ok(())) => Ok(rt),
            Ok(Err(e)) => {
                let _ = rt.child.kill().await;
                Err(format!("python3 bootstrap failed: {e}"))
            }
            Err(_) => {
                let _ = rt.child.kill().await;
                Err(format!(
                    "python3 bootstrap did not signal ready within {}s",
                    SPAWN_READY_TIMEOUT.as_secs()
                ))
            }
        }
    }

    async fn read_until_ready(&mut self, ready_sentinel: &str) -> Result<(), String> {
        loop {
            let mut line = String::new();
            let n = self
                .stdout
                .read_line(&mut line)
                .await
                .map_err(|e| format!("stdout read: {e}"))?;
            if n == 0 {
                return Err("python3 closed stdout before ready signal".to_string());
            }
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed == ready_sentinel {
                return Ok(());
            }
            // Pre-ready output is rare; ignore it.
        }
    }

    /// Execute a Python code block with no RPC dispatcher. Used for inline
    /// `repl` blocks where `llm_query()` should fall back to a sentinel.
    pub async fn execute(&mut self, code: &str) -> Result<ReplRound, String> {
        self.run(code, None::<&dyn RpcDispatcher>).await
    }

    /// Execute a code block, dispatching any sub-LLM RPCs through `bridge`.
    ///
    /// Returns once Python emits `__RLM_DONE_<sid>__` or the round timeout
    /// elapses (whichever happens first).
    pub async fn run<D>(&mut self, code: &str, bridge: Option<&D>) -> Result<ReplRound, String>
    where
        D: RpcDispatcher + ?Sized,
    {
        let started = Instant::now();
        self.round_count += 1;
        let round_id = self.round_count;

        // Send the code header + body + end marker in one write.
        let header = format!("__RLM_RUN_{}__::{round_id}\n", self.session_id);
        let footer = format!("__RLM_END_{}__\n", self.session_id);
        let payload = format!("{header}{code}\n{footer}");
        self.stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| format!("stdin write: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("stdin flush: {e}"))?;

        // Sentinels for this session.
        let req_prefix = format!("__RLM_REQ_{}__::", self.session_id);
        let final_prefix = format!("__RLM_FINAL_{}__::", self.session_id);
        let err_prefix = format!("__RLM_ERR_{}__::", self.session_id);
        let done_prefix = format!("__RLM_DONE_{}__::", self.session_id);

        let mut stdout_buf = String::new();
        let mut final_value: Option<String> = None;
        let mut had_error = false;
        let mut rpc_count: u32 = 0;
        let round_timeout = self.round_timeout;

        let read_loop = async {
            loop {
                let mut line = String::new();
                let n = self
                    .stdout
                    .read_line(&mut line)
                    .await
                    .map_err(|e| format!("stdout read: {e}"))?;
                if n == 0 {
                    return Err("python3 closed stdout mid-round".to_string());
                }
                let trimmed = line.trim_end_matches(['\n', '\r']);

                if let Some(rest) = trimmed.strip_prefix(&done_prefix) {
                    let _ = rest;
                    break;
                }
                if let Some(rest) = trimmed.strip_prefix(&final_prefix) {
                    // Stored as a JSON-encoded string.
                    let v =
                        serde_json::from_str::<String>(rest).unwrap_or_else(|_| rest.to_string());
                    final_value = Some(v);
                    continue;
                }
                if let Some(rest) = trimmed.strip_prefix(&err_prefix) {
                    let traceback =
                        serde_json::from_str::<String>(rest).unwrap_or_else(|_| rest.to_string());
                    had_error = true;
                    stdout_buf.push_str(&format!("[traceback]\n{traceback}\n"));
                    continue;
                }
                if let Some(rest) = trimmed.strip_prefix(&req_prefix) {
                    rpc_count = rpc_count.saturating_add(1);
                    let req: RpcRequest = match serde_json::from_str(rest) {
                        Ok(r) => r,
                        Err(e) => {
                            // Send an error response so Python isn't blocked.
                            self.send_resp(&RpcResponse::Single(SingleResp {
                                text: String::new(),
                                error: Some(format!("malformed RPC: {e}")),
                            }))
                            .await?;
                            continue;
                        }
                    };
                    let resp = match bridge {
                        Some(b) => b.dispatch(req).await,
                        None => RpcResponse::Single(SingleResp {
                            text: String::new(),
                            error: Some("no LLM bridge bound to this REPL".to_string()),
                        }),
                    };
                    self.send_resp(&resp).await?;
                    continue;
                }

                stdout_buf.push_str(&line);
            }
            Ok::<_, String>(())
        };

        if let Some(round_timeout) = round_timeout {
            match tokio::time::timeout(round_timeout, read_loop).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(format!(
                        "REPL round timed out after {}s",
                        round_timeout.as_secs()
                    ));
                }
            }
        } else {
            read_loop.await?;
        }

        let stderr = self.drain_stderr().await;
        let display = truncate_stdout(stdout_buf.trim_end_matches('\n'), self.stdout_limit);

        Ok(ReplRound {
            stdout: display,
            full_stdout: stdout_buf,
            stderr,
            has_error: had_error,
            final_value,
            rpc_count,
            elapsed: started.elapsed(),
        })
    }

    async fn send_resp(&mut self, resp: &RpcResponse) -> Result<(), String> {
        let body = serde_json::to_string(resp).map_err(|e| format!("encode rpc resp: {e}"))?;
        let line = format!("__RLM_RESP_{}__::{body}\n", self.session_id);
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("stdin write resp: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("stdin flush resp: {e}"))?;
        Ok(())
    }

    async fn drain_stderr(&mut self) -> String {
        // We don't continuously read stderr — drain whatever's pending after
        // a round so it can show up in error reports without deadlocking
        // anything during normal operation.
        let Some(stderr) = self.child.stderr.as_mut() else {
            return String::new();
        };
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        // Best-effort read with a tight deadline; we don't want to block.
        let fut = async {
            let mut chunk = [0u8; 4096];
            loop {
                match tokio::time::timeout(Duration::from_millis(20), stderr.read(&mut chunk)).await
                {
                    Ok(Ok(0)) => break,
                    Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
            }
        };
        let _ = fut.await;
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Total rounds executed.
    pub fn round_count(&self) -> u64 {
        self.round_count
    }

    /// Current per-round timeout policy. RLM context runs intentionally return
    /// `None` so long map-reduce jobs are not killed by the old 180s cap.
    pub fn round_timeout(&self) -> Option<Duration> {
        self.round_timeout
    }

    /// Wall-clock uptime since spawn.
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Cleanly tear down the subprocess.
    pub async fn shutdown(mut self) {
        let _ = self.stdin.shutdown().await;
        let _ = self.child.kill().await;
        if let Some(path) = self.context_path.take() {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

impl Drop for PythonRuntime {
    fn drop(&mut self) {
        // tokio sets `kill_on_drop(true)` on the child; the context file
        // (if any) is removed on `shutdown()` — drop is best-effort.
        if let Some(path) = self.context_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ---------------------------------------------------------------------------
// Bootstrap script
// ---------------------------------------------------------------------------

/// Render the Python bootstrap with session-specific sentinels baked in.
/// The sentinels include a UUID to prevent user prints from being mistaken
/// for control messages.
fn render_bootstrap(session_id: &str) -> String {
    BOOTSTRAP_TEMPLATE.replace("__SID__", session_id)
}

const BOOTSTRAP_TEMPLATE: &str = r#"
import json as _json
import os as _os
import sys as _sys
import traceback as _traceback

_SID = "__SID__"
_REQ = f"__RLM_REQ_{_SID}__::"
_RESP = f"__RLM_RESP_{_SID}__::"
_FINAL = f"__RLM_FINAL_{_SID}__::"
_ERR = f"__RLM_ERR_{_SID}__::"
_RUN = f"__RLM_RUN_{_SID}__::"
_END = f"__RLM_END_{_SID}__"
_DONE = f"__RLM_DONE_{_SID}__::"
_READY = f"__RLM_READY_{_SID}__"

def _rpc(req):
    _sys.stdout.write(_REQ + _json.dumps(req) + "\n")
    _sys.stdout.flush()
    line = _sys.stdin.readline()
    if not line:
        return {"error": "rust driver closed stdin"}
    if line.startswith(_RESP):
        try:
            return _json.loads(line[len(_RESP):])
        except Exception as e:
            return {"error": f"malformed rpc resp: {e}"}
    return {"error": f"unexpected protocol line: {line[:120]!r}"}

def llm_query(prompt, model=None, max_tokens=None, system=None):
    """One-shot sub-LLM call. The model arg is accepted for compatibility but ignored by Rust."""
    resp = _rpc({"type":"llm","prompt":str(prompt),"model":model,
                 "max_tokens":max_tokens,"system":system})
    if isinstance(resp, dict) and resp.get("error"):
        return f"[llm_query error: {resp['error']}]"
    if isinstance(resp, dict):
        return resp.get("text","")
    return str(resp)

def llm_query_batched(prompts, model=None):
    """Run multiple sub-LLM calls concurrently. The model arg is accepted for compatibility but ignored."""
    if not isinstance(prompts, (list, tuple)):
        return ["[llm_query_batched: prompts must be a list]"]
    resp = _rpc({"type":"llm_batch","prompts":[str(p) for p in prompts],"model":model})
    if isinstance(resp, dict) and resp.get("error"):
        return [f"[llm_query_batched: {resp['error']}]" for _ in prompts]
    results = (resp or {}).get("results", []) if isinstance(resp, dict) else []
    if len(results) != len(prompts):
        return [f"[llm_query_batched: size mismatch ({len(results)}/{len(prompts)})]" for _ in prompts]
    out = []
    for r in results:
        if r.get("error"):
            out.append(f"[child err: {r['error']}]")
        else:
            out.append(r.get("text",""))
    return out

def rlm_query(prompt, model=None):
    """Recursive sub-RLM. The model arg is accepted for compatibility but ignored by Rust."""
    resp = _rpc({"type":"rlm","prompt":str(prompt),"model":model})
    if isinstance(resp, dict) and resp.get("error"):
        return f"[rlm_query error: {resp['error']}]"
    if isinstance(resp, dict):
        return resp.get("text","")
    return str(resp)

def rlm_query_batched(prompts, model=None):
    """Run multiple recursive sub-RLMs in parallel. The model arg is accepted for compatibility but ignored."""
    if not isinstance(prompts, (list, tuple)):
        return ["[rlm_query_batched: prompts must be a list]"]
    resp = _rpc({"type":"rlm_batch","prompts":[str(p) for p in prompts],"model":model})
    if isinstance(resp, dict) and resp.get("error"):
        return [f"[rlm_query_batched: {resp['error']}]" for _ in prompts]
    results = (resp or {}).get("results", []) if isinstance(resp, dict) else []
    if len(results) != len(prompts):
        return [f"[rlm_query_batched: size mismatch ({len(results)}/{len(prompts)})]" for _ in prompts]
    out = []
    for r in results:
        if r.get("error"):
            out.append(f"[child err: {r['error']}]")
        else:
            out.append(r.get("text",""))
    return out

def FINAL(value):
    """Signal the loop to stop with this final answer."""
    _sys.stdout.write(_FINAL + _json.dumps(str(value)) + "\n")
    _sys.stdout.flush()

def FINAL_VAR(name):
    """Signal the loop to stop, returning the value of a named variable."""
    name_str = str(name).strip().strip("'\"")
    if name_str in globals():
        FINAL(globals()[name_str])
    else:
        print(f"FINAL_VAR error: variable '{name_str}' not found. "
              f"Use SHOW_VARS() to list available variables.", flush=True)

def SHOW_VARS():
    """Return a dict of {name: type-name} for all user variables in the REPL."""
    out = {}
    for k, v in list(globals().items()):
        if k.startswith('_') or k in _BOOTSTRAP_NAMES:
            continue
        out[k] = type(v).__name__
    return out

def repl_get(name, default=None):
    return globals().get(str(name), default)

def repl_set(name, value):
    globals()[str(name)] = value

def chunk_context(max_chars=20000, overlap=0):
    """Return full-coverage context chunks with index/start/end/text fields."""
    max_chars = int(max_chars)
    overlap = max(0, int(overlap))
    if max_chars <= 0:
        raise ValueError("max_chars must be > 0")
    if overlap >= max_chars:
        raise ValueError("overlap must be smaller than max_chars")
    chunks = []
    start = 0
    idx = 0
    total = len(context)
    while start < total:
        end = min(total, start + max_chars)
        chunks.append({"index": idx, "start": start, "end": end, "text": context[start:end]})
        idx += 1
        if end >= total:
            break
        start = end - overlap
    return chunks

def chunk_coverage(chunks):
    """Summarize coverage for chunks produced by chunk_context()."""
    spans = []
    for c in chunks:
        try:
            spans.append((int(c["start"]), int(c["end"])))
        except Exception:
            continue
    spans.sort()
    covered = 0
    cursor = 0
    gaps = []
    for start, end in spans:
        if start > cursor:
            gaps.append((cursor, start))
        if end > cursor:
            covered += end - max(start, cursor)
            cursor = end
    if cursor < len(context):
        gaps.append((cursor, len(context)))
    return {
        "chunks": len(chunks),
        "context_chars": len(context),
        "covered_chars": covered,
        "gaps": gaps,
        "complete": covered >= len(context) and not gaps,
    }

# Load the long input as `context` (and `ctx`) from a file. This keeps the
# big string out of the process command-line and out of the LLM's window.
_ctx_file = _os.environ.get("RLM_CONTEXT_FILE","")
context = ""
if _ctx_file:
    try:
        with open(_ctx_file, "r", encoding="utf-8", errors="replace") as f:
            context = f.read()
    except Exception as e:
        _sys.stderr.write(f"[bootstrap] failed to load context: {e}\n")
ctx = context  # short alias matching aleph

_BOOTSTRAP_NAMES = {
    "_SID","_REQ","_RESP","_FINAL","_ERR","_RUN","_END","_DONE","_READY",
    "_rpc","_ctx_file","_BOOTSTRAP_NAMES","_main_loop",
    "llm_query","llm_query_batched","rlm_query","rlm_query_batched",
    "FINAL","FINAL_VAR","SHOW_VARS","repl_get","repl_set",
    "chunk_context","chunk_coverage",
    "context","ctx",
    "_json","_os","_sys","_traceback",
}

def _main_loop():
    _sys.stdout.write(_READY + "\n")
    _sys.stdout.flush()
    while True:
        header = _sys.stdin.readline()
        if not header:
            return
        if not header.startswith(_RUN):
            continue
        round_id = header.rstrip("\n")[len(_RUN):]
        code_lines = []
        while True:
            line = _sys.stdin.readline()
            if not line:
                return
            if line.rstrip("\n") == _END:
                break
            code_lines.append(line)
        code = "".join(code_lines)
        try:
            exec(compile(code, f"<repl-{round_id}>", "exec"), globals())
        except SystemExit:
            _sys.stdout.write(_DONE + round_id + "\n")
            _sys.stdout.flush()
            return
        except BaseException:
            tb = _traceback.format_exc()
            _sys.stdout.write(_ERR + _json.dumps(tb) + "\n")
            _sys.stdout.flush()
        _sys.stdout.write(_DONE + round_id + "\n")
        _sys.stdout.flush()

_main_loop()
"#;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate_stdout(stdout: &str, limit: usize) -> String {
    if stdout.len() <= limit {
        return stdout.to_string();
    }
    let take = limit.saturating_sub(80);
    let mut out: String = stdout.chars().take(take).collect();
    let omitted = stdout.len().saturating_sub(out.len());
    out.push_str(&format!(
        "\n\n[... REPL output truncated: {omitted} bytes omitted ...]\n"
    ));
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::Mutex;

    /// In-process dispatcher that records what was asked and replies with
    /// canned text. Lets tests verify the round-trip without real network.
    struct StubBridge {
        calls: Arc<Mutex<Vec<RpcRequest>>>,
        canned: Arc<AtomicU32>,
    }

    impl StubBridge {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                canned: Arc::new(AtomicU32::new(0)),
            }
        }
    }

    impl RpcDispatcher for StubBridge {
        fn dispatch<'a>(
            &'a self,
            req: RpcRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RpcResponse> + Send + 'a>> {
            Box::pin(async move {
                self.calls.lock().await.push(req.clone());
                let n = self.canned.fetch_add(1, Ordering::Relaxed);
                match req {
                    RpcRequest::Llm { prompt, .. } | RpcRequest::Rlm { prompt, .. } => {
                        RpcResponse::Single(SingleResp {
                            text: format!("stub#{n}: {prompt}"),
                            error: None,
                        })
                    }
                    RpcRequest::LlmBatch { prompts, .. } | RpcRequest::RlmBatch { prompts, .. } => {
                        let results = prompts
                            .into_iter()
                            .enumerate()
                            .map(|(i, p)| SingleResp {
                                text: format!("stub#{n}.{i}: {p}"),
                                error: None,
                            })
                            .collect();
                        RpcResponse::Batch(BatchResp { results })
                    }
                }
            })
        }
    }

    fn write_temp_context(body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("deepseek_repl_runtime_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("ctx_{}_{}.txt", std::process::id(), Uuid::new_v4()));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[tokio::test]
    async fn spawns_and_executes_simple_print() {
        let mut rt = PythonRuntime::new().await.expect("spawn");
        let round = rt.execute("print('hello world')").await.expect("execute");
        assert!(round.stdout.contains("hello world"));
        assert!(!round.has_error);
        assert!(round.final_value.is_none());
        assert_eq!(round.rpc_count, 0);
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn variables_persist_across_rounds() {
        let mut rt = PythonRuntime::new().await.expect("spawn");
        rt.execute("x = [1, 2, 3]").await.expect("r1");
        rt.execute("x.append(99)").await.expect("r2");
        let round = rt.execute("print(x)").await.expect("r3");
        assert!(round.stdout.contains("[1, 2, 3, 99]"));
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn imports_persist_across_rounds() {
        let mut rt = PythonRuntime::new().await.expect("spawn");
        rt.execute("import math").await.expect("r1");
        let round = rt.execute("print(math.pi)").await.expect("r2");
        assert!(round.stdout.contains("3.14"));
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn context_loads_from_file() {
        let path = write_temp_context("the quick brown fox");
        let mut rt = PythonRuntime::spawn_with_context(&path)
            .await
            .expect("spawn");
        let round = rt
            .execute("print(len(context), context[:5])")
            .await
            .expect("execute");
        assert!(round.stdout.contains("19"));
        assert!(round.stdout.contains("the q"));
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn ctx_alias_works() {
        let path = write_temp_context("aleph-style");
        let mut rt = PythonRuntime::spawn_with_context(&path)
            .await
            .expect("spawn");
        let round = rt.execute("print(ctx)").await.expect("execute");
        assert!(round.stdout.contains("aleph-style"));
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn context_chunk_helpers_report_full_coverage() {
        let path = write_temp_context("abcdefghijklmnopqrstuvwxyz");
        let mut rt = PythonRuntime::spawn_with_context(&path)
            .await
            .expect("spawn");
        let round = rt
            .execute(
                "chunks = chunk_context(max_chars=10)\n\
                 coverage = chunk_coverage(chunks)\n\
                 print(len(chunks), coverage['covered_chars'], coverage['complete'])",
            )
            .await
            .expect("execute");
        assert!(round.stdout.contains("3 26 True"), "{}", round.stdout);
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn rlm_context_runtime_has_no_fixed_round_timeout() {
        let path = write_temp_context("long input");
        let rt = PythonRuntime::spawn_with_context(&path)
            .await
            .expect("spawn");
        assert!(
            rt.round_timeout().is_none(),
            "RLM context runs must not inherit the old 180s REPL round timeout"
        );
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn inline_runtime_keeps_bounded_round_timeout() {
        let rt = PythonRuntime::new().await.expect("spawn");
        assert_eq!(rt.round_timeout(), Some(ROUND_TIMEOUT));
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn final_is_captured() {
        let mut rt = PythonRuntime::new().await.expect("spawn");
        let round = rt
            .execute("FINAL('the answer is 42')")
            .await
            .expect("execute");
        assert_eq!(round.final_value.as_deref(), Some("the answer is 42"));
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn final_var_is_captured() {
        let mut rt = PythonRuntime::new().await.expect("spawn");
        rt.execute("answer = 'computed'").await.expect("r1");
        let round = rt.execute("FINAL_VAR('answer')").await.expect("r2");
        assert_eq!(round.final_value.as_deref(), Some("computed"));
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn errors_are_reported_without_killing_runtime() {
        let mut rt = PythonRuntime::new().await.expect("spawn");
        let r1 = rt.execute("raise ValueError('boom')").await.expect("r1");
        assert!(r1.has_error);
        assert!(r1.full_stdout.contains("boom") || r1.stdout.contains("boom"));
        // The runtime is still alive — next round should work.
        let r2 = rt.execute("print('still here')").await.expect("r2");
        assert!(r2.stdout.contains("still here"));
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn rpc_dispatcher_round_trips_llm_query() {
        let bridge = StubBridge::new();
        let calls = Arc::clone(&bridge.calls);

        let mut rt = PythonRuntime::new().await.expect("spawn");
        let round = rt
            .run("print(llm_query('hello'))", Some(&bridge))
            .await
            .expect("execute");
        assert!(
            round.stdout.contains("stub#0: hello"),
            "stdout: {:?}",
            round.stdout
        );
        assert_eq!(round.rpc_count, 1);

        let recorded = calls.lock().await;
        assert_eq!(recorded.len(), 1);
        match &recorded[0] {
            RpcRequest::Llm { prompt, .. } => assert_eq!(prompt, "hello"),
            other => panic!("expected Llm request, got {other:?}"),
        }
        drop(recorded);
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn rpc_dispatcher_round_trips_batch() {
        let bridge = StubBridge::new();
        let mut rt = PythonRuntime::new().await.expect("spawn");
        let round = rt
            .run(
                "outs = llm_query_batched(['a','b','c']); print('|'.join(outs))",
                Some(&bridge),
            )
            .await
            .expect("execute");
        assert!(round.stdout.contains("stub#0.0: a"));
        assert!(round.stdout.contains("stub#0.1: b"));
        assert!(round.stdout.contains("stub#0.2: c"));
        assert_eq!(round.rpc_count, 1);
        rt.shutdown().await;
    }

    #[tokio::test]
    async fn no_dispatcher_returns_unavailable_sentinel() {
        let mut rt = PythonRuntime::new().await.expect("spawn");
        let round = rt.execute("print(llm_query('hi'))").await.expect("execute");
        assert!(
            round.stdout.contains("[llm_query error:") || round.stdout.contains("no LLM bridge"),
            "stdout: {:?}",
            round.stdout
        );
        rt.shutdown().await;
    }

    #[test]
    fn truncate_keeps_short_unchanged() {
        assert_eq!(truncate_stdout("hello", 100), "hello");
    }

    #[test]
    fn truncate_clips_long() {
        let long = "a".repeat(10_000);
        let out = truncate_stdout(&long, 1024);
        assert!(out.len() < 1500);
        assert!(out.contains("truncated"));
    }
}
