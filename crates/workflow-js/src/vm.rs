//! The sandboxed QuickJS VM that executes Workflow scripts.
//!
//! Threading model (design §2.2): `rquickjs` contexts and every `'js` value
//! are `!Send`, so each run gets a dedicated OS thread with its own
//! current-thread tokio reactor. Host functions do no heavy work inline —
//! only `Send` data (JSON strings, [`TaskRequest`]s, oneshot replies) crosses
//! to the driver; conversion back into JS values happens on the VM thread
//! after the await resolves.
//!
//! Sandbox: the context registers only standard ECMAScript intrinsics plus
//! the Workflow globals (`task`, `parallel`, `pipeline`, `log`, `phase`,
//! `budget`, `args`). There is no module loader, no fs/net/process access,
//! and `Date`/`Math.random` are overridden to throw so recorded runs stay
//! deterministic for replay.

use std::cell::Cell;
use std::env;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use rquickjs::function::{Async, Func};
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Ctx, Promise, Value};
use serde::Deserialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch};

use crate::driver::{ProgressEvent, TaskCompletion, TaskRequest, WorkflowDriver};
use crate::error::WorkflowJsError;
use crate::schema::{compile_schema, decode_reply};
use crate::{PARALLEL_MAX_ITEMS, WORKFLOW_LIFETIME_CAP, normalize_profile};

const DEFAULT_VM_MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const MIN_VM_MEMORY_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const MAX_VM_MEMORY_LIMIT_BYTES: usize = 512 * 1024 * 1024;
const DEFAULT_VM_STACK_BYTES: usize = 1024 * 1024;
const MIN_VM_STACK_BYTES: usize = 128 * 1024;
const MAX_VM_STACK_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_VM_THREAD_STACK_BYTES: usize = 2 * 1024 * 1024;
const MIN_VM_THREAD_STACK_BYTES: usize = 512 * 1024;
const MAX_VM_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_MAX_CONCURRENT_VMS: usize = 4;
const MAX_CONCURRENT_VMS: usize = 256;

const VM_MEMORY_LIMIT_MB_ENV: &str = "CODEWHALE_WORKFLOW_JS_MEMORY_LIMIT_MB";
const VM_STACK_KB_ENV: &str = "CODEWHALE_WORKFLOW_JS_STACK_KB";
const VM_THREAD_STACK_KB_ENV: &str = "CODEWHALE_WORKFLOW_JS_THREAD_STACK_KB";
const VM_MAX_CONCURRENT_ENV: &str = "CODEWHALE_WORKFLOW_JS_MAX_CONCURRENT";

/// Resource limits applied to the QuickJS runtime before any script runs.
///
/// There is deliberately no wall-clock timeout here: cancellation (dropping
/// the run future, or the driver's cancel cascade) is the deadline mechanism.
#[derive(Debug, Clone, Copy)]
pub struct VmLimits {
    /// QuickJS heap ceiling in bytes (default 32 MiB).
    pub memory_limit_bytes: usize,
    /// Maximum interpreter stack in bytes (default 1 MiB).
    pub max_stack_bytes: usize,
}

impl Default for VmLimits {
    fn default() -> Self {
        Self::from_env()
    }
}

impl VmLimits {
    pub fn from_env() -> Self {
        Self {
            memory_limit_bytes: env_usize_bytes(
                VM_MEMORY_LIMIT_MB_ENV,
                1024 * 1024,
                MIN_VM_MEMORY_LIMIT_BYTES,
                MAX_VM_MEMORY_LIMIT_BYTES,
                DEFAULT_VM_MEMORY_LIMIT_BYTES,
            ),
            max_stack_bytes: env_usize_bytes(
                VM_STACK_KB_ENV,
                1024,
                MIN_VM_STACK_BYTES,
                MAX_VM_STACK_BYTES,
                DEFAULT_VM_STACK_BYTES,
            ),
        }
    }
}

fn env_usize_bytes(name: &str, unit: usize, min: usize, max: usize, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .and_then(|value| value.checked_mul(unit))
        .map(|bytes| bytes.clamp(min, max))
        .unwrap_or(default)
}

fn max_concurrent_vms() -> usize {
    env::var(VM_MAX_CONCURRENT_ENV)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .map(|value| value.clamp(1, MAX_CONCURRENT_VMS))
        .unwrap_or(DEFAULT_MAX_CONCURRENT_VMS)
}

