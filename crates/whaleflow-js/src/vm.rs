//! The QuickJS VM host: one dedicated OS thread running a current-thread
//! tokio runtime + `LocalSet` (design §2.2). Host functions do no heavy work
//! inline — they send `Send`-only requests over the bridge channels and
//! `await` a `oneshot`, so `Promise.all([task(a), task(b)])` fans out through
//! the driver's persistent completion pump.

use std::time::{Duration, Instant};

use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Exception, Function, Object, Value,
    async_with, function::Async,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::bridge::{BudgetQuery, HostChannels, SpawnRequest, ToolCallRequest};
use codewhale_tools::ToolCallSource;

/// Default VM heap ceiling. WhaleFlow scripts are orchestration glue, not
/// data pipelines; 64 MiB is generous while still bounding a runaway script.
pub const DEFAULT_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
/// Default VM stack ceiling (QuickJS ships 256 KiB; recursion-heavy
/// orchestration scripts get headroom without permitting unbounded growth).
pub const DEFAULT_MAX_STACK_BYTES: usize = 1024 * 1024;

/// Sandbox limits and host surface for one script run. Everything here is
/// default-closed: the only globals beyond standard ECMAScript intrinsics are
/// the ones this crate installs (design §9).
#[derive(Clone)]
pub struct VmOptions {
    pub memory_limit_bytes: usize,
    pub max_stack_bytes: usize,
    /// Wall-clock deadline enforced by the interrupt handler. `None` = no
    /// script-side deadline (the driver may still cancel via the token).
    pub timeout: Option<Duration>,
    /// Cooperative cancellation: polled by the interrupt handler (aborts
    /// running JS, uncatchable) and selected against long host awaits.
    pub cancel_token: CancellationToken,
    /// Value surfaced as `budget.total` (the active scope limit).
    pub budget_total: Option<u64>,
    /// Tool names exposed as `tools.<name>`. The driver exposes the calling
    /// context's full tool surface (minus delegation tools) and enforces the
    /// session's approval semantics per call: approval-gated calls prompt
    /// the user on interactive hosts and throw a catchable error on
    /// child-hosted / headless runs.
    pub tool_names: Vec<String>,
}

impl Default for VmOptions {
    fn default() -> Self {
        Self {
            memory_limit_bytes: DEFAULT_MEMORY_LIMIT_BYTES,
            max_stack_bytes: DEFAULT_MAX_STACK_BYTES,
            timeout: None,
            cancel_token: CancellationToken::new(),
            budget_total: None,
            tool_names: Vec::new(),
        }
    }
}

/// JS stdlib prelude injected into every WhaleFlow script (design §7).
/// `task`, `__whaleflow_tool_call`, `__whaleflow_budget_snapshot`, and
/// `__whaleflow_log` are host-bound before this runs.
const PRELUDE: &str = r#"
"use strict";
(() => {
  const MAX_FANOUT_ITEMS = 4096;

  globalThis.tools = {};
  for (const __name of __whaleflow_tool_names) {
    globalThis.tools[__name] = (args) =>
      __whaleflow_tool_call(__name, args === undefined ? {} : args);
  }

  globalThis.log = (msg) => {
    __whaleflow_log(typeof msg === "string" ? msg : JSON.stringify(msg));
  };

  globalThis.budget = {
    total: __whaleflow_budget_total,
    async spent() {
      return (await __whaleflow_budget_snapshot()).spent;
    },
    async remaining() {
      const snap = await __whaleflow_budget_snapshot();
      return snap.remaining === null || snap.remaining === undefined
        ? Infinity
        : snap.remaining;
    },
  };

  // Barrier fan-out: all-settled, per-item errors -> null.
  globalThis.parallel = async function parallel(thunks) {
    if (!Array.isArray(thunks)) {
      throw new Error("parallel(): expected an array of thunks");
    }
    if (thunks.length > MAX_FANOUT_ITEMS) {
      throw new Error(`parallel(): max ${MAX_FANOUT_ITEMS} items`);
    }
    return Promise.all(
      thunks.map((t) =>
        Promise.resolve()
          .then(() => (typeof t === "function" ? t() : t))
          .catch(() => null),
      ),
    );
  };

  // Per-item staged pipeline, no barrier between stages; a stage error
  // drops that item to null without stalling siblings.
  globalThis.pipeline = async function pipeline(items, ...stages) {
    if (!Array.isArray(items)) {
      throw new Error("pipeline(): expected an array of items");
    }
    if (items.length > MAX_FANOUT_ITEMS) {
      throw new Error(`pipeline(): max ${MAX_FANOUT_ITEMS} items`);
    }
    return Promise.all(
      items.map(async (it, i) => {
        let v = it;
        for (const s of stages) {
          try {
            v = await s(v, it, i);
          } catch {
            return null;
          }
        }
        return v;
      }),
    );
  };
})();
"#;

