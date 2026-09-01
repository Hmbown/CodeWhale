//! Provider-accepted Computer active-second receipts.
//!
//! **Scope: this module is not billing input.** The control plane in
//! codewhale-apps is the billing authority. It writes sandbox state intervals
//! itself and derives receipts server-side from them; it never accepts a
//! receipt from a client, correctly, because the CLI runs on the customer's
//! machine. Anything issued here is a local display or self-check.
//!
//! "Codewhale is the billing authority" below is a statement about *what is
//! billable*, not about which service decides: entitlement moves for
//! provider-accepted **active** seconds rather than Daytona's provisioned wall
//! clock. An audit read it as core-versus-control-plane and carried that
//! misreading into codewhale-apps, where it shaped a PR before it was caught.
//!
//! Codewhale is the billing authority over the *measure*. Daytona (or a future
//! adapter) supplies infrastructure only. Entitlement moves solely for
//! provider-accepted *active* seconds, per second, multiplied by the selected
//! profile's 1x/2x/4x rate.
//!
//! **Unwired as of 2026-09-01.** The only consumer, `cloud_dispatch::
//! meter_cloud_job`, is itself called only from `#[cfg(test)] mod tests`
//! (cloud_dispatch.rs:1210 and :1227, inside the module opening at :917). No
//! `ComputerMeterReceipt` is transmitted anywhere. This is a complete
//! implementation that was never connected; it must be deliberately wired as a
//! local self-check or deleted, because a dead module that reads as
//! authoritative is exactly how the drift above happened.
//!
//! Wall-clock-if-idle, requested, queued, rejected, stopped, suspended,
//! archived, failed-before-acceptance, and teardown-tail time cannot mint a
//! receipt. Provider-observed CPU/RAM/disk must equal the admitted profile.
//! Corrections are append-only and preserve the original receipt.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hashing::sha256_hex;

/// Pinned v3 meter revision. Bump only with a catalog owner change.
pub const COMPUTER_METER_REVISION: &str = "computer-meter-v3.20260831";
/// Pinned v3 profile catalog revision.
pub const COMPUTER_CATALOG_REVISION: &str = "computer-profiles-v3.20260831";
/// Admission record schema.
pub const COMPUTER_ADMISSION_SCHEMA: &str = "codewhale.computer-admission/v1";
/// Meter receipt schema.
pub const COMPUTER_METER_RECEIPT_SCHEMA: &str = "codewhale.computer-meter-receipt/v1";
/// Receipt kind for one accepted active interval.
pub const COMPUTER_METER_RECEIPT_KIND: &str = "computer.meter.active_seconds";

const ADMISSION_DIGEST_NS: &str = "codewhale/computer-admission/v1";
const RECEIPT_DIGEST_NS: &str = "codewhale/computer-meter-receipt/v1";
const MAX_REF_CHARS: usize = 240;

/// Ratified v3 launch profiles. Historic `standard`/`large`/`xl` decode only.
pub const COMPUTER_PROFILES: [ComputerProfile; 3] = [
    ComputerProfile {
        id: ComputerProfileId::Standard8,
        label: "8 GB",
        cpu: 2,
        memory_gib: 8,
        disk_gib: 8,
        multiplier: 1,
    },
    ComputerProfile {
        id: ComputerProfileId::Standard16,
        label: "16 GB",
        cpu: 4,
        memory_gib: 16,
        disk_gib: 16,
        multiplier: 2,
    },
    ComputerProfile {
        id: ComputerProfileId::Standard32,
        label: "32 GB",
        cpu: 8,
        memory_gib: 32,
        disk_gib: 32,
        multiplier: 4,
    },
];

/// Selectable, quotable, creatable v3 Computer profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComputerProfileId {
    /// 2 vCPU / 8 GiB RAM / 8 GiB disk at 1x.
    #[serde(rename = "standard-8")]
    Standard8,
    /// 4 vCPU / 16 GiB RAM / 16 GiB disk at 2x.
    #[serde(rename = "standard-16")]
    Standard16,
    /// 8 vCPU / 32 GiB RAM / 32 GiB disk at 4x.
    #[serde(rename = "standard-32")]
    Standard32,
}

