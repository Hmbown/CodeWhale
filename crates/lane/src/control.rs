//! Shared command / control-plane contract (#1888, #4022).
//!
//! Slash commands, hotbar actions, and CLI entrypoints for the same lifecycle
//! operation must agree on *one* typed descriptor, one target parser, one
//! result/receipt shape, and one renderer. This module owns that contract.
//!
//! Vocabulary is the shipped public vocabulary and nothing else:
//! **Fleet** = who, **Workflow** = order, **Lane** = one running Workflow,
//! **Runtime** = where/how. There is no "Operation" product noun here — the
//! `ControlOperation` type names *control-plane verbs*, which is an internal
//! contract detail, never a user-facing noun.
//!
//! Why this lives in `codewhale-lane`: it is the lowest crate that both the
//! thin `codewhale` CLI facade and the TUI (slash commands, hotbar, and the
//! `codewhale fleet …` entrypoints that the facade delegates to) already
//! depend on. Putting the contract anywhere else would fork it.

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::registry::{LaneRecord, LaneStatus, TerminalTransition};

/// Maximum rows any surface may render for a run list in one payload.
pub const DEFAULT_RUN_LIST_LIMIT: usize = 50;
/// Hard ceiling for a run list, even when a caller asks for more.
pub const MAX_RUN_LIST_LIMIT: usize = 200;
/// Maximum characters in one sanitized detail/failure line.
pub const MAX_DETAIL_LINE_CHARS: usize = 240;
/// Maximum sanitized detail lines carried on one receipt.
pub const MAX_DETAIL_LINES: usize = 40;
/// Replacement token written wherever a secret-shaped value was removed.
pub const REDACTED: &str = "[redacted]";

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

/// A user-facing command surface that can invoke a control-plane verb.
///
/// There are exactly two. **The hotbar is not a surface**: a hotbar slot binds
/// a slash command and fires it through `commands::execute` with no argument,
/// so what actually runs is the slash surface and the receipt says `slash`.
/// Modelling the hotbar as a third surface let the contract advertise
/// target-taking verbs (`lane.interrupt`, `fleet.resume`) as hotbar-reachable
/// when a bare press can never supply an id. See
/// [`OperationDescriptor::hotbar_bare_dispatch`] for what a press really does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlSurface {
    /// `codewhale …` (and the `codewhale-tui …` entrypoints it delegates to).
    Cli,
    /// A `/command` typed into the composer — or dispatched by a hotbar slot,
    /// which is the same code path with the same authority.
    Slash,
}

impl ControlSurface {
    pub const ALL: &'static [ControlSurface] = &[Self::Cli, Self::Slash];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Slash => "slash",
        }
    }

    /// Whether this surface may block on Runtime teardown (subprocesses,
    /// advisory locks, worktree removal).
    ///
    /// The CLI owns its process and may block. The slash surface runs on the
    /// TUI composer thread, where a `tmux kill-session` or a `git worktree`
    /// removal would freeze the UI, so it may not.
    #[must_use]
    pub const fn may_block(self) -> bool {
        matches!(self, Self::Cli)
    }
}

impl fmt::Display for ControlSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

const ALL_SURFACES: &[ControlSurface] = ControlSurface::ALL;
const CLI_ONLY: &[ControlSurface] = &[ControlSurface::Cli];

/// How much work a caller is allowed to do on the thread it is running on.
///
/// Reconciliation folds a finished Runtime exit into the durable record, which
/// for tmux means probing `tmux has-session` (a subprocess) and taking the
/// per-Lane advisory lock. That is correct on the CLI and unacceptable on the
/// TUI composer thread, so the slash surface reads the registry without it and
/// says so on the receipt rather than freezing the UI or lying about freshness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlExecution {
    /// Reconcile durable state and perform Runtime teardown. CLI only.
    Blocking,
    /// Registry reads only: no subprocess, no teardown, no reconciliation.
    NonBlocking,
}

impl ControlExecution {
    /// The execution mode a surface is allowed to use.
    #[must_use]
    pub const fn for_surface(surface: ControlSurface) -> Self {
        if surface.may_block() {
            Self::Blocking
        } else {
            Self::NonBlocking
        }
    }

    #[must_use]
    pub const fn reconciles(self) -> bool {
        matches!(self, Self::Blocking)
    }
}

// ---------------------------------------------------------------------------
// Domain / authority / persistence / target
// ---------------------------------------------------------------------------

/// Which durable control plane a verb acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlDomain {
    /// One running Workflow, recorded in `$CODEWHALE_HOME/lanes/`.
    Lane,
    /// fleet workers and runs, recorded in `<workspace>/.codewhale/fleet.jsonl`.
    Fleet,
}

impl ControlDomain {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lane => "lane",
            Self::Fleet => "fleet",
        }
    }

    /// Customer-facing spelling. `as_str` remains the durable wire/storage key.
    #[must_use]
    pub const fn public_name(self) -> &'static str {
        match self {
            Self::Lane => "lane",
            Self::Fleet => "fleet",
        }
    }
}

/// Read-vs-write authority a verb needs.
///
/// This is *not* a permission posture. Auto-Review is a permission posture;
/// this says whether the verb only observes durable state or mutates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAuthority {
    Read,
    Write,
}

impl ControlAuthority {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    #[must_use]
    pub const fn is_write(self) -> bool {
        matches!(self, Self::Write)
    }
}

/// Where the durable effect of a verb lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistenceScope {
    /// Nothing outlives the process.
    Ephemeral,
    /// Current TUI session state only.
    Session,
    /// `$CODEWHALE_HOME/lanes/` records and logs.
    LaneRegistry,
    /// `<workspace>/.codewhale/fleet.jsonl`.
    FleetLedger,
}

impl PersistenceScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ephemeral => "ephemeral",
            Self::Session => "session",
            Self::LaneRegistry => "lane_registry",
            Self::FleetLedger => "fleet_ledger",
        }
    }

    #[must_use]
    pub const fn is_durable(self) -> bool {
        matches!(self, Self::LaneRegistry | Self::FleetLedger)
    }
}

/// What kind of exact identity a verb targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// The verb acts on the whole ledger/registry; it takes no target.
    None,
    /// One Lane id (`lane-a1b2c3d4`).
    LaneRun,
    /// One Fleet worker id.
    FleetWorker,
    /// One Fleet run id.
    FleetRun,
}

impl TargetKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::LaneRun => "lane_run",
            Self::FleetWorker => "fleet_worker",
            Self::FleetRun => "fleet_run",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "target",
            Self::LaneRun => "lane id",
            Self::FleetWorker => "worker id",
            Self::FleetRun => "run id",
        }
    }

    #[must_use]
    pub const fn requires_identity(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Whether re-issuing the verb after a failure is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retryability {
    /// Idempotent: repeating it converges on the same durable state.
    Idempotent,
    /// Repeating it may produce additional work; ask before retrying.
    Unsafe,
}

impl Retryability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idempotent => "idempotent",
            Self::Unsafe => "unsafe",
        }
    }
}

// ---------------------------------------------------------------------------
// Verbs
// ---------------------------------------------------------------------------

/// The lifecycle verbs every surface shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlOperation {
    LaneList,
    LaneStatus,
    LaneInterrupt,
    LaneRestart,
    LaneResume,
    FleetList,
    FleetStatus,
    FleetInterrupt,
    FleetRestart,
    FleetResume,
}

impl ControlOperation {
    pub const ALL: &'static [ControlOperation] = &[
        Self::LaneList,
        Self::LaneStatus,
        Self::LaneInterrupt,
        Self::LaneRestart,
        Self::LaneResume,
        Self::FleetList,
        Self::FleetStatus,
        Self::FleetInterrupt,
        Self::FleetRestart,
        Self::FleetResume,
    ];

    /// Stable wire id shared by every surface, receipt, and test.
    #[must_use]
    pub fn id(self) -> &'static str {
        self.descriptor().id
    }

    #[must_use]
    pub fn descriptor(self) -> &'static OperationDescriptor {
        OPERATIONS
            .iter()
            .find(|descriptor| descriptor.operation == self)
            .expect("every ControlOperation has exactly one descriptor")
    }

    /// Resolve a descriptor from its stable id (`"lane.status"`).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        OPERATIONS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .map(|descriptor| descriptor.operation)
    }

    /// Resolve a descriptor from a domain plus the verb word a user typed.
    ///
    /// Every surface routes through this so `/lane interrupt`, a hotbar
    /// dispatch of the same command, and `codewhale lane interrupt` cannot
    /// drift onto different verbs. Compatibility spellings live here once.
    #[must_use]
    pub fn parse_verb(domain: ControlDomain, verb: &str) -> Option<Self> {
        let verb = verb.trim().to_ascii_lowercase();
        let canonical = match verb.as_str() {
            "list" | "ls" | "runs" => "list",
            "status" | "show" | "info" | "inspect" => "status",
            // `stop` and `cancel` are the historical Lane/Fleet spellings for
            // the same durable transition; they are aliases, not new verbs.
            "interrupt" | "stop" | "cancel" | "kill" => "interrupt",
            "restart" | "retry" => "restart",
            "resume" | "reconcile" => "resume",
            _ => return None,
        };
        OPERATIONS
            .iter()
            .find(|descriptor| descriptor.domain == domain && descriptor.verb == canonical)
            .map(|descriptor| descriptor.operation)
    }
}

impl fmt::Display for ControlOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

// ---------------------------------------------------------------------------
// Backend capability + availability
// ---------------------------------------------------------------------------

/// Whether a backend exists for a verb at all, and on which surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendCapability {
    /// Wired end to end on every surface the descriptor lists.
    Implemented,
    /// Declared in the contract but not built. No surface may offer it.
    NotImplemented { hint: &'static str },
    /// Built, but only reachable from some surfaces. The rest must say so.
    SurfaceLimited {
        available_on: &'static [ControlSurface],
        hint: &'static str,
    },
}

/// Typed reason a surface cannot run a verb right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// The descriptor does not offer this verb on this surface.
    SurfaceNotOffered,
    /// The backend exists but is not reachable from this surface.
    SurfaceNotSupported,
    /// No backend has been built for this verb.
    BackendNotImplemented,
    /// `$CODEWHALE_HOME/lanes/` has no records yet.
    NoLaneRegistry,
    /// This workspace has no `.codewhale/fleet.jsonl`.
    NoFleetLedger,
}

impl UnavailableReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SurfaceNotOffered => "surface_not_offered",
            Self::SurfaceNotSupported => "surface_not_supported",
            Self::BackendNotImplemented => "backend_not_implemented",
            Self::NoLaneRegistry => "no_lane_registry",
            Self::NoFleetLedger => "no_fleet_ledger",
        }
    }
}

/// Availability of one verb on one surface in one context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Availability {
    Available,
    Unavailable {
        reason: UnavailableReason,
        /// Sanitized, bounded operator-facing explanation.
        hint: String,
    },
}

impl Availability {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    #[must_use]
    pub fn reason(&self) -> Option<UnavailableReason> {
        match self {
            Self::Available => None,
            Self::Unavailable { reason, .. } => Some(*reason),
        }
    }

    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Available => None,
            Self::Unavailable { hint, .. } => Some(hint.as_str()),
        }
    }

    fn unavailable(reason: UnavailableReason, hint: impl AsRef<str>) -> Self {
        Self::Unavailable {
            reason,
            hint: sanitize_line(hint.as_ref()),
        }
    }
}

/// Observed environment used to decide availability.
///
/// Probing is deliberately read-only: a status command must never create the
/// durable store it is reporting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ControlContext {
    pub lane_registry_present: bool,
    pub fleet_ledger_present: bool,
}

impl ControlContext {
    #[must_use]
    pub const fn new(lane_registry_present: bool, fleet_ledger_present: bool) -> Self {
        Self {
            lane_registry_present,
            fleet_ledger_present,
        }
    }

    /// Probe both durable stores without creating either of them.
    #[must_use]
    pub fn probe(lane_registry_root: Option<&Path>, fleet_ledger_path: Option<&Path>) -> Self {
        Self {
            lane_registry_present: lane_registry_root.is_some_and(Path::is_dir),
            fleet_ledger_present: fleet_ledger_path.is_some_and(Path::is_file),
        }
    }
}

// ---------------------------------------------------------------------------
// Descriptor
// ---------------------------------------------------------------------------

/// The single typed descriptor every surface reads.
#[derive(Debug, Clone, Copy)]
pub struct OperationDescriptor {
    pub operation: ControlOperation,
    /// Stable wire id, `"<domain>.<verb>"`.
    pub id: &'static str,
    pub domain: ControlDomain,
    /// Canonical verb word (`list`, `status`, `interrupt`, `restart`, `resume`).
    pub verb: &'static str,
    pub authority: ControlAuthority,
    pub persistence: PersistenceScope,
    pub target: TargetKind,
    pub retry: Retryability,
    pub surfaces: &'static [ControlSurface],
    pub backend: BackendCapability,
    /// Whether a read of this verb may fold a finished Runtime exit into the
    /// durable record (see [`ControlExecution::Blocking`]).
    ///
    /// This is declared, not discovered: a `Read` verb that can transition a
    /// record must say so up front, and the receipt reports whether it
    /// actually did (`ControlReceipt::reconciled`).
    pub reconciles: bool,
    /// Whether a bare hotbar press of the owning slash command reaches *this*
    /// verb.
    ///
    /// A hotbar slot fires `/<slash_command>` with no argument. Only the verb
    /// that a bare invocation resolves to is reachable, and it necessarily
    /// takes no target. Everything else needs an id the press cannot supply.
    pub hotbar_bare_dispatch: bool,
    /// Slash command name that owns this verb (hotbar id is `slash.<name>`).
    pub slash_command: &'static str,
    /// Exact CLI invocation, for cross-surface hints and docs.
    pub cli_invocation: &'static str,
    /// One-line summary, shared by every surface's help text.
    pub summary: &'static str,
}