/// Run one WhaleFlow script to completion on a dedicated VM thread and
/// return its JSON-converted result. The calling task stays on the host
/// multi-thread runtime; only `Send` data crosses the thread boundary.
pub async fn run_script(
    script: String,
    channels: HostChannels,
    options: VmOptions,
) -> Result<serde_json::Value, String> {
    let (done_tx, done_rx) = oneshot::channel();
    let cancel = options.cancel_token.clone();
    std::thread::Builder::new()
        .name("whaleflow-js-vm".to_string())
        .spawn(move || {
            let outcome = vm_thread_main(script, channels, options);
            let _ = done_tx.send(outcome);
        })
        .map_err(|err| format!("whaleflow: failed to spawn VM thread: {err}"))?;

    // Hard-kill path (design §9 / M7): on cancellation the interrupt handler
    // aborts running JS (uncatchable) and every host await selects on the
    // token, so the VM thread unwinds on its own. Give it a short grace
    // window to report, then detach — the caller must not hang on a runaway
    // script even if the thread is wedged in native code.
    let mut done_rx = done_rx;
    tokio::select! {
        outcome = &mut done_rx => {
            outcome.map_err(|_| "whaleflow: VM thread terminated unexpectedly".to_string())?
        }
        () = cancel.cancelled() => {
            match tokio::time::timeout(Duration::from_secs(5), done_rx).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(_)) => Err("whaleflow: script cancelled".to_string()),
                Err(_) => Err(
                    "whaleflow: script cancelled; VM thread detached after grace timeout"
                        .to_string(),
                ),
            }
        }
    }
}

fn vm_thread_main(
    script: String,
    channels: HostChannels,
    options: VmOptions,
) -> Result<serde_json::Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|err| format!("whaleflow: failed to build VM runtime: {err}"))?;
    // `Async(..)` host futures are `!Send`; a LocalSet allows them.
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, run_vm(script, channels, options))
}

async fn run_vm(
    script: String,
    channels: HostChannels,
    options: VmOptions,
) -> Result<serde_json::Value, String> {
    let rt = AsyncRuntime::new().map_err(|err| format!("whaleflow: runtime init failed: {err}"))?;
    rt.set_memory_limit(options.memory_limit_bytes).await;
    rt.set_max_stack_size(options.max_stack_bytes).await;

    // Interrupt handler: aborts running JS (uncatchable) on cancellation and
    // doubles as the execution-timeout deadline (design §9).
    let cancel_for_interrupt = options.cancel_token.clone();
    let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
    rt.set_interrupt_handler(Some(Box::new(move || {
        cancel_for_interrupt.is_cancelled()
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
    })))
    .await;

    // `AsyncContext::full` registers only standard ECMAScript intrinsics —
    // no fs/net/process/require. The globals installed below are the entire
    // host attack surface. No module loader is set, so `import` is inert.
    let context = AsyncContext::full(&rt)
        .await
        .map_err(|err| format!("whaleflow: context init failed: {err}"))?;

    let cancel = options.cancel_token.clone();
    let budget_total = options.budget_total;
    let tool_names = options.tool_names.clone();
    async_with!(context => |ctx| {
        run_in_ctx(ctx, script, channels, tool_names, budget_total, cancel).await
    })
    .await
}