impl ComputerProfileId {
    /// Stable catalog id.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard8 => "standard-8",
            Self::Standard16 => "standard-16",
            Self::Standard32 => "standard-32",
        }
    }
}

/// Fixed resource envelope and allowance multiplier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerProfile {
    /// Catalog id.
    pub id: ComputerProfileId,
    /// Customer label.
    pub label: &'static str,
    /// vCPU count.
    pub cpu: u32,
    /// RAM in GiB.
    #[serde(rename = "memoryGiB")]
    pub memory_gib: u32,
    /// Disk in GiB.
    #[serde(rename = "diskGiB")]
    pub disk_gib: u32,
    /// Standard-equivalent multiplier (1, 2, or 4).
    pub multiplier: u32,
}

/// How an interval claims to have been measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterBasis {
    /// Provider confirmed the allocation was actively running.
    ProviderAcceptedActive,
    /// Wall-clock elapsed time. Never entitlement.
    WallClock,
}

/// Immutable pre-dispatch binding. CWC remains the commercial owner; Engine
/// refuses to meter anything that is not this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerAdmission {
    /// Schema id.
    pub schema_id: String,
    /// Caller-supplied admission identity.
    pub admission_id: String,
    /// Account that funds the Computer.
    pub account_id: String,
    /// Durable Computer id.
    pub computer_id: String,
    /// Optional run this admission authorizes.
    #[serde(default)]
    pub run_id: String,
    /// Infrastructure provider (currently `daytona`).
    pub provider: String,
    /// Selected v3 profile.
    pub profile_id: ComputerProfileId,
    /// Bound vCPU.
    pub cpu: u32,
    /// Bound RAM GiB.
    #[serde(rename = "memoryGiB")]
    pub memory_gib: u32,
    /// Bound disk GiB.
    #[serde(rename = "diskGiB")]
    pub disk_gib: u32,
    /// Bound multiplier.
    pub multiplier: u32,
    /// Meter revision at bind time.
    pub meter_revision: String,
    /// Catalog revision at bind time.
    pub catalog_revision: String,
    /// Funding authority (membership included seconds or a time pack).
    pub funding_authority: String,
    /// Quote identity bound before dispatch.
    pub quote_id: String,
    /// Admission expiry (inclusive bound is refused).
    pub expires_at: String,
    /// Digest of the bound fields.
    pub binding_digest: String,
}

/// Provider-observed allocation at one instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAllocation {
    /// Provider name.
    pub provider: String,
    /// Provider sandbox / allocation id.
    pub provider_sandbox_id: String,
    /// Observed vCPU.
    pub cpu: u32,
    /// Observed RAM GiB.
    #[serde(rename = "memoryGiB", alias = "memoryGb")]
    pub memory_gib: u32,
    /// Observed disk GiB.
    #[serde(rename = "diskGiB", alias = "diskGb")]
    pub disk_gib: u32,
    /// Provider lifecycle state.
    pub state: String,
    /// Whether the provider accepted this as a live allocation.
    pub accepted: bool,
}

/// One closed provider observation used to mint a receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderObservation {
    /// Infrastructure provider.
    pub provider: String,
    /// Provider sandbox / allocation id.
    pub provider_sandbox_id: String,
    /// Idempotent provider event / interval reference.
    pub provider_event_ref: String,
    /// Provider lifecycle state.
    pub state: String,
    /// True when the allocation is allocated but idle.
    #[serde(default)]
    pub idle: bool,
    /// True only after the provider accepted the live allocation.
    #[serde(default)]
    pub provider_accepted: bool,
    /// Measurement basis. Wall-clock is never entitlement.
    pub meter_basis: MeterBasis,
    /// Observed vCPU.
    pub cpu: u32,
    /// Observed RAM GiB.
    #[serde(rename = "memoryGiB", alias = "memoryGb")]
    pub memory_gib: u32,
    /// Observed disk GiB.
    #[serde(rename = "diskGiB", alias = "diskGb")]
    pub disk_gib: u32,
    /// Interval start (inclusive).
    pub started_at: String,
    /// Interval end (exclusive).
    pub ended_at: String,
}

