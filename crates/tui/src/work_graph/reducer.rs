//! The only write path for the work graph.
//!
//! [`apply`] is pure and deterministic: no clock reads, no RNG, no I/O — the
//! caller-supplied [`ChangeCtx`] carries timestamps and session identity, and
//! every derived ID comes from SHA-256 over `(session_id, discriminator)`.
//! The same snapshot + change + ctx always produce byte-identical results.
//!
//! Contract per change:
//! 1. idempotency-key duplicates are acknowledged as no-op receipts;
//! 2. the change is applied to a copy;
//! 3. the candidate is validated fail-closed — on any violation the input
//!    snapshot is untouched and the caller gets the full report;
//! 4. the revision increments exactly once;
//! 5. the receipt is pushed onto the bounded history.
//!
//! Callers on `Ok`: derive compat snapshots → validate combined → persist →
//! publish projections (publish AFTER the persist enqueue, never before).
//! On `Err`: surface the report; no state changed.

use super::events::{
    CancelOutcome, ChangeCtx, ChangeReceipt, ObservationSummary, OperationObservation, OwnerState,
    WorkGraphChange, WorkGraphProposal, WorkNodePatch,
};
use super::ids::{ChangeId, WorkEdgeId, WorkNodeId};
use super::model::{
    EdgeKind, EvidenceRef, NodeKind, NodeState, OperationBinding, Provenance, WorkEdge,
    WorkGraphSnapshot, WorkNode,
};
use super::validate::{ValidationCode, ValidationReport, validate};

/// Apply one change, producing the next snapshot and a receipt, or a
/// validation report with the input snapshot untouched.
pub fn apply(
    g: &WorkGraphSnapshot,
    change: WorkGraphChange,
    ctx: ChangeCtx,
) -> Result<(WorkGraphSnapshot, ChangeReceipt), ValidationReport> {
    if let Some(key) = &ctx.idempotency_key
        && g.seen_keys.contains(key)
    {
        // Duplicate runtime event: acknowledge without effect.
        let receipt = ChangeReceipt {
            change_id: ChangeId::derive(
                &ctx.session_id,
                &format!("noop:{}:{}", key.binding.as_str(), key.seq),
            ),
            revision: g.revision,
            summary: format!("{} (duplicate)", change.kind_name()),
            applied_at: ctx.now,
            idempotency_key: Some(key.clone()),
            no_op: true,
        };
        return Ok((g.clone(), receipt));
    }

    let mut next = apply_pure(g, &change, &ctx)?;
    validate(&next)?; // fail closed: `g` unchanged on Err
    next.revision = g.revision + 1; // exactly once
    let receipt = ChangeReceipt::of(&change, next.revision, &ctx);
    next.history.push_bounded(receipt.clone());
    if let Some(key) = ctx.idempotency_key {
        next.seen_keys.insert(key);
    }
    Ok((next, receipt))
}

fn structural(message: impl Into<String>) -> ValidationReport {
    ValidationReport::single(ValidationCode::Structural, message)
}