async fn run_in_ctx<'js>(
    ctx: Ctx<'js>,
    script: String,
    channels: HostChannels,
    tool_names: Vec<String>,
    budget_total: Option<u64>,
    cancel: CancellationToken,
) -> Result<serde_json::Value, String> {
    install_host_bindings(&ctx, &channels, &tool_names, budget_total, &cancel)
        .map_err(|err| describe_error(&ctx, err))?;
    ctx.eval::<(), _>(PRELUDE)
        .map_err(|err| format!("whaleflow: prelude failed: {}", describe_error(&ctx, err)))?;

    // Wrap in an async IIFE so scripts get top-level `await` and `return`.
    // The IIFE expression's completion value IS the promise, so a plain eval
    // hands us the exact promise to await (eval_promise's implicit wrapper
    // would resolve to the script completion value instead).
    let wrapped = format!("(async () => {{\n{script}\n}})()");
    let promise: rquickjs::Promise<'js> =
        ctx.eval(wrapped).map_err(|err| describe_error(&ctx, err))?;
    let value: Value<'js> = promise
        .into_future()
        .await
        .map_err(|err| describe_error(&ctx, err))?;
    js_to_json(&ctx, &value).map_err(|err| describe_error(&ctx, err))
}

fn install_host_bindings<'js>(
    ctx: &Ctx<'js>,
    channels: &HostChannels,
    tool_names: &[String],
    budget_total: Option<u64>,
    cancel: &CancellationToken,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    // task({...}) -> Promise<string>
    let spawn_tx = channels.spawn_tx.clone();
    let task_cancel = cancel.clone();
    globals.set(
        "task",
        Function::new(
            ctx.clone(),
            Async(move |ctx: Ctx<'js>, opts: Value<'js>| {
                let spawn_tx = spawn_tx.clone();
                let cancel = task_cancel.clone();
                async move { host_task(ctx, opts, spawn_tx, cancel).await }
            }),
        )?,
    )?;

    // __whaleflow_tool_call(name, args) -> Promise<any>; the prelude fans
    // this out into `tools.<name>` wrappers.
    let tool_tx = channels.tool_tx.clone();
    let tool_cancel = cancel.clone();
    globals.set(
        "__whaleflow_tool_call",
        Function::new(
            ctx.clone(),
            Async(move |ctx: Ctx<'js>, name: String, args: Value<'js>| {
                let tool_tx = tool_tx.clone();
                let cancel = tool_cancel.clone();
                async move { host_tool_call(ctx, name, args, tool_tx, cancel).await }
            }),
        )?,
    )?;

    // __whaleflow_budget_snapshot() -> Promise<{total, spent, remaining}>
    let budget_tx = channels.budget_tx.clone();
    globals.set(
        "__whaleflow_budget_snapshot",
        Function::new(
            ctx.clone(),
            Async(move |ctx: Ctx<'js>| {
                let budget_tx = budget_tx.clone();
                async move { host_budget_snapshot(ctx, budget_tx).await }
            }),
        )?,
    )?;

    // __whaleflow_log(msg): fire-and-forget narrator line.
    let log_tx = channels.log_tx.clone();
    globals.set(
        "__whaleflow_log",
        Function::new(ctx.clone(), move |message: String| {
            let _ = log_tx.send(message);
        })?,
    )?;

    globals.set("__whaleflow_tool_names", tool_names.to_vec())?;
    globals.set("__whaleflow_budget_total", budget_total)?;
    Ok(())
}

async fn host_task<'js>(
    ctx: Ctx<'js>,
    opts: Value<'js>,
    spawn_tx: tokio::sync::mpsc::Sender<SpawnRequest>,
    cancel: CancellationToken,
) -> rquickjs::Result<Value<'js>> {
    let input = js_to_json(&ctx, &opts)?;
    if !input.is_object() {
        return Err(Exception::throw_message(
            &ctx,
            "task() requires an options object, e.g. task({ description: \"...\" })",
        ));
    }
    let (reply_tx, reply_rx) = oneshot::channel();
    if spawn_tx
        .send(SpawnRequest {
            input,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Err(Exception::throw_message(
            &ctx,
            "whaleflow: driver channel closed",
        ));
    }
    let reply = tokio::select! {
        reply = reply_rx => reply,
        () = cancel.cancelled() => {
            return Err(Exception::throw_message(&ctx, "whaleflow: script cancelled"));
        }
    };
    match reply {
        Ok(Ok(done)) => rquickjs::IntoJs::into_js(done.text, &ctx),
        Ok(Err(message)) => Err(Exception::throw_message(
            &ctx,
            &format!("task() failed: {message}"),
        )),
        Err(_) => Err(Exception::throw_message(
            &ctx,
            "whaleflow: driver dropped the task reply",
        )),
    }
}