/// Immutable receipt for one provider-accepted active interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerMeterReceipt {
    /// Schema id.
    pub schema_id: String,
    /// Receipt identity derived from the bound work.
    pub receipt_id: String,
    /// Receipt kind.
    pub kind: String,
    /// Funding account.
    pub account_id: String,
    /// Run, when the interval is run-scoped.
    #[serde(default)]
    pub run_id: String,
    /// Computer id.
    pub computer_id: String,
    /// Admission this interval was authorized under.
    pub admission_id: String,
    /// Provider name.
    pub provider: String,
    /// Profile billed at.
    pub profile_id: ComputerProfileId,
    /// Multiplier billed at.
    pub multiplier: u32,
    /// Bound vCPU.
    pub cpu: u32,
    /// Bound RAM GiB.
    #[serde(rename = "memoryGiB")]
    pub memory_gib: u32,
    /// Bound disk GiB.
    #[serde(rename = "diskGiB")]
    pub disk_gib: u32,
    /// Meter revision.
    pub meter_revision: String,
    /// Catalog revision.
    pub catalog_revision: String,
    /// Funding authority copied from admission.
    pub funding_authority: String,
    /// Quote identity copied from admission.
    pub quote_id: String,
    /// Interval start.
    pub started_at: String,
    /// Interval end.
    pub ended_at: String,
    /// Provider-accepted active whole seconds.
    pub accepted_seconds: u64,
    /// `accepted_seconds * multiplier`.
    pub standard_equivalent_seconds: u64,
    /// Allocation snapshot the provider accepted.
    pub provider_allocation: ProviderAllocation,
    /// Provider event / interval reference.
    pub provider_event_ref: String,
    /// Prior receipt this exact replay matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_of: Option<String>,
    /// Original receipt this append-only correction restates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correction_of: Option<String>,
    /// Prior receipt ids in this lineage.
    #[serde(default)]
    pub lineage: Vec<String>,
    /// Digest of the bound receipt fields.
    pub binding_digest: String,
}

/// Inputs required to bind an admission before dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerAdmissionRequest {
    /// Caller-supplied admission identity.
    pub admission_id: String,
    /// Account that funds the Computer.
    pub account_id: String,
    /// Durable Computer id.
    pub computer_id: String,
    /// Optional run.
    #[serde(default)]
    pub run_id: String,
    /// Infrastructure provider.
    pub provider: String,
    /// Selected profile id (`standard-8` / `standard-16` / `standard-32`).
    pub profile_id: String,
    /// Funding authority.
    pub funding_authority: String,
    /// Quote identity.
    pub quote_id: String,
    /// Expiry timestamp.
    pub expires_at: String,
    /// Optional meter revision; must match v3 when supplied.
    #[serde(default)]
    pub meter_revision: String,
    /// Optional catalog revision; must match v3 when supplied.
    #[serde(default)]
    pub catalog_revision: String,
}

/// Fail-closed Computer meter errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ComputerMeterError {
    /// Profile id is not a v3 launch profile.
    #[error("{message}")]
    ProfileUnknown { message: String },
    /// Historic Large/XL/Auto/standard aliases cannot admit new work.
    #[error("{message}")]
    HistoricProfileNotAdmissible { message: String },
    /// Required identity field is missing or hostile.
    #[error("{message}")]
    ReferenceInvalid { message: String },
    /// Timestamp is not RFC 3339.
    #[error("{message}")]
    TimestampInvalid { message: String },
    /// Interval ends before it starts.
    #[error("A Computer meter interval cannot end before it starts.")]
    IntervalReversed,
    /// Admission has expired before the interval started.
    #[error("The Computer admission expired before this interval.")]
    AdmissionExpired,
    /// Meter or catalog revision is not the pinned v3 revision.
    #[error("{message}")]
    RevisionMismatch { message: String },
    /// Caller asked to meter wall-clock, including idle wall-clock.
    #[error(
        "Computer entitlement meters provider-accepted active seconds only, never wall-clock-if-idle."
    )]
    WallClockIdle,
    /// Interval is not a provider-accepted active state.
    #[error("{message}")]
    NotProviderAcceptedActive { message: String },
    /// Observed allocation is not exactly the admitted profile.
    #[error("{message}")]
    AllocationMismatch { message: String },
    /// Replay disagrees with the original bound receipt.
    #[error("This Computer meter receipt is already bound to different terms.")]
    ReplayConflict,
}