fn apply_pure(
    g: &WorkGraphSnapshot,
    change: &WorkGraphChange,
    ctx: &ChangeCtx,
) -> Result<WorkGraphSnapshot, ValidationReport> {
    let mut next = g.clone();
    match change {
        WorkGraphChange::AddNode { node } => {
            add_node(&mut next, node.clone())?;
        }
        WorkGraphChange::UpdateNode { id, patch } => {
            patch_node(&mut next, id, patch, ctx.now)?;
        }
        WorkGraphChange::AddEdge { edge } => {
            add_edge(&mut next, edge.clone())?;
        }
        WorkGraphChange::RemoveEdge { id } => {
            let before = next.edges.len();
            next.edges.retain(|e| &e.id != id);
            if next.edges.len() == before {
                return Err(structural(format!("edge {id} not found")));
            }
        }
        WorkGraphChange::BindOperation { node, binding } => {
            let now = ctx.now;
            let target = next
                .node_mut(node)
                .ok_or_else(|| structural(format!("node {node} not found")))?;
            target.binding = Some(binding.clone());
            target.updated_at = now;
        }
        WorkGraphChange::ReconcileOperation { node, obs } => {
            reconcile(&mut next, node, obs, ctx)?;
        }
        WorkGraphChange::AttachEvidence { node, evidence } => {
            attach_evidence(&mut next, node, evidence, ctx)?;
        }
        WorkGraphChange::ProposePlanDiff { proposal } => {
            if next.proposals.iter().any(|p| p.id == proposal.id) {
                return Err(structural(format!("duplicate proposal {}", proposal.id)));
            }
            validate_proposal(&next, proposal, ctx)?;
            next.proposals.push(proposal.clone());
        }
        WorkGraphChange::WithdrawPlanDiff { proposal_id } => {
            let before = next.proposals.len();
            next.proposals
                .retain(|proposal| proposal.id != *proposal_id);
            if next.proposals.len() == before {
                return Err(structural(format!("proposal {proposal_id} not found")));
            }
        }
        WorkGraphChange::AcceptPlanDiff {
            proposal_id,
            approval,
        } => {
            let index = next
                .proposals
                .iter()
                .position(|p| p.id == *proposal_id)
                .ok_or_else(|| structural(format!("proposal {proposal_id} not found")))?;
            let proposal = next.proposals.remove(index);
            apply_proposal(&mut next, &proposal, ctx)?;
            // Record the acceptance as an Approval node so the review action
            // itself is part of the graph's history.
            let approval_node = WorkNode {
                id: WorkNodeId::derive(
                    &ctx.session_id,
                    &format!("approval:{}", proposal_id.as_str()),
                ),
                kind: NodeKind::Approval,
                title: format!("plan diff approved: {}", approval.reference),
                state: NodeState::Completed,
                acceptance: Vec::new(),
                binding: None,
                evidence: None,
                provenance: Provenance::UserEdit {
                    proposal_id: proposal_id.clone(),
                },
                created_at: ctx.now,
                updated_at: ctx.now,
            };
            add_node(&mut next, approval_node)?;
        }
        WorkGraphChange::Supersede { old, replacement } => {
            if old == replacement {
                return Err(structural("node cannot supersede itself"));
            }
            if next.node(replacement).is_none() {
                return Err(structural(format!(
                    "replacement node {replacement} not found"
                )));
            }
            let now = ctx.now;
            {
                let old_node = next
                    .node_mut(old)
                    .ok_or_else(|| structural(format!("node {old} not found")))?;
                // Explicit supersede is the sanctioned way past V9's
                // terminal-state protection.
                old_node.state = NodeState::Superseded;
                old_node.updated_at = now;
            }
            let edge = WorkEdge {
                id: WorkEdgeId::derive(
                    &ctx.session_id,
                    &format!("supersedes:{}:{}", replacement.as_str(), old.as_str()),
                ),
                kind: EdgeKind::Supersedes,
                from: replacement.clone(),
                to: old.clone(),
            };
            add_edge(&mut next, edge)?;
        }
        WorkGraphChange::ReplaceCompatProjection { compat } => {
            next.compat = compat.clone();
        }
        WorkGraphChange::SetImportDigest { digest } => {
            if digest.is_empty() {
                return Err(structural("legacy import digest cannot be empty"));
            }
            next.import_digest = Some(digest.clone());
        }
    }
    Ok(next)
}

fn add_node(next: &mut WorkGraphSnapshot, node: WorkNode) -> Result<(), ValidationReport> {
    if next.node(&node.id).is_some() {
        return Err(structural(format!("duplicate node {}", node.id)));
    }
    next.nodes.push(node);
    Ok(())
}

fn add_edge(next: &mut WorkGraphSnapshot, edge: WorkEdge) -> Result<(), ValidationReport> {
    if next.edge(&edge.id).is_some() {
        return Err(structural(format!("duplicate edge {}", edge.id)));
    }
    for endpoint in [&edge.from, &edge.to] {
        if next.node(endpoint).is_none() {
            return Err(structural(format!(
                "edge {} references missing node {endpoint}",
                edge.id
            )));
        }
    }
    next.edges.push(edge);
    Ok(())
}

/// V9 at the write path: terminal states are never overwritten by patches;
/// only the explicit `Supersede` change (or a reconcile-rule change) may move
/// a node out of a terminal state.
fn patch_node(
    next: &mut WorkGraphSnapshot,
    id: &WorkNodeId,
    patch: &WorkNodePatch,
    now: i64,
) -> Result<(), ValidationReport> {
    let node = next
        .node_mut(id)
        .ok_or_else(|| structural(format!("node {id} not found")))?;
    if node.state.is_terminal() && patch.state.is_some() {
        return Err(ValidationReport::single(
            ValidationCode::V9,
            format!(
                "node {id} is terminal ({:?}); use Supersede to replace it",
                node.state
            ),
        ));
    }
    if let Some(title) = &patch.title {
        node.title = title.clone();
    }
    if let Some(state) = patch.state {
        node.state = state;
    }
    if let Some(acceptance) = &patch.acceptance {
        node.acceptance = acceptance.clone();
    }
    if let Some(provenance) = &patch.provenance {
        node.provenance = provenance.clone();
    }
    node.updated_at = now;
    Ok(())
}

