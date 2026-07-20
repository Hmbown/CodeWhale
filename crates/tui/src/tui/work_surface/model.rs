use std::collections::HashSet;
use std::fmt::Write as _;

use ratatui::layout::Rect;

use crate::tui::app::{App, SidebarRowAction};
use crate::work_graph::{
    AcceptanceRequirement, EdgeKind, EvidenceKind, EvidenceKindTag, NodeKind, NodeState,
    OperationBinding, OwnerState, Provenance, WorkGraphSnapshot, WorkNode,
};

/// Persisted Ocean work-surface placement. Bottom is deliberately absent: the
/// composer and phase footer own the shell's lower edge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkSurfacePlacement {
    #[default]
    Top,
    Left,
    Right,
}

impl WorkSurfacePlacement {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" => Self::Left,
            "right" => Self::Right,
            _ => Self::Top,
        }
    }

    #[must_use]
    pub const fn as_setting(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkRowId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkTone {
    Heading,
    Live,
    Attention,
    Success,
    Muted,
}

#[derive(Debug, Clone)]
pub(super) struct WorkRow {
    pub id: WorkRowId,
    pub mark: &'static str,
    pub label: String,
    pub detail: String,
    pub tone: WorkTone,
    pub selectable: bool,
    pub primary_action: Option<SidebarRowAction>,
}

#[derive(Debug, Clone)]
pub(super) struct WorkHitbox {
    pub id: WorkRowId,
    pub row_y: u16,
}

#[derive(Debug, Clone)]
enum WorkSourceState {
    Empty,
    Error(String),
    Disconnected,
}

impl WorkSourceState {
    const fn label(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Error(_) => "error",
            Self::Disconnected => "disconnected",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Empty => "No graph-owned work in the active session",
            Self::Error(error) => error,
            Self::Disconnected => "Work Graph runtime is not attached",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkSurfaceState {
    pub placement: WorkSurfacePlacement,
    pub(super) effective_placement: WorkSurfacePlacement,
    /// Focus owner axis — distinct from selection and detail-open.
    pub focused: bool,
    /// Keyboard/mouse selection highlight.
    pub selected: Option<WorkRowId>,
    /// Which row currently owns an open detail (pager / agent card).
    pub opened: Option<WorkRowId>,
    pub scroll_offset: usize,
    pub last_area: Option<Rect>,
    pub visible_rows: usize,
    pub total_rows: usize,
    pub(super) hovered: Option<WorkRowId>,
    pub(super) hitboxes: Vec<WorkHitbox>,
    pub(super) cached_graph: Option<WorkGraphSnapshot>,
    pub(super) latest_rows: Vec<WorkRow>,
}

impl Default for WorkSurfaceState {
    fn default() -> Self {
        Self::with_placement(WorkSurfacePlacement::Top)
    }
}

impl WorkSurfaceState {
    #[must_use]
    pub fn with_placement(placement: WorkSurfacePlacement) -> Self {
        Self {
            placement,
            effective_placement: placement,
            focused: false,
            selected: None,
            opened: None,
            scroll_offset: 0,
            last_area: None,
            visible_rows: 0,
            total_rows: 0,
            hovered: None,
            hitboxes: Vec::new(),
            cached_graph: None,
            latest_rows: Vec::new(),
        }
    }

    pub(super) fn selected_index(&self, rows: &[WorkRow]) -> Option<usize> {
        self.selected
            .as_ref()
            .and_then(|selected| rows.iter().position(|row| &row.id == selected))
    }

    pub(super) fn clamp_selection(&mut self, rows: &[WorkRow]) {
        let selectable = rows.iter().filter(|row| row.selectable).collect::<Vec<_>>();
        if selectable.is_empty() {
            self.selected = None;
            self.focused = false;
            self.scroll_offset = 0;
            return;
        }
        if !selectable
            .iter()
            .any(|row| Some(&row.id) == self.selected.as_ref())
        {
            self.selected = Some(selectable[0].id.clone());
        }
        let selected = self.selected_index(rows).unwrap_or_default();
        if selected < self.scroll_offset {
            self.scroll_offset = selected;
        } else if self.visible_rows > 0
            && selected >= self.scroll_offset.saturating_add(self.visible_rows)
        {
            self.scroll_offset = selected.saturating_add(1).saturating_sub(self.visible_rows);
        }
        self.scroll_offset = self
            .scroll_offset
            .min(rows.len().saturating_sub(self.visible_rows.max(1)));
    }
}

pub(super) fn project(app: &mut App) -> Vec<WorkRow> {
    let active_session = app.current_session_id.is_some();
    let capture = app.runtime_services.work.as_ref().map(|work| {
        work.try_capture(app.current_session_id.as_deref())
            .map(|snapshot| snapshot.map(|snapshot| snapshot.graph))
    });

    let (graph, source_state) = match capture {
        Some(Ok(Some(graph))) => {
            app.work_surface.cached_graph = Some(graph.clone());
            (Some(graph), None)
        }
        Some(Ok(None)) => {
            app.work_surface.cached_graph = None;
            (None, active_session.then_some(WorkSourceState::Empty))
        }
        Some(Err(error)) => (
            app.work_surface.cached_graph.clone(),
            active_session.then_some(WorkSourceState::Error(error)),
        ),
        None => (
            app.work_surface.cached_graph.clone(),
            active_session.then_some(WorkSourceState::Disconnected),
        ),
    };

    let rows = match graph {
        Some(graph) => graph_rows(&graph, source_state.as_ref()),
        None => source_state.map_or_else(Vec::new, |state| {
            vec![heading(
                &format!("Work · {}", state.label()),
                state.detail(),
            )]
        }),
    };
    app.work_surface.latest_rows = rows.clone();
    if let Some(opened) = app.work_surface.opened.as_ref()
        && !rows.iter().any(|row| &row.id == opened)
    {
        app.work_surface.opened = None;
    }
    rows
}

fn graph_rows(
    snapshot: &WorkGraphSnapshot,
    source_state: Option<&WorkSourceState>,
) -> Vec<WorkRow> {
    let visible = snapshot
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                NodeKind::PlanStep | NodeKind::Operation | NodeKind::Blocker
            )
        })
        .collect::<Vec<_>>();
    let running = visible
        .iter()
        .filter(|node| matches!(node.state, NodeState::Initializing | NodeState::Active))
        .count();
    let waiting = visible
        .iter()
        .filter(|node| node.state == NodeState::Waiting)
        .count();
    let ready = visible
        .iter()
        .filter(|node| node.state == NodeState::Ready)
        .count();
    let blocked = visible
        .iter()
        .filter(|node| node_is_attention(node))
        .count();
    let status = source_state
        .map(|state| format!(" · {} · cached r{}", state.label(), snapshot.revision))
        .unwrap_or_default();
    let detail = source_state.map_or_else(
        || format!("graph revision {}", snapshot.revision),
        |state| format!("graph revision {} · {}", snapshot.revision, state.detail()),
    );
    let waiting = if waiting > 0 {
        format!(" · {waiting} waiting")
    } else {
        String::new()
    };
    let mut rows = vec![heading(
        &format!("Work · {running} running{waiting} · {ready} ready · {blocked} blocked{status}"),
        &detail,
    )];
    rows.extend(
        visible
            .into_iter()
            .map(|node| graph_node_row(snapshot, node)),
    );
    rows
}