impl ComputerMeterError {
    /// Stable error code for tests and CWC.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProfileUnknown { .. } => "computer_profile_unknown",
            Self::HistoricProfileNotAdmissible { .. } => "computer_profile_historic_not_admissible",
            Self::ReferenceInvalid { .. } => "computer_meter_reference_invalid",
            Self::TimestampInvalid { .. } => "computer_meter_timestamp_invalid",
            Self::IntervalReversed => "computer_meter_interval_reversed",
            Self::AdmissionExpired => "computer_admission_expired",
            Self::RevisionMismatch { .. } => "computer_meter_revision_mismatch",
            Self::WallClockIdle => "computer_meter_wall_clock_idle",
            Self::NotProviderAcceptedActive { .. } => "computer_meter_not_provider_accepted_active",
            Self::AllocationMismatch { .. } => "computer_meter_allocation_mismatch",
            Self::ReplayConflict => "computer_meter_receipt_replay_conflict",
        }
    }
}

/// Decode a profile for reading historic or v3 ids. Does not authorize admission.
pub fn decode_computer_profile(input: &str) -> Result<ComputerProfile, ComputerMeterError> {
    match normalize_token(input).as_str() {
        "standard-8" | "standard" | "8" | "8gb" | "8gib" => Ok(COMPUTER_PROFILES[0]),
        "standard-16" | "large" | "16" | "16gb" | "16gib" => Ok(COMPUTER_PROFILES[1]),
        "standard-32" | "xl" | "32" | "32gb" | "32gib" => Ok(COMPUTER_PROFILES[2]),
        other => Err(ComputerMeterError::ProfileUnknown {
            message: format!("Unknown Computer profile: {other}."),
        }),
    }
}

/// Resolve a profile that may be used to create, quote, resume, or bill v3 work.
pub fn admit_computer_profile(input: &str) -> Result<ComputerProfile, ComputerMeterError> {
    let normalized = normalize_token(input);
    match normalized.as_str() {
        "standard-8" | "standard-16" | "standard-32" => decode_computer_profile(&normalized),
        "standard" | "large" | "xl" | "auto" => {
            Err(ComputerMeterError::HistoricProfileNotAdmissible {
                message: format!(
                    "Historic Computer profile `{normalized}` remains readable but cannot admit, resume, or meter new v3 work."
                ),
            })
        }
        other => Err(ComputerMeterError::ProfileUnknown {
            message: format!("Unknown Computer profile: {other}."),
        }),
    }
}