fn vm_thread_stack_bytes() -> usize {
    env_usize_bytes(
        VM_THREAD_STACK_KB_ENV,
        1024,
        MIN_VM_THREAD_STACK_BYTES,
        MAX_VM_THREAD_STACK_BYTES,
        DEFAULT_VM_THREAD_STACK_BYTES,
    )
}

fn vm_admission() -> &'static Arc<Semaphore> {
    static ADMISSION: OnceLock<Arc<Semaphore>> = OnceLock::new();
    ADMISSION.get_or_init(|| Arc::new(Semaphore::new(max_concurrent_vms())))
}

/// Executes Workflow scripts, one isolated QuickJS runtime per run.
///
/// Every [`WorkflowVm::run_script`] call spins up a fresh interpreter on a
/// dedicated thread, so runs share nothing (globals, heap, interned atoms)
/// and a wedged script can never stall a sibling run.
#[derive(Debug, Clone, Default)]
pub struct WorkflowVm {
    limits: VmLimits,
}

impl WorkflowVm {
    /// A VM with the default [`VmLimits`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A VM with explicit resource limits.
    pub fn with_limits(limits: VmLimits) -> Self {
        Self { limits }
    }

    /// Run one Workflow script to completion.
    ///
    /// * `source` is the script body; it is wrapped in an async function, so
    ///   top-level `await` and `return` both work. The returned value is the
    ///   script's `return` value, JSON-encoded (`undefined` becomes `null`).
    /// * `args` is exposed verbatim to the script as the `args` global.
    /// * `driver` executes `task()` spawns and receives progress events. A
    ///   driver instance is scoped to exactly one run: `cancel_all` is always
    ///   invoked at run teardown (success, script error, or cancellation), so
    ///   stray children never outlive the script that spawned them.
    ///
    /// Cancellation cascade (design §9): dropping the returned future cancels
    /// the run — the interrupt handler aborts executing JS, pending `task()`
    /// awaits resolve to errors, and `driver.cancel_all()` is invoked
    /// immediately from the dropping thread.
    pub async fn run_script(
        &self,
        source: &str,
        args: serde_json::Value,
        driver: Arc<dyn WorkflowDriver>,
    ) -> Result<serde_json::Value, WorkflowJsError> {
        self.run_script_with_cancel(source, args, driver, WorkflowRunCancel::new())
            .await
    }

    /// Like [`Self::run_script`], but accepts an external cancel handle so the
    /// host can interrupt the VM without dropping the run future.
    pub async fn run_script_with_cancel(
        &self,
        source: &str,
        args: serde_json::Value,
        driver: Arc<dyn WorkflowDriver>,
        cancel: WorkflowRunCancel,
    ) -> Result<serde_json::Value, WorkflowJsError> {
        let args_json = serde_json::to_string(&args)
            .map_err(|err| WorkflowJsError::InvalidArgs(err.to_string()))?;
        let cancel = cancel.0;
        let (result_tx, result_rx) = oneshot::channel();
        let mut guard = RunGuard {
            cancel: cancel.clone(),
            driver: driver.clone(),
            armed: true,
        };

        let permit = vm_admission()
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| WorkflowJsError::VmInit("VM admission gate closed".to_string()))?;
        let limits = self.limits;
        let source = source.to_string();
        let thread_driver = driver.clone();
        let thread_cancel = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("workflow-js-vm".to_string())
            .stack_size(vm_thread_stack_bytes())
            .spawn(move || {
                let _permit: OwnedSemaphorePermit = permit;
                let outcome = vm_thread_main(
                    source,
                    args_json,
                    thread_driver.clone(),
                    thread_cancel,
                    limits,
                );
                // Run teardown: this driver is scoped to one run, so any task
                // still in flight is unreachable now — cancel the cascade.
                thread_driver.cancel_all();
                let _ = result_tx.send(outcome);
            });
        if let Err(err) = spawned {
            guard.armed = false;
            return Err(WorkflowJsError::VmInit(format!(
                "failed to spawn VM thread: {err}"
            )));
        }

