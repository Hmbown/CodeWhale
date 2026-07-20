use std::collections::HashSet;
use std::fmt::Write as _;

use ratatui::layout::Rect;

use crate::localization::MessageId;
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

    /// Keep row identity and the viewport offset valid without moving the
    /// viewport to the remembered keyboard selection. Mouse-wheel scrolling
    /// is allowed to leave that selection off-screen until keyboard
    /// navigation resumes.
    pub(super) fn clamp_viewport(&mut self, rows: &[WorkRow]) {
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
        self.scroll_offset = self
            .scroll_offset
            .min(rows.len().saturating_sub(self.visible_rows.max(1)));
    }

    /// Reveal the remembered selection after keyboard navigation. Rendering
    /// alone must use `clamp_viewport`; otherwise every redraw undoes a mouse
    /// wheel offset when the selection is above the viewport.
    pub(super) fn clamp_selection(&mut self, rows: &[WorkRow]) {
        self.clamp_viewport(rows);
        let Some(selected) = self.selected_index(rows) else {
            return;
        };
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
    let todo_label = app.tr(MessageId::SidebarTodoLabel).into_owned();
    let todo_progress = app.tr(MessageId::WorkSurfaceTodoProgress).into_owned();
    let plan_label = app.tr(MessageId::AppModePlan).into_owned();
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

    let mut rows = match graph {
        Some(graph) => graph_rows(
            &graph,
            source_state.as_ref(),
            &todo_label,
            &todo_progress,
            &plan_label,
        ),
        None => source_state.map_or_else(Vec::new, |state| {
            vec![section_heading(
                "work",
                &format!("Work · {}", state.label()),
                state.detail(),
            )]
        }),
    };
    let attention_rows = pending_attention_rows(app);
    if !attention_rows.is_empty() {
        let count = attention_rows.len();
        let count_label = app
            .tr(MessageId::WorkSurfaceNeedsInputCount)
            .replace("{count}", &count.to_string());
        let detail = app.tr(MessageId::WorkSurfaceNeedsInputDetail).into_owned();
        if rows.is_empty() {
            rows.push(section_heading(
                "work",
                &format!("Work · {count_label}"),
                &detail,
            ));
        } else if let Some(heading) = rows.first_mut() {
            if heading.label == "Work · empty" {
                heading.label = format!("Work · {count_label}");
                heading.detail = detail;
            } else {
                heading.label.push_str(&format!(" · {count_label}"));
            }
        }
        rows.splice(1..1, attention_rows);
    }
    app.work_surface.latest_rows = rows.clone();
    if let Some(opened) = app.work_surface.opened.as_ref()
        && !rows.iter().any(|row| &row.id == opened)
    {
        app.work_surface.opened = None;
    }
    rows
}

fn pending_attention_rows(app: &App) -> Vec<WorkRow> {
    let mut rows = Vec::with_capacity(2);
    let waiting_detail = app.tr(MessageId::WorkSurfaceDecisionWaiting).into_owned();
    if let Some(request) = app.pending_work_approval.as_ref() {
        rows.push(WorkRow {
            id: WorkRowId(format!("approval:{}", request.id)),
            mark: "?",
            label: app
                .tr(MessageId::WorkSurfaceReviewTool)
                .replace("{tool}", &request.tool_name),
            detail: waiting_detail.clone(),
            tone: WorkTone::Attention,
            selectable: true,
            primary_action: Some(SidebarRowAction::ReviewApproval {
                tool_id: request.id.clone(),
            }),
        });
    }
    if let Some((tool_id, request)) = app.pending_user_input_prompt.as_ref() {
        let question = request
            .questions
            .first()
            .map(|question| compact_attention_label(&question.question))
            .filter(|question| !question.is_empty())
            .unwrap_or_else(|| app.tr(MessageId::WorkSurfaceQuestionFallback).into_owned());
        rows.push(WorkRow {
            id: WorkRowId(format!("question:{tool_id}")),
            mark: "?",
            label: question,
            detail: waiting_detail,
            tone: WorkTone::Attention,
            selectable: true,
            primary_action: Some(SidebarRowAction::AnswerUserInput {
                tool_id: tool_id.clone(),
            }),
        });
    }
    rows
}