fn heading(label: &str, detail: &str) -> WorkRow {
    WorkRow {
        id: WorkRowId("section:work".to_string()),
        mark: "▾",
        label: label.to_string(),
        detail: detail.to_string(),
        tone: WorkTone::Heading,
        selectable: false,
        primary_action: None,
    }
}

fn graph_node_row(snapshot: &WorkGraphSnapshot, node: &WorkNode) -> WorkRow {
    let (mark, tone) = match node.state {
        NodeState::Ready => (crate::tui::glyphs::READY, WorkTone::Muted),
        NodeState::Initializing => (crate::tui::glyphs::SELECTION, WorkTone::Live),
        NodeState::Active => (crate::tui::glyphs::SELECTION, WorkTone::Live),
        NodeState::Waiting => (crate::tui::glyphs::ATTENTION, WorkTone::Attention),
        NodeState::Blocked => ("!", WorkTone::Attention),
        NodeState::Completed if node.acceptance.is_empty() => {
            (crate::tui::glyphs::DONE, WorkTone::Success)
        }
        NodeState::Completed => ("!", WorkTone::Attention),
        NodeState::Verified => (crate::tui::glyphs::DONE, WorkTone::Success),
        NodeState::Stale => ("?", WorkTone::Attention),
        NodeState::Superseded | NodeState::Cancelled => ("−", WorkTone::Muted),
        NodeState::Failed => (crate::tui::glyphs::FAILED, WorkTone::Attention),
    };
    let state = state_label(node);
    let kind = kind_label(node.kind);
    let stop_action = node
        .state
        .is_live()
        .then(|| stop_action(node.binding.as_ref()))
        .flatten();
    WorkRow {
        id: WorkRowId(format!("graph:{}", node.id.as_str())),
        mark,
        label: node.title.clone(),
        detail: format!("{state} · {kind}"),
        tone,
        selectable: true,
        primary_action: Some(SidebarRowAction::InspectWork {
            title: format!("Work · {}", node.title),
            body: inspector_text(snapshot, node),
            stop_action: stop_action.map(Box::new),
        }),
    }
}