/// Pure application of an owner observation. The owner is authoritative for
/// lifecycle; the graph never invents liveness:
/// - a missing owner maps to `Stale` — NEVER to Active or Completed;
/// - a terminal owner lifecycle maps to `Completed` — never `Verified`
///   (verification only ever comes from evidence, V4);
/// - nodes already in a terminal state keep it (V9); only the observation
///   summary is updated.
fn reconcile(
    next: &mut WorkGraphSnapshot,
    id: &WorkNodeId,
    obs: &OperationObservation,
    ctx: &ChangeCtx,
) -> Result<(), ValidationReport> {
    let now = ctx.now;
    let node = next
        .node_mut(id)
        .ok_or_else(|| structural(format!("node {id} not found")))?;
    let binding = node
        .binding
        .as_mut()
        .ok_or_else(|| structural(format!("node {id} has no operation binding")))?;

    let new_state = match obs {
        OperationObservation::OwnerReported { state, seq, at } => {
            binding.last_observation = Some(ObservationSummary {
                owner_state: *state,
                seq: *seq,
                observed_at: *at,
            });
            Some(match state {
                OwnerState::Running => NodeState::Active,
                OwnerState::Waiting => NodeState::Waiting,
                OwnerState::Completed => NodeState::Completed,
                OwnerState::Failed => NodeState::Failed,
                OwnerState::Cancelled => NodeState::Cancelled,
            })
        }
        OperationObservation::OwnerMissing { .. } => Some(NodeState::Stale),
        OperationObservation::CancelUpdate { outcome, .. } => match outcome {
            // In-flight acknowledgements: record only, no state claim yet.
            CancelOutcome::Requested | CancelOutcome::Acknowledged => None,
            CancelOutcome::Forced => Some(NodeState::Cancelled),
            CancelOutcome::AlreadyFinished => Some(NodeState::Completed),
            CancelOutcome::NotFound | CancelOutcome::StaleUnknown => Some(NodeState::Stale),
        },
    };
    if let Some(state) = new_state
        && !node.state.is_terminal()
    {
        node.state = state;
    }
    node.updated_at = now;
    Ok(())
}

/// Materialize evidence as an Evidence node plus a `Verifies` edge onto the
/// target, both with deterministically derived IDs (so the same evidence
/// reference attaches at most once — a repeat is a structural rejection, not
/// a duplicate node).
fn attach_evidence(
    next: &mut WorkGraphSnapshot,
    target: &WorkNodeId,
    evidence: &EvidenceRef,
    ctx: &ChangeCtx,
) -> Result<(), ValidationReport> {
    if next.node(target).is_none() {
        return Err(structural(format!("node {target} not found")));
    }
    let evidence_id = WorkNodeId::derive(
        &ctx.session_id,
        &format!("evidence:{}:{}", target.as_str(), evidence.reference()),
    );
    let node = WorkNode {
        id: evidence_id.clone(),
        kind: NodeKind::Evidence,
        title: format!("evidence: {}", evidence.reference()),
        state: NodeState::Completed,
        acceptance: Vec::new(),
        binding: None,
        evidence: Some(evidence.clone()),
        provenance: Provenance::RuntimeReconcile {
            source: "attach_evidence".to_string(),
            observed_at: ctx.now,
        },
        created_at: ctx.now,
        updated_at: ctx.now,
    };
    add_node(next, node)?;
    let edge = WorkEdge {
        id: WorkEdgeId::derive(
            &ctx.session_id,
            &format!("verifies:{}:{}", evidence_id.as_str(), target.as_str()),
        ),
        kind: EdgeKind::Verifies,
        from: evidence_id,
        to: target.clone(),
    };
    add_edge(next, edge)
}

/// Apply an accepted proposal atomically: nodes first (so added edges may
/// reference them), then edges, then patches, then removals. Any failure
/// rejects the whole acceptance (the caller's snapshot stays untouched).
fn apply_proposal(
    next: &mut WorkGraphSnapshot,
    proposal: &WorkGraphProposal,
    ctx: &ChangeCtx,
) -> Result<(), ValidationReport> {
    for node in &proposal.added_nodes {
        add_node(next, node.clone())?;
    }
    for edge in &proposal.added_edges {
        add_edge(next, edge.clone())?;
    }
    for update in &proposal.updated_nodes {
        patch_node(next, &update.id, &update.patch, ctx.now)?;
    }
    for edge_id in &proposal.removed_edges {
        let before = next.edges.len();
        next.edges.retain(|e| &e.id != edge_id);
        if next.edges.len() == before {
            return Err(structural(format!("edge {edge_id} not found")));
        }
    }
    for node_id in &proposal.removed_nodes {
        if next
            .edges
            .iter()
            .any(|edge| edge.from == *node_id || edge.to == *node_id)
        {
            return Err(structural(format!(
                "node {node_id} still has edges after proposed removals"
            )));
        }
        let before = next.nodes.len();
        next.nodes.retain(|node| node.id != *node_id);
        if next.nodes.len() == before {
            return Err(structural(format!("node {node_id} not found")));
        }
    }
    if let Some(compat) = &proposal.replacement_compat {
        next.compat.clone_from(compat);
    }
    Ok(())
}

/// Reject malformed or invariant-breaking plan edits before they become
/// user-reviewable. Acceptance reruns the same atomic application and the
/// outer reducer validation, so reviewed and accepted semantics cannot drift.
fn validate_proposal(
    current: &WorkGraphSnapshot,
    proposal: &WorkGraphProposal,
    ctx: &ChangeCtx,
) -> Result<(), ValidationReport> {
    preview_plan_diff(current, proposal, ctx).map(|_| ())
}

pub(super) fn preview_plan_diff(
    current: &WorkGraphSnapshot,
    proposal: &WorkGraphProposal,
    ctx: &ChangeCtx,
) -> Result<WorkGraphSnapshot, ValidationReport> {
    let mut candidate = current.clone();
    apply_proposal(&mut candidate, proposal, ctx)?;
    validate(&candidate)?;
    Ok(candidate)
}
