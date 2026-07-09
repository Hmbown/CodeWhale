//! Unified setup-state model for the v0.8.67 constitution-first setup lane
//! (#3403).
//!
//! This is the single record every setup step (#3404–#3412) reads and writes so
//! that "configured", "skipped", "verified", and "ready" mean the same thing
//! everywhere. It is persisted as a JSON sidecar (`setup_state.json`) under
//! `$CODEWHALE_HOME`, written atomically through [`crate::persistence`] so it is
//! independent of `config.toml`'s comment-preserving writes and can never leave
//! a half-written file.
//!
//! The record holds two things:
//!
//! 1. A per-[`SetupStep`] [`StepEntry`] (status, required, safe summary,
//!    writing lane version).
//! 2. The constitution-first fields the wizard, the update checkpoint, and
//!    `/constitution` all coordinate on.
//!
//! Readiness is a *derived* property ([`first_run_ready`](SetupState::first_run_ready)
//! / [`update_ready`](SetupState::update_ready)); it is never persisted, so the
//! rules can evolve without a migration.
//!
//! Secrets never appear here: [`StepEntry::result`] is a short human-facing
//! summary (provider name, model id, mode name), never a key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::persistence;

/// Current schema version of the persisted setup-state record.
pub const SETUP_STATE_SCHEMA_VERSION: u32 = 1;

/// Filename of the setup-state sidecar under `$CODEWHALE_HOME`.
pub const SETUP_STATE_FILE_NAME: &str = "setup_state.json";

/// Canonical setup step ids. The ordering matches the first-run spine so a
/// `BTreeMap<SetupStep, _>` renders in wizard order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStep {
    /// Language first, so later screens and constitution prose are localized.
    Language,
    /// Provider + key (or local runtime) and a default model.
    ProviderModel,
    /// Trust, approvals, sandbox, network — runtime posture (#3406).
    TrustSandbox,
    /// User-global constitution choice / checkpoint.
    Constitution,
    /// Operate/Fleet readiness: provider auth, worker runtime, roster, and
    /// concurrency review. Plan-limit detection remains a separate product
    /// decision; this step only records reviewed current facts.
    OperateFleet,
    /// Hotbar shortcuts are optional, but now have a first-class setup card.
    Hotbar,
    /// Tools / MCP / skills / plugins (later lanes; tracked for completeness).
    ToolsMcp,
    /// Remote / mobile runtime (later lane; tracked for completeness).
    RemoteRuntime,
    /// Persistence paths for setup state, config, constitution, memory, and notes.
    Persistence,
    /// Final verification / doctor / ready summary.
    Verification,
}

impl SetupStep {
    /// All steps in canonical first-run order.
    pub const ALL: [SetupStep; 10] = [
        SetupStep::Language,
        SetupStep::ProviderModel,
        SetupStep::TrustSandbox,
        SetupStep::Constitution,
        SetupStep::OperateFleet,
        SetupStep::Hotbar,
        SetupStep::ToolsMcp,
        SetupStep::RemoteRuntime,
        SetupStep::Persistence,
        SetupStep::Verification,
    ];
}

/// Status of a single setup step. Shared vocabulary so `/setup`, `doctor`, and
/// the context report never invent their own meanings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// Never visited.
    NotStarted,
    /// Suggested for a good first-run experience but not required.
    Recommended,
    /// Available but entirely optional.
    Optional,
    /// Intentionally postponed; surfaces in the report, does not block.
    Deferred,
    /// Currently being worked on.
    InProgress,
    /// Completed and checked (e.g. key validated, mode confirmed).
    Verified,
    /// Reached a usable-but-incomplete state needing user action
    /// (e.g. a key that failed validation). Does not block the ready screen.
    NeedsAction,
    /// Attempted and errored.
    Failed,
    /// Explicitly skipped by the user.
    Skipped,
}

impl StepStatus {
    /// True for statuses that count as "the user dealt with this step" for the
    /// purpose of reaching the ready screen.
    #[must_use]
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            StepStatus::Verified
                | StepStatus::NeedsAction
                | StepStatus::Deferred
                | StepStatus::Optional
                | StepStatus::Skipped
        )
    }
}

