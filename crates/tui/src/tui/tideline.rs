//! Read-only projections for the Tideline terminal workbench contract.
//!
//! [`App`] remains the sole owner of runtime state. This module neither
//! replaces it nor introduces another settings store, event loop, or engine;
//! it gives render and input code typed snapshots of facts that existing
//! owners have already resolved.

use ratatui::layout::{Position, Rect};

use crate::tui::app::App;

/// A bounded view of the context window currently owned by the active route.
///
/// Percent is stored in basis points (`10_000 == 100%`) so snapshots remain
/// equality-testable without making renderers compare floating-point values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudgetSnapshot {
    pub used_tokens: u32,
    pub max_tokens: u32,
    pub percent_basis_points: u16,
}

impl ContextBudgetSnapshot {
    /// Project the existing context estimator without becoming a second
    /// context-budget owner.
    #[must_use]
    pub(crate) fn from_app(app: &App) -> Option<Self> {
        let (used, max_tokens, _) = crate::tui::ui::context_usage_snapshot(app)?;
        let used_tokens = u32::try_from(used.max(0))
            .unwrap_or(u32::MAX)
            .min(max_tokens);
        let percent_basis_points = if max_tokens == 0 {
            0
        } else {
            let numerator = u64::from(used_tokens).saturating_mul(10_000);
            let rounded = numerator
                .saturating_add(u64::from(max_tokens) / 2)
                .checked_div(u64::from(max_tokens))
                .unwrap_or(0)
                .min(10_000);
            u16::try_from(rounded).unwrap_or(10_000)
        };

        Some(Self {
            used_tokens,
            max_tokens,
            percent_basis_points,
        })
    }
}

/// Pod capacity as the topbar's `pod n/m` segment states it: live workers
/// over the configured maximum, plus the known-member count the ledger
/// lists (wiring manifest `header.pod` — state: SubAgentManager snapshot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PodCapacitySnapshot {
    /// Workers in the water right now (running sub-agents).
    pub live: usize,
    /// Configured worker maximum for this session.
    pub max: usize,
    /// Members the manager knows about this session, completed included.
    pub known_members: usize,
}

impl PodCapacitySnapshot {
    /// Project the sub-agent manager's TUI-side cache without becoming a
    /// second owner of worker state. `live` unions the running cache entries
    /// with progress-only workers exactly like the segment builder does.
    #[must_use]
    pub(crate) fn from_app(app: &App) -> Self {
        let live = crate::tui::subagent_routing::running_agent_count(app);
        Self {
            live,
            max: app.max_subagents.max(1),
            known_members: app.subagent_cache.len(),
        }
    }

    /// The segment renders once a pod is or was active this session.
    #[must_use]
    pub const fn is_active(self) -> bool {
        self.live > 0 || self.known_members > 0
    }
}

/// Attention inbox counts for the topbar's notifications segment (wiring
/// manifest `header.notifications` — state: the session's notification
/// records). Gold is reserved for unseen action-required attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttentionInboxSnapshot {
    /// Records currently retained in the inbox.
    pub records: usize,
    /// Unseen records that ask the user for a decision or answer.
    pub unseen_attention: usize,
    /// Unseen records of any kind.
    pub unseen: usize,
}

impl AttentionInboxSnapshot {
    #[must_use]
    pub(crate) fn from_app(app: &App) -> Self {
        let read_at = app.notifications_read_at;
        let mut records = 0usize;
        let mut unseen = 0usize;
        let mut unseen_attention = 0usize;
        for record in &app.notification_records {
            records += 1;
            let unseen_record = read_at.is_none_or(|seen| record.at > seen);
            if unseen_record {
                unseen += 1;
                if record.requires_attention() {
                    unseen_attention += 1;
                }
            }
        }
        Self {
            records,
            unseen_attention,
            unseen,
        }
    }

    /// The segment paints only when the inbox holds something this session.
    #[must_use]
    pub const fn is_active(self) -> bool {
        self.records > 0
    }

    /// Gold only for genuine attention: an unseen record that asks the user
    /// to act (approval / question / sandbox elevation).
    #[must_use]
    pub const fn demands_attention(self) -> bool {
        self.unseen_attention > 0
    }
}