impl OperationDescriptor {
    /// Hotbar action id derived from the owning slash command.
    ///
    /// The hotbar registers one action per slash command and dispatches it
    /// through `commands::execute`, so this is the whole binding — there is no
    /// second hotbar-side verb table to drift. Binding the action does **not**
    /// mean this verb runs when the slot is pressed; see
    /// [`Self::hotbar_bare_dispatch`].
    #[must_use]
    pub fn hotbar_action_id(&self) -> String {
        format!("slash.{}", self.slash_command)
    }

    /// Exact slash invocation for this verb.
    #[must_use]
    pub fn slash_invocation(&self) -> String {
        if self.target.requires_identity() {
            format!(
                "/{} {} <{}>",
                self.slash_command,
                self.verb,
                self.target.label()
            )
        } else {
            format!("/{} {}", self.slash_command, self.verb)
        }
    }

    #[must_use]
    pub fn offers(&self, surface: ControlSurface) -> bool {
        self.surfaces.contains(&surface)
    }

    /// Availability of this verb on `surface`, given a probed context.
    #[must_use]
    pub fn availability(&self, surface: ControlSurface, ctx: ControlContext) -> Availability {
        if !self.offers(surface) {
            return Availability::unavailable(
                UnavailableReason::SurfaceNotOffered,
                format!("{} is not offered on the {surface} surface", self.id),
            );
        }
        match self.backend {
            BackendCapability::NotImplemented { hint } => {
                return Availability::unavailable(UnavailableReason::BackendNotImplemented, hint);
            }
            BackendCapability::SurfaceLimited { available_on, hint } => {
                if !available_on.contains(&surface) {
                    return Availability::unavailable(UnavailableReason::SurfaceNotSupported, hint);
                }
            }
            BackendCapability::Implemented => {}
        }
        match self.persistence {
            PersistenceScope::LaneRegistry if !ctx.lane_registry_present => {
                Availability::unavailable(
                    UnavailableReason::NoLaneRegistry,
                    "no Lane registry yet; start one with `codewhale lane start`",
                )
            }
            PersistenceScope::FleetLedger if !ctx.fleet_ledger_present => {
                Availability::unavailable(
                    UnavailableReason::NoFleetLedger,
                    "this workspace has no .codewhale/fleet.jsonl; create it with \
                     `codewhale fleet init`",
                )
            }
            _ => Availability::Available,
        }
    }
}

const LANE_RESTART_HINT: &str = "Lane restart has no backend: a Lane is one running Workflow and is re-created by \
     `codewhale lane start` / `codewhale workflow run`, not restarted in place.";
const LANE_RESUME_HINT: &str = "Lane resume has no backend: a stopped Lane's Runtime session is gone, so there is \
     nothing to resume. Start a new Lane against the same issue/goal.";
const FLEET_RESTART_HINT: &str = "Fleet restart re-leases a task and then drives the manager loop to completion, which \
     only the CLI runs. Use `codewhale fleet restart <worker-id>`.";
/// Lane interrupt tears down the Runtime (tmux kill-session, worktree TTL
/// cleanup), which must never run on the TUI composer thread. It is *not*
/// CLI-only: the slash surface submits it to an off-loop worker and returns a
/// `queued` receipt with a ticket. See `codewhale-tui::lane_control`.
const LANE_INTERRUPT_OFF_LOOP: &str =
    "submitted to the Lane control worker; the terminal receipt arrives under this ticket";

/// The one descriptor table. Every surface reads it; none copies it.
pub static OPERATIONS: &[OperationDescriptor] = &[
    OperationDescriptor {
        operation: ControlOperation::LaneList,
        id: "lane.list",
        domain: ControlDomain::Lane,
        verb: "list",
        authority: ControlAuthority::Read,
        persistence: PersistenceScope::LaneRegistry,
        target: TargetKind::None,
        retry: Retryability::Idempotent,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::Implemented,
        reconciles: true,
        hotbar_bare_dispatch: true,
        slash_command: "lane",
        cli_invocation: "codewhale lane list",
        summary: "List durable Lanes newest first.",
    },
    OperationDescriptor {
        operation: ControlOperation::LaneStatus,
        id: "lane.status",
        domain: ControlDomain::Lane,
        verb: "status",
        authority: ControlAuthority::Read,
        persistence: PersistenceScope::LaneRegistry,
        target: TargetKind::LaneRun,
        retry: Retryability::Idempotent,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::Implemented,
        reconciles: true,
        hotbar_bare_dispatch: false,
        slash_command: "lane",
        cli_invocation: "codewhale lane status <lane-id>",
        summary: "Show one Lane's durable status, Runtime, and attach metadata.",
    },
    OperationDescriptor {
        operation: ControlOperation::LaneInterrupt,
        id: "lane.interrupt",
        domain: ControlDomain::Lane,
        verb: "interrupt",
        authority: ControlAuthority::Write,
        persistence: PersistenceScope::LaneRegistry,
        target: TargetKind::LaneRun,
        retry: Retryability::Idempotent,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::Implemented,
        reconciles: true,
        hotbar_bare_dispatch: false,
        slash_command: "lane",
        cli_invocation: "codewhale lane interrupt <lane-id>",
        summary: "Stop a running Lane and run its worktree TTL cleanup.",
    },
    OperationDescriptor {
        operation: ControlOperation::LaneRestart,
        id: "lane.restart",
        domain: ControlDomain::Lane,
        verb: "restart",
        authority: ControlAuthority::Write,
        persistence: PersistenceScope::LaneRegistry,
        target: TargetKind::LaneRun,
        retry: Retryability::Unsafe,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::NotImplemented {
            hint: LANE_RESTART_HINT,
        },
        reconciles: false,
        hotbar_bare_dispatch: false,
        slash_command: "lane",
        cli_invocation: "codewhale lane restart <lane-id>",
        summary: "Restart a Lane in place (no backend).",
    },
    OperationDescriptor {
        operation: ControlOperation::LaneResume,
        id: "lane.resume",
        domain: ControlDomain::Lane,
        verb: "resume",
        authority: ControlAuthority::Write,
        persistence: PersistenceScope::LaneRegistry,
        target: TargetKind::LaneRun,
        retry: Retryability::Unsafe,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::NotImplemented {
            hint: LANE_RESUME_HINT,
        },
        reconciles: false,
        hotbar_bare_dispatch: false,
        slash_command: "lane",
        cli_invocation: "codewhale lane resume <lane-id>",
        summary: "Resume a stopped Lane (no backend).",
    },
    OperationDescriptor {
        operation: ControlOperation::FleetList,
        id: "fleet.list",
        domain: ControlDomain::Fleet,
        verb: "list",
        authority: ControlAuthority::Read,
        persistence: PersistenceScope::FleetLedger,
        target: TargetKind::None,
        retry: Retryability::Idempotent,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::Implemented,
        reconciles: false,
        hotbar_bare_dispatch: false,
        slash_command: "fleet",
        cli_invocation: "codewhale fleet list",
        summary: "List durable fleet runs from the workspace ledger.",
    },
    OperationDescriptor {
        operation: ControlOperation::FleetStatus,
        id: "fleet.status",
        domain: ControlDomain::Fleet,
        verb: "status",
        authority: ControlAuthority::Read,
        persistence: PersistenceScope::FleetLedger,
        target: TargetKind::None,
        retry: Retryability::Idempotent,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::Implemented,
        reconciles: false,
        hotbar_bare_dispatch: false,
        slash_command: "fleet",
        cli_invocation: "codewhale fleet status",
        summary: "Show durable Fleet run/worker counts from the workspace ledger.",
    },
    OperationDescriptor {
        operation: ControlOperation::FleetInterrupt,
        id: "fleet.interrupt",
        domain: ControlDomain::Fleet,
        verb: "interrupt",
        authority: ControlAuthority::Write,
        persistence: PersistenceScope::FleetLedger,
        target: TargetKind::FleetWorker,
        retry: Retryability::Idempotent,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::Implemented,
        reconciles: false,
        hotbar_bare_dispatch: false,
        slash_command: "fleet",
        cli_invocation: "codewhale fleet interrupt <worker-id>",
        summary: "Cancel a fleet worker's active task in the durable ledger.",
    },
    OperationDescriptor {
        operation: ControlOperation::FleetRestart,
        id: "fleet.restart",
        domain: ControlDomain::Fleet,
        verb: "restart",
        authority: ControlAuthority::Write,
        persistence: PersistenceScope::FleetLedger,
        target: TargetKind::FleetWorker,
        retry: Retryability::Unsafe,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::SurfaceLimited {
            available_on: CLI_ONLY,
            hint: FLEET_RESTART_HINT,
        },
        reconciles: false,
        hotbar_bare_dispatch: false,
        slash_command: "fleet",
        cli_invocation: "codewhale fleet restart <worker-id>",
        summary: "Re-lease a fleet worker's task and drive the manager loop.",
    },
    OperationDescriptor {
        operation: ControlOperation::FleetResume,
        id: "fleet.resume",
        domain: ControlDomain::Fleet,
        verb: "resume",
        authority: ControlAuthority::Write,
        persistence: PersistenceScope::FleetLedger,
        target: TargetKind::FleetRun,
        retry: Retryability::Idempotent,
        surfaces: ALL_SURFACES,
        backend: BackendCapability::Implemented,
        reconciles: false,
        hotbar_bare_dispatch: false,
        slash_command: "fleet",
        cli_invocation: "codewhale fleet resume <run-id>",
        summary: "Reconcile a durable Fleet run's orphaned leases after a manager restart.",
    },
];

/// Descriptors for one domain, in table order.
#[must_use]
pub fn operations_for_domain(domain: ControlDomain) -> Vec<&'static OperationDescriptor> {
    OPERATIONS
        .iter()
        .filter(|descriptor| descriptor.domain == domain)
        .collect()
}

// ---------------------------------------------------------------------------
// Target identity
// ---------------------------------------------------------------------------

/// Exact identity a write verb acts on.
///
/// `expected_lifecycle_seq` is the caller's fence: when present, the executor
/// must refuse to act if the durable record has moved on. That is what makes
/// interrupt/restart/resume act on *this* run rather than "whatever is there
/// now".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlTarget {
    pub kind: TargetKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_lifecycle_seq: Option<u64>,
}

impl ControlTarget {
    #[must_use]
    pub fn new(kind: TargetKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
            expected_lifecycle_seq: None,
        }
    }

    /// Whether `observed` satisfies this target's fence.
    #[must_use]
    pub fn matches_lifecycle(&self, observed: u64) -> bool {
        self.expected_lifecycle_seq
            .is_none_or(|expected| expected == observed)
    }
}

impl fmt::Display for ControlTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.expected_lifecycle_seq {
            Some(seq) => write!(f, "{}@{seq}", self.id),
            None => f.write_str(&self.id),
        }
    }
}

/// Maximum characters in a run identity accepted by any surface.
pub const MAX_TARGET_ID_CHARS: usize = 128;

fn is_valid_identity(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_TARGET_ID_CHARS
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        // `.` is allowed inside ids but a bare traversal segment is not.
        && id != "."
        && id != ".."
}

/// Parse the target for `descriptor` out of the raw argument tail.
///
/// Every surface calls this, so target selection cannot diverge: exact ids
/// only (no prefix or fuzzy matching), one token, optional `@<lifecycle-seq>`
/// fence, and a hard reject when a targetless verb is handed an argument.
pub fn parse_target(
    descriptor: &OperationDescriptor,
    raw: Option<&str>,
) -> Result<Option<ControlTarget>, ControlFailure> {
    let raw = raw.map(str::trim).filter(|value| !value.is_empty());
    if !descriptor.target.requires_identity() {
        return match raw {
            None => Ok(None),
            Some(extra) => Err(ControlFailure::invalid_target(format!(
                "{} takes no {}; got {:?}",
                descriptor.id,
                descriptor.target.label(),
                sanitize_line(extra)
            ))),
        };
    }
    let Some(raw) = raw else {
        return Err(ControlFailure::invalid_target(format!(
            "{} needs an exact {}: {}",
            descriptor.id,
            descriptor.target.label(),
            descriptor.cli_invocation
        )));
    };
    let mut tokens = raw.split_whitespace();
    let token = tokens.next().unwrap_or_default();
    if tokens.next().is_some() {
        return Err(ControlFailure::invalid_target(format!(
            "{} takes exactly one {}",
            descriptor.id,
            descriptor.target.label()
        )));
    }
    let (id, expected_lifecycle_seq) = match token.rsplit_once('@') {
        Some((id, seq)) => {
            let parsed = seq.parse::<u64>().map_err(|_| {
                ControlFailure::invalid_target(format!(
                    "lifecycle fence after '@' must be a number; got {:?}",
                    sanitize_line(seq)
                ))
            })?;
            (id, Some(parsed))
        }
        None => (token, None),
    };
    if !is_valid_identity(id) {
        return Err(ControlFailure::invalid_target(format!(
            "{:?} is not a valid {}",
            sanitize_line(id),
            descriptor.target.label()
        )));
    }
    Ok(Some(ControlTarget {
        kind: descriptor.target,
        id: id.to_string(),
        expected_lifecycle_seq,
    }))
}