/// One persisted entry per setup step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepEntry {
    pub status: StepStatus,
    /// Whether this step blocks "ready" for the lane that owns it. First-run and
    /// update lanes differ; see the readiness helpers on [`SetupState`].
    #[serde(default)]
    pub required: bool,
    /// Short, safe human-facing summary — provider name, model id, mode name,
    /// health. **Never a secret.**
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Lane (e.g. `"0.8.67"`) that last wrote this entry, so staleness is
    /// visible to `/setup`, `doctor`, and the context report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl StepEntry {
    /// A freshly-visited entry written by `version`.
    #[must_use]
    pub fn new(status: StepStatus, required: bool, version: impl Into<String>) -> Self {
        Self {
            status,
            required,
            result: None,
            version: Some(version.into()),
        }
    }

    #[must_use]
    pub fn with_result(mut self, result: impl Into<String>) -> Self {
        self.result = Some(result.into());
        self
    }
}

/// The user's constitution decision. Every value except [`Unset`] counts as an
/// explicit choice for readiness.
///
/// [`Unset`]: ConstitutionChoice::Unset
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstitutionChoice {
    /// No decision recorded yet.
    #[default]
    Unset,
    /// Accepted the bundled/default constitution floor. Creates no custom file.
    Bundled,
    /// Created a guided structured user-global constitution.
    GuidedCustom,
    /// Expert full-Markdown override
    /// (`$CODEWHALE_HOME/prompts/constitution.md` + opt-in env).
    ExpertOverride,
    /// Explicitly postponed; bundled law applies until the user returns.
    Deferred,
}

impl ConstitutionChoice {
    /// True for any value other than [`Unset`](ConstitutionChoice::Unset).
    #[must_use]
    pub fn is_explicit(self) -> bool {
        !matches!(self, ConstitutionChoice::Unset)
    }
}

/// How the active custom constitution was authored. Recorded alongside
/// [`ConstitutionChoice::GuidedCustom`] so `/setup`, `doctor`, and the report
/// can show provenance without parsing free-text step results.
///
/// This is a *new optional field* rather than a new [`ConstitutionChoice`]
/// variant so records written by this lane still load in older binaries
/// (unknown fields are ignored on read; an unknown enum variant would fail the
/// whole parse and force the inherited-state fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstitutionAuthoring {
    /// Deterministically rendered from the guided answers.
    Guided,
    /// Drafted by the user's configured model from the guided answers, then
    /// schema-validated, bounded, previewed, and ratified. Advisory authorship
    /// only — the drafting model gains no authority from having written it.
    ModelDrafted,
}

/// Which constitution surface is currently the active user-global law.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstitutionSource {
    /// Only the bundled floor is active.
    #[default]
    Bundled,
    /// A structured `constitution.json` under `$CODEWHALE_HOME`.
    UserGlobal,
    /// An expert full-Markdown override file.
    ExpertOverride,
}

/// Validity of the active user-global constitution file, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstitutionValidity {
    /// No custom file, or validity not yet evaluated.
    #[default]
    Unknown,
    /// Parsed and usable.
    Valid,
    /// Present but failed to parse / structurally invalid.
    Invalid,
    /// Present but carried no usable policy.
    Empty,
    /// Present but could not be read.
    Unreadable,
}

/// Where the current runtime posture came from. Mirrors the rule that a
/// constitution may *recommend* posture but only an explicit config action
/// (#3406) applies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePostureSource {
    /// Not yet reviewed.
    #[default]
    Unset,
    /// Carried over from existing config without an explicit confirmation.
    Inherited,
    /// The user explicitly reviewed and confirmed the posture in setup.
    Confirmed,
}

impl RuntimePostureSource {
    /// True when posture has been inherited or confirmed (either satisfies
    /// first-run readiness).
    #[must_use]
    pub fn is_reviewed(self) -> bool {
        matches!(
            self,
            RuntimePostureSource::Inherited | RuntimePostureSource::Confirmed
        )
    }
}