        match result_rx.await {
            Ok(outcome) => {
                // The VM thread has already torn down and cancelled children.
                guard.armed = false;
                outcome
            }
            // VM thread panicked before reporting; leave the guard armed so
            // its drop (right now, at return) cancels outstanding tasks.
            Err(_) => Err(WorkflowJsError::VmTerminated(
                "VM thread exited without reporting a result".to_string(),
            )),
        }
    }
}

/// Cooperative cancel signal shared by the run future (guard side) and the VM
/// thread. The atomic flag feeds the QuickJS interrupt handler (sync, called
/// mid-bytecode); the watch channel wakes host futures parked on driver
/// completions.
#[derive(Clone)]
pub struct WorkflowRunCancel(CancelHandle);

impl WorkflowRunCancel {
    #[must_use]
    pub fn new() -> Self {
        Self(CancelHandle::new())
    }

    pub fn cancel(&self) {
        self.0.cancel();
    }
}

impl Default for WorkflowRunCancel {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct CancelHandle {
    flag: Arc<AtomicBool>,
    tx: Arc<watch::Sender<bool>>,
}

impl CancelHandle {
    fn new() -> Self {
        let (tx, _rx) = watch::channel(false);
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            tx: Arc::new(tx),
        }
    }

    fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.tx.send_replace(true);
    }

    fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        let _ = rx.wait_for(|cancelled| *cancelled).await;
    }

    fn flag_arc(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }
}

/// Fires the cancel cascade if the caller drops the run future before the VM
/// reports a result.
struct RunGuard {
    cancel: CancelHandle,
    driver: Arc<dyn WorkflowDriver>,
    armed: bool,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.cancel();
            self.driver.cancel_all();
        }
    }
}

fn vm_thread_main(
    source: String,
    args_json: String,
    driver: Arc<dyn WorkflowDriver>,
    cancel: CancelHandle,
    limits: VmLimits,
) -> Result<serde_json::Value, WorkflowJsError> {
    let reactor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| WorkflowJsError::VmInit(format!("failed to build VM reactor: {err}")))?;
    reactor.block_on(run_in_vm(source, args_json, driver, cancel, limits))
}

async fn run_in_vm(
    source: String,
    args_json: String,
    driver: Arc<dyn WorkflowDriver>,
    cancel: CancelHandle,
    limits: VmLimits,
) -> Result<serde_json::Value, WorkflowJsError> {
    let runtime = AsyncRuntime::new().map_err(|err| WorkflowJsError::VmInit(err.to_string()))?;
    runtime.set_memory_limit(limits.memory_limit_bytes).await;
    runtime.set_max_stack_size(limits.max_stack_bytes).await;
    let interrupt_flag = cancel.flag_arc();
    runtime
        .set_interrupt_handler(Some(Box::new(move || {
            interrupt_flag.load(Ordering::Acquire)
        })))
        .await;
    let context = AsyncContext::full(&runtime)
        .await
        .map_err(|err| WorkflowJsError::VmInit(err.to_string()))?;

    let result = context
        .async_with(async |ctx| run_in_ctx(ctx, source, args_json, driver, cancel).await)
        .await;
    drop(context);
    runtime.run_gc().await;
    result
}

async fn run_in_ctx(
    ctx: Ctx<'_>,
    source: String,
    args_json: String,
    driver: Arc<dyn WorkflowDriver>,
    cancel: CancelHandle,
) -> Result<serde_json::Value, WorkflowJsError> {
    install_host(&ctx, driver, cancel.clone(), &args_json)?;
    ctx.eval::<(), _>(prelude())
        .catch(&ctx)
        .map_err(|err| WorkflowJsError::VmInit(format!("prelude failed: {err}")))?;

    let desugared = desugar_export_default(&source);
    let wrapped = format!("(async () => {{\n{desugared}\n}})()");
    let promise = ctx
        .eval::<Promise, _>(wrapped)
        .catch(&ctx)
        .map_err(|err| script_error(&cancel, err))?;
    let value = promise
        .into_future::<Value>()
        .await
        .catch(&ctx)
        .map_err(|err| script_error(&cancel, err))?;
    js_value_to_json(&ctx, value)
}