// ---------------------------------------------------------------------------
// Failure
// ---------------------------------------------------------------------------

/// Typed failure class shared by every surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlFailureKind {
    /// The verb is not available here; see the availability reason.
    Unavailable,
    /// The argument was not an exact identity of the required kind.
    InvalidTarget,
    /// No durable record with that exact identity.
    NotFound,
    /// The record moved on (lifecycle fence or terminal state).
    Conflict,
    /// The backend refused or errored.
    Backend,
    /// The off-loop worker queue is full. The verb was not started; retrying
    /// after the queue drains is safe.
    Saturated,
}

impl ControlFailureKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::InvalidTarget => "invalid_target",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Backend => "backend",
            Self::Saturated => "saturated",
        }
    }

    #[must_use]
    const fn default_retryable(self) -> bool {
        matches!(self, Self::Backend | Self::Saturated)
    }
}

/// A bounded, sanitized failure. Never carries a raw path or secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlFailure {
    pub kind: ControlFailureKind,
    pub message: String,
    pub retryable: bool,
}

impl ControlFailure {
    #[must_use]
    pub fn new(kind: ControlFailureKind, message: impl AsRef<str>) -> Self {
        Self {
            kind,
            message: sanitize_line(message.as_ref()),
            retryable: kind.default_retryable(),
        }
    }

    #[must_use]
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    #[must_use]
    pub fn invalid_target(message: impl AsRef<str>) -> Self {
        Self::new(ControlFailureKind::InvalidTarget, message)
    }

    #[must_use]
    pub fn not_found(message: impl AsRef<str>) -> Self {
        Self::new(ControlFailureKind::NotFound, message)
    }

    #[must_use]
    pub fn conflict(message: impl AsRef<str>) -> Self {
        Self::new(ControlFailureKind::Conflict, message)
    }

    #[must_use]
    pub fn backend(message: impl AsRef<str>) -> Self {
        Self::new(ControlFailureKind::Backend, message)
    }

    #[must_use]
    pub fn unavailable(availability: &Availability) -> Self {
        Self::new(
            ControlFailureKind::Unavailable,
            availability.hint().unwrap_or("unavailable"),
        )
    }
}

impl fmt::Display for ControlFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

// ---------------------------------------------------------------------------
// Outcome + receipt
// ---------------------------------------------------------------------------

/// What the verb actually did to durable lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOutcome {
    /// Read-only: durable state was observed, not changed.
    Inspected,
    /// Accepted and handed to an off-loop worker. Nothing has happened to
    /// durable state *yet*; the receipt carries a ticket and the terminal
    /// outcome arrives later. This is never reported as success.
    Queued,
    /// A durable lifecycle transition happened.
    Transitioned,
    /// Already in the requested state; nothing changed.
    NoChange,
    /// Refused before touching durable state.
    Rejected,
    /// Attempted and failed.
    Failed,
}

impl LifecycleOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inspected => "inspected",
            Self::Queued => "queued",
            Self::Transitioned => "transitioned",
            Self::NoChange => "no_change",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Rejected | Self::Failed)
    }
}

/// The single result every surface returns and renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlReceipt {
    pub operation: ControlOperation,
    pub operation_id: String,
    pub surface: ControlSurface,
    pub authority: ControlAuthority,
    pub persistence: PersistenceScope,
    pub availability: Availability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ControlTarget>,
    pub outcome: LifecycleOutcome,
    /// Durable lifecycle sequence actually observed, when the store records one.
    pub observed_lifecycle_seq: Known<u64>,
    /// Whether serving this verb *changed* durable state as a side effect of
    /// reconciliation. A `Read` verb may fold a finished Runtime exit into the
    /// record; when it does, it says so here instead of reporting a pure
    /// observation (#4022).
    #[serde(default)]
    pub reconciled: bool,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ControlFailure>,
    /// Bounded, sanitized human detail lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<String>,
    /// Identifies an off-loop submission, so the caller can correlate this
    /// receipt with the terminal one that arrives later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    /// Bounded run payload, when the verb produced one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs: Option<RunListPage>,
    /// The raw durable Lane records this verb observed.
    ///
    /// Deliberately **not** serialized: it exists so `codewhale lane
    /// list|status --json` can keep emitting the exact `LaneRecord` shape it
    /// has always emitted, without a second read of the registry and without
    /// leaking a Lane-shaped payload into the cross-domain receipt wire
    /// format. Empty for Fleet verbs.
    #[serde(skip)]
    pub lane_records: Vec<LaneRecord>,
}

impl ControlReceipt {
    fn base(
        descriptor: &OperationDescriptor,
        surface: ControlSurface,
        availability: Availability,
        target: Option<ControlTarget>,
        outcome: LifecycleOutcome,
    ) -> Self {
        Self {
            operation: descriptor.operation,
            operation_id: descriptor.id.to_string(),
            surface,
            authority: descriptor.authority,
            persistence: descriptor.persistence,
            availability,
            target,
            outcome,
            observed_lifecycle_seq: Known::unknown(),
            reconciled: false,
            retryable: matches!(descriptor.retry, Retryability::Idempotent),
            failure: None,
            detail: Vec::new(),
            ticket: None,
            runs: None,
            lane_records: Vec::new(),
        }
    }

    /// Accepted for off-loop execution. Durable state is untouched so far, so
    /// this is explicitly not a success: `outcome` is `queued` and the terminal
    /// receipt arrives under the same `ticket`.
    #[must_use]
    pub fn queued(
        descriptor: &OperationDescriptor,
        surface: ControlSurface,
        target: Option<ControlTarget>,
        ticket: impl Into<String>,
    ) -> Self {
        let mut receipt = Self::base(
            descriptor,
            surface,
            Availability::Available,
            target,
            LifecycleOutcome::Queued,
        );
        receipt.ticket = Some(ticket.into());
        receipt
    }

    /// Correlate a terminal receipt with the submission that produced it.
    #[must_use]
    pub fn with_ticket(mut self, ticket: impl Into<String>) -> Self {
        self.ticket = Some(ticket.into());
        self
    }

    /// Carry the raw durable Lane records alongside the projected page, for
    /// the CLI's legacy `--json` shape.
    #[must_use]
    pub fn with_lane_records(mut self, records: Vec<LaneRecord>) -> Self {
        self.lane_records = records;
        self
    }

    /// A successful read.
    #[must_use]
    pub fn inspected(
        descriptor: &OperationDescriptor,
        surface: ControlSurface,
        target: Option<ControlTarget>,
    ) -> Self {
        Self::base(
            descriptor,
            surface,
            Availability::Available,
            target,
            LifecycleOutcome::Inspected,
        )
    }

    /// A durable transition.
    #[must_use]
    pub fn transitioned(
        descriptor: &OperationDescriptor,
        surface: ControlSurface,
        target: Option<ControlTarget>,
    ) -> Self {
        Self::base(
            descriptor,
            surface,
            Availability::Available,
            target,
            LifecycleOutcome::Transitioned,
        )
    }

    /// A no-op because durable state was already there.
    #[must_use]
    pub fn no_change(
        descriptor: &OperationDescriptor,
        surface: ControlSurface,
        target: Option<ControlTarget>,
    ) -> Self {
        Self::base(
            descriptor,
            surface,
            Availability::Available,
            target,
            LifecycleOutcome::NoChange,
        )
    }

    /// Refused before touching durable state.
    #[must_use]
    pub fn rejected(
        descriptor: &OperationDescriptor,
        surface: ControlSurface,
        target: Option<ControlTarget>,
        failure: ControlFailure,
    ) -> Self {
        let mut receipt = Self::base(
            descriptor,
            surface,
            Availability::Available,
            target,
            LifecycleOutcome::Rejected,
        );
        receipt.retryable = failure.retryable;
        receipt.failure = Some(failure);
        receipt
    }

    /// Refused because the verb is not available on this surface/context.
    #[must_use]
    pub fn unavailable(
        descriptor: &OperationDescriptor,
        surface: ControlSurface,
        availability: Availability,
    ) -> Self {
        let failure = ControlFailure::unavailable(&availability);
        let mut receipt = Self::base(
            descriptor,
            surface,
            availability,
            None,
            LifecycleOutcome::Rejected,
        );
        receipt.retryable = false;
        receipt.failure = Some(failure);
        receipt
    }

    /// Attempted and failed inside the backend.
    #[must_use]
    pub fn failed(
        descriptor: &OperationDescriptor,
        surface: ControlSurface,
        target: Option<ControlTarget>,
        failure: ControlFailure,
    ) -> Self {
        let mut receipt = Self::base(
            descriptor,
            surface,
            Availability::Available,
            target,
            LifecycleOutcome::Failed,
        );
        receipt.retryable = failure.retryable;
        receipt.failure = Some(failure);
        receipt
    }

    /// Record that reconciliation changed durable state while serving this
    /// verb.
    #[must_use]
    pub fn with_reconciled(mut self, reconciled: bool) -> Self {
        self.reconciled = reconciled;
        self
    }

    #[must_use]
    pub fn with_lifecycle_seq(mut self, seq: u64) -> Self {
        self.observed_lifecycle_seq = Known::Known(seq);
        self
    }

    #[must_use]
    pub fn with_runs(mut self, runs: RunListPage) -> Self {
        self.runs = Some(runs);
        self
    }

    /// Append bounded, sanitized detail lines.
    #[must_use]
    pub fn with_detail<I, S>(mut self, lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for line in lines {
            if self.detail.len() >= MAX_DETAIL_LINES {
                self.detail
                    .push(format!("[detail truncated at {MAX_DETAIL_LINES} lines]"));
                break;
            }
            self.detail.push(sanitize_line(line.as_ref()));
        }
        self
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.outcome.is_failure()
    }

    /// One renderer for every surface.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{} [{} · {} · {}]",
            self.operation_id,
            self.surface.as_str(),
            self.authority.as_str(),
            self.persistence.as_str()
        ));
        if let Some(target) = &self.target {
            out.push_str(&format!("\ntarget:  {} {target}", target.kind.as_str()));
        }
        out.push_str(&format!("\noutcome: {}", self.outcome.as_str()));
        if let Known::Known(seq) = self.observed_lifecycle_seq {
            out.push_str(&format!(" (lifecycle_seq={seq})"));
        }
        if self.reconciled {
            out.push_str("\nreconciled: durable state was updated from Runtime while reading");
        }
        if let Some(ticket) = &self.ticket {
            out.push_str(&format!("\nticket:  {ticket}"));
            if self.outcome == LifecycleOutcome::Queued {
                out.push_str(&format!("\n{LANE_INTERRUPT_OFF_LOOP}"));
            }
        }
        if let Availability::Unavailable { reason, hint } = &self.availability {
            out.push_str(&format!("\nunavailable: {} — {hint}", reason.as_str()));
        }
        if let Some(failure) = &self.failure {
            out.push_str(&format!(
                "\nfailure: {} — {} (retryable={})",
                failure.kind.as_str(),
                failure.message,
                failure.retryable
            ));
        }
        for line in &self.detail {
            out.push('\n');
            out.push_str(line);
        }
        if let Some(runs) = &self.runs {
            out.push('\n');
            if self.operation.descriptor().verb == "list" {
                out.push_str(&render_run_table(runs));
            } else {
                for run in &runs.runs {
                    out.push_str(&run.render_detail());
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Typed unknown
// ---------------------------------------------------------------------------

/// Why a value is not present. Absence is always explained, never implied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    /// The durable store does not record this value.
    NotRecorded,
    /// The value cannot apply to this record shape.
    NotApplicable,
    /// Present but withheld from this payload.
    Redacted,
}

impl UnknownReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRecorded => "not_recorded",
            Self::NotApplicable => "not_applicable",
            Self::Redacted => "redacted",
        }
    }
}

/// A value that is either exactly known or explicitly, typed-unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Known<T> {
    Known(T),
    Unknown(UnknownReason),
}

impl<T> Known<T> {
    #[must_use]
    pub fn unknown() -> Self {
        Self::Unknown(UnknownReason::NotRecorded)
    }

    #[must_use]
    pub fn not_applicable() -> Self {
        Self::Unknown(UnknownReason::NotApplicable)
    }

    #[must_use]
    pub fn redacted() -> Self {
        Self::Unknown(UnknownReason::Redacted)
    }

