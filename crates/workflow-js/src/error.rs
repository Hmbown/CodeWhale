//! Error types for the dynamic Workflow runtime.

use thiserror::Error;

/// Errors surfaced by [`crate::WorkflowVm::run_script`].
///
/// Script-visible failures (thrown JS exceptions, rejected promises, host
/// function errors that were not caught inside the script) all collapse into
/// [`WorkflowJsError::Script`] with the exception message and stack. The
/// remaining variants describe runtime-level failures that never reached the
/// script.
#[derive(Debug, Error)]
pub enum WorkflowJsError {
    /// The QuickJS runtime or context could not be created.
    #[error("failed to initialize the Workflow JS VM: {0}")]
    VmInit(String),
    /// The script threw (or a promise rejected) and nothing caught it.
    /// Carries the exception message plus stack when available.
    #[error("script error: {0}")]
    Script(String),
    /// The run was cancelled — either the caller dropped the run future or
    /// the cooperative cancel signal fired mid-script.
    #[error("workflow run cancelled")]
    Cancelled,
    /// The script completed but its return value could not be encoded as
    /// JSON (e.g. it returned a function or a cyclic object).
    #[error("script result is not JSON-encodable: {0}")]
    ResultEncoding(String),
    /// The invocation arguments could not be injected into the VM.
    #[error("invalid workflow arguments: {0}")]
    InvalidArgs(String),
    /// The dedicated VM thread exited without reporting a result (panic or
    /// spawn failure). Outstanding driver tasks are cancelled when this is
    /// observed.
    #[error("Workflow VM thread terminated unexpectedly: {0}")]
    VmTerminated(String),
}

/// Errors a [`crate::WorkflowDriver`] can return from `spawn_task`.
///
/// Both variants surface inside the script as a thrown exception on the
/// corresponding `task()` call, so a script can `try`/`catch` an individual
/// rejection (admission, depth, budget) without the whole run failing.
#[derive(Debug, Clone, Error)]
pub enum DriverError {
    /// The driver refused to spawn this task (admission cap, depth ceiling,
    /// budget reservation failure, invalid subagent type, ...).
    #[error("spawn rejected: {0}")]
    Rejected(String),
    /// The driver is gone or its channel closed; no more spawns will work.
    #[error("driver unavailable: {0}")]
    Unavailable(String),
}