fn node_is_attention(node: &WorkNode) -> bool {
    matches!(
        node.state,
        NodeState::Blocked | NodeState::Stale | NodeState::Failed
    ) || (node.state == NodeState::Completed && !node.acceptance.is_empty())
}

fn state_label(node: &WorkNode) -> &'static str {
    match node.state {
        NodeState::Ready => "ready",
        NodeState::Initializing => "initializing",
        NodeState::Active => "running",
        NodeState::Waiting => "waiting",
        NodeState::Blocked => "blocked",
        NodeState::Completed if node.acceptance.is_empty() => "completed",
        NodeState::Completed => "completed · evidence pending",
        NodeState::Verified => "verified",
        NodeState::Stale => "stale",
        NodeState::Superseded => "superseded",
        NodeState::Cancelled => "cancelled",
        NodeState::Failed => "failed",
    }
}

const fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Objective => "objective",
        NodeKind::PlanStep => "plan step",
        NodeKind::Operation => "operation",
        NodeKind::Evidence => "evidence",
        NodeKind::Blocker => "blocker",
        NodeKind::Approval => "approval",
        NodeKind::RuntimeRef => "runtime",
        NodeKind::LaneRef => "lane",
    }
}

fn stop_action(binding: Option<&OperationBinding>) -> Option<SidebarRowAction> {
    let binding = binding?;
    if let Some(id) = binding.external.strip_prefix("task:") {
        Some(SidebarRowAction::Command(format!("/task cancel {id}")))
    } else if let Some(id) = binding.external.strip_prefix("shell:") {
        Some(SidebarRowAction::Command(format!("/jobs cancel {id}")))
    } else if let Some(id) = binding.external.strip_prefix("worker:") {
        Some(SidebarRowAction::CancelAgent {
            agent_id: id.to_string(),
        })
    } else {
        binding
            .external
            .strip_prefix("workflow:")
            .map(|id| SidebarRowAction::Command(format!("/workflow cancel {id}")))
    }
}