/// Rewrite the documented module-style authoring shape
/// (`export default async function (args) { ... }`) into the script form the
/// VM actually evals. Sources are wrapped in an async IIFE, where the
/// module-only `export` keyword is a syntax error, so without this every
/// imperative `export default` workflow (including the #4131 dogfood
/// fixtures) failed to parse. The default export is captured, invoked with
/// the `args` global when it is a function, and its result becomes the run
/// result; a non-function default export is returned as-is.
fn desugar_export_default(source: &str) -> String {
    const EXPORT_DEFAULT: &str = "export default";
    let Some(offset) = line_leading_export_default(source) else {
        return source.to_string();
    };
    let mut out = source.to_string();
    out.replace_range(
        offset..offset + EXPORT_DEFAULT.len(),
        "globalThis.__workflow_default =",
    );
    out.push('\n');
    out.push_str(
        ";{\n  const __wf_default = globalThis.__workflow_default;\n  delete globalThis.__workflow_default;\n  if (typeof __wf_default === \"function\") {\n    return await __wf_default(args);\n  }\n  if (__wf_default !== undefined) {\n    return __wf_default;\n  }\n}\n",
    );
    out
}

/// Return the byte offset of a line-leading `export default` token that is
/// actual JavaScript syntax, not text inside a string, template literal, or
/// comment. This intentionally recognizes only the documented authoring shape
/// instead of attempting to implement a general JavaScript module parser.
fn line_leading_export_default(source: &str) -> Option<usize> {
    const EXPORT_DEFAULT: &[u8] = b"export default";
    let bytes = source.as_bytes();
    let mut idx = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut line_has_only_whitespace = true;

    while idx < bytes.len() {
        let byte = bytes[idx];

        if line_comment {
            if byte == b'\n' {
                line_comment = false;
                line_has_only_whitespace = true;
            }
            idx += 1;
            continue;
        }

        if block_comment {
            if byte == b'*' && bytes.get(idx + 1) == Some(&b'/') {
                block_comment = false;
                line_has_only_whitespace = false;
                idx += 2;
                continue;
            }
            if byte == b'\n' {
                line_has_only_whitespace = true;
            } else if !byte.is_ascii_whitespace() {
                line_has_only_whitespace = false;
            }
            idx += 1;
            continue;
        }

        if let Some(active_quote) = quote {
            if byte == b'\n' {
                line_has_only_whitespace = true;
                escaped = false;
            } else {
                if !byte.is_ascii_whitespace() {
                    line_has_only_whitespace = false;
                }
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == active_quote {
                    quote = None;
                }
            }
            idx += 1;
            continue;
        }

        if byte == b'\n' {
            line_has_only_whitespace = true;
            idx += 1;
            continue;
        }
        if line_has_only_whitespace && byte.is_ascii_whitespace() {
            idx += 1;
            continue;
        }
        if line_has_only_whitespace && bytes[idx..].starts_with(EXPORT_DEFAULT) {
            return Some(idx);
        }

        line_has_only_whitespace = false;
        if byte == b'/' && bytes.get(idx + 1) == Some(&b'/') {
            line_comment = true;
            idx += 2;
        } else if byte == b'/' && bytes.get(idx + 1) == Some(&b'*') {
            block_comment = true;
            idx += 2;
        } else {
            if matches!(byte, b'\'' | b'"' | b'`') {
                quote = Some(byte);
            }
            idx += 1;
        }
    }

    None
}

fn script_error(cancel: &CancelHandle, err: CaughtError<'_>) -> WorkflowJsError {
    if cancel.is_cancelled() {
        WorkflowJsError::Cancelled
    } else {
        WorkflowJsError::Script(err.to_string())
    }
}

fn js_value_to_json<'js>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
) -> Result<serde_json::Value, WorkflowJsError> {
    if value.is_undefined() {
        return Ok(serde_json::Value::Null);
    }
    let text = ctx
        .json_stringify(value)
        .map_err(|err| WorkflowJsError::ResultEncoding(err.to_string()))?;
    match text {
        None => Ok(serde_json::Value::Null),
        Some(text) => {
            let text = text
                .to_string()
                .map_err(|err| WorkflowJsError::ResultEncoding(err.to_string()))?;
            serde_json::from_str(&text)
                .map_err(|err| WorkflowJsError::ResultEncoding(err.to_string()))
        }
    }
}