async fn host_tool_call<'js>(
    ctx: Ctx<'js>,
    name: String,
    args: Value<'js>,
    tool_tx: tokio::sync::mpsc::Sender<ToolCallRequest>,
    cancel: CancellationToken,
) -> rquickjs::Result<Value<'js>> {
    let mut input = js_to_json(&ctx, &args)?;
    if input.is_null() {
        input = serde_json::Value::Object(serde_json::Map::new());
    }
    if !input.is_object() {
        return Err(Exception::throw_message(
            &ctx,
            &format!("tools.{name}(args): args must be an object"),
        ));
    }
    let (reply_tx, reply_rx) = oneshot::channel();
    if tool_tx
        .send(ToolCallRequest {
            name: name.clone(),
            input,
            source: ToolCallSource::JsRepl,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Err(Exception::throw_message(
            &ctx,
            "whaleflow: driver channel closed",
        ));
    }
    let reply = tokio::select! {
        reply = reply_rx => reply,
        () = cancel.cancelled() => {
            return Err(Exception::throw_message(&ctx, "whaleflow: script cancelled"));
        }
    };
    match reply {
        Ok(Ok(outcome)) => {
            if !outcome.success {
                return Err(Exception::throw_message(
                    &ctx,
                    &format!("tools.{name} failed: {}", outcome.content),
                ));
            }
            // Native JSON decode rule: metadata ?? JSON.parse(content) ??
            // String(content) (design §8.1).
            let value = match outcome.metadata {
                Some(metadata) => metadata,
                None => serde_json::from_str::<serde_json::Value>(&outcome.content)
                    .unwrap_or(serde_json::Value::String(outcome.content)),
            };
            json_to_js(&ctx, &value)
        }
        Ok(Err(message)) => Err(Exception::throw_message(
            &ctx,
            &format!("tools.{name} failed: {message}"),
        )),
        Err(_) => Err(Exception::throw_message(
            &ctx,
            "whaleflow: driver dropped the tool reply",
        )),
    }
}

async fn host_budget_snapshot<'js>(
    ctx: Ctx<'js>,
    budget_tx: tokio::sync::mpsc::Sender<BudgetQuery>,
) -> rquickjs::Result<Object<'js>> {
    let (reply_tx, reply_rx) = oneshot::channel();
    if budget_tx
        .send(BudgetQuery { reply: reply_tx })
        .await
        .is_err()
    {
        return Err(Exception::throw_message(
            &ctx,
            "whaleflow: driver channel closed",
        ));
    }
    let snapshot = reply_rx.await.map_err(|_| {
        Exception::throw_message(&ctx, "whaleflow: driver dropped the budget reply")
    })?;
    let object = Object::new(ctx.clone())?;
    object.set("total", snapshot.total)?;
    object.set("spent", snapshot.spent)?;
    object.set("remaining", snapshot.remaining)?;
    Ok(object)
}

/// JS value → `serde_json::Value` via the context's own JSON serializer.
/// `undefined` (not representable in JSON) maps to `null`.
fn js_to_json<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> rquickjs::Result<serde_json::Value> {
    if value.is_undefined() {
        return Ok(serde_json::Value::Null);
    }
    match ctx.json_stringify(value.clone())? {
        Some(text) => {
            let text = text.to_string()?;
            serde_json::from_str(&text).map_err(|_| {
                Exception::throw_message(ctx, "whaleflow: value is not JSON-representable")
            })
        }
        None => Ok(serde_json::Value::Null),
    }
}

fn json_to_js<'js>(ctx: &Ctx<'js>, value: &serde_json::Value) -> rquickjs::Result<Value<'js>> {
    let text = serde_json::to_string(value)
        .map_err(|_| Exception::throw_message(ctx, "whaleflow: failed to serialize host value"))?;
    ctx.json_parse(text)
}

