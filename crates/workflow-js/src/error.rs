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

/// Why a `task()` call failed, as a stable machine kind (R9).
///
/// The host assigns the kind at the point the failure actually happens and
/// ships it on the task envelope as `error_kind`; the JS prelude copies it
/// onto the thrown `Error` as `.kind`, and `parallel()` / `pipeline()`
/// classify slots from that field alone.
///
/// This exists because the classification used to be a substring match on the
/// operator-facing message. A child whose own reply text contained the words
/// "budget exhausted" — or a script that threw `new Error("run cancelled")` —
/// could forge a fatal classification and abort a healthy run, and a genuine
/// subagent failure was indistinguishable from a plain script throw. The kind
/// is now data, not prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskErrorKind {
    /// The call never became a child: malformed options, an invalid
    /// `responseSchema`, the workflow lifetime cap, or a driver admission
    /// refusal. Nothing ran, so nothing was spent.
    Admission,
    /// The run's shared token pool is exhausted — either the pre-spawn gate
    /// or a child that reported [`crate::TaskCompletion::BudgetExhausted`].
    Budget,
    /// The run or the child was cancelled. Always fatal to the fan-out:
    /// cancellation is the run's own deadline, never a per-slot outcome.
    Cancelled,
    /// The child ran and failed (error result, timeout, tool refusal, ...).
    Agent,
    /// The reply did not satisfy `responseSchema` and no bounded repair
    /// remained.
    Schema,
    /// The driver seam broke: unavailable, or the completion channel dropped
    /// before a terminal outcome arrived.
    Driver,
}

impl TaskErrorKind {
    /// The wire spelling carried on the envelope and on JS `Error.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admission => "admission",
            Self::Budget => "budget",
            Self::Cancelled => "cancelled",
            Self::Agent => "agent",
            Self::Schema => "schema",
            Self::Driver => "driver",
        }
    }
}

/// One `task()` failure: the operator-facing message plus its typed kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskError {
    pub(crate) kind: TaskErrorKind,
    pub(crate) message: String,
}

impl TaskError {
    pub(crate) fn new(kind: TaskErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<&DriverError> for TaskErrorKind {
    fn from(err: &DriverError) -> Self {
        match err {
            // A refused spawn never produced a child; it is admission, not a
            // failure of work that ran.
            DriverError::Rejected(_) => Self::Admission,
            DriverError::Unavailable(_) => Self::Driver,
        }
    }
}