/// The persisted, per-version setup-state record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupState {
    pub schema_version: u32,

    /// Per-step status entries.
    #[serde(default)]
    pub steps: BTreeMap<SetupStep, StepEntry>,

    // ── Constitution-first fields ───────────────────────────────────────
    /// The user's constitution decision.
    #[serde(default)]
    pub constitution_choice: ConstitutionChoice,
    /// Lane version (e.g. `"0.8.67"`) whose constitution checkpoint the user has
    /// completed. Drives the once-per-version update checkpoint (#3794).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constitution_checkpoint_completed_for: Option<String>,
    /// Language the constitution prose was authored/reviewed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constitution_language: Option<String>,
    /// Which surface is the active user-global law.
    #[serde(default)]
    pub constitution_source: ConstitutionSource,
    /// Validity of the active user-global constitution file.
    #[serde(default)]
    pub constitution_validity: ConstitutionValidity,
    /// How the active custom constitution was authored (guided deterministic
    /// vs model-drafted-then-ratified). `None` for bundled/deferred/inherited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constitution_authoring: Option<ConstitutionAuthoring>,
    /// Stable content hash of the most recently previewed/accepted rendered
    /// constitution (see [`crate::user_constitution::UserConstitution::preview_hash`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constitution_preview_hash: Option<String>,
    /// Monotonic counter bumped each time a custom constitution is saved, so the
    /// report and `/constitution` can show which revision is live.
    #[serde(default)]
    pub constitution_preview_version: u32,
    /// Where the current runtime posture came from.
    #[serde(default)]
    pub runtime_posture_source: RuntimePostureSource,

    /// True when this record was *derived* from existing config rather than
    /// persisted by an explicit setup run. Lets `/setup` and `doctor` explain
    /// why an updating user is not treated as a broken fresh install.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherited: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            schema_version: SETUP_STATE_SCHEMA_VERSION,
            steps: BTreeMap::new(),
            constitution_choice: ConstitutionChoice::default(),
            constitution_checkpoint_completed_for: None,
            constitution_language: None,
            constitution_source: ConstitutionSource::default(),
            constitution_validity: ConstitutionValidity::default(),
            constitution_authoring: None,
            constitution_preview_hash: None,
            constitution_preview_version: 0,
            runtime_posture_source: RuntimePostureSource::default(),
            inherited: false,
        }
    }
}

/// Observable, secret-free facts about existing config used to derive a safe
/// inherited setup-state for users who upgrade without a `setup_state.json`.
///
/// The caller (TUI/CLI) gathers these from `ConfigToml`, the trust marker, and
/// the constitution files; keeping them as plain data keeps this module pure and
/// unit-testable.
#[derive(Debug, Clone, Default)]
pub struct InheritedConfigFacts {
    /// A provider/model route is configured.
    pub has_provider_route: bool,
    /// A key or local runtime is available (presence only — never the value).
    pub has_credentials_or_local_runtime: bool,
    /// The user has previously made a trust/approval decision.
    pub trust_chosen: bool,
    /// Onboarding language, if known.
    pub language: Option<String>,
    /// A structured user-global `constitution.json` exists.
    pub has_user_constitution: bool,
    /// An expert full-Markdown override is active.
    pub has_expert_override: bool,
    /// Validity of the user-global constitution, if present.
    pub user_constitution_validity: ConstitutionValidity,
}

impl SetupState {
    /// Status for a step, defaulting to [`StepStatus::NotStarted`].
    #[must_use]
    pub fn status(&self, step: SetupStep) -> StepStatus {
        self.steps
            .get(&step)
            .map_or(StepStatus::NotStarted, |e| e.status)
    }

    /// Record (insert or replace) an entry for `step`.
    pub fn set_step(&mut self, step: SetupStep, entry: StepEntry) -> &mut Self {
        self.steps.insert(step, entry);
        self
    }

    #[must_use]
    fn step_verified(&self, step: SetupStep) -> bool {
        self.status(step) == StepStatus::Verified
    }

    /// Provider/model is acceptable for first-run readiness when it is either
    /// verified or in an actionable needs-action state (the EPIC keeps a
    /// failed-key path reaching the ready screen).
    #[must_use]
    fn provider_model_ready_or_needs_action(&self) -> bool {
        matches!(
            self.status(SetupStep::ProviderModel),
            StepStatus::Verified | StepStatus::NeedsAction
        )
    }

    /// First-run "ready": language verified, provider/model ready-or-needs-action,
    /// runtime posture inherited/confirmed, and an explicit constitution choice.
    #[must_use]
    pub fn first_run_ready(&self) -> bool {
        self.step_verified(SetupStep::Language)
            && self.provider_model_ready_or_needs_action()
            && self.runtime_posture_source.is_reviewed()
            && self.constitution_choice.is_explicit()
    }

    /// Operate/Fleet "ready": provider credentials are verified, runtime
    /// posture has been reviewed, and the user has explicitly reviewed the
    /// Fleet/Operate on-ramp. This is intentionally separate from
    /// [`first_run_ready`](Self::first_run_ready): a local-first user can be
    /// ready for ordinary first use before enabling durable multi-worker work.
    #[must_use]
    pub fn operate_ready(&self) -> bool {
        self.first_run_ready()
            && self.step_verified(SetupStep::ProviderModel)
            && self.step_verified(SetupStep::OperateFleet)
    }