/// Render an rquickjs error, materializing thrown JS exceptions (message +
/// stack) instead of the generic "exception generated by QuickJS".
fn describe_error(ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
    match Result::<(), _>::Err(err).catch(ctx) {
        Err(rquickjs::CaughtError::Exception(exception)) => {
            let message = exception
                .message()
                .unwrap_or_else(|| "uncaught exception".to_string());
            match exception.stack() {
                Some(stack) if !stack.trim().is_empty() => format!("{message}\n{stack}"),
                _ => message,
            }
        }
        Err(rquickjs::CaughtError::Value(value)) => {
            let rendered = js_to_json(ctx, &value)
                .ok()
                .map(|json| json.to_string())
                .unwrap_or_else(|| "unknown thrown value".to_string());
            format!("uncaught value: {rendered}")
        }
        Err(rquickjs::CaughtError::Error(error)) => error.to_string(),
        Ok(()) => "unknown error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn open_channels() -> (
        HostChannels,
        mpsc::Receiver<SpawnRequest>,
        mpsc::Receiver<ToolCallRequest>,
        mpsc::Receiver<BudgetQuery>,
        mpsc::UnboundedReceiver<String>,
    ) {
        let (spawn_tx, spawn_rx) = mpsc::channel(16);
        let (tool_tx, tool_rx) = mpsc::channel(16);
        let (budget_tx, budget_rx) = mpsc::channel(16);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        (
            HostChannels {
                spawn_tx,
                tool_tx,
                budget_tx,
                log_tx,
            },
            spawn_rx,
            tool_rx,
            budget_rx,
            log_rx,
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn whaleflow_vm_runs_plain_scripts_and_prelude() {
        let (channels, _spawn_rx, _tool_rx, _budget_rx, mut log_rx) = open_channels();
        let options = VmOptions {
            budget_total: Some(1000),
            tool_names: vec!["read_file".to_string()],
            ..VmOptions::default()
        };
        let value = run_script(
            r#"
            log("hello from whaleflow");
            const doubled = await parallel([() => 1, () => 2].map((t) => () => t() * 2));
            return {
                doubled,
                total: budget.total,
                hasReadFile: typeof tools.read_file === "function",
                hasWrite: typeof tools.write_file === "function",
            };
            "#
            .to_string(),
            channels,
            options,
        )
        .await
        .expect("script should run");
        assert_eq!(value["doubled"], serde_json::json!([2, 4]));
        assert_eq!(value["total"], serde_json::json!(1000));
        assert_eq!(value["hasReadFile"], serde_json::json!(true));
        assert_eq!(value["hasWrite"], serde_json::json!(false));
        assert_eq!(
            log_rx.try_recv().ok().as_deref(),
            Some("hello from whaleflow")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn whaleflow_vm_parallel_guard_rejects_over_4096_items() {
        let (channels, _spawn_rx, _tool_rx, _budget_rx, _log_rx) = open_channels();
        let err = run_script(
            "await parallel(new Array(4097).fill(() => 1)); return 0;".to_string(),
            channels,
            VmOptions::default(),
        )
        .await
        .expect_err("guard must reject");
        assert!(err.contains("max 4096"), "unexpected error: {err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn whaleflow_vm_cancellation_hard_kills_runaway_script() {
        let (channels, _spawn_rx, _tool_rx, _budget_rx, _log_rx) = open_channels();
        let cancel = CancellationToken::new();
        let options = VmOptions {
            cancel_token: cancel.clone(),
            ..VmOptions::default()
        };
        let killer = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                cancel.cancel();
            }
        });
        // A busy-loop script never yields to the promise queue; only the
        // interrupt handler can stop it.
        let outcome = tokio::time::timeout(
            Duration::from_secs(20),
            run_script("for (;;) {}".to_string(), channels, options),
        )
        .await
        .expect("cancellation must terminate a runaway script");
        killer.await.expect("killer task");
        let err = outcome.expect_err("runaway script must not succeed");
        assert!(
            err.contains("interrupt") || err.contains("cancel"),
            "cancellation surfaces in the error: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn whaleflow_vm_task_error_reply_becomes_js_exception() {
        let (channels, mut spawn_rx, _tool_rx, _budget_rx, _log_rx) = open_channels();
        let driver = tokio::spawn(async move {
            let request = spawn_rx.recv().await.expect("task request");
            assert_eq!(request.input["description"], serde_json::json!("x"));
            let _ = request.reply.send(Err("depth limit reached".to_string()));
        });
        let value = run_script(
            r#"
            try {
                await task({ description: "x" });
                return "no-throw";
            } catch (err) {
                return "caught: " + String(err.message ?? err);
            }
            "#
            .to_string(),
            channels,
            VmOptions::default(),
        )
        .await
        .expect("script should run");
        driver.await.expect("driver task");
        let text = value.as_str().expect("string result");
        assert!(
            text.starts_with("caught:") && text.contains("depth limit reached"),
            "unexpected result: {text}"
        );
    }
}