/// Bind provider, profile, resources, multiplier, revisions, account, funding,
/// quote, and expiry before dispatch.
pub fn bind_computer_admission(
    request: ComputerAdmissionRequest,
) -> Result<ComputerAdmission, ComputerMeterError> {
    let profile = admit_computer_profile(&request.profile_id)?;
    let admission_id = require_ref(&request.admission_id, "admissionId")?;
    let account_id = require_ref(&request.account_id, "accountId")?;
    let computer_id = require_ref(&request.computer_id, "computerId")?;
    let run_id = optional_ref(&request.run_id, "runId")?;
    let provider = require_ref(&request.provider, "provider")?;
    let funding_authority = require_ref(&request.funding_authority, "fundingAuthority")?;
    let quote_id = require_ref(&request.quote_id, "quoteId")?;
    let expires_at = normalize_timestamp(&request.expires_at, "expiresAt")?;
    let meter_revision = require_revision(&request.meter_revision, COMPUTER_METER_REVISION)?;
    let catalog_revision = require_revision(&request.catalog_revision, COMPUTER_CATALOG_REVISION)?;
    let bound = ComputerAdmission {
        schema_id: COMPUTER_ADMISSION_SCHEMA.to_string(),
        admission_id,
        account_id,
        computer_id,
        run_id,
        provider,
        profile_id: profile.id,
        cpu: profile.cpu,
        memory_gib: profile.memory_gib,
        disk_gib: profile.disk_gib,
        multiplier: profile.multiplier,
        meter_revision,
        catalog_revision,
        funding_authority,
        quote_id,
        expires_at,
        binding_digest: String::new(),
    };
    let binding_digest = admission_binding_digest(&bound);
    Ok(ComputerAdmission {
        binding_digest,
        ..bound
    })
}

/// Mint an immutable receipt for one provider-accepted active interval.
pub fn issue_computer_meter_receipt(
    admission: &ComputerAdmission,
    observation: ProviderObservation,
) -> Result<ComputerMeterReceipt, ComputerMeterError> {
    assert_active_observation(&observation)?;
    assert_allocation_matches(admission, &observation)?;
    let started_at = normalize_timestamp(&observation.started_at, "startedAt")?;
    let ended_at = normalize_timestamp(&observation.ended_at, "endedAt")?;
    let started = parse_timestamp(&started_at, "startedAt")?;
    let ended = parse_timestamp(&ended_at, "endedAt")?;
    if ended < started {
        return Err(ComputerMeterError::IntervalReversed);
    }
    let expires = parse_timestamp(&admission.expires_at, "expiresAt")?;
    if started >= expires {
        return Err(ComputerMeterError::AdmissionExpired);
    }
    let accepted_seconds = elapsed_whole_seconds(started, ended);
    let standard_equivalent_seconds =
        accepted_seconds.saturating_mul(u64::from(admission.multiplier));
    let provider = require_ref(&observation.provider, "provider")?;
    if provider != admission.provider {
        return Err(ComputerMeterError::AllocationMismatch {
            message: format!(
                "Provider `{}` does not match admitted provider `{}`.",
                provider, admission.provider
            ),
        });
    }
    let provider_sandbox_id = require_ref(&observation.provider_sandbox_id, "providerSandboxId")?;
    let provider_event_ref = require_ref(&observation.provider_event_ref, "providerEventRef")?;
    let allocation = ProviderAllocation {
        provider: provider.clone(),
        provider_sandbox_id: provider_sandbox_id.clone(),
        cpu: observation.cpu,
        memory_gib: observation.memory_gib,
        disk_gib: observation.disk_gib,
        state: normalize_token(&observation.state),
        accepted: true,
    };
    let mut receipt = ComputerMeterReceipt {
        schema_id: COMPUTER_METER_RECEIPT_SCHEMA.to_string(),
        receipt_id: String::new(),
        kind: COMPUTER_METER_RECEIPT_KIND.to_string(),
        account_id: admission.account_id.clone(),
        run_id: admission.run_id.clone(),
        computer_id: admission.computer_id.clone(),
        admission_id: admission.admission_id.clone(),
        provider,
        profile_id: admission.profile_id,
        multiplier: admission.multiplier,
        cpu: admission.cpu,
        memory_gib: admission.memory_gib,
        disk_gib: admission.disk_gib,
        meter_revision: admission.meter_revision.clone(),
        catalog_revision: admission.catalog_revision.clone(),
        funding_authority: admission.funding_authority.clone(),
        quote_id: admission.quote_id.clone(),
        started_at,
        ended_at,
        accepted_seconds,
        standard_equivalent_seconds,
        provider_allocation: allocation,
        provider_event_ref,
        replay_of: None,
        correction_of: None,
        lineage: Vec::new(),
        binding_digest: String::new(),
    };
    receipt.binding_digest = receipt_binding_digest(&receipt);
    receipt.receipt_id = receipt_id_for(&receipt);
    Ok(receipt)
}