    #[must_use]
    pub fn from_option(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Known(value),
            None => Self::unknown(),
        }
    }

    #[must_use]
    pub fn is_known(&self) -> bool {
        matches!(self, Self::Known(_))
    }

    #[must_use]
    pub fn as_known(&self) -> Option<&T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown(_) => None,
        }
    }

    #[must_use]
    pub fn unknown_reason(&self) -> Option<UnknownReason> {
        match self {
            Self::Known(_) => None,
            Self::Unknown(reason) => Some(*reason),
        }
    }
}

impl<T: fmt::Display> Known<T> {
    /// Render for humans. Unknown renders as its typed reason, never as a
    /// blank or a plausible-looking default.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Known(value) => value.to_string(),
            Self::Unknown(reason) => format!("<{}>", reason.as_str()),
        }
    }
}

fn known_string(value: Option<&str>) -> Known<String> {
    Known::from_option(value.map(str::to_string))
}

// ---------------------------------------------------------------------------
// Run DTOs
// ---------------------------------------------------------------------------

/// Exact route identity for a run, with typed unknowns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRouteDto {
    pub provider_id: Known<String>,
    /// The exact configured provider-table id, when one was used.
    pub provider_exact_id: Known<String>,
    pub model: Known<String>,
    /// Reasoning tier the caller asked for.
    pub requested_reasoning: Known<String>,
    /// Reasoning tier actually placed on the request.
    pub effective_reasoning: Known<String>,
    /// How the route was produced (`resolver`, `profile`, …).
    pub route_source: Known<String>,
}

impl RunRouteDto {
    /// Every field typed-unknown for the same reason.
    #[must_use]
    pub fn all_unknown(reason: UnknownReason) -> Self {
        Self {
            provider_id: Known::Unknown(reason),
            provider_exact_id: Known::Unknown(reason),
            model: Known::Unknown(reason),
            requested_reasoning: Known::Unknown(reason),
            effective_reasoning: Known::Unknown(reason),
            route_source: Known::Unknown(reason),
        }
    }

    /// Whether the requested tier survived into the effective tier.
    ///
    /// `None` when either side is unknown — a downgrade must never be inferred
    /// from missing data.
    #[must_use]
    pub fn reasoning_downgraded(&self) -> Option<bool> {
        match (&self.requested_reasoning, &self.effective_reasoning) {
            (Known::Known(requested), Known::Known(effective)) => Some(requested != effective),
            _ => None,
        }
    }

    #[must_use]
    pub fn render_line(&self) -> String {
        let arrow = match self.reasoning_downgraded() {
            Some(true) => format!(
                "{} -> {}",
                self.requested_reasoning.render(),
                self.effective_reasoning.render()
            ),
            Some(false) => self.effective_reasoning.render(),
            None => format!(
                "{} -> {}",
                self.requested_reasoning.render(),
                self.effective_reasoning.render()
            ),
        };
        format!(
            "provider={} exact={} model={} reasoning={} route_source={}",
            self.provider_id.render(),
            self.provider_exact_id.render(),
            self.model.render(),
            arrow,
            self.route_source.render()
        )
    }
}

/// Exact usage for a run, with typed unknowns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunUsageDto {
    pub input_tokens: Known<u64>,
    pub output_tokens: Known<u64>,
    pub total_tokens: Known<u64>,
    pub duration_secs: Known<u64>,
}

impl RunUsageDto {
    #[must_use]
    pub fn all_unknown(reason: UnknownReason) -> Self {
        Self {
            input_tokens: Known::Unknown(reason),
            output_tokens: Known::Unknown(reason),
            total_tokens: Known::Unknown(reason),
            duration_secs: Known::Unknown(reason),
        }
    }

    #[must_use]
    pub fn render_line(&self) -> String {
        format!(
            "in={} out={} total={} duration_s={}",
            self.input_tokens.render(),
            self.output_tokens.render(),
            self.total_tokens.render(),
            self.duration_secs.render()
        )
    }
}

/// One durable run, shared by CLI and TUI for list and status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummaryDto {
    pub domain: ControlDomain,
    /// Exact identity. Never a prefix.
    pub run_id: String,
    pub status: String,
    /// Durable lifecycle sequence, when the store records one.
    pub lifecycle_seq: Known<u64>,
    /// Runtime = where/how.
    pub runtime: Known<String>,
    /// Workflow = order.
    pub workflow: Known<String>,
    /// Fleet = who. The field name stays `fleet` for serialized compatibility.
    pub fleet: Known<String>,
    pub issue: Known<String>,
    pub goal: Known<String>,
    pub started_at: Known<String>,
    pub stopped_at: Known<String>,
    /// Redacted worktree location, when there is one.
    pub location: Known<String>,
    /// Git branch backing the run's worktree.
    #[serde(default = "Known::unknown")]
    pub branch: Known<String>,
    /// Runtime session handle (tmux session name), when the Runtime has one.
    #[serde(default = "Known::unknown")]
    pub runtime_session: Known<String>,
    /// Redacted Runtime socket path, when the Runtime has one.
    #[serde(default = "Known::unknown")]
    pub runtime_socket: Known<String>,
    /// Exact command that re-attaches to a running Lane.
    #[serde(default = "Known::unknown")]
    pub attach: Known<String>,
    /// Redacted stream-json log path.
    #[serde(default = "Known::unknown")]
    pub log: Known<String>,
    pub route: RunRouteDto,
    pub usage: RunUsageDto,
}

impl RunSummaryDto {
    /// Full stable receipt-detail rendering, shared by status surfaces.
    ///
    /// Public commands call the Fleet domain a Fleet, but these field labels are
    /// part of the serialized receipt/detail compatibility boundary. Keep the
    /// durable domain and `fleet` field spellings here.
    #[must_use]
    pub fn render_detail(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("{}:      {}\n", self.domain.as_str(), self.run_id));
        out.push_str(&format!("status:    {}\n", self.status));
        out.push_str(&format!("lifecycle: {}\n", self.lifecycle_seq.render()));
        out.push_str(&format!("runtime:   {}\n", self.runtime.render()));
        out.push_str(&format!("workflow:  {}\n", self.workflow.render()));
        out.push_str(&format!("fleet:     {}\n", self.fleet.render()));
        out.push_str(&format!("issue:     {}\n", self.issue.render()));
        out.push_str(&format!("goal:      {}\n", self.goal.render()));
        out.push_str(&format!("started:   {}\n", self.started_at.render()));
        out.push_str(&format!("stopped:   {}\n", self.stopped_at.render()));
        out.push_str(&format!("location:  {}\n", self.location.render()));
        out.push_str(&format!("branch:    {}\n", self.branch.render()));
        out.push_str(&format!("session:   {}\n", self.runtime_session.render()));
        out.push_str(&format!("socket:    {}\n", self.runtime_socket.render()));
        out.push_str(&format!("attach:    {}\n", self.attach.render()));
        out.push_str(&format!("log:       {}\n", self.log.render()));
        out.push_str(&format!("route:     {}\n", self.route.render_line()));
        out.push_str(&format!("usage:     {}", self.usage.render_line()));
        out
    }
}

/// A bounded page of runs. List payloads are never unbounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunListPage {
    pub runs: Vec<RunSummaryDto>,
    /// How many durable runs matched before bounding.
    pub total: usize,
    /// How many were dropped to respect `limit`.
    pub truncated: usize,
    pub limit: usize,
}