fn inspector_text(snapshot: &WorkGraphSnapshot, node: &WorkNode) -> String {
    let mut out = String::new();
    section_text(
        &mut out,
        "Objective",
        objective_for(snapshot, node)
            .as_deref()
            .unwrap_or("Not connected"),
    );
    section_list(
        &mut out,
        "Prerequisites",
        related_nodes(snapshot, node, EdgeKind::DependsOn, true),
    );
    section_text(
        &mut out,
        "Current",
        &format!("{} · {}", state_label(node), kind_label(node.kind)),
    );
    section_list(
        &mut out,
        "Downstream impact",
        related_nodes(snapshot, node, EdgeKind::DependsOn, false),
    );
    section_text(&mut out, "Binding + lifecycle owner", &binding_text(node));
    section_text(
        &mut out,
        "Evidence vs acceptance",
        &evidence_text(snapshot, node),
    );
    section_text(
        &mut out,
        "Blockers / approvals",
        &blockers_approvals_text(snapshot, node),
    );
    section_text(&mut out, "Why next", &why_next(snapshot, node));
    section_text(
        &mut out,
        "Provenance + last reconcile",
        &provenance_text(node),
    );
    if node.state == NodeState::Stale {
        section_text(
            &mut out,
            "Last bounded output",
            last_output_ref(snapshot, node)
                .as_deref()
                .unwrap_or("No output receipt"),
        );
    }
    out.trim_end().to_string()
}

fn objective_for(snapshot: &WorkGraphSnapshot, node: &WorkNode) -> Option<String> {
    if node.kind == NodeKind::Objective {
        return Some(node.title.clone());
    }
    let mut current = node.id.clone();
    let mut seen = HashSet::new();
    while seen.insert(current.clone()) {
        let Some(parent) = snapshot.edges.iter().find_map(|edge| {
            (edge.kind == EdgeKind::Contains && edge.to == current).then(|| edge.from.clone())
        }) else {
            break;
        };
        let Some(parent_node) = snapshot.node(&parent) else {
            break;
        };
        if parent_node.kind == NodeKind::Objective {
            return Some(parent_node.title.clone());
        }
        current = parent;
    }
    snapshot.compat.plan.objective.clone()
}

fn related_nodes(
    snapshot: &WorkGraphSnapshot,
    node: &WorkNode,
    kind: EdgeKind,
    outgoing: bool,
) -> Vec<String> {
    snapshot
        .edges
        .iter()
        .filter(|edge| edge.kind == kind)
        .filter_map(|edge| {
            let related = if outgoing && edge.from == node.id {
                Some(&edge.to)
            } else if !outgoing && edge.to == node.id {
                Some(&edge.from)
            } else {
                None
            }?;
            snapshot
                .node(related)
                .map(|related| format!("{} · {}", related.title, state_label(related)))
        })
        .collect()
}

fn binding_text(node: &WorkNode) -> String {
    let Some(binding) = node.binding.as_ref() else {
        return "Not bound".to_string();
    };
    let mut text = format!(
        "Owner: {}\nDurable: {}",
        binding.external,
        if binding.durable { "yes" } else { "no" }
    );
    if let Some(observation) = binding.last_observation.as_ref() {
        let owner_state = match observation.owner_state {
            OwnerState::Initializing => "initializing",
            OwnerState::Running => "running",
            OwnerState::Waiting => "waiting",
            OwnerState::Completed => "completed",
            OwnerState::Failed => "failed",
            OwnerState::Cancelled => "cancelled",
        };
        let _ = write!(
            text,
            "\nLast owner state: {owner_state}\nLast reconcile: {} ms UTC · sequence {}",
            observation.observed_at, observation.seq
        );
    } else {
        text.push_str("\nLast reconcile: never");
    }
    text
}

