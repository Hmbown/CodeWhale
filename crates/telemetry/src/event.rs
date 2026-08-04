//! The wire schema. This module is the whole of what may leave the machine.
//!
//! Every field below is an integer, a boolean, or a **closed enum string**,
//! except exactly three bounded strings: `app_version`, `git_sha`, and
//! `panic_site`. Each of those three has a written rule and a test pinning the
//! rule. There is no free-form string type here and no open-keyed map, which is
//! what makes "aggregates and events, never content" a property the compiler
//! and the test suite can enforce rather than a promise.
//!
//! Two standing rules for anyone extending this file:
//!
//! 1. **Never `#[derive(Serialize)]` over an existing state type.**
//!    `codewhale_state::Thread` carries `git_sha`, `git_branch`,
//!    `git_origin_url`, `cwd`, and `path`. A payload builder that accepts one
//!    and derives breaches the red lines in a single line. Every struct here is
//!    built from scratch with explicit fields.
//! 2. **Bump [`SCHEMA_VERSION`] on any field add, remove, or retype**, and
//!    never reuse a number. The golden snapshot test fails until you do.

use serde::{Deserialize, Serialize};

/// Wire schema version. Bumped on any field add, remove, or retype; never
/// reused. `crates/telemetry/tests/golden/v1.json` pins what v1 was.
pub const SCHEMA_VERSION: u32 = 1;

/// Which product surface produced a batch.
///
/// Deliberately **not** derived from the executable: `codewhale-tui` serves at
/// least five surfaces, and app-server runs in-process inside `codewhale`, so
/// `current_exe()` would report every app-server session as CLI. Each
/// subcommand dispatch names its own surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    /// The interactive terminal UI.
    Tui,
    /// `codewhale exec` — one non-interactive run.
    Exec,
    /// Terminal `config` / `auth` / `update` subcommands.
    Cli,
    /// The app-server protocol surface (in-process inside `codewhale`).
    AppServer,
    /// The MCP server surface.
    McpServer,
    /// `codewhale serve`.
    Serve,
}

impl Surface {
    /// Every surface, for exhaustive iteration in tests and schema checks.
    pub const ALL: &'static [Self] = &[
        Self::Tui,
        Self::Exec,
        Self::Cli,
        Self::AppServer,
        Self::McpServer,
        Self::Serve,
    ];

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Exec => "exec",
            Self::Cli => "cli",
            Self::AppServer => "app-server",
            Self::McpServer => "mcp-server",
            Self::Serve => "serve",
        }
    }
}

/// Operating system family. A closed whitelist, so an unrecognised
/// `std::env::consts::OS` reports `other` rather than shipping a novel string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
    /// Windows.
    Windows,
    /// FreeBSD.
    Freebsd,
    /// Android.
    Android,
    /// Anything else.
    Other,
}

impl Os {
    /// Every value, for exhaustive iteration.
    pub const ALL: &'static [Self] = &[
        Self::Linux,
        Self::Macos,
        Self::Windows,
        Self::Freebsd,
        Self::Android,
        Self::Other,
    ];

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Freebsd => "freebsd",
            Self::Android => "android",
            Self::Other => "other",
        }
    }
}

/// CPU family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
    /// Anything else.
    Other,
}

impl Arch {
    /// Every value, for exhaustive iteration.
    pub const ALL: &'static [Self] = &[Self::X86_64, Self::Aarch64, Self::Other];

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Other => "other",
        }
    }
}

/// C runtime the binary was **compiled** against.
///
/// Compile-time (`cfg!(target_env)`), never runtime-detected: the only way to
/// read this at runtime is `/etc/os-release` or shelling to `ldd`, both of which
/// surface corporate golden-image vendor strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Libc {
    /// glibc.
    Gnu,
    /// musl.
    Musl,
    /// Not a libc target (macOS, Windows, and anything else).
    None,
}

impl Libc {
    /// Every value, for exhaustive iteration.
    pub const ALL: &'static [Self] = &[Self::Gnu, Self::Musl, Self::None];

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gnu => "gnu",
            Self::Musl => "musl",
            Self::None => "none",
        }
    }
}