/// The owner whose value currently wins for a setting fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "later settings surfaces consume the non-session authority variants"
)]
pub enum SettingAuthority {
    Session,
    UserSettings,
    WorkspaceConfiguration,
    ManagedPolicy,
}

/// When an edit to a setting becomes observable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "later settings editors consume the non-current apply variants"
)]
pub enum SettingApplySemantics {
    EffectiveNow,
    Immediate,
    NextSession,
    RestartRequired,
    ReadOnly,
}

/// One setting without collapsing live, resolved, startup, and persisted
/// values into an ambiguous `Session`/`Saved` label.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "the settings view consumes this projection in the next Tideline slice"
)]
pub struct SettingFact<T> {
    /// Value currently held by the live owner before further resolution.
    pub current: Option<T>,
    /// Value actually in force after route, policy, or session overrides.
    pub effective: Option<T>,
    /// Value a fresh session is expected to start with, when observed.
    pub startup: Option<T>,
    /// Exact persisted value last read from its owning store, when observed.
    pub saved: Option<T>,
    pub authority: SettingAuthority,
    pub apply: SettingApplySemantics,
}

#[allow(
    dead_code,
    reason = "the settings view consumes this projection in the next Tideline slice"
)]
impl<T: Clone> SettingFact<T> {
    /// A fact already owned by the active session.
    #[must_use]
    pub fn active_session(value: T) -> Self {
        Self {
            current: Some(value.clone()),
            effective: Some(value),
            startup: None,
            saved: None,
            authority: SettingAuthority::Session,
            apply: SettingApplySemantics::EffectiveNow,
        }
    }
}

/// The narrow, read-only workbench projection available in this slice.
///
/// `App` intentionally does not retain a resident [`crate::settings::Settings`]
/// value. Consequently this projection never reloads disk or guesses startup
/// defaults: those lanes remain `None` until the settings owner supplies them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "the composed workbench consumes this projection in the next Tideline slice"
)]
pub struct UiSnapshot {
    pub context_budget: Option<ContextBudgetSnapshot>,
    pub provider: SettingFact<String>,
    pub model: SettingFact<String>,
}

#[allow(
    dead_code,
    reason = "the composed workbench consumes this projection in the next Tideline slice"
)]
impl UiSnapshot {
    #[must_use]
    pub(crate) fn from_app(app: &App) -> Self {
        let (provider, model) = app.effective_route_identity_display();
        Self {
            context_budget: ContextBudgetSnapshot::from_app(app),
            provider: SettingFact::active_session(provider),
            model: SettingFact::active_session(model),
        }
    }
}

/// Stable identifier from the Tideline wiring manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InteractionTargetId(&'static str);

impl InteractionTargetId {
    pub const HEADER_CONTEXT: Self = Self("header.context");
    pub const HEADER_POD: Self = Self("header.pod");
    pub const HEADER_NOTIFICATIONS: Self = Self("header.notifications");
}

/// Typed destination shared by keyboard and mouse input routes. The shared
/// `Inspect` prefix is the grammar, not an accident: every topbar segment's
/// action opens an inspector over state the renderer already holds (wiring
/// manifest `header.*` rows all say "action opens the ...").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // the prefix is the contract, not noise
pub enum InteractionAction {
    InspectContext,
    /// Open the pod ledger — the workers register over `SubAgentResult[]`
    /// (wiring manifest `header.pod` / `pod.ledger`).
    InspectPod,
    /// Open the notification center — the attention inbox over the
    /// session's notification records (wiring manifest
    /// `header.notifications`).
    InspectNotifications,
}

/// Focus metadata for a selectable target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "ordered focus traversal lands with the later multi-target surfaces"
)]
pub enum InteractionFocus {
    /// The target has a direct keyboard shortcut but is not in traversal yet.
    Direct,
    /// The target participates in ordered focus traversal.
    Traversable { order: u16, focused: bool },
}

/// Typed, non-prose evidence made available to an inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectDetail {
    ContextBudget(ContextBudgetSnapshot),
    PodCapacity(PodCapacitySnapshot),
    AttentionInbox(AttentionInboxSnapshot),
}