fn evidence_text(snapshot: &WorkGraphSnapshot, node: &WorkNode) -> String {
    let acceptance = if node.acceptance.is_empty() {
        vec!["- No evidence requirement".to_string()]
    } else {
        node.acceptance
            .iter()
            .map(|requirement| format!("- {}", acceptance_label(requirement)))
            .collect()
    };
    let evidence = evidence_for(snapshot, node);
    let evidence = if evidence.is_empty() {
        vec!["- None attached".to_string()]
    } else {
        evidence
            .into_iter()
            .map(|evidence| {
                let reference = evidence
                    .evidence
                    .as_ref()
                    .map(|item| item.reference())
                    .unwrap_or("invalid evidence node");
                format!("- {reference} · {}", state_label(evidence))
            })
            .collect()
    };
    format!(
        "Acceptance:\n{}\nEvidence:\n{}",
        acceptance.join("\n"),
        evidence.join("\n")
    )
}

fn acceptance_label(requirement: &AcceptanceRequirement) -> String {
    match requirement {
        AcceptanceRequirement::EvidenceOfKind { kind } => {
            let kind = match kind {
                EvidenceKindTag::ToolRun => "tool run",
                EvidenceKindTag::Artifact => "artifact",
                EvidenceKindTag::TestSummary => "test summary",
                EvidenceKindTag::Receipt => "receipt",
                EvidenceKindTag::Approval => "approval",
                EvidenceKindTag::Route => "route",
                EvidenceKindTag::WebCitation => "web citation",
            };
            format!("evidence of kind {kind}")
        }
    }
}

fn evidence_for<'a>(snapshot: &'a WorkGraphSnapshot, node: &WorkNode) -> Vec<&'a WorkNode> {
    snapshot
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Verifies && edge.to == node.id)
        .filter_map(|edge| snapshot.node(&edge.from))
        .collect()
}

fn blockers_approvals_text(snapshot: &WorkGraphSnapshot, node: &WorkNode) -> String {
    let mut lines = Vec::new();
    lines.extend(
        related_nodes(snapshot, node, EdgeKind::Blocks, false)
            .into_iter()
            .map(|item| format!("- Blocked by {item}")),
    );
    lines.extend(
        related_nodes(snapshot, node, EdgeKind::RequiresApproval, true)
            .into_iter()
            .map(|item| format!("- Approval {item}")),
    );
    if node.kind == NodeKind::PlanStep {
        lines.extend(
            snapshot
                .nodes
                .iter()
                .filter(|candidate| candidate.kind == NodeKind::Approval)
                .map(|approval| format!("- {} · {}", approval.title, state_label(approval))),
        );
    }
    if lines.is_empty() {
        "None".to_string()
    } else {
        lines.join("\n")
    }
}

fn why_next(snapshot: &WorkGraphSnapshot, node: &WorkNode) -> String {
    match node.state {
        NodeState::Ready => {
            let pending = related_nodes(snapshot, node, EdgeKind::DependsOn, true);
            if pending.is_empty() {
                "Ready with no recorded prerequisite".to_string()
            } else {
                format!("Ready after: {}", pending.join(", "))
            }
        }
        NodeState::Initializing => "Spawn intent is registered; awaiting owner handle".to_string(),
        NodeState::Active => "Lifecycle owner reports active work".to_string(),
        NodeState::Waiting => "Waiting on an owner or approval".to_string(),
        NodeState::Blocked => "Blocked; resolve the causes above".to_string(),
        NodeState::Completed if !node.acceptance.is_empty() => {
            "Execution ended, but acceptance evidence is still missing".to_string()
        }
        NodeState::Stale => "Owner cannot confirm liveness after reconciliation".to_string(),
        NodeState::Verified => "Acceptance evidence is satisfied".to_string(),
        NodeState::Completed => "Completed with no evidence requirement".to_string(),
        NodeState::Superseded => "A replacement node owns this work".to_string(),
        NodeState::Cancelled => "Cancelled by lifecycle owner".to_string(),
        NodeState::Failed => "Failed; inspect owner output before retrying".to_string(),
    }
}