impl RunListPage {
    /// Bound `runs` to `limit` (itself clamped to [`MAX_RUN_LIST_LIMIT`]).
    #[must_use]
    pub fn bounded(runs: Vec<RunSummaryDto>, limit: usize) -> Self {
        let limit = limit.clamp(1, MAX_RUN_LIST_LIMIT);
        let total = runs.len();
        let mut runs = runs;
        runs.truncate(limit);
        Self {
            truncated: total.saturating_sub(runs.len()),
            runs,
            total,
            limit,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// One table renderer for `lane list`, `/lane list`, and the hotbar dispatch
/// of the same command.
/// Fit one cell to `width`, truncating with an ellipsis rather than pushing
/// every later column out of alignment. Ids are bounded at
/// [`MAX_TARGET_ID_CHARS`], which is far wider than any column here.
fn cell(value: &str, width: usize) -> String {
    let fitted = truncate_chars(value, width);
    let pad = width.saturating_sub(fitted.chars().count());
    format!("{fitted}{}", " ".repeat(pad))
}

#[must_use]
pub fn render_run_table(page: &RunListPage) -> String {
    if page.runs.is_empty() {
        return "no durable runs".to_string();
    }
    let mut out = format!(
        "{} {} {} {} {} {}",
        cell("ID", 18),
        cell("STATUS", 10),
        cell("RUNTIME", 9),
        cell("WORKFLOW", 16),
        cell("FLEET", 14),
        "STARTED"
    );
    for run in &page.runs {
        out.push_str(&format!(
            "\n{} {} {} {} {} {}",
            cell(&run.run_id, 18),
            cell(&run.status, 10),
            cell(&run.runtime.render(), 9),
            cell(&run.workflow.render(), 16),
            cell(&run.fleet.render(), 14),
            truncate_chars(&run.started_at.render(), 32)
        ));
    }
    if page.truncated > 0 {
        out.push_str(&format!(
            "\n[{} of {} shown; {} omitted by the {}-row bound]",
            page.runs.len(),
            page.total,
            page.truncated,
            page.limit
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Lane adapter
// ---------------------------------------------------------------------------

/// Project a durable Lane record into the shared run DTO.
///
/// Route and usage are typed-unknown here because the Lane registry genuinely
/// does not record them. Fleet receipts do, and the Fleet adapter fills them.
#[must_use]
pub fn lane_run_summary(record: &LaneRecord) -> RunSummaryDto {
    RunSummaryDto {
        domain: ControlDomain::Lane,
        run_id: record.id.clone(),
        status: record.status.as_str().to_string(),
        lifecycle_seq: Known::Known(record.lifecycle_seq),
        runtime: Known::Known(record.runtime.as_str().to_string()),
        workflow: known_string(record.workflow.as_deref()),
        fleet: known_string(record.fleet.as_deref()),
        issue: known_string(record.issue.as_deref()),
        goal: known_string(record.goal.as_deref()),
        started_at: Known::Known(record.started_at.clone()),
        stopped_at: known_string(record.stopped_at.as_deref()),
        location: Known::from_option(record.worktree_path.as_deref().map(redact_path)),
        branch: known_string(record.branch.as_deref()),
        runtime_session: known_string(record.tmux_session.as_deref()),
        runtime_socket: Known::from_option(record.tmux_socket.as_deref().map(redact_path)),
        attach: known_string(record.attach_target.as_deref()),
        log: Known::Known(redact_path(&record.log_path)),
        route: RunRouteDto::all_unknown(UnknownReason::NotRecorded),
        usage: RunUsageDto::all_unknown(UnknownReason::NotRecorded),
    }
}

/// Bounded page of Lane summaries, newest-first order preserved.
#[must_use]
pub fn lane_run_page(records: &[LaneRecord], limit: usize) -> RunListPage {
    RunListPage::bounded(records.iter().map(lane_run_summary).collect(), limit)
}

/// Whether a Lane is still interruptible.
#[must_use]
pub fn lane_is_interruptible(status: LaneStatus) -> bool {
    status.is_active()
}

// ---------------------------------------------------------------------------
// Lane executor — the one code path behind every surface
// ---------------------------------------------------------------------------

/// Run a Lane control verb against the durable registry.
///
/// `codewhale lane …`, `/lane …`, and the hotbar dispatch of `/lane` all call
/// exactly this function. There is no second implementation to drift: the
/// availability check, the target parser, the lifecycle fence, the outcome,
/// and the sanitized failure are decided here once.
#[must_use]
pub fn execute_lane_control(
    surface: ControlSurface,
    operation: ControlOperation,
    raw_target: Option<&str>,
) -> ControlReceipt {
    execute_lane_control_in(surface, operation, raw_target, None)
}

/// [`execute_lane_control`] against an explicit registry root (tests, and any
/// caller that already resolved `$CODEWHALE_HOME/lanes`).
#[must_use]
pub fn execute_lane_control_in(
    surface: ControlSurface,
    operation: ControlOperation,
    raw_target: Option<&str>,
    registry_root: Option<&Path>,
) -> ControlReceipt {
    let descriptor = operation.descriptor();
    if descriptor.domain != ControlDomain::Lane {
        return ControlReceipt::rejected(
            descriptor,
            surface,
            None,
            ControlFailure::new(
                ControlFailureKind::InvalidTarget,
                format!("{} is not a Lane verb", descriptor.id),
            ),
        );
    }

    let root = match registry_root
        .map(|root| Ok(root.to_path_buf()))
        .unwrap_or_else(crate::registry::lane_registry_root)
    {
        Ok(root) => root,
        Err(err) => {
            return ControlReceipt::failed(
                descriptor,
                surface,
                None,
                ControlFailure::backend(format!("{err:#}")),
            );
        }
    };

    // Probe before opening: a read verb must not create the registry it is
    // reporting on, or "no Lanes yet" becomes indistinguishable from "there
    // is a registry and it is empty".
    let availability = descriptor.availability(surface, ControlContext::probe(Some(&root), None));
    if !availability.is_available() {
        return ControlReceipt::unavailable(descriptor, surface, availability);
    }

    let target = match parse_target(descriptor, raw_target) {
        Ok(target) => target,
        Err(failure) => return ControlReceipt::rejected(descriptor, surface, None, failure),
    };

    let registry = match crate::registry::LaneRegistry::open(&root) {
        Ok(registry) => registry,
        Err(err) => {
            return ControlReceipt::failed(
                descriptor,
                surface,
                target,
                ControlFailure::backend(format!("{err:#}")),
            );
        }
    };

    let execution = ControlExecution::for_surface(surface);
    match operation {
        ControlOperation::LaneList => lane_list(descriptor, surface, execution, &registry),
        ControlOperation::LaneStatus | ControlOperation::LaneInterrupt => {
            let Some(target) = target else {
                return ControlReceipt::rejected(
                    descriptor,
                    surface,
                    None,
                    ControlFailure::invalid_target(format!(
                        "{} needs an exact {}",
                        descriptor.id,
                        descriptor.target.label()
                    )),
                );
            };
            lane_one(descriptor, surface, execution, &registry, target)
        }
        // Unreachable in practice: both are `NotImplemented`, so the
        // availability gate above already rejected them on every surface.
        // Kept explicit so adding a backend cannot silently fall through.
        _ => ControlReceipt::unavailable(
            descriptor,
            surface,
            descriptor.availability(surface, ControlContext::new(true, false)),
        ),
    }
}

fn lane_list(
    descriptor: &'static OperationDescriptor,
    surface: ControlSurface,
    execution: ControlExecution,
    registry: &crate::registry::LaneRegistry,
) -> ControlReceipt {
    let mut records = match registry.list() {
        Ok(records) => records,
        Err(err) => {
            return ControlReceipt::failed(
                descriptor,
                surface,
                None,
                ControlFailure::backend(format!("{err:#}")),
            );
        }
    };
    let mut warnings = Vec::new();
    let mut reconciled = false;
    if execution.reconciles() {
        for record in &mut records {
            match crate::runtime::backend_for(record).reconcile(registry, record) {
                Ok(changed) => {
                    if changed {
                        reconciled = true;
                        warnings.push(format!(
                            "reconciled {}: durable status is now {}",
                            record.id,
                            record.status.as_str()
                        ));
                    }
                }
                Err(err) => warnings.push(format!("could not reconcile {}: {err:#}", record.id)),
            }
        }
    } else {
        warnings.push(
            "runtime reconciliation skipped on this surface; statuses are as last recorded. \
             Run `codewhale lane list` for a reconciled view."
                .to_string(),
        );
    }
    ControlReceipt::inspected(descriptor, surface, None)
        .with_reconciled(reconciled)
        .with_runs(lane_run_page(&records, DEFAULT_RUN_LIST_LIMIT))
        .with_lane_records(records)
        .with_detail(warnings)
}

fn lane_one(
    descriptor: &'static OperationDescriptor,
    surface: ControlSurface,
    execution: ControlExecution,
    registry: &crate::registry::LaneRegistry,
    target: ControlTarget,
) -> ControlReceipt {
    let mut record = match registry.load(&target.id) {
        Ok(record) => record,
        Err(err)
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|source| source.kind() == std::io::ErrorKind::NotFound) =>
        {
            return ControlReceipt::rejected(
                descriptor,
                surface,
                Some(target.clone()),
                ControlFailure::not_found(format!("no Lane with id {}", target.id)),
            );
        }
        Err(err) => {
            return ControlReceipt::failed(
                descriptor,
                surface,
                Some(target),
                ControlFailure::backend(format!("{err:#}")),
            );
        }
    };

    let mut detail = Vec::new();
    let mut reconciled = false;
    let backend = crate::runtime::backend_for(&record);
    if execution.reconciles() {
        match backend.reconcile(registry, &mut record) {
            Ok(changed) => {
                if changed {
                    reconciled = true;
                    detail.push(format!(
                        "reconciled {}: durable status is now {}",
                        record.id,
                        record.status.as_str()
                    ));
                }
            }
            Err(err) => detail.push(format!("could not reconcile {}: {err:#}", record.id)),
        }
    } else {
        detail.push(
            "runtime reconciliation skipped on this surface; status is as last recorded. \
             Run `codewhale lane status <lane-id>` for a reconciled view."
                .to_string(),
        );
    }

    // Read verbs check the fence against what they just observed: there is no
    // mutation to protect, so a mismatch is simply "that generation is gone".
    // Write verbs deliberately do *not* check it here — see below.
    if descriptor.authority == ControlAuthority::Read {
        if !target.matches_lifecycle(record.lifecycle_seq) {
            return ControlReceipt::rejected(
                descriptor,
                surface,
                Some(target.clone()),
                ControlFailure::conflict(format!(
                    "Lane {} is at lifecycle_seq {}, not the requested {}",
                    record.id,
                    record.lifecycle_seq,
                    target
                        .expected_lifecycle_seq
                        .map(|seq| seq.to_string())
                        .unwrap_or_else(|| "-".to_string())
                )),
            )
            .with_reconciled(reconciled)
            .with_lifecycle_seq(record.lifecycle_seq);
        }
        return ControlReceipt::inspected(descriptor, surface, Some(target))
            .with_reconciled(reconciled)
            .with_lifecycle_seq(record.lifecycle_seq)
            .with_runs(lane_run_page(std::slice::from_ref(&record), 1))
            .with_lane_records(vec![record])
            .with_detail(detail);
    }

    // The fence is *not* evaluated here. Checking it against this read and then
    // stopping would be a TOCTOU: another process can transition the record in
    // between, and we would tear down a generation the caller never observed.
    // It travels into the registry instead and is checked under the same
    // per-Lane lock that performs the mutation.
    let fence = target.expected_lifecycle_seq;
    let stopped = backend.stop(registry, &mut record, fence);
    match stopped {
        Ok(TerminalTransition::Transitioned) => {
            ControlReceipt::transitioned(descriptor, surface, Some(target))
                .with_reconciled(reconciled)
                .with_lifecycle_seq(record.lifecycle_seq)
                .with_runs(lane_run_page(std::slice::from_ref(&record), 1))
                .with_detail(detail)
        }
        // Already terminal — ours or another process's doing. Either way this
        // call changed nothing, and saying "transitioned" would credit us with
        // someone else's transition.
        Ok(TerminalTransition::AlreadyTerminal) => {
            ControlReceipt::no_change(descriptor, surface, Some(target))
                .with_reconciled(reconciled)
                .with_lifecycle_seq(record.lifecycle_seq)
                .with_runs(lane_run_page(std::slice::from_ref(&record), 1))
                .with_detail(
                    detail
                        .into_iter()
                        .chain([format!("Lane is already {}", record.status.as_str())]),
                )
        }
        Ok(TerminalTransition::FenceMismatch { observed }) => ControlReceipt::rejected(
            descriptor,
            surface,
            Some(target.clone()),
            ControlFailure::conflict(format!(
                "Lane {} is at lifecycle_seq {observed}, not the requested {}; nothing was stopped",
                record.id,
                target
                    .expected_lifecycle_seq
                    .map(|seq| seq.to_string())
                    .unwrap_or_else(|| "-".to_string())
            )),
        )
        .with_reconciled(reconciled)
        .with_lifecycle_seq(observed)
        .with_detail(detail),
        Err(err) => ControlReceipt::failed(
            descriptor,
            surface,
            Some(target),
            ControlFailure::backend(format!("{err:#}")),
        )
        .with_reconciled(reconciled)
        .with_lifecycle_seq(record.lifecycle_seq)
        .with_detail(detail),
    }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

const SECRET_KEY_HINTS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "key",
    "authorization",
    "credential",
    "cookie",
    "session_id",
    "webhook",
];

const SECRET_VALUE_PREFIXES: &[&str] = &[
    "sk-",
    "sk_",
    "ghp_",
    "gho_",
    "ghu_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "hf_",
    "pk_live_",
    "rk_live_",
    "AKIA",
    "Bearer",
    "bearer",
];

fn home_prefix() -> Option<&'static str> {
    static HOME: OnceLock<Option<String>> = OnceLock::new();
    HOME.get_or_init(|| {
        std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())
            .filter(|home| !home.is_empty() && home != "/")
    })
    .as_deref()
}

/// Replace an absolute path under `$HOME` with `~/…`.
#[must_use]
pub fn redact_path(path: &Path) -> String {
    redact_path_str(&path.to_string_lossy())
}

fn redact_path_str(value: &str) -> String {
    let Some(home) = home_prefix() else {
        return value.to_string();
    };
    let Some(rest) = value.strip_prefix(home) else {
        return value.to_string();
    };
    // Boundary check: `$HOME` is `/Users/ada`, so `/Users/ada-backup` is a
    // *different* directory and must not be abbreviated to `~-backup`. Only an
    // exact match or a real path separator after the prefix is `$HOME`.
    match rest.chars().next() {
        None => "~".to_string(),
        Some('/') | Some('\\') => {
            let rest = rest.trim_start_matches(['/', '\\']);
            if rest.is_empty() {
                "~".to_string()
            } else {
                format!("~/{rest}")
            }
        }
        Some(_) => value.to_string(),
    }
}

fn redact_token(token: &str) -> String {
    // `key=value` / `key:value` pairs whose key looks credential-bearing.
    for separator in ['=', ':'] {
        if let Some((key, value)) = token.split_once(separator)
            && !value.is_empty()
        {
            let lowered = key.to_ascii_lowercase();
            if SECRET_KEY_HINTS
                .iter()
                .any(|hint| lowered.ends_with(hint) || lowered == *hint)
            {
                return format!("{key}{separator}{REDACTED}");
            }
        }
    }
    // Case-insensitive: a provider that spells its key `SK-live-…` leaks
    // under an exact-case match (2026-08-04 audit).
    let lowered_token = token.to_ascii_lowercase();
    if SECRET_VALUE_PREFIXES.iter().any(|prefix| {
        let lowered_prefix = prefix.to_ascii_lowercase();
        lowered_token.starts_with(&lowered_prefix) && token.len() > prefix.len()
    }) {
        return REDACTED.to_string();
    }
    redact_path_str(token)
}

/// Authentication scheme words that carry their secret in the NEXT
/// whitespace-separated token.
///
/// `Authorization: Bearer <jwt>` used to leak the JWT in full: the bare
/// `Bearer` token failed the `len() > prefix.len()` guard (it IS the prefix),
/// and the JWT after it matches no prefix and no `key=value` hint. Every
/// operator-visible `ControlReceipt` string goes through this sanitizer, so
/// that was a live credential leak into transcripts, `--json` payloads, and
/// screenshots (2026-08-04 audit).
const SECRET_SCHEME_WORDS: &[&str] = &["bearer", "basic", "token", "apikey", "api_key"];

/// Whether this token is a bare auth scheme word, meaning the token after it
/// is the secret.
fn is_secret_scheme_word(token: &str) -> bool {
    let trimmed = token.trim_end_matches([':', ',', ';']);
    SECRET_SCHEME_WORDS
        .iter()
        .any(|word| trimmed.eq_ignore_ascii_case(word))
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Sanitize one line: redact secrets and home-rooted paths, collapse
/// whitespace, and bound the length.
///
/// Every operator-visible string on a [`ControlReceipt`] goes through this,
/// so a backend error carrying an absolute path or a bearer token cannot leak
/// into a transcript, a `--json` payload, or a shared screenshot.
/// Leading indentation is structure, not whitespace noise: nested worker and
/// artifact rows are only readable if their indent survives sanitization.
/// Bounded so a crafted line cannot pad a receipt out to the length cap.
const MAX_PRESERVED_INDENT: usize = 8;

#[must_use]
pub fn sanitize_line(input: &str) -> String {
    let indent = input
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .count()
        .min(MAX_PRESERVED_INDENT);
    let mut out = " ".repeat(indent);
    let mut first = true;
    // `Bearer <jwt>` splits into two tokens and the secret is the second one,
    // so a scheme word arms redaction of whatever follows it.
    let mut redact_next = false;
    for token in input.split_whitespace() {
        if first {
            first = false;
        } else {
            out.push(' ');
        }
        if std::mem::take(&mut redact_next) {
            out.push_str(REDACTED);
            continue;
        }
        redact_next = is_secret_scheme_word(token);
        out.push_str(&redact_token(token));
    }
    if first {
        // Whitespace-only input carries no content; do not emit bare indent.
        return String::new();
    }
    truncate_chars(&out, MAX_DETAIL_LINE_CHARS)
}

/// Sanitize an arbitrary multi-line blob into bounded, sanitized lines.
#[must_use]
pub fn sanitize_lines(input: &str) -> Vec<String> {
    let mut lines: Vec<String> = input
        .lines()
        .map(sanitize_line)
        .filter(|line| !line.is_empty())
        .take(MAX_DETAIL_LINES)
        .collect();
    if input.lines().filter(|line| !line.trim().is_empty()).count() > lines.len() {
        lines.push(format!("[detail truncated at {MAX_DETAIL_LINES} lines]"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeBackendKind;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn lane_record(id: &str) -> LaneRecord {
        LaneRecord {
            id: id.to_string(),
            workflow: Some("stopship".into()),
            fleet: Some("stopship".into()),
            issue: Some("4022".into()),
            goal: None,
            runtime: RuntimeBackendKind::Tmux,
            status: LaneStatus::Running,
            lifecycle_seq: 2,
            worktree_path: Some(PathBuf::from("/tmp/lanes/x")),
            branch: Some("lane/x".into()),
            tmux_session: Some("cw-x".into()),
            tmux_socket: None,
            log_path: PathBuf::from("/tmp/lanes/logs/x.ndjson"),
            started_at: "2026-07-26T00:00:00Z".into(),
            stopped_at: None,
            attach_target: None,
            worktree_ttl_secs: None,
        }
    }

    // -- descriptor table integrity ------------------------------------

    #[test]
    fn every_operation_has_exactly_one_descriptor_with_a_stable_id() {
        let mut ids = BTreeSet::new();
        for operation in ControlOperation::ALL {
            let descriptor = operation.descriptor();
            assert_eq!(descriptor.operation, *operation);
            assert_eq!(
                descriptor.id,
                format!("{}.{}", descriptor.domain.as_str(), descriptor.verb),
                "descriptor id must be <domain>.<verb>"
            );
            assert!(ids.insert(descriptor.id), "duplicate id {}", descriptor.id);
            assert_eq!(ControlOperation::from_id(descriptor.id), Some(*operation));
        }
        assert_eq!(ids.len(), OPERATIONS.len());
    }

    #[test]
    fn both_domains_declare_the_same_five_lifecycle_verbs() {
        let lane: BTreeSet<&str> = operations_for_domain(ControlDomain::Lane)
            .iter()
            .map(|descriptor| descriptor.verb)
            .collect();
        let fleet: BTreeSet<&str> = operations_for_domain(ControlDomain::Fleet)
            .iter()
            .map(|descriptor| descriptor.verb)
            .collect();
        let expected: BTreeSet<&str> = ["list", "status", "interrupt", "restart", "resume"]
            .into_iter()
            .collect();
        assert_eq!(lane, expected);
        assert_eq!(fleet, expected);
    }

    /// #1888: the hotbar is not a surface. It binds the owning slash command
    /// and fires it with no argument, so only the verb a bare invocation
    /// resolves to is actually reachable — and that verb cannot take a target.
    #[test]
    fn hotbar_reachability_is_declared_honestly() {
        for descriptor in OPERATIONS {
            assert_eq!(
                descriptor.hotbar_action_id(),
                format!("slash.{}", descriptor.slash_command)
            );
            if descriptor.hotbar_bare_dispatch {
                assert_eq!(
                    descriptor.target,
                    TargetKind::None,
                    "{} takes a target a bare hotbar press cannot supply",
                    descriptor.id
                );
                assert_eq!(
                    descriptor.authority,
                    ControlAuthority::Read,
                    "{} would mutate durable state from a single keypress",
                    descriptor.id
                );
                assert!(
                    descriptor.offers(ControlSurface::Slash),
                    "{} dispatches through the slash surface",
                    descriptor.id
                );
            }
        }
        // Exactly one verb is reachable from a bare press today: `/lane` with
        // no argument lists. `/fleet` with no argument opens the roster, so no
        // Fleet verb is bare-dispatchable.
        let reachable: Vec<&str> = OPERATIONS
            .iter()
            .filter(|descriptor| descriptor.hotbar_bare_dispatch)
            .map(|descriptor| descriptor.id)
            .collect();
        assert_eq!(reachable, vec!["lane.list"]);
    }

    #[test]
    fn both_surfaces_map_to_the_same_operation_ids() {
        // #1888: slash and CLI must not have separate verb tables.
        for descriptor in OPERATIONS {
            for surface in ControlSurface::ALL {
                assert!(
                    descriptor.offers(*surface),
                    "{} must be declared on {surface}",
                    descriptor.id
                );
            }
            assert_eq!(
                descriptor.hotbar_action_id(),
                format!("slash.{}", descriptor.slash_command),
                "hotbar binds the owning slash command; there is no second table"
            );
            assert!(
                descriptor.cli_invocation.starts_with("codewhale "),
                "{} needs an exact CLI invocation",
                descriptor.id
            );
            // The CLI and slash surfaces must name the same canonical noun.
            // `domain.as_str()` is the serialization key ("fleet"), which the
            // ledger, receipts, and config tables still use; the customer-
            // facing spelling is the slash command ("fleet").
            assert!(
                descriptor.cli_invocation.contains(descriptor.slash_command),
                "{} CLI invocation must name the same canonical noun as its slash command",
                descriptor.id
            );
            assert!(
                descriptor.slash_invocation().starts_with(&format!(
                    "/{} {}",
                    descriptor.slash_command, descriptor.verb
                )),
                "{} slash invocation must name the same verb",
                descriptor.id
            );
        }
    }

    #[test]
    fn verb_aliases_resolve_to_one_operation_per_domain() {
        for (alias, expected) in [
            ("list", ControlOperation::LaneList),
            ("ls", ControlOperation::LaneList),
            ("status", ControlOperation::LaneStatus),
            ("inspect", ControlOperation::LaneStatus),
            ("interrupt", ControlOperation::LaneInterrupt),
            ("stop", ControlOperation::LaneInterrupt),
            ("cancel", ControlOperation::LaneInterrupt),
            ("restart", ControlOperation::LaneRestart),
            ("resume", ControlOperation::LaneResume),
        ] {
            assert_eq!(
                ControlOperation::parse_verb(ControlDomain::Lane, alias),
                Some(expected),
                "lane alias {alias}"
            );
        }
        assert_eq!(
            ControlOperation::parse_verb(ControlDomain::Fleet, "STOP"),
            Some(ControlOperation::FleetInterrupt)
        );
        assert_eq!(
            ControlOperation::parse_verb(ControlDomain::Fleet, "nope"),
            None
        );
    }

    #[test]
    fn fleet_is_public_and_remains_the_durable_domain_key() {
        assert_eq!(ControlDomain::Fleet.public_name(), "fleet");
        assert_eq!(ControlDomain::Fleet.as_str(), "fleet");
        assert_eq!(
            serde_json::to_string(&ControlDomain::Fleet).expect("serialize durable domain"),
            "\"fleet\""
        );
    }

    #[test]
    fn authority_and_persistence_are_identical_across_surfaces() {
        for descriptor in OPERATIONS {
            let by_surface: Vec<_> = ControlSurface::ALL
                .iter()
                .map(|surface| {
                    let receipt = ControlReceipt::inspected(descriptor, *surface, None);
                    (receipt.authority, receipt.persistence, receipt.operation_id)
                })
                .collect();
            let first = by_surface[0].clone();
            for entry in &by_surface {
                assert_eq!(*entry, first, "{} drifted across surfaces", descriptor.id);
            }
        }
    }

    #[test]
    fn read_verbs_are_read_authority_and_write_verbs_are_write() {
        for descriptor in OPERATIONS {
            let expected = match descriptor.verb {
                "list" | "status" => ControlAuthority::Read,
                _ => ControlAuthority::Write,
            };
            assert_eq!(descriptor.authority, expected, "{}", descriptor.id);
            assert!(
                descriptor.persistence.is_durable(),
                "{} must name a durable store, not session state",
                descriptor.id
            );
        }
    }

    #[test]
    fn target_kinds_match_the_verbs_that_need_exact_identity() {
        for descriptor in OPERATIONS {
            let needs_identity = match (descriptor.domain, descriptor.verb) {
                // Both `list` verbs and `fleet status` report on the whole
                // durable store; every other verb acts on one exact run.
                (_, "list") => false,
                (ControlDomain::Fleet, "status") => false,
                _ => true,
            };
            if needs_identity {
                assert!(
                    descriptor.target.requires_identity(),
                    "{} acts on one run and must require an exact id",
                    descriptor.id
                );
            } else {
                assert_eq!(
                    descriptor.target,
                    TargetKind::None,
                    "{} reports on the whole store and must not take a target",
                    descriptor.id
                );
            }
        }
    }

    // -- availability ---------------------------------------------------

    #[test]
    fn no_surface_advertises_an_unimplemented_backend() {
        let ctx = ControlContext::new(true, true);
        for descriptor in OPERATIONS {
            if let BackendCapability::NotImplemented { .. } = descriptor.backend {
                for surface in ControlSurface::ALL {
                    let availability = descriptor.availability(*surface, ctx);
                    assert_eq!(
                        availability.reason(),
                        Some(UnavailableReason::BackendNotImplemented),
                        "{} must be unavailable on {surface}",
                        descriptor.id
                    );
                    assert!(
                        availability.hint().is_some_and(|hint| !hint.is_empty()),
                        "{} must explain why",
                        descriptor.id
                    );
                }
            }
        }
        // Both Lane write-restart verbs are the concrete case today.
        assert!(
            !ControlOperation::LaneRestart
                .descriptor()
                .availability(ControlSurface::Cli, ctx)
                .is_available()
        );
        assert!(
            !ControlOperation::LaneResume
                .descriptor()
                .availability(ControlSurface::Slash, ctx)
                .is_available()
        );
    }

    #[test]
    fn surface_limited_backends_are_available_only_where_they_exist() {
        let ctx = ControlContext::new(true, true);
        let descriptor = ControlOperation::FleetRestart.descriptor();
        assert!(
            descriptor
                .availability(ControlSurface::Cli, ctx)
                .is_available()
        );
        {
            let surface = ControlSurface::Slash;
            let availability = descriptor.availability(surface, ctx);
            assert_eq!(
                availability.reason(),
                Some(UnavailableReason::SurfaceNotSupported)
            );
            assert!(
                availability
                    .hint()
                    .is_some_and(|hint| hint.contains("codewhale fleet restart")),
                "an unavailable surface must point at the one that works"
            );
        }
    }

    #[test]
    fn missing_durable_stores_are_typed_unavailability_not_silence() {
        let empty = ControlContext::default();
        let lane = ControlOperation::LaneList.descriptor();
        let fleet = ControlOperation::FleetStatus.descriptor();
        for surface in ControlSurface::ALL {
            assert_eq!(
                lane.availability(*surface, empty).reason(),
                Some(UnavailableReason::NoLaneRegistry)
            );
            assert_eq!(
                fleet.availability(*surface, empty).reason(),
                Some(UnavailableReason::NoFleetLedger)
            );
        }
        let ready = ControlContext::new(true, true);
        assert!(
            lane.availability(ControlSurface::Slash, ready)
                .is_available()
        );
        assert!(
            fleet
                .availability(ControlSurface::Slash, ready)
                .is_available()
        );
    }

    #[test]
    fn availability_is_identical_on_every_surface_for_implemented_verbs() {
        let ctx = ControlContext::new(true, true);
        for descriptor in OPERATIONS {
            if !matches!(descriptor.backend, BackendCapability::Implemented) {
                continue;
            }
            let reasons: BTreeSet<_> = ControlSurface::ALL
                .iter()
                .map(|surface| descriptor.availability(*surface, ctx).reason())
                .collect();
            assert_eq!(
                reasons.len(),
                1,
                "{} drifted across surfaces",
                descriptor.id
            );
        }
    }

    // -- target selection ------------------------------------------------

    #[test]
    fn target_selection_is_exact_and_shared() {
        let status = ControlOperation::LaneStatus.descriptor();
        let target = parse_target(status, Some(" lane-a1b2c3d4 "))
            .expect("valid id")
            .expect("target present");
        assert_eq!(target.kind, TargetKind::LaneRun);
        assert_eq!(target.id, "lane-a1b2c3d4");
        assert_eq!(target.expected_lifecycle_seq, None);

        // Same parser, same result, whichever surface calls it.
        for raw in ["lane-a1b2c3d4", " lane-a1b2c3d4"] {
            assert_eq!(
                parse_target(status, Some(raw)).unwrap().unwrap().id,
                "lane-a1b2c3d4"
            );
        }
    }

    #[test]
    fn target_selection_rejects_prefixes_paths_and_extra_tokens() {
        let interrupt = ControlOperation::LaneInterrupt.descriptor();
        for bad in ["", "   "] {
            let failure = parse_target(interrupt, Some(bad)).unwrap_err();
            assert_eq!(failure.kind, ControlFailureKind::InvalidTarget);
        }
        for bad in [
            "lane-a1b2 lane-c3d4",
            "../../etc/passwd",
            "lane/a1b2",
            "lane a1b2",
        ] {
            let failure = parse_target(interrupt, Some(bad)).unwrap_err();
            assert_eq!(
                failure.kind,
                ControlFailureKind::InvalidTarget,
                "{bad} must be rejected"
            );
        }
        assert_eq!(
            parse_target(interrupt, None).unwrap_err().kind,
            ControlFailureKind::InvalidTarget
        );
    }

    #[test]
    fn targetless_verbs_reject_stray_arguments() {
        let list = ControlOperation::LaneList.descriptor();
        assert_eq!(parse_target(list, None).unwrap(), None);
        assert_eq!(parse_target(list, Some("  ")).unwrap(), None);
        assert_eq!(
            parse_target(list, Some("lane-a1b2")).unwrap_err().kind,
            ControlFailureKind::InvalidTarget
        );
    }

    #[test]
    fn lifecycle_fence_pins_exact_run_identity() {
        let interrupt = ControlOperation::LaneInterrupt.descriptor();
        let target = parse_target(interrupt, Some("lane-a1b2c3d4@7"))
            .unwrap()
            .unwrap();
        assert_eq!(target.id, "lane-a1b2c3d4");
        assert_eq!(target.expected_lifecycle_seq, Some(7));
        assert!(target.matches_lifecycle(7));
        assert!(!target.matches_lifecycle(8));
        assert_eq!(target.to_string(), "lane-a1b2c3d4@7");

        let unfenced = parse_target(interrupt, Some("lane-a1b2c3d4"))
            .unwrap()
            .unwrap();
        assert!(unfenced.matches_lifecycle(1));
        assert!(unfenced.matches_lifecycle(99));

        assert_eq!(
            parse_target(interrupt, Some("lane-a1b2c3d4@later"))
                .unwrap_err()
                .kind,
            ControlFailureKind::InvalidTarget
        );
    }

    // -- receipts --------------------------------------------------------

    #[test]
    fn receipts_carry_the_descriptor_contract_and_round_trip() {
        let descriptor = ControlOperation::LaneInterrupt.descriptor();
        let target = parse_target(descriptor, Some("lane-a1b2c3d4@3"))
            .unwrap()
            .unwrap();
        let receipt = ControlReceipt::transitioned(descriptor, ControlSurface::Slash, Some(target))
            .with_lifecycle_seq(4)
            .with_detail(["stopped tmux session"]);
        assert_eq!(receipt.operation_id, "lane.interrupt");
        assert_eq!(receipt.authority, ControlAuthority::Write);
        assert_eq!(receipt.persistence, PersistenceScope::LaneRegistry);
        assert_eq!(receipt.outcome, LifecycleOutcome::Transitioned);
        assert!(receipt.retryable, "interrupt is idempotent");
        assert!(!receipt.is_error());

        let json = serde_json::to_string(&receipt).unwrap();
        let back: ControlReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(back, receipt);
        let rendered = receipt.render();
        assert!(rendered.contains("lane.interrupt"));
        assert!(rendered.contains("lifecycle_seq=4"));
    }

    #[test]
    fn conflict_and_unavailable_receipts_are_not_retryable() {
        let descriptor = ControlOperation::LaneInterrupt.descriptor();
        let conflict = ControlReceipt::rejected(
            descriptor,
            ControlSurface::Cli,
            None,
            ControlFailure::conflict("lane moved to stopped"),
        );
        assert!(!conflict.retryable);
        assert!(conflict.is_error());

        let availability = ControlOperation::LaneRestart
            .descriptor()
            .availability(ControlSurface::Cli, ControlContext::new(true, true));
        let unavailable = ControlReceipt::unavailable(
            ControlOperation::LaneRestart.descriptor(),
            ControlSurface::Cli,
            availability,
        );
        assert!(!unavailable.retryable);
        assert_eq!(
            unavailable.availability.reason(),
            Some(UnavailableReason::BackendNotImplemented)
        );
        assert!(unavailable.render().contains("backend_not_implemented"));
    }

    #[test]
    fn backend_failures_are_retryable_and_sanitized() {
        let descriptor = ControlOperation::FleetInterrupt.descriptor();
        let receipt = ControlReceipt::failed(
            descriptor,
            ControlSurface::Cli,
            None,
            ControlFailure::backend("ledger append failed token=abcd1234"),
        );
        assert!(receipt.retryable);
        let message = &receipt.failure.as_ref().unwrap().message;
        assert!(message.contains(REDACTED), "{message}");
        assert!(!message.contains("abcd1234"));
    }

    #[test]
    fn receipt_detail_is_bounded() {
        let descriptor = ControlOperation::LaneList.descriptor();
        let receipt = ControlReceipt::inspected(descriptor, ControlSurface::Cli, None)
            .with_detail((0..MAX_DETAIL_LINES * 2).map(|index| format!("line {index}")));
        assert_eq!(receipt.detail.len(), MAX_DETAIL_LINES + 1);
        assert!(receipt.detail.last().unwrap().contains("truncated"));
    }

    // -- typed unknown ----------------------------------------------------

    #[test]
    fn unknown_values_render_their_typed_reason() {
        let known: Known<u64> = Known::Known(12);
        assert_eq!(known.render(), "12");
        assert!(known.is_known());
        let unknown: Known<u64> = Known::unknown();
        assert_eq!(unknown.render(), "<not_recorded>");
        assert_eq!(unknown.unknown_reason(), Some(UnknownReason::NotRecorded));
        let na: Known<String> = Known::not_applicable();
        assert_eq!(na.render(), "<not_applicable>");
        let json = serde_json::to_string(&na).unwrap();
        assert_eq!(json, r#"{"unknown":"not_applicable"}"#);
        let back: Known<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, na);
    }

    #[test]
    fn reasoning_downgrade_is_never_inferred_from_missing_data() {
        let mut route = RunRouteDto::all_unknown(UnknownReason::NotRecorded);
        assert_eq!(route.reasoning_downgraded(), None);
        route.requested_reasoning = Known::Known("high".into());
        assert_eq!(
            route.reasoning_downgraded(),
            None,
            "one side is still unknown"
        );
        route.effective_reasoning = Known::Known("high".into());
        assert_eq!(route.reasoning_downgraded(), Some(false));
        route.effective_reasoning = Known::Known("medium".into());
        assert_eq!(route.reasoning_downgraded(), Some(true));
        assert!(route.render_line().contains("high -> medium"));
    }

    // -- DTOs and bounding -------------------------------------------------

    #[test]
    fn lane_summary_keeps_exact_identity_and_types_its_unknowns() {
        let summary = lane_run_summary(&lane_record("lane-a1b2c3d4"));
        assert_eq!(summary.run_id, "lane-a1b2c3d4");
        assert_eq!(summary.domain, ControlDomain::Lane);
        assert_eq!(summary.lifecycle_seq, Known::Known(2));
        assert_eq!(summary.runtime, Known::Known("tmux".to_string()));
        assert_eq!(summary.workflow, Known::Known("stopship".to_string()));
        assert_eq!(summary.fleet, Known::Known("stopship".to_string()));
        // The Lane registry does not record route or usage; say so in types.
        assert_eq!(
            summary.route.provider_id.unknown_reason(),
            Some(UnknownReason::NotRecorded)
        );
        assert_eq!(
            summary.usage.total_tokens.unknown_reason(),
            Some(UnknownReason::NotRecorded)
        );
        assert_eq!(
            summary.goal.unknown_reason(),
            Some(UnknownReason::NotRecorded)
        );
        let detail = summary.render_detail();
        assert!(detail.contains("<not_recorded>"));
        assert!(detail.contains("lane-a1b2c3d4"));
    }

    #[test]
    fn run_list_pages_are_bounded_and_report_what_they_dropped() {
        let records: Vec<LaneRecord> = (0..10)
            .map(|index| lane_record(&format!("lane-{index:08}")))
            .collect();
        let page = lane_run_page(&records, 4);
        assert_eq!(page.runs.len(), 4);
        assert_eq!(page.total, 10);
        assert_eq!(page.truncated, 6);
        let rendered = render_run_table(&page);
        assert!(rendered.contains("6 omitted"));

        // A caller cannot opt out of the ceiling.
        let page = lane_run_page(&records, usize::MAX);
        assert_eq!(page.limit, MAX_RUN_LIST_LIMIT);
        assert_eq!(page.truncated, 0);

        let empty = RunListPage::bounded(Vec::new(), DEFAULT_RUN_LIST_LIMIT);
        assert!(empty.is_empty());
        assert_eq!(render_run_table(&empty), "no durable runs");
    }

    #[test]
    fn run_dtos_round_trip_as_json() {
        let page = lane_run_page(&[lane_record("lane-a1b2c3d4")], DEFAULT_RUN_LIST_LIMIT);
        let json = serde_json::to_string(&page).unwrap();
        let back: RunListPage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, page);
    }

    // -- redaction ---------------------------------------------------------

    #[test]
    fn sanitize_redacts_secret_shaped_tokens() {
        for raw in [
            "authorization=Bearer-xyz",
            "api_key=abcdef",
            "SLACK_WEBHOOK=https://hooks.example/abc",
            "password=hunter2",
        ] {
            let sanitized = sanitize_line(raw);
            assert!(sanitized.contains(REDACTED), "{raw} -> {sanitized}");
        }
        for raw in [
            "sk-livekey123",
            "ghp_abcdefghij",
            "xoxb-1-2-3",
            "AKIAEXAMPLE1",
        ] {
            assert_eq!(sanitize_line(raw), REDACTED, "{raw}");
        }
        // Ordinary text survives, and leading indentation is structure: it is
        // preserved (bounded) while interior runs are still collapsed.
        assert_eq!(
            sanitize_line("  lane   stopped  cleanly "),
            "  lane stopped cleanly"
        );
        assert_eq!(sanitize_line("lane stopped"), "lane stopped");
        assert_eq!(sanitize_line("      "), "");
        assert_eq!(
            sanitize_line(&format!("{}deep", " ".repeat(40))),
            format!("{}deep", " ".repeat(MAX_PRESERVED_INDENT)),
            "indent is bounded so it cannot pad a receipt"
        );
    }

    #[test]
    fn sanitize_bounds_line_length_and_line_count() {
        let long = "x".repeat(MAX_DETAIL_LINE_CHARS * 3);
        let sanitized = sanitize_line(&long);
        assert_eq!(sanitized.chars().count(), MAX_DETAIL_LINE_CHARS);
        assert!(sanitized.ends_with('…'));

        let blob = (0..MAX_DETAIL_LINES * 2)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = sanitize_lines(&blob);
        assert_eq!(lines.len(), MAX_DETAIL_LINES + 1);
        assert!(lines.last().unwrap().contains("truncated"));
    }

    // -- shared executor: one code path, three surfaces --------------------

    fn seeded_registry() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let registry = crate::registry::LaneRegistry::open(dir.path()).unwrap();
        let record = registry
            .create_pending(
                Some("stopship".into()),
                Some("stopship".into()),
                Some("4022".into()),
                None,
                RuntimeBackendKind::Inline,
                None,
            )
            .unwrap();
        let id = record.id.clone();
        (dir, id)
    }

    #[test]
    fn every_surface_gets_the_same_receipt_for_the_same_lane_verb() {
        // #1888/#4022: the CLI, a slash command, and a hotbar dispatch must
        // observe the same durable Lane through the same contract.
        let (dir, id) = seeded_registry();
        let mut payloads = BTreeSet::new();
        for surface in ControlSurface::ALL {
            let receipt = execute_lane_control_in(
                *surface,
                ControlOperation::LaneStatus,
                Some(id.as_str()),
                Some(dir.path()),
            );
            assert_eq!(receipt.surface, *surface);
            assert_eq!(receipt.operation_id, "lane.status");
            assert_eq!(receipt.authority, ControlAuthority::Read);
            assert_eq!(receipt.persistence, PersistenceScope::LaneRegistry);
            assert_eq!(receipt.outcome, LifecycleOutcome::Inspected);
            assert_eq!(receipt.observed_lifecycle_seq, Known::Known(1));
            let page = receipt.runs.as_ref().expect("status carries the run DTO");
            assert_eq!(page.runs.len(), 1);
            assert_eq!(page.runs[0].run_id, id);
            assert_eq!(page.runs[0].runtime, Known::Known("inline".to_string()));
            // The observed durable payload must be identical. The detail lines
            // deliberately are not: the slash surface discloses that it skipped
            // reconciliation, which is a truthful difference, not drift.
            payloads.insert(serde_json::to_string(page).unwrap());
        }
        assert_eq!(
            payloads.len(),
            1,
            "surfaces observed different durable state"
        );
    }

    #[test]
    fn lane_list_is_bounded_and_identical_across_surfaces() {
        let (dir, id) = seeded_registry();
        let mut payloads = BTreeSet::new();
        for surface in ControlSurface::ALL {
            let receipt = execute_lane_control_in(
                *surface,
                ControlOperation::LaneList,
                None,
                Some(dir.path()),
            );
            assert_eq!(receipt.outcome, LifecycleOutcome::Inspected);
            let page = receipt.runs.as_ref().expect("list carries a bounded page");
            assert_eq!(page.limit, DEFAULT_RUN_LIST_LIMIT);
            assert_eq!(page.total, 1);
            assert_eq!(page.truncated, 0);
            assert!(page.runs.iter().any(|run| run.run_id == id));
            payloads.insert(serde_json::to_string(page).unwrap());
        }
        assert_eq!(payloads.len(), 1);
    }

    #[test]
    fn interrupt_acts_on_exact_run_identity_and_is_idempotent() {
        let (dir, id) = seeded_registry();
        let stale_fence = format!("{id}@99");
        let exact_fence = format!("{id}@1");

        // A stale fence must not act on a record that moved on.
        let stale = execute_lane_control_in(
            ControlSurface::Slash,
            ControlOperation::LaneInterrupt,
            Some(stale_fence.as_str()),
            Some(dir.path()),
        );
        assert_eq!(stale.outcome, LifecycleOutcome::Rejected);
        assert_eq!(
            stale.failure.as_ref().map(|failure| failure.kind),
            Some(ControlFailureKind::Conflict)
        );
        assert_eq!(stale.observed_lifecycle_seq, Known::Known(1));

        // The exact fence transitions it once.
        let first = execute_lane_control_in(
            ControlSurface::Cli,
            ControlOperation::LaneInterrupt,
            Some(exact_fence.as_str()),
            Some(dir.path()),
        );
        assert_eq!(first.outcome, LifecycleOutcome::Transitioned);
        assert!(first.retryable, "interrupt is declared idempotent");

        // Re-issuing converges rather than repeating the transition.
        let second = execute_lane_control_in(
            ControlSurface::Cli,
            ControlOperation::LaneInterrupt,
            Some(id.as_str()),
            Some(dir.path()),
        );
        assert_eq!(second.outcome, LifecycleOutcome::NoChange);
        assert!(
            second
                .detail
                .iter()
                .any(|line| line.contains("already stopped"))
        );
    }

    /// #4022: a no-op stop must not be reported as a transition. The backend
    /// distinguishes the three cases; the receipt must carry that through.
    #[test]
    fn an_already_terminal_lane_reports_no_change_not_transitioned() {
        let (dir, id) = seeded_registry();
        let first = execute_lane_control_in(
            ControlSurface::Cli,
            ControlOperation::LaneInterrupt,
            Some(id.as_str()),
            Some(dir.path()),
        );
        assert_eq!(first.outcome, LifecycleOutcome::Transitioned);
        let observed = first.observed_lifecycle_seq.clone();

        // Whoever stopped it, this call changed nothing and says so — and it
        // does not claim credit by advancing the lifecycle sequence.
        let second = execute_lane_control_in(
            ControlSurface::Cli,
            ControlOperation::LaneInterrupt,
            Some(id.as_str()),
            Some(dir.path()),
        );
        assert_eq!(second.outcome, LifecycleOutcome::NoChange);
        assert_eq!(second.observed_lifecycle_seq, observed);
    }

    /// #1888: the lifecycle fence is enforced by the registry under the same
    /// lock that mutates, so a stale fence refuses *and leaves the record
    /// untouched* rather than being pre-checked and then racing.
    #[test]
    fn a_stale_fence_refuses_under_the_lock_and_changes_nothing() {
        let (dir, id) = seeded_registry();
        let before = crate::registry::LaneRegistry::open(dir.path())
            .unwrap()
            .load(&id)
            .unwrap();

        let receipt = execute_lane_control_in(
            ControlSurface::Cli,
            ControlOperation::LaneInterrupt,
            Some(format!("{id}@{}", before.lifecycle_seq + 41).as_str()),
            Some(dir.path()),
        );
        assert_eq!(receipt.outcome, LifecycleOutcome::Rejected);
        assert_eq!(
            receipt.failure.as_ref().map(|failure| failure.kind),
            Some(ControlFailureKind::Conflict)
        );
        assert_eq!(
            receipt.observed_lifecycle_seq,
            Known::Known(before.lifecycle_seq),
            "the receipt reports the generation the registry actually saw"
        );

        let after = crate::registry::LaneRegistry::open(dir.path())
            .unwrap()
            .load(&id)
            .unwrap();
        assert_eq!(after, before, "a refused fence must not mutate the record");
    }

    /// #1888: two concurrent interrupts of the same Lane produce exactly one
    /// transition. The loser reports `no_change`, never a second transition.
    #[test]
    fn concurrent_interrupts_produce_exactly_one_transition() {
        use std::sync::mpsc;

        let (dir, id) = seeded_registry();
        let root = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let root = root.clone();
                let id = id.clone();
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let receipt = execute_lane_control_in(
                        ControlSurface::Cli,
                        ControlOperation::LaneInterrupt,
                        Some(id.as_str()),
                        Some(&root),
                    );
                    tx.send(receipt.outcome).unwrap();
                })
            })
            .collect();
        drop(tx);
        for handle in handles {
            handle.join().unwrap();
        }
        let outcomes: Vec<_> = rx.iter().collect();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == LifecycleOutcome::Transitioned)
                .count(),
            1,
            "exactly one caller may claim the transition: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == LifecycleOutcome::NoChange)
                .count(),
            1,
            "the loser reports no_change: {outcomes:?}"
        );
    }