/// Whether this binary is newly installed, upgraded, or downgraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// No prior version record on this machine.
    Install,
    /// The recorded version is older than this one.
    Upgrade,
    /// The recorded version is newer than this one.
    Downgrade,
}

impl InstallKind {
    /// Every value, for exhaustive iteration.
    pub const ALL: &'static [Self] = &[Self::Install, Self::Upgrade, Self::Downgrade];

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Upgrade => "upgrade",
            Self::Downgrade => "downgrade",
        }
    }
}

/// How a session was started.
///
/// Mirrors `codewhale_state::SessionSource` by value, deliberately re-declared
/// here rather than imported: this crate must not depend on the thread store,
/// which is the one crate whose types carry paths and git identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    /// A user opened a session directly.
    Interactive,
    /// Resumed from a persisted session.
    Resume,
    /// Forked from an existing conversation.
    Fork,
    /// Started programmatically.
    Api,
    /// Not stated.
    Unknown,
}

impl SessionSource {
    /// Every value, for exhaustive iteration.
    pub const ALL: &'static [Self] = &[
        Self::Interactive,
        Self::Resume,
        Self::Fork,
        Self::Api,
        Self::Unknown,
    ];

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Resume => "resume",
            Self::Fork => "fork",
            Self::Api => "api",
            Self::Unknown => "unknown",
        }
    }
}

/// How long a session lasted, bucketed. Half-open intervals, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurationBucket {
    /// `d < 60`
    #[serde(rename = "lt_1m")]
    Lt1m,
    /// `60 <= d < 600`
    #[serde(rename = "1m_10m")]
    OneToTen,
    /// `600 <= d < 3600`
    #[serde(rename = "10m_60m")]
    TenToSixty,
    /// `d >= 3600`
    #[serde(rename = "gt_60m")]
    Gt60m,
}

impl DurationBucket {
    /// Every value, for exhaustive iteration.
    pub const ALL: &'static [Self] = &[Self::Lt1m, Self::OneToTen, Self::TenToSixty, Self::Gt60m];

    /// Bucket a session duration in whole seconds.
    #[must_use]
    pub fn from_secs(secs: u64) -> Self {
        match secs {
            0..60 => Self::Lt1m,
            60..600 => Self::OneToTen,
            600..3600 => Self::TenToSixty,
            _ => Self::Gt60m,
        }
    }

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lt1m => "lt_1m",
            Self::OneToTen => "1m_10m",
            Self::TenToSixty => "10m_60m",
            Self::Gt60m => "gt_60m",
        }
    }
}

/// How the process ended.
///
/// Derived from an explicit atomic set by the panic hook, the signal task, and
/// the clean path — **never from an exit code**. `RunTerminationReason::Canceled`
/// maps to exit 130, the same value the SIGINT path uses, so a code-based
/// derivation would report every Esc-cancelled turn as a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitClass {
    /// Ordinary successful exit.
    Clean,
    /// Terminated by a signal.
    Signal,
    /// Terminated by a panic.
    Panic,
    /// Exited non-successfully without a signal or a panic.
    Error,
}

impl ExitClass {
    /// Every value, for exhaustive iteration.
    pub const ALL: &'static [Self] = &[Self::Clean, Self::Signal, Self::Panic, Self::Error];

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Signal => "signal",
            Self::Panic => "panic",
            Self::Error => "error",
        }
    }

    /// Stable numeric encoding for the process-wide `AtomicU8`.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Clean => 0,
            Self::Signal => 1,
            Self::Panic => 2,
            Self::Error => 3,
        }
    }

    /// Inverse of [`Self::as_u8`]; anything unrecognised reads as `Clean`.
    #[must_use]
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Signal,
            2 => Self::Panic,
            3 => Self::Error,
            _ => Self::Clean,
        }
    }
}