/// A selectable region painted in the current frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionTarget {
    pub id: InteractionTargetId,
    pub area: Rect,
    pub focus: InteractionFocus,
    pub keyboard_action: Option<InteractionAction>,
    pub mouse_action: Option<InteractionAction>,
    pub inspect_detail: InspectDetail,
}

/// Frame-scoped interaction geometry.
///
/// Targets are cleared before every render. Hit testing runs newest-first so a
/// later modal or overlay can safely own cells also covered by a lower layer.
#[derive(Debug, Default)]
pub struct InteractionRegistry {
    targets: Vec<InteractionTarget>,
}

impl InteractionRegistry {
    pub fn clear(&mut self) {
        self.targets.clear();
    }

    pub fn register(&mut self, target: InteractionTarget) {
        if target.area.width > 0 && target.area.height > 0 {
            self.targets.push(target);
        }
    }

    #[must_use]
    pub fn target_at(&self, column: u16, row: u16) -> Option<&InteractionTarget> {
        let position = Position::new(column, row);
        self.targets
            .iter()
            .rev()
            .find(|target| target.area.contains(position))
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &InteractionTarget> {
        self.targets.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttentionInboxSnapshot, ContextBudgetSnapshot, InspectDetail, InteractionAction,
        InteractionFocus, InteractionRegistry, InteractionTarget, InteractionTargetId,
        PodCapacitySnapshot, SettingApplySemantics, SettingAuthority, SettingFact, UiSnapshot,
    };
    use crate::config::ApiProvider;
    use ratatui::layout::Rect;

    fn target(area: Rect, used_tokens: u32) -> InteractionTarget {
        InteractionTarget {
            id: InteractionTargetId::HEADER_CONTEXT,
            area,
            focus: InteractionFocus::Direct,
            keyboard_action: Some(InteractionAction::InspectContext),
            mouse_action: Some(InteractionAction::InspectContext),
            inspect_detail: InspectDetail::ContextBudget(ContextBudgetSnapshot {
                used_tokens,
                max_tokens: 10_000,
                percent_basis_points: 3_000,
            }),
        }
    }

    #[test]
    fn ui_snapshot_uses_active_route_without_claiming_saved_defaults() {
        let mut app =
            crate::test_support::test_app_with_options(crate::test_support::test_tui_options("."));
        app.pending_turn_route = Some((ApiProvider::Zai, "GLM-5.3".to_string(), false));

        let snapshot = UiSnapshot::from_app(&app);

        assert_eq!(
            snapshot.provider.current.as_deref(),
            Some(ApiProvider::Zai.display_name())
        );
        assert_eq!(snapshot.provider.current, snapshot.provider.effective);
        assert_eq!(snapshot.model.current.as_deref(), Some("GLM-5.3"));
        assert_eq!(snapshot.model.current, snapshot.model.effective);
        assert!(snapshot.provider.startup.is_none());
        assert!(snapshot.provider.saved.is_none());
        assert_eq!(snapshot.provider.authority, SettingAuthority::Session);
        assert_eq!(snapshot.provider.apply, SettingApplySemantics::EffectiveNow);
    }

    #[test]
    fn context_budget_projection_reuses_and_bounds_the_existing_estimate() {
        let app =
            crate::test_support::test_app_with_options(crate::test_support::test_tui_options("."));
        let (used, max_tokens, _) =
            crate::tui::ui::context_usage_snapshot(&app).expect("existing context estimate");
        let snapshot = ContextBudgetSnapshot::from_app(&app).expect("Tideline projection");

        assert_eq!(snapshot.used_tokens, u32::try_from(used).unwrap());
        assert_eq!(snapshot.max_tokens, max_tokens);
        assert!(snapshot.used_tokens <= snapshot.max_tokens);
        assert!(snapshot.percent_basis_points <= 10_000);
    }

    #[test]
    fn attention_inbox_snapshot_reserves_gold_for_unseen_asks_only() {
        let mut app =
            crate::test_support::test_app_with_options(crate::test_support::test_tui_options("."));
        let empty = AttentionInboxSnapshot::from_app(&app);
        assert!(!empty.is_active(), "quiet session paints no segment");
        assert!(!empty.demands_attention());

        // One ask + one completion: only the ask is attention, and only
        // while unseen.
        app.record_notification_payload(
            &crate::tui::notifications::NotificationPayload::approval_needed(
                "Approval needed",
                "bash",
            ),
        );
        app.record_notification_payload(
            &crate::tui::notifications::NotificationPayload::turn_complete("Turn complete"),
        );
        let inbox = AttentionInboxSnapshot::from_app(&app);
        assert_eq!(inbox.records, 2);
        assert_eq!(inbox.unseen, 2);
        assert_eq!(inbox.unseen_attention, 1);
        assert!(inbox.is_active());
        assert!(inbox.demands_attention());

        app.mark_notifications_read();
        let read = AttentionInboxSnapshot::from_app(&app);
        assert_eq!(read.unseen, 0);
        assert_eq!(read.unseen_attention, 0);
        assert!(read.is_active(), "records stay inspectable after reading");
        assert!(!read.demands_attention(), "read asks are not gold");
    }

    #[test]
    fn pod_capacity_snapshot_activates_on_any_session_pod_history() {
        let mut app =
            crate::test_support::test_app_with_options(crate::test_support::test_tui_options("."));
        let idle = PodCapacitySnapshot::from_app(&app);
        assert!(!idle.is_active(), "no pod, no segment");
        assert_eq!(idle.max, app.max_subagents.max(1));

        // A completed worker keeps the capacity fact alive this session
        // (pod is/was active), with zero live.
        app.subagent_cache
            .push(crate::tools::subagent::SubAgentResult {
                name: "agent_a".to_string(),
                agent_id: "agent_a".to_string(),
                context_mode: "fresh".to_string(),
                fork_context: false,
                workspace: None,
                git_branch: None,
                agent_type: crate::tools::subagent::FleetRole::Worker,
                assignment: crate::tools::subagent::SubAgentAssignment {
                    objective: "objective".to_string(),
                    role: None,
                },
                model: "deepseek-v4-flash".to_string(),
                nickname: None,
                status: crate::tools::subagent::SubAgentStatus::Completed,
                worker_status: None,
                runtime_permissions: None,
                parent_run_id: None,
                spawn_depth: 0,
                child_route: None,
                result: None,
                steps_taken: 0,
                checkpoint: None,
                needs_input: None,
                duration_ms: 0,
                started_at: None,
                from_prior_session: false,
            });
        let done = PodCapacitySnapshot::from_app(&app);
        assert_eq!(done.known_members, 1);
        assert_eq!(done.live, 0, "a completed worker is not live");
        assert!(done.is_active(), "pod was active this session");
    }

    #[test]
    fn setting_fact_keeps_live_startup_and_saved_lanes_distinct() {
        let fact = SettingFact {
            current: Some("session"),
            effective: Some("managed"),
            startup: Some("next"),
            saved: Some("disk"),
            authority: SettingAuthority::ManagedPolicy,
            apply: SettingApplySemantics::NextSession,
        };

        assert_eq!(fact.current, Some("session"));
        assert_eq!(fact.effective, Some("managed"));
        assert_eq!(fact.startup, Some("next"));
        assert_eq!(fact.saved, Some("disk"));
    }

    #[test]
    fn registry_ignores_empty_geometry_and_prefers_the_topmost_target() {
        let mut registry = InteractionRegistry::default();

        registry.register(target(Rect::new(2, 2, 6, 3), 3_000));
        registry.register(target(Rect::new(4, 3, 6, 3), 4_000));
        registry.register(target(Rect::new(0, 0, 0, 1), 5_000));

        assert_eq!(registry.iter().count(), 2);
        assert_eq!(
            registry.target_at(5, 3).map(|target| target.area),
            Some(Rect::new(4, 3, 6, 3))
        );
        assert_eq!(
            registry.target_at(2, 2).map(|target| target.area),
            Some(Rect::new(2, 2, 6, 3))
        );
        assert!(registry.target_at(20, 20).is_none());

        registry.clear();
        assert_eq!(registry.iter().count(), 0);
    }
}