    /// #4022: `lane.interrupt` is a real write on every surface, including the
    /// composer. Runtime teardown must not run on the composer thread, but the
    /// answer is the off-loop executor in `codewhale-tui::lane_control` — not a
    /// surface refusal. This executor is the shared, blocking body that both the
    /// CLI and that worker thread call, so the slash surface must transition
    /// durable state exactly like the CLI does.
    #[test]
    fn lane_interrupt_is_a_real_write_on_the_slash_surface() {
        let (dir, id) = seeded_registry();
        let descriptor = ControlOperation::LaneInterrupt.descriptor();
        assert!(
            descriptor.offers(ControlSurface::Slash),
            "interrupt must stay offered on the composer surface"
        );
        assert!(
            descriptor
                .availability(ControlSurface::Slash, ControlContext::new(true, true))
                .is_available(),
            "interrupt must stay available, not surface-limited"
        );

        let receipt = execute_lane_control_in(
            ControlSurface::Slash,
            ControlOperation::LaneInterrupt,
            Some(id.as_str()),
            Some(dir.path()),
        );
        assert_eq!(receipt.outcome, LifecycleOutcome::Transitioned);
        assert!(receipt.availability.is_available());
        assert!(receipt.failure.is_none());

        // The write reached durable state rather than being deferred away.
        let record = crate::registry::LaneRegistry::open(dir.path())
            .unwrap()
            .load(&id)
            .unwrap();
        assert_ne!(record.status, LaneStatus::Pending);
    }