    /// Update "ready" for `version`: the constitution checkpoint for that lane is
    /// complete. Everything else is inherited from existing config.
    #[must_use]
    pub fn update_ready(&self, version: &str) -> bool {
        self.constitution_checkpoint_completed_for.as_deref() == Some(version)
    }

    /// Whether the once-per-version update checkpoint should still be shown.
    #[must_use]
    pub fn needs_constitution_checkpoint(&self, version: &str) -> bool {
        !self.update_ready(version)
    }

    /// Mark the constitution checkpoint complete for `version` (the bundled /
    /// default path is a valid completion).
    pub fn complete_constitution_checkpoint(
        &mut self,
        version: impl Into<String>,
        choice: ConstitutionChoice,
    ) -> &mut Self {
        self.constitution_checkpoint_completed_for = Some(version.into());
        self.constitution_choice = choice;
        self
    }

    /// Derive a safe inherited state for an existing user with no persisted
    /// `setup_state.json`. Surfaces they already configured become
    /// [`StepStatus::Verified`]; an update never looks like a fresh, broken
    /// setup. The constitution checkpoint is intentionally left incomplete so
    /// updating users still see it once.
    #[must_use]
    pub fn derive_inherited(facts: &InheritedConfigFacts) -> Self {
        let mut state = SetupState {
            inherited: true,
            ..SetupState::default()
        };
        let inherited = "inherited";

        if facts.language.is_some() {
            state.set_step(
                SetupStep::Language,
                StepEntry::new(StepStatus::Verified, true, inherited),
            );
            state.constitution_language = facts.language.clone();
        }

        if facts.has_provider_route && facts.has_credentials_or_local_runtime {
            state.set_step(
                SetupStep::ProviderModel,
                StepEntry::new(StepStatus::Verified, true, inherited),
            );
        } else if facts.has_provider_route {
            state.set_step(
                SetupStep::ProviderModel,
                StepEntry::new(StepStatus::NeedsAction, true, inherited),
            );
        }

        if facts.trust_chosen {
            state.set_step(
                SetupStep::TrustSandbox,
                StepEntry::new(StepStatus::Verified, true, inherited),
            );
            state.runtime_posture_source = RuntimePostureSource::Inherited;
        }

        // Constitution: classify the active surface, but never auto-complete the
        // checkpoint — the update lane requires the user to acknowledge it once.
        if facts.has_expert_override {
            state.constitution_source = ConstitutionSource::ExpertOverride;
            state.constitution_choice = ConstitutionChoice::ExpertOverride;
        } else if facts.has_user_constitution {
            state.constitution_source = ConstitutionSource::UserGlobal;
            state.constitution_validity = facts.user_constitution_validity;
            if facts.user_constitution_validity == ConstitutionValidity::Valid {
                state.constitution_choice = ConstitutionChoice::GuidedCustom;
            }
        } else {
            state.constitution_source = ConstitutionSource::Bundled;
        }

        state
    }

    /// Path to the setup-state sidecar under `$CODEWHALE_HOME`.
    pub fn path() -> Result<PathBuf> {
        Ok(crate::codewhale_home()?.join(SETUP_STATE_FILE_NAME))
    }

    /// Load the persisted setup-state from the home sidecar.
    ///
    /// Returns `Ok(None)` when the file is missing **or** unreadable/corrupt, so
    /// callers fall back to [`derive_inherited`](Self::derive_inherited) rather
    /// than forcing a fresh wizard. A corrupt record is logged, never fatal.
    pub fn load() -> Result<Option<Self>> {
        Ok(Self::load_from(&Self::path()?))
    }