fn install_host(
    ctx: &Ctx<'_>,
    driver: Arc<dyn WorkflowDriver>,
    cancel: CancelHandle,
    args_json: &str,
) -> Result<(), WorkflowJsError> {
    let globals = ctx.globals();

    let args_value: Value = ctx
        .json_parse(args_json)
        .map_err(|err| WorkflowJsError::InvalidArgs(err.to_string()))?;
    globals.set("args", args_value).map_err(init_err)?;

    // Per-run lifetime counter (design §4.3): counts spawn *attempts*, and the
    // check + increment happen with no await in between so a parallel burst
    // cannot slip past the cap on the single-threaded VM.
    let spawned = Rc::new(Cell::new(0u64));

    let task_driver = driver.clone();
    let task_cancel = cancel.clone();
    globals
        .set(
            "__workflow_task",
            Func::from(Async(move |opts_json: String| {
                let driver = task_driver.clone();
                let cancel = task_cancel.clone();
                let spawned = spawned.clone();
                async move { task_host(opts_json, driver, cancel, spawned).await }
            })),
        )
        .map_err(init_err)?;

    let log_driver = driver.clone();
    globals
        .set(
            "__workflow_log",
            Func::from(move |message: String| {
                log_driver.progress(ProgressEvent::Log { message });
            }),
        )
        .map_err(init_err)?;

    let phase_driver = driver.clone();
    globals
        .set(
            "__workflow_phase",
            Func::from(move |title: String| {
                phase_driver.progress(ProgressEvent::Phase { title });
            }),
        )
        .map_err(init_err)?;

    // Budget reads are live driver snapshots (design §5.2). NaN encodes
    // "no ceiling" for `total`; the prelude maps it to `null`.
    let total_driver = driver.clone();
    globals
        .set(
            "__workflow_budget_total",
            Func::from(move || -> f64 {
                match total_driver.budget().total {
                    Some(total) => total as f64,
                    None => f64::NAN,
                }
            }),
        )
        .map_err(init_err)?;

    let spent_driver = driver.clone();
    globals
        .set(
            "__workflow_budget_spent",
            Func::from(move || -> f64 { spent_driver.budget().spent as f64 }),
        )
        .map_err(init_err)?;

    globals
        .set(
            "__workflow_budget_remaining",
            Func::from(move || -> f64 {
                match driver.budget().remaining() {
                    Some(remaining) => remaining as f64,
                    None => f64::INFINITY,
                }
            }),
        )
        .map_err(init_err)?;

    Ok(())
}

fn init_err(err: rquickjs::Error) -> WorkflowJsError {
    WorkflowJsError::VmInit(err.to_string())
}

/// The `task()` host call. Everything that can go wrong is reported through
/// the JSON envelope (`{"error": ...}`) so the prelude re-throws it as a real
/// JS `Error` with a script-side stack.
async fn task_host(
    opts_json: String,
    driver: Arc<dyn WorkflowDriver>,
    cancel: CancelHandle,
    spawned: Rc<Cell<u64>>,
) -> String {
    let outcome = task_host_inner(opts_json, driver, cancel, spawned).await;
    let envelope = match outcome {
        Ok(value) => serde_json::json!({ "value": value }),
        Err(message) => serde_json::json!({ "error": message }),
    };
    envelope.to_string()
}

async fn task_host_inner(
    opts_json: String,
    driver: Arc<dyn WorkflowDriver>,
    cancel: CancelHandle,
    spawned: Rc<Cell<u64>>,
) -> Result<serde_json::Value, String> {
    let request = parse_task_options(&opts_json)?;
    // Compile the schema before spawning so a malformed one fails fast
    // instead of burning a subagent.
    let validator = request
        .response_schema
        .as_ref()
        .map(compile_schema)
        .transpose()?;

    // Lifetime backstop (design §4.3) — checked and bumped before any await.
    if spawned.get() >= WORKFLOW_LIFETIME_CAP {
        return Err(format!(
            "task(): Workflow lifetime agent cap ({WORKFLOW_LIFETIME_CAP}) reached for this run"
        ));
    }
    // Fast-fail budget gate. The authoritative reservation lives in the
    // driver (design §5.3); this only stops obviously-doomed spawns early.
    let snapshot = driver.budget();
    if snapshot.exhausted() {
        return Err(format!(
            "task(): budget exhausted ({} of {} tokens spent)",
            snapshot.spent,
            snapshot.total.unwrap_or(0)
        ));
    }
    if cancel.is_cancelled() {
        return Err("task(): run cancelled".to_string());
    }
    spawned.set(spawned.get() + 1);

    let spawned_task = driver
        .spawn_task(request)
        .await
        .map_err(|err| err.to_string())?;
    let task_id = spawned_task.task_id;
    let completion_rx = spawned_task.completion;
    let completion = tokio::select! {
        _ = cancel.cancelled() => return Err("task(): run cancelled".to_string()),
        completion = completion_rx => completion
            .map_err(|_| "task(): driver dropped the completion channel".to_string())?,
    };

    match completion {
        TaskCompletion::Completed { text } => match &validator {
            None => Ok(serde_json::Value::String(text)),
            Some(validator) => match decode_reply(&text, validator) {
                Ok(value) => Ok(value),
                Err(message) => {
                    driver.progress(ProgressEvent::TaskSchemaValidationFailed {
                        task_id,
                        message: message.clone(),
                    });
                    Err(message)
                }
            },
        },
        TaskCompletion::Failed { message } => Err(format!("task(): subagent failed: {message}")),
        TaskCompletion::Cancelled => Err("task(): subagent cancelled".to_string()),
        TaskCompletion::BudgetExhausted { message } => {
            Err(format!("task(): budget exhausted: {message}"))
        }
    }
}