    /// #4022: a read on the slash surface does no reconciliation (no tmux
    /// subprocess, no lock) and says so instead of implying freshness.
    #[test]
    fn slash_reads_skip_reconciliation_and_disclose_it() {
        let (dir, id) = seeded_registry();
        for operation in [ControlOperation::LaneList, ControlOperation::LaneStatus] {
            let target = (operation == ControlOperation::LaneStatus).then_some(id.as_str());
            let receipt =
                execute_lane_control_in(ControlSurface::Slash, operation, target, Some(dir.path()));
            assert_eq!(receipt.outcome, LifecycleOutcome::Inspected);
            assert!(!receipt.reconciled);
            assert!(
                receipt
                    .detail
                    .iter()
                    .any(|line| line.contains("reconciliation skipped")),
                "{} must disclose the skipped reconciliation",
                receipt.operation_id
            );
        }
    }

    /// #4022: `lane status` must keep reporting the fields operators use to
    /// attach to and tail a Lane. Dropping them silently was a regression.
    #[test]
    fn lane_status_preserves_attach_branch_session_and_log_fields() {
        let record = lane_record("lane-a1b2c3d4");
        let summary = lane_run_summary(&record);
        assert_eq!(summary.branch, Known::Known("lane/x".to_string()));
        assert_eq!(summary.runtime_session, Known::Known("cw-x".to_string()));
        assert!(summary.log.is_known(), "the log path must survive");
        let detail = summary.render_detail();
        for field in ["branch:", "session:", "socket:", "attach:", "log:"] {
            assert!(detail.contains(field), "{field} missing from {detail}");
        }
    }