fn compact_attention_label(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn graph_rows(
    snapshot: &WorkGraphSnapshot,
    source_state: Option<&WorkSourceState>,
    todo_label: &str,
    todo_progress: &str,
    plan_label: &str,
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
        .filter(|node| !is_settled_transient_operation(node))
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
    let mut rows = vec![section_heading(
        "work",
        &format!("Work · {running} running{waiting} · {ready} ready · {blocked} blocked{status}"),
        &detail,
    )];

    // Runtime operations and blockers are live/attention state. Cleanly
    // settled non-durable operations are graph receipts, not permanent chrome,
    // and were filtered above. Keep the remaining operational rows first.
    rows.extend(
        visible
            .iter()
            .copied()
            .filter(|node| node.kind != NodeKind::PlanStep)
            .map(|node| graph_node_row(snapshot, node)),
    );

    // The Work Graph is authoritative, but the explicit To-do grouping is a
    // useful projection contract. Preserve compat ordering and do not flatten
    // durable checklist rows into transient shell activity.
    let mut rendered_plan_nodes = HashSet::new();
    let todo_nodes = snapshot
        .compat
        .todos
        .iter()
        .filter_map(|binding| snapshot.node(&binding.node))
        .filter(|node| visible.iter().any(|candidate| candidate.id == node.id))
        .collect::<Vec<_>>();
    if !todo_nodes.is_empty() {
        let completed = todo_nodes
            .iter()
            .filter(|node| matches!(node.state, NodeState::Completed | NodeState::Verified))
            .count();
        let total = todo_nodes.len();
        let progress = todo_progress
            .replace("{label}", todo_label)
            .replace("{completed}", &completed.to_string())
            .replace("{total}", &total.to_string())
            .replace("{remaining}", &total.saturating_sub(completed).to_string());
        rows.push(section_heading("todo", &progress, todo_label));
        for node in todo_nodes {
            rendered_plan_nodes.insert(node.id.clone());
            rows.push(graph_node_row(snapshot, node));
        }
    }

    // Plan-only steps remain visible without duplicating nodes that also back
    // the To-do compatibility projection.
    let mut strategy_nodes = snapshot
        .compat
        .plan_order
        .iter()
        .filter(|id| !rendered_plan_nodes.contains(*id))
        .filter_map(|id| snapshot.node(id))
        .filter(|node| visible.iter().any(|candidate| candidate.id == node.id))
        .collect::<Vec<_>>();
    for node in visible
        .iter()
        .copied()
        .filter(|node| node.kind == NodeKind::PlanStep)
    {
        if !rendered_plan_nodes.contains(&node.id)
            && !strategy_nodes
                .iter()
                .any(|candidate| candidate.id == node.id)
        {
            strategy_nodes.push(node);
        }
    }
    if !strategy_nodes.is_empty() {
        let completed = strategy_nodes
            .iter()
            .filter(|node| matches!(node.state, NodeState::Completed | NodeState::Verified))
            .count();
        rows.push(section_heading(
            "strategy",
            &format!("{plan_label} {completed}/{}", strategy_nodes.len()),
            plan_label,
        ));
        rows.extend(
            strategy_nodes
                .into_iter()
                .map(|node| graph_node_row(snapshot, node)),
        );
    }
    rows
}

fn is_settled_transient_operation(node: &WorkNode) -> bool {
    node.kind == NodeKind::Operation
        && node
            .binding
            .as_ref()
            .is_some_and(|binding| !binding.durable)
        && match node.state {
            NodeState::Completed => node.acceptance.is_empty(),
            NodeState::Verified | NodeState::Superseded | NodeState::Cancelled => true,
            _ => false,
        }
}

fn section_heading(id: &str, label: &str, detail: &str) -> WorkRow {
    WorkRow {
        id: WorkRowId(format!("section:{id}")),
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
    use crate::work_graph::{CompatTodoBinding, OperationBinding, WorkNodeId};

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

        let rows = graph_rows(
            &snapshot,
            None,
            "To-do",
            "{label} {completed}/{total} · {remaining} left",
            "Plan",
        );

        assert_eq!(
            rows.first().map(|row| row.label.as_str()),
            Some("Work · 2 running · 1 ready · 0 blocked")
        );
    }

    #[test]
    fn live_projection_hides_clean_transient_receipts_and_restores_todo_group() {
        let todo_id = WorkNodeId::derive("work-surface-test", "todo:1");
        let todo = WorkNode {
            id: todo_id.clone(),
            kind: NodeKind::PlanStep,
            title: "Keep the durable checklist visible".to_string(),
            state: NodeState::Ready,
            acceptance: Vec::new(),
            binding: None,
            evidence: None,
            provenance: Provenance::ToolUpdate {
                tool: "work_update".to_string(),
                call_id: "todo-1".to_string(),
            },
            created_at: 1,
            updated_at: 1,
        };
        let mut snapshot = WorkGraphSnapshot::new();
        snapshot.nodes = vec![
            operation(NodeState::Completed, "settled"),
            operation(NodeState::Active, "running"),
            todo,
        ];
        snapshot.compat.todos.push(CompatTodoBinding {
            legacy_id: 1,
            node: todo_id,
            plan_index: None,
        });

        let rows = graph_rows(
            &snapshot,
            None,
            "To-do",
            "{label} {completed}/{total} · {remaining} left",
            "Plan",
        );
        let labels = rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>();

        assert!(labels.contains(&"operation running"), "{labels:?}");
        assert!(!labels.contains(&"operation settled"), "{labels:?}");
        assert!(labels.contains(&"To-do 0/1 · 1 left"), "{labels:?}");
        assert!(
            labels.contains(&"Keep the durable checklist visible"),
            "{labels:?}"
        );
        assert!(
            snapshot
                .nodes
                .iter()
                .any(|node| node.title == "operation settled"),
            "projection filtering must retain the historical graph receipt"
        );
    }

    #[test]
    fn projection_keeps_durable_and_attention_terminal_operations() {
        let mut durable = operation(NodeState::Completed, "durable");
        durable.binding.as_mut().expect("binding").durable = true;
        let failed = operation(NodeState::Failed, "failed");
        let mut evidence_pending = operation(NodeState::Completed, "evidence-pending");
        evidence_pending.acceptance = vec![AcceptanceRequirement::EvidenceOfKind {
            kind: EvidenceKindTag::ToolRun,
        }];
        let mut snapshot = WorkGraphSnapshot::new();
        snapshot.nodes = vec![durable, failed, evidence_pending];

        let rows = graph_rows(
            &snapshot,
            None,
            "To-do",
            "{label} {completed}/{total} · {remaining} left",
            "Plan",
        );
        let labels = rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "operation durable",
            "operation failed",
            "operation evidence-pending",
        ] {
            assert!(labels.contains(&expected), "missing {expected}: {labels:?}");
        }
    }
}