/// Cold-start time, bucketed, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColdStartBucket {
    /// `ms < 250`
    #[serde(rename = "lt_250")]
    Lt250,
    /// `250 <= ms < 1000`
    #[serde(rename = "250_1000")]
    Mid,
    /// `1000 <= ms < 3000`
    #[serde(rename = "1000_3000")]
    Slow,
    /// `ms >= 3000`
    #[serde(rename = "gte_3000")]
    Gte3000,
}

impl ColdStartBucket {
    /// Every value, for exhaustive iteration.
    pub const ALL: &'static [Self] = &[Self::Lt250, Self::Mid, Self::Slow, Self::Gte3000];

    /// Bucket a cold-start measurement in milliseconds.
    #[must_use]
    pub fn from_millis(ms: u64) -> Self {
        match ms {
            0..250 => Self::Lt250,
            250..1000 => Self::Mid,
            1000..3000 => Self::Slow,
            _ => Self::Gte3000,
        }
    }

    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lt250 => "lt_250",
            Self::Mid => "250_1000",
            Self::Slow => "1000_3000",
            Self::Gte3000 => "gte_3000",
        }
    }
}

/// Feature-use counts for one session.
///
/// A struct of named `u32`s rather than a map, deliberately: a
/// `BTreeMap<&'static str, u32>` is an open key set the compiler cannot police,
/// so the doc-match test would be asserting that the doc matches a fixture
/// rather than the binary. Adding a counter now requires editing this file,
/// which is where that test lives. Every field serializes, including zeros.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    /// Model turns completed.
    pub turns: u32,
    /// Tool calls executed, across every surface.
    pub tool_calls: u32,
    /// Fleet dispatches started.
    pub fleet_dispatch: u32,
    /// `workflow_run` invocations, keyed off the parsed action discriminant.
    pub workflow_run: u32,
    /// Sub-agents spawned.
    pub subagent_spawn: u32,
    /// MCP servers that reached `connected`.
    pub mcp_server_connected: u32,
    /// Native-memory searches.
    pub memory_search: u32,
    /// Approval modals shown.
    pub approval_modal_shown: u32,
    /// Approvals granted by an auto-allow rule.
    pub approval_auto_allowed: u32,
    /// Command-palette opens.
    pub command_palette_open: u32,
}

impl Counters {
    /// Field names in declaration order, for the doc-match test.
    pub const FIELDS: &'static [&'static str] = &[
        "turns",
        "tool_calls",
        "fleet_dispatch",
        "workflow_run",
        "subagent_spawn",
        "mcp_server_connected",
        "memory_search",
        "approval_modal_shown",
        "approval_auto_allowed",
        "command_palette_open",
    ];
}

/// Error counts for one session.
///
/// Every value is a count of a **variant discriminant**, never of an
/// `err.to_string()`. `ToolError::PathEscape`'s `Display` *is* an absolute path;
/// the secret store's *is* the store's absolute path; every `LlmError` variant
/// carries the raw provider HTTP body verbatim, and a 400 from a content filter
/// routinely echoes the prompt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Errors {
    /// Credential preflight rejected the route.
    pub auth_preflight_failed: u32,
    /// Provider responded 4xx.
    pub provider_http_4xx: u32,
    /// Provider responded 5xx.
    pub provider_http_5xx: u32,
    /// A tool call was denied by policy.
    pub tool_denied_by_policy: u32,
    /// A tool call timed out.
    pub tool_timeout: u32,
    /// A request failed below HTTP — DNS, connect, TLS, or timeout.
    pub network_error: u32,
}

impl Errors {
    /// Field names in declaration order, for the doc-match test.
    pub const FIELDS: &'static [&'static str] = &[
        "auth_preflight_failed",
        "provider_http_4xx",
        "provider_http_5xx",
        "tool_denied_by_policy",
        "tool_timeout",
        "network_error",
    ];
}

/// Per-session histogram of turn wall-clock time.
///
/// A histogram, never a per-turn series: a timestamped stream of turn durations
/// reconstructs a session's working rhythm, which is the same objection that
/// rules out per-tool-call phone-home.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWall {
    /// Turns under 5 seconds.
    pub lt_5s: u32,
    /// Turns in `[5s, 30s)`.
    #[serde(rename = "5_30s")]
    pub five_to_thirty: u32,
    /// Turns in `[30s, 120s)`.
    #[serde(rename = "30_120s")]
    pub thirty_to_onetwenty: u32,
    /// Turns at or over 120 seconds.
    pub gte_120s: u32,
}