    #[test]
    fn unknown_lane_ids_fail_identically_on_every_surface() {
        let (dir, _id) = seeded_registry();
        for (surface, operation) in [
            (ControlSurface::Cli, ControlOperation::LaneStatus),
            (ControlSurface::Slash, ControlOperation::LaneStatus),
            (ControlSurface::Cli, ControlOperation::LaneInterrupt),
            (ControlSurface::Slash, ControlOperation::LaneInterrupt),
        ] {
            {
                let receipt = execute_lane_control_in(
                    surface,
                    operation,
                    Some("lane-doesnotexist"),
                    Some(dir.path()),
                );
                assert_eq!(receipt.outcome, LifecycleOutcome::Rejected);
                assert_eq!(
                    receipt.failure.as_ref().map(|failure| failure.kind),
                    Some(ControlFailureKind::NotFound)
                );
                assert!(!receipt.retryable);
            }
        }
    }

    #[test]
    fn corrupt_lane_records_are_retryable_backend_failures() {
        let (dir, id) = seeded_registry();
        let registry = crate::registry::LaneRegistry::open(dir.path()).unwrap();
        std::fs::write(registry.record_path(&id), b"{not-json").unwrap();

        let receipt = execute_lane_control_in(
            ControlSurface::Cli,
            ControlOperation::LaneStatus,
            Some(&id),
            Some(dir.path()),
        );

        assert_eq!(receipt.outcome, LifecycleOutcome::Failed);
        assert_eq!(
            receipt.failure.as_ref().map(|failure| failure.kind),
            Some(ControlFailureKind::Backend)
        );
        assert!(receipt.retryable);
        assert!(
            receipt
                .failure
                .as_ref()
                .is_some_and(|failure| failure.message.contains("parse lane record"))
        );
    }

    #[test]
    fn unimplemented_lane_verbs_are_refused_before_touching_the_registry() {
        let (dir, id) = seeded_registry();
        for operation in [ControlOperation::LaneRestart, ControlOperation::LaneResume] {
            for surface in ControlSurface::ALL {
                let receipt = execute_lane_control_in(
                    *surface,
                    operation,
                    Some(id.as_str()),
                    Some(dir.path()),
                );
                assert_eq!(receipt.outcome, LifecycleOutcome::Rejected);
                assert_eq!(
                    receipt.availability.reason(),
                    Some(UnavailableReason::BackendNotImplemented)
                );
                assert!(!receipt.retryable);
            }
        }
        // The refusal did not mutate durable state.
        let registry = crate::registry::LaneRegistry::open(dir.path()).unwrap();
        assert_eq!(registry.load(&id).unwrap().status, LaneStatus::Pending);
    }

    #[test]
    fn a_missing_registry_is_reported_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("never-created");
        let receipt = execute_lane_control_in(
            ControlSurface::Slash,
            ControlOperation::LaneList,
            None,
            Some(&absent),
        );
        assert_eq!(
            receipt.availability.reason(),
            Some(UnavailableReason::NoLaneRegistry)
        );
        assert!(!absent.exists(), "a read verb must not create the registry");
    }

    #[test]
    fn home_rooted_paths_are_collapsed() {
        // `redact_path` is a no-op outside $HOME and never panics on either.
        let outside = redact_path(Path::new("/tmp/lanes/logs/x.ndjson"));
        assert_eq!(outside, "/tmp/lanes/logs/x.ndjson");
        if let Some(home) = home_prefix() {
            let inside = redact_path(&PathBuf::from(home).join("lanes").join("x"));
            assert!(inside.starts_with("~/"), "{inside}");
            assert!(!inside.contains(home));
            assert_eq!(redact_path(Path::new(home)), "~");

            // Prefix confusion: a sibling directory that merely *starts with*
            // $HOME's text is not inside $HOME and must not be abbreviated.
            let sibling = format!("{home}-backup/secrets");
            assert_eq!(
                redact_path_str(&sibling),
                sibling,
                "a path boundary is a separator, not a string prefix"
            );
        }
    }

    /// 2026-08-04 audit: `Authorization: Bearer <jwt>` leaked the JWT in
    /// full. The bare `Bearer` token failed the `len() > prefix.len()` guard
    /// (it IS the prefix) and the JWT after it matched nothing. Every
    /// operator-visible ControlReceipt string goes through this sanitizer.
    #[test]
    fn bearer_and_case_variant_secrets_do_not_survive_sanitization() {
        // Assembled at runtime so no scanner-shaped JWT literal sits in the
        // source tree — same precedent as the AWS fixture in
        // `crates/workflow/src/redaction.rs`.
        let jwt = ["eyJhbGciOiJIUzI1NiJ9", "eyJzdWIiOiIxIn0", "c2lnbmF0dXJl"].join(".");

        let line = sanitize_line(&format!("request failed: Authorization: Bearer {jwt}"));
        assert!(!line.contains(&jwt), "bearer JWT leaked: {line}");

        // Lowercase scheme, and a trailing comma after the scheme word.
        let line = sanitize_line(&format!("hdr bearer {jwt}"));
        assert!(!line.contains(&jwt), "lowercase bearer leaked: {line}");
        let line = sanitize_line(&format!("token, {jwt}"));
        assert!(
            !line.contains(&jwt),
            "scheme word with punctuation leaked: {line}"
        );

        // Case-insensitive value prefixes.
        for secret in [
            "SK-live-abc123def456",
            "sk-live-abc123def456",
            "GHP_abcdef123456",
        ] {
            let line = sanitize_line(&format!("using {secret} now"));
            assert!(!line.contains(secret), "prefixed secret leaked: {line}");
        }

        // Ordinary prose must survive: the scheme word only arms the NEXT
        // token, and only when it is a bare scheme word.
        let line = sanitize_line("the bearer of this token is unknown");
        assert!(line.contains("the bearer"), "over-redacted prose: {line}");
        assert!(line.contains("unknown"), "over-redacted prose: {line}");
    }
}