    /// Load from an explicit path (testable). See [`load`](Self::load) for the
    /// missing/corrupt fallback contract.
    #[must_use]
    pub fn load_from(path: &Path) -> Option<Self> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(
                    target: "config::setup_state",
                    "could not read {} ({e}); deriving status from existing config",
                    path.display()
                );
                return None;
            }
        };
        match serde_json::from_str::<SetupState>(&raw) {
            Ok(state) => Some(state),
            Err(e) => {
                tracing::warn!(
                    target: "config::setup_state",
                    "{} is not a valid setup-state record ({e}); deriving status from existing config",
                    path.display()
                );
                None
            }
        }
    }

    /// Atomically persist this record to the home sidecar.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        self.save_to(&path)
    }

    /// Atomically persist to an explicit path (testable).
    pub fn save_to(&self, path: &Path) -> Result<()> {
        persistence::atomic_write_json(path, self)
            .with_context(|| format!("failed to persist setup state to {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verified(version: &str) -> StepEntry {
        StepEntry::new(StepStatus::Verified, true, version)
    }

    #[test]
    fn default_is_not_first_run_ready() {
        let state = SetupState::default();
        assert!(!state.first_run_ready());
        assert_eq!(state.constitution_choice, ConstitutionChoice::Unset);
    }

    #[test]
    fn persistence_is_optional_before_verification() {
        let persistence_index = SetupStep::ALL
            .iter()
            .position(|step| *step == SetupStep::Persistence)
            .expect("persistence step");
        let verification_index = SetupStep::ALL
            .iter()
            .position(|step| *step == SetupStep::Verification)
            .expect("verification step");

        assert!(persistence_index < verification_index);

        let mut state = SetupState::default();
        state.set_step(SetupStep::Language, verified("0.8.67"));
        state.set_step(SetupStep::ProviderModel, verified("0.8.67"));
        state.runtime_posture_source = RuntimePostureSource::Confirmed;
        state.constitution_choice = ConstitutionChoice::Bundled;
        assert!(state.first_run_ready());

        state.set_step(
            SetupStep::Persistence,
            StepEntry::new(StepStatus::NeedsAction, false, "0.8.67"),
        );
        assert!(state.first_run_ready());
        assert!(!state.operate_ready());
    }

    #[test]
    fn first_run_ready_requires_all_pillars() {
        let mut state = SetupState::default();
        state.set_step(SetupStep::Language, verified("0.8.67"));
        state.set_step(SetupStep::ProviderModel, verified("0.8.67"));
        state.runtime_posture_source = RuntimePostureSource::Confirmed;
        // Still missing an explicit constitution choice.
        assert!(!state.first_run_ready());
        state.constitution_choice = ConstitutionChoice::Bundled;
        assert!(state.first_run_ready());
    }

    #[test]
    fn operate_ready_is_separate_from_first_run_ready() {
        let mut state = SetupState::default();
        state.set_step(SetupStep::Language, verified("0.8.67"));
        state.set_step(SetupStep::ProviderModel, verified("0.8.67"));
        state.runtime_posture_source = RuntimePostureSource::Confirmed;
        state.constitution_choice = ConstitutionChoice::Bundled;
        assert!(state.first_run_ready());
        assert!(!state.operate_ready());

        state.set_step(SetupStep::OperateFleet, verified("0.8.67"));
        assert!(state.operate_ready());
    }

    #[test]
    fn operate_ready_requires_verified_provider_not_needs_action() {
        let mut state = SetupState::default();
        state.set_step(
            SetupStep::ProviderModel,
            StepEntry::new(StepStatus::NeedsAction, true, "0.8.67"),
        );
        state.runtime_posture_source = RuntimePostureSource::Confirmed;
        state.set_step(SetupStep::OperateFleet, verified("0.8.67"));

        assert!(!state.operate_ready());
    }

    #[test]
    fn needs_action_provider_still_reaches_ready() {
        let mut state = SetupState::default();
        state.set_step(SetupStep::Language, verified("0.8.67"));
        state.set_step(
            SetupStep::ProviderModel,
            StepEntry::new(StepStatus::NeedsAction, true, "0.8.67"),
        );
        state.runtime_posture_source = RuntimePostureSource::Inherited;
        state.constitution_choice = ConstitutionChoice::Deferred;
        assert!(state.first_run_ready());
    }

    #[test]
    fn deferred_constitution_counts_as_explicit_choice() {
        assert!(ConstitutionChoice::Deferred.is_explicit());
        assert!(ConstitutionChoice::Bundled.is_explicit());
        assert!(!ConstitutionChoice::Unset.is_explicit());
    }

    #[test]
    fn update_ready_tracks_checkpoint_version() {
        let mut state = SetupState::default();
        assert!(state.needs_constitution_checkpoint("0.8.67"));
        state.complete_constitution_checkpoint("0.8.67", ConstitutionChoice::Bundled);
        assert!(state.update_ready("0.8.67"));
        assert!(!state.needs_constitution_checkpoint("0.8.67"));
        // A later lane re-arms the checkpoint.
        assert!(state.needs_constitution_checkpoint("0.8.68"));
    }

    #[test]
    fn derive_inherited_marks_existing_user_safe() {
        let facts = InheritedConfigFacts {
            has_provider_route: true,
            has_credentials_or_local_runtime: true,
            trust_chosen: true,
            language: Some("en".to_string()),
            has_user_constitution: false,
            has_expert_override: false,
            user_constitution_validity: ConstitutionValidity::Unknown,
        };
        let state = SetupState::derive_inherited(&facts);
        assert!(state.inherited);
        assert_eq!(state.status(SetupStep::Language), StepStatus::Verified);
        assert_eq!(state.status(SetupStep::ProviderModel), StepStatus::Verified);
        assert_eq!(state.status(SetupStep::TrustSandbox), StepStatus::Verified);
        assert_eq!(state.constitution_source, ConstitutionSource::Bundled);
        // The update checkpoint must still be shown to an upgrading user.
        assert!(state.needs_constitution_checkpoint("0.8.67"));
    }

    #[test]
    fn derive_inherited_classifies_provider_without_key_as_needs_action() {
        let facts = InheritedConfigFacts {
            has_provider_route: true,
            has_credentials_or_local_runtime: false,
            ..InheritedConfigFacts::default()
        };
        let state = SetupState::derive_inherited(&facts);
        assert_eq!(
            state.status(SetupStep::ProviderModel),
            StepStatus::NeedsAction
        );
    }

    #[test]
    fn derive_inherited_picks_up_existing_user_constitution() {
        let facts = InheritedConfigFacts {
            has_user_constitution: true,
            user_constitution_validity: ConstitutionValidity::Valid,
            ..InheritedConfigFacts::default()
        };
        let state = SetupState::derive_inherited(&facts);
        assert_eq!(state.constitution_source, ConstitutionSource::UserGlobal);
        assert_eq!(state.constitution_choice, ConstitutionChoice::GuidedCustom);
        assert_eq!(state.constitution_validity, ConstitutionValidity::Valid);
    }

    #[test]
    fn round_trips_through_json_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SETUP_STATE_FILE_NAME);

        let mut state = SetupState::default();
        state.set_step(
            SetupStep::ProviderModel,
            verified("0.8.67").with_result("openai · mimo-ultraspeed"),
        );
        state.constitution_choice = ConstitutionChoice::GuidedCustom;
        state.constitution_preview_version = 3;
        state.save_to(&path).unwrap();

        let loaded = SetupState::load_from(&path).expect("record should load");
        assert_eq!(loaded, state);
        // Enum keys serialize as snake_case strings.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"provider_model\""), "{raw}");
        assert!(raw.contains("openai · mimo-ultraspeed"));
    }

    #[test]
    fn constitution_authoring_round_trips_and_stays_optional() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SETUP_STATE_FILE_NAME);

        let state = SetupState {
            constitution_choice: ConstitutionChoice::GuidedCustom,
            constitution_authoring: Some(ConstitutionAuthoring::ModelDrafted),
            ..Default::default()
        };
        state.save_to(&path).unwrap();

        let loaded = SetupState::load_from(&path).expect("record should load");
        assert_eq!(
            loaded.constitution_authoring,
            Some(ConstitutionAuthoring::ModelDrafted)
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"model_drafted\""), "{raw}");
    }

    #[test]
    fn record_without_authoring_field_still_loads() {
        // Records written before the model-drafting lane carry no
        // constitution_authoring key; they must load with None, not fail.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SETUP_STATE_FILE_NAME);
        std::fs::write(
            &path,
            r#"{"schema_version":1,"constitution_choice":"guided_custom"}"#,
        )
        .unwrap();
        let loaded = SetupState::load_from(&path).expect("legacy record should load");
        assert_eq!(loaded.constitution_authoring, None);
        assert_eq!(loaded.constitution_choice, ConstitutionChoice::GuidedCustom);
    }

    #[test]
    fn corrupt_record_falls_back_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SETUP_STATE_FILE_NAME);
        std::fs::write(&path, "{ not valid json").unwrap();
        assert!(SetupState::load_from(&path).is_none());
    }

    #[test]
    fn missing_record_is_none_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.json");
        assert!(SetupState::load_from(&path).is_none());
    }

    #[test]
    fn step_result_carries_no_secret_by_construction() {
        // The result field is a caller-supplied safe summary; this documents the
        // contract that callers pass names, not keys.
        let entry = verified("0.8.67").with_result("provider: openai, model: mimo");
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.to_lowercase().contains("sk-"));
    }
}