impl TurnWall {
    /// Field names in wire spelling, in declaration order.
    pub const FIELDS: &'static [&'static str] = &["lt_5s", "5_30s", "30_120s", "gte_120s"];

    /// Record one turn of `secs` wall-clock seconds.
    pub fn observe_secs(&mut self, secs: u64) {
        match secs {
            0..5 => self.lt_5s = self.lt_5s.saturating_add(1),
            5..30 => self.five_to_thirty = self.five_to_thirty.saturating_add(1),
            30..120 => self.thirty_to_onetwenty = self.thirty_to_onetwenty.saturating_add(1),
            _ => self.gte_120s = self.gte_120s.saturating_add(1),
        }
    }
}

/// One telemetry event.
///
/// The tag is the `event` key, so the wire form is flat and the variant set is
/// closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// This binary's version differs from the one last recorded on this machine.
    InstallOrUpgrade {
        /// Install, upgrade, or downgrade.
        kind: InstallKind,
        /// The previously recorded version, read from the telemetry state file
        /// **only** — never from session history or config mtimes, which have a
        /// different privacy contract.
        previous_version: Option<String>,
    },
    /// A session began.
    SessionStart {
        /// How it was started.
        source: SessionSource,
    },
    /// A session ended. Everything the session accumulated ships here, once.
    SessionEnd {
        /// Bucketed wall-clock session length.
        duration_bucket: DurationBucket,
        /// How the process ended.
        exit_class: ExitClass,
        /// Bucketed cold start. `null` on surfaces that do not measure it.
        cold_start_bucket: Option<ColdStartBucket>,
        /// Sorted, deduplicated `ProviderKind` names. A custom provider yields
        /// the literal `"custom"`, never the customer's `[providers.<name>]`
        /// table key, and no model id is sent for any provider.
        providers: Vec<String>,
        /// Feature-use counts.
        counters: Counters,
        /// Error counts.
        errors: Errors,
        /// Turn wall-clock histogram.
        turn_wall: TurnWall,
    },
    /// The process panicked.
    Panic {
        /// Source location, reduced by the `crates/` allowlist. Never the panic
        /// *message*: a slicing panic embeds the entire string being sliced, and
        /// this tree slices user and model text in dozens of places.
        site: String,
    },
}

impl Event {
    /// The `event` discriminant, for the doc-match test.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::InstallOrUpgrade { .. } => "install_or_upgrade",
            Self::SessionStart { .. } => "session_start",
            Self::SessionEnd { .. } => "session_end",
            Self::Panic { .. } => "panic",
        }
    }
}

/// The POST body. One per flush.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// RFC3339 UTC, second precision. Per-**batch** only — events carry no
    /// timestamps at all.
    pub sent_at: String,
    /// Random v4 UUID stored on this machine, rotated every 90 days.
    pub install_id: String,
    /// `CARGO_PKG_VERSION`. Must match `^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$`.
    pub app_version: String,
    /// First 12 hex chars of the release-CI build sha, or `null` for every
    /// locally built binary.
    pub git_sha: Option<String>,
    /// Which surface produced this batch.
    pub surface: Surface,
    /// OS family.
    pub os: Os,
    /// CPU family.
    pub arch: Arch,
    /// Compile-time libc.
    pub libc: Libc,
    /// Whether both stdin and stdout were terminals.
    pub tty: bool,
    /// The events.
    pub events: Vec<Event>,
}

impl Batch {
    /// Envelope field names in declaration order, for the doc-match test.
    pub const FIELDS: &'static [&'static str] = &[
        "schema_version",
        "sent_at",
        "install_id",
        "app_version",
        "git_sha",
        "surface",
        "os",
        "arch",
        "libc",
        "tty",
        "events",
    ];
}