/// JS-facing option names for `task()` (design §3.3). Unknown fields are
/// rejected so a typo (`responseschema`) fails loudly instead of being
/// silently dropped.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskOptions {
    description: Option<String>,
    prompt: Option<String>,
    #[serde(alias = "type")]
    subagent_type: Option<String>,
    /// Fleet role name (#4177). Preferred step identity field.
    role: Option<String>,
    profile: Option<String>,
    model: Option<String>,
    model_strength: Option<String>,
    thinking: Option<String>,
    #[serde(default)]
    worktree: bool,
    write_authority: Option<String>,
    #[serde(default)]
    write_roots: Vec<String>,
    #[serde(default)]
    exact_files: Vec<String>,
    #[serde(default)]
    coordination_contracts: Vec<String>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    acceptance: Vec<String>,
    allowed_tools: Option<Vec<String>>,
    max_depth: Option<u32>,
    token_budget: Option<u64>,
    max_steps: Option<u32>,
    wall_time_secs: Option<u64>,
    response_schema: Option<serde_json::Value>,
    label: Option<String>,
    phase: Option<String>,
}

fn parse_task_options(opts_json: &str) -> Result<TaskRequest, String> {
    let mut options: TaskOptions =
        serde_json::from_str(opts_json).map_err(|err| format!("task(): invalid options: {err}"))?;
    let description = options
        .prompt
        .or(options.description)
        .filter(|description| !description.trim().is_empty())
        .ok_or_else(|| "task(): 'description' (or 'prompt') is required".to_string())?;
    let role = options
        .role
        .as_deref()
        .map(normalize_profile)
        .transpose()
        .map_err(|err| format!("task(): role: {err}"))?;
    let profile = options
        .profile
        .as_deref()
        .map(normalize_profile)
        .transpose()
        .map_err(|err| format!("task(): {err}"))?;
    options.write_roots = normalize_task_paths("writeRoots", options.write_roots, 32)?;
    options.exact_files = normalize_task_paths("exactFiles", options.exact_files, 32)?;
    options.coordination_contracts =
        normalize_task_string_list("coordinationContracts", options.coordination_contracts, 16)?;
    options.dependencies = normalize_task_string_list("dependencies", options.dependencies, 8)?;
    options.acceptance = normalize_task_string_list("acceptance", options.acceptance, 8)?;
    let write_authority = options
        .write_authority
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .map(|value| match value.as_str() {
            "read_only" | "workspace_write" | "worktree_write" => Ok(value),
            _ => Err(format!(
                "task(): writeAuthority must be read_only, workspace_write, or worktree_write; got {value:?}"
            )),
        })
        .transpose()?;
    if write_authority.as_deref() == Some("worktree_write") && !options.worktree {
        return Err("task(): writeAuthority worktree_write requires worktree: true".to_string());
    }
    let role_kind = role.as_deref().and_then(task_role_kind);
    let type_kind = options.subagent_type.as_deref().and_then(task_role_kind);
    if let (Some(role_kind), Some(type_kind)) = (role_kind, type_kind)
        && role_kind != type_kind
    {
        return Err("task(): role and subagentType declare contradictory authorities".to_string());
    }
    let declared_kind = role_kind.or(type_kind);
    if matches!(declared_kind, Some(TaskRoleKind::ReadOnly))
        && write_authority
            .as_deref()
            .is_some_and(|authority| authority != "read_only")
    {
        return Err("task(): read-only roles cannot declare write-capable authority".to_string());
    }
    if declared_kind == Some(TaskRoleKind::Implementer)
        && write_authority.as_deref() == Some("read_only")
    {
        return Err("task(): implementer cannot declare read_only authority".to_string());
    }
    if write_authority
        .as_deref()
        .is_some_and(|authority| authority != "read_only")
        && options.write_roots.is_empty()
        && options.exact_files.is_empty()
        && options.coordination_contracts.is_empty()
    {
        return Err(
            "task(): write-capable authority requires writeRoots, exactFiles, or coordinationContracts"
                .to_string(),
        );
    }
    let explicit_write_identity = declared_kind == Some(TaskRoleKind::Implementer)
        || (declared_kind == Some(TaskRoleKind::General)
            && (role.is_some() || options.subagent_type.is_some()))
        || (profile.is_some() && declared_kind.is_none());
    if explicit_write_identity
        && write_authority.as_deref() != Some("read_only")
        && options.write_roots.is_empty()
        && options.exact_files.is_empty()
        && options.coordination_contracts.is_empty()
    {
        return Err(
            "task(): explicit write-capable identities require writeRoots, exactFiles, or coordinationContracts"
                .to_string(),
        );
    }
    Ok(TaskRequest {
        description,
        subagent_type: options.subagent_type,
        role,
        profile,
        model: options.model,
        model_strength: options.model_strength,
        thinking: options.thinking,
        worktree: options.worktree,
        write_authority,
        write_roots: options.write_roots,
        exact_files: options.exact_files,
        coordination_contracts: options.coordination_contracts,
        dependencies: options.dependencies,
        acceptance: options.acceptance,
        allowed_tools: options.allowed_tools,
        max_depth: options.max_depth,
        token_budget: options.token_budget,
        max_steps: options.max_steps,
        wall_time_secs: options.wall_time_secs,
        response_schema: options.response_schema,
        label: options.label,
        phase: options.phase,
    })
}