/// Exact replay of an existing receipt. Identity and digest must match.
pub fn assert_computer_meter_receipt_replay(
    existing: &ComputerMeterReceipt,
    incoming: &ComputerMeterReceipt,
) -> Result<(), ComputerMeterError> {
    if existing.receipt_id == incoming.receipt_id
        && existing.binding_digest == incoming.binding_digest
        && existing.admission_id == incoming.admission_id
        && existing.account_id == incoming.account_id
        && existing.provider_event_ref == incoming.provider_event_ref
        && existing.accepted_seconds == incoming.accepted_seconds
        && existing.standard_equivalent_seconds == incoming.standard_equivalent_seconds
        && existing.profile_id == incoming.profile_id
        && existing.multiplier == incoming.multiplier
    {
        return Ok(());
    }
    Err(ComputerMeterError::ReplayConflict)
}

/// Append-only correction. The original receipt is not mutated.
pub fn correct_computer_meter_receipt(
    original: &ComputerMeterReceipt,
    admission: &ComputerAdmission,
    observation: ProviderObservation,
) -> Result<ComputerMeterReceipt, ComputerMeterError> {
    if original.admission_id != admission.admission_id
        || original.account_id != admission.account_id
    {
        return Err(ComputerMeterError::ReplayConflict);
    }
    let mut correction = issue_computer_meter_receipt(admission, observation)?;
    if correction.profile_id != original.profile_id || correction.multiplier != original.multiplier
    {
        return Err(ComputerMeterError::ReplayConflict);
    }
    correction.correction_of = Some(original.receipt_id.clone());
    correction.lineage = {
        let mut lineage = original.lineage.clone();
        lineage.push(original.receipt_id.clone());
        lineage
    };
    correction.binding_digest = receipt_binding_digest(&correction);
    correction.receipt_id = receipt_id_for(&correction);
    Ok(correction)
}

/// Sum Standard-equivalent seconds across independent Computer receipts.
#[must_use]
pub fn sum_standard_equivalent_seconds(receipts: &[ComputerMeterReceipt]) -> u64 {
    receipts
        .iter()
        .map(|receipt| receipt.standard_equivalent_seconds)
        .fold(0, u64::saturating_add)
}

fn assert_active_observation(observation: &ProviderObservation) -> Result<(), ComputerMeterError> {
    if observation.meter_basis != MeterBasis::ProviderAcceptedActive || observation.idle {
        return Err(ComputerMeterError::WallClockIdle);
    }
    if !observation.provider_accepted {
        return Err(ComputerMeterError::NotProviderAcceptedActive {
            message: "A Computer meter receipt requires a provider-accepted live allocation."
                .to_string(),
        });
    }
    let state = normalize_token(&observation.state);
    if !matches!(state.as_str(), "running" | "started" | "active") {
        return Err(ComputerMeterError::NotProviderAcceptedActive {
            message: format!(
                "Computer entitlement does not accrue in `{state}` (requested, queued, rejected, stopped, suspended, archived, failed-before-acceptance, and teardown-tail are excluded)."
            ),
        });
    }
    Ok(())
}

fn assert_allocation_matches(
    admission: &ComputerAdmission,
    observation: &ProviderObservation,
) -> Result<(), ComputerMeterError> {
    if observation.cpu == admission.cpu
        && observation.memory_gib == admission.memory_gib
        && observation.disk_gib == admission.disk_gib
    {
        return Ok(());
    }
    Err(ComputerMeterError::AllocationMismatch {
        message: format!(
            "Provider allocation {} vCPU / {} GiB RAM / {} GiB disk does not equal admitted profile {} ({} / {} / {}). Smaller is not a cost-saving substitution and larger is not an upgrade.",
            observation.cpu,
            observation.memory_gib,
            observation.disk_gib,
            admission.profile_id.as_str(),
            admission.cpu,
            admission.memory_gib,
            admission.disk_gib
        ),
    })
}