fn provenance_text(node: &WorkNode) -> String {
    let provenance = match &node.provenance {
        Provenance::Import { ordinal, .. } => ordinal
            .map(|ordinal| format!("legacy import · ordinal {ordinal}"))
            .unwrap_or_else(|| "legacy import".to_string()),
        Provenance::ToolUpdate { tool, call_id } => {
            format!("tool {tool} · call {call_id}")
        }
        Provenance::RuntimeReconcile {
            source,
            observed_at,
        } => format!("runtime {source} · {observed_at} ms UTC"),
        Provenance::UserEdit { proposal_id } => format!("user-approved diff {proposal_id}"),
    };
    let reconcile = node
        .binding
        .as_ref()
        .and_then(|binding| binding.last_observation.as_ref())
        .map(|observation| format!("{} ms UTC", observation.observed_at))
        .unwrap_or_else(|| "never".to_string());
    format!("Source: {provenance}\nLast reconcile: {reconcile}")
}

fn last_output_ref(snapshot: &WorkGraphSnapshot, node: &WorkNode) -> Option<String> {
    node.binding
        .as_ref()
        .and_then(|binding| binding.last_observation.as_ref())
        .and_then(|observation| observation.output.as_ref())
        .map(format_evidence_ref)
        .or_else(|| {
            evidence_for(snapshot, node)
                .into_iter()
                .max_by_key(|evidence| evidence.updated_at)
                .and_then(|evidence| evidence.evidence.as_ref())
                .map(format_evidence_ref)
        })
}

fn format_evidence_ref(evidence: &crate::work_graph::EvidenceRef) -> String {
    let kind = match evidence.kind() {
        EvidenceKind::ToolRun => "tool run".to_string(),
        EvidenceKind::Artifact { .. } => "artifact".to_string(),
        EvidenceKind::TestSummary => "test summary".to_string(),
        EvidenceKind::Receipt { .. } => "receipt".to_string(),
        EvidenceKind::Approval => "approval".to_string(),
        EvidenceKind::Route => "route".to_string(),
        EvidenceKind::WebCitation {
            url, retrieved_at, ..
        } => format!("web citation · {url} · retrieved {retrieved_at}"),
    };
    let bytes = evidence
        .raw_bytes()
        .map(|bytes| format!(" · {bytes} raw bytes"))
        .unwrap_or_default();
    let truncation = if evidence.truncated() {
        " · truncated"
    } else {
        ""
    };
    format!("{} · {kind}{bytes}{truncation}", evidence.reference())
}

fn section_text(out: &mut String, title: &str, body: &str) {
    let _ = writeln!(out, "{title}\n{body}\n");
}

fn section_list(out: &mut String, title: &str, items: Vec<String>) {
    if items.is_empty() {
        section_text(out, title, "None");
    } else {
        section_text(
            out,
            title,
            &items
                .into_iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_graph::{OperationBinding, WorkNodeId};

    fn operation(state: NodeState, suffix: &str) -> WorkNode {
        WorkNode {
            id: WorkNodeId::derive("work-surface-test", suffix),
            kind: NodeKind::Operation,
            title: format!("operation {suffix}"),
            state,
            acceptance: Vec::new(),
            binding: Some(OperationBinding {
                external: format!("shell:{suffix}"),
                durable: false,
                last_observation: None,
            }),
            evidence: None,
            provenance: Provenance::ToolUpdate {
                tool: "test".to_string(),
                call_id: suffix.to_string(),
            },
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn heading_counts_initializing_and_active_operations_as_running() {
        let mut snapshot = WorkGraphSnapshot::new();
        snapshot.nodes = vec![
            operation(NodeState::Initializing, "initializing"),
            operation(NodeState::Active, "active"),
            operation(NodeState::Ready, "ready"),
        ];

        let rows = graph_rows(&snapshot, None);

        assert_eq!(
            rows.first().map(|row| row.label.as_str()),
            Some("Work · 2 running · 1 ready · 0 blocked")
        );
    }
}