fn normalize_task_string_list(
    field: &str,
    values: Vec<String>,
    limit: usize,
) -> Result<Vec<String>, String> {
    if values.len() > limit {
        return Err(format!("task(): {field} accepts at most {limit} entries"));
    }
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > 512 {
            return Err(format!(
                "task(): {field} entries must be 1..=512 characters"
            ));
        }
        if !normalized.iter().any(|existing| existing == value) {
            normalized.push(value.to_string());
        }
    }
    Ok(normalized)
}

fn normalize_task_paths(
    field: &str,
    values: Vec<String>,
    limit: usize,
) -> Result<Vec<String>, String> {
    if values.len() > limit {
        return Err(format!("task(): {field} accepts at most {limit} entries"));
    }
    let mut normalized = Vec::new();
    for raw in values {
        let raw = raw.trim().replace('\\', "/");
        let windows_drive = raw.as_bytes().get(1) == Some(&b':')
            && raw.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
        if raw.is_empty()
            || raw.chars().count() > 512
            || raw.starts_with('/')
            || raw.starts_with("//")
            || windows_drive
            || raw.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n'))
        {
            return Err(format!(
                "task(): {field} entries must be bounded repo-relative paths"
            ));
        }
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    return Err(format!(
                        "task(): {field} paths cannot contain parent traversal"
                    ));
                }
                value => segments.push(value),
            }
        }
        let path = if segments.is_empty() {
            ".".to_string()
        } else {
            segments.join("/")
        };
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    Ok(normalized)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskRoleKind {
    ReadOnly,
    General,
    Implementer,
}

fn task_role_kind(value: &str) -> Option<TaskRoleKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "explore" | "explorer" | "plan" | "planner" | "review" | "reviewer" | "verify"
        | "verifier" => Some(TaskRoleKind::ReadOnly),
        "general" | "worker" => Some(TaskRoleKind::General),
        "implement" | "implementer" => Some(TaskRoleKind::Implementer),
        _ => None,
    }
}