fn admission_binding_digest(admission: &ComputerAdmission) -> String {
    sha256_hex(
        [
            ADMISSION_DIGEST_NS,
            admission.admission_id.as_str(),
            admission.account_id.as_str(),
            admission.computer_id.as_str(),
            admission.run_id.as_str(),
            admission.provider.as_str(),
            admission.profile_id.as_str(),
            &admission.cpu.to_string(),
            &admission.memory_gib.to_string(),
            &admission.disk_gib.to_string(),
            &admission.multiplier.to_string(),
            admission.meter_revision.as_str(),
            admission.catalog_revision.as_str(),
            admission.funding_authority.as_str(),
            admission.quote_id.as_str(),
            admission.expires_at.as_str(),
        ]
        .join("\0"),
    )
}

fn receipt_binding_digest(receipt: &ComputerMeterReceipt) -> String {
    sha256_hex(
        [
            RECEIPT_DIGEST_NS,
            receipt.account_id.as_str(),
            receipt.run_id.as_str(),
            receipt.computer_id.as_str(),
            receipt.admission_id.as_str(),
            receipt.provider.as_str(),
            receipt.profile_id.as_str(),
            &receipt.multiplier.to_string(),
            &receipt.cpu.to_string(),
            &receipt.memory_gib.to_string(),
            &receipt.disk_gib.to_string(),
            receipt.meter_revision.as_str(),
            receipt.catalog_revision.as_str(),
            receipt.funding_authority.as_str(),
            receipt.quote_id.as_str(),
            receipt.started_at.as_str(),
            receipt.ended_at.as_str(),
            &receipt.accepted_seconds.to_string(),
            &receipt.standard_equivalent_seconds.to_string(),
            receipt.provider_event_ref.as_str(),
            receipt.provider_allocation.provider_sandbox_id.as_str(),
            &receipt.provider_allocation.cpu.to_string(),
            &receipt.provider_allocation.memory_gib.to_string(),
            &receipt.provider_allocation.disk_gib.to_string(),
            receipt.provider_allocation.state.as_str(),
            receipt.correction_of.as_deref().unwrap_or(""),
            &receipt.lineage.join(","),
        ]
        .join("\0"),
    )
}

fn receipt_id_for(receipt: &ComputerMeterReceipt) -> String {
    format!("cmr_{}", &receipt.binding_digest[..32])
}

fn elapsed_whole_seconds(started: DateTime<Utc>, ended: DateTime<Utc>) -> u64 {
    ended
        .signed_duration_since(started)
        .num_milliseconds()
        .max(0)
        .unsigned_abs()
        / 1000
}

fn require_revision(value: &str, expected: &str) -> Result<String, ComputerMeterError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Ok(expected.to_string());
    }
    if normalized == expected {
        return Ok(expected.to_string());
    }
    Err(ComputerMeterError::RevisionMismatch {
        message: format!("Computer meter revision `{normalized}` is not {expected}."),
    })
}

fn require_ref(value: &str, field: &str) -> Result<String, ComputerMeterError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.len() > MAX_REF_CHARS
        || normalized.chars().any(|ch| ch.is_control())
    {
        return Err(ComputerMeterError::ReferenceInvalid {
            message: format!(
                "Computer meter field `{field}` is required and must be a bounded token."
            ),
        });
    }
    Ok(normalized.to_string())
}

fn optional_ref(value: &str, field: &str) -> Result<String, ComputerMeterError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Ok(String::new());
    }
    require_ref(normalized, field)
}

fn normalize_token(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_timestamp(value: &str, field: &str) -> Result<String, ComputerMeterError> {
    Ok(parse_timestamp(value, field)?.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn parse_timestamp(value: &str, field: &str) -> Result<DateTime<Utc>, ComputerMeterError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| ComputerMeterError::TimestampInvalid {
            message: format!("Computer meter field `{field}` must be an RFC 3339 timestamp."),
        })
}

#[cfg(test)]
mod tests;