/// The JS prelude injected before every script: determinism bans, the
/// `task`/`parallel`/`pipeline`/`log`/`phase` stdlib (design §7), and the
/// `budget` global.
fn prelude() -> String {
    PRELUDE_TEMPLATE.replace("__MAX_ITEMS__", &PARALLEL_MAX_ITEMS.to_string())
}

const PRELUDE_TEMPLATE: &str = r#""use strict";
(() => {
  const banned = (name) => () => {
    throw new Error(name + " is unavailable in Workflow scripts: runs must be deterministic for record/replay");
  };
  const BannedDate = function Date() {
    throw new Error("new Date()/Date() is unavailable in Workflow scripts: runs must be deterministic for record/replay");
  };
  BannedDate.now = banned("Date.now()");
  BannedDate.parse = banned("Date.parse()");
  BannedDate.UTC = banned("Date.UTC()");
  globalThis.Date = BannedDate;
  Math.random = banned("Math.random()");

  // Capture temporary host bindings into this closure, then strip them from
  // globalThis so scripts only see the documented Workflow surface (#4129).
  const hostTask = __workflow_task;
  const hostLog = __workflow_log;
  const hostPhase = __workflow_phase;
  const hostBudgetTotal = __workflow_budget_total;
  const hostBudgetSpent = __workflow_budget_spent;
  const hostBudgetRemaining = __workflow_budget_remaining;

  const MAX_ITEMS = __MAX_ITEMS__;
  const taskErrorText = (err) => String(err && err.message !== undefined ? err.message : err);
  const isFatalTaskError = (err) => {
    const text = taskErrorText(err);
    return text.includes("responseSchema") || text.includes("run cancelled");
  };

  globalThis.task = async (opts) => {
    if (opts === null || typeof opts !== "object") {
      throw new TypeError("task(): expected an options object");
    }
    const envelope = JSON.parse(await hostTask(JSON.stringify(opts)));
    if (envelope.error !== undefined) {
      throw new Error(envelope.error);
    }
    return envelope.value;
  };

  globalThis.parallel = (thunks) => {
    if (!Array.isArray(thunks)) {
      throw new TypeError("parallel(): expected an array of thunks");
    }
    if (thunks.length > MAX_ITEMS) {
      throw new Error("parallel(): max " + MAX_ITEMS + " items per call");
    }
    return Promise.all(thunks.map((thunk) => {
      try {
        return Promise.resolve(typeof thunk === "function" ? thunk() : thunk).catch((err) => {
          if (isFatalTaskError(err)) throw err;
          hostLog("parallel(): dropped a failed slot as null: " + String((err && err.message) || err));
          return null;
        });
      } catch (err) {
        if (isFatalTaskError(err)) return Promise.reject(err);
        hostLog("parallel(): dropped a failed slot as null: " + String((err && err.message) || err));
        return null;
      }
    }));
  };

  globalThis.pipeline = (items, ...stages) => {
    if (!Array.isArray(items)) {
      throw new TypeError("pipeline(): expected an array of items");
    }
    if (items.length > MAX_ITEMS) {
      throw new Error("pipeline(): max " + MAX_ITEMS + " items per call");
    }
    return Promise.all(items.map(async (item, index) => {
      let value = item;
      for (const stage of stages) {
        try {
          value = await stage(value, item, index);
        } catch (err) {
          if (isFatalTaskError(err)) throw err;
          hostLog("pipeline(): dropped item " + index + " as null: " + String((err && err.message) || err));
          return null;
        }
      }
      return value;
    }));
  };

  globalThis.log = (message) => {
    hostLog(typeof message === "string" ? message : (JSON.stringify(message) ?? String(message)));
  };
  globalThis.phase = (title) => {
    hostPhase(String(title));
  };

  const total = hostBudgetTotal();
  globalThis.budget = Object.freeze({
    total: Number.isNaN(total) ? null : total,
    spent: () => hostBudgetSpent(),
    remaining: () => hostBudgetRemaining(),
  });

  for (const name of [
    "__workflow_task",
    "__workflow_log",
    "__workflow_phase",
    "__workflow_budget_total",
    "__workflow_budget_spent",
    "__workflow_budget_remaining",
  ]) {
    try {
      delete globalThis[name];
    } catch (_) {
      // Non-configurable bindings stay; the inventory test will fail closed.
    }
  }
})();
"#;
