//! Capacity-aware guardrail controller for context pressure management.

use std::collections::{HashMap, VecDeque};

/// Controller settings.
#[derive(Debug, Clone, PartialEq)]
pub struct CapacityControllerConfig {
    pub enabled: bool,
    pub low_risk_max: f64,
    pub medium_risk_max: f64,
    pub severe_min_slack: f64,
    pub severe_violation_ratio: f64,
    pub refresh_cooldown_turns: u64,
    pub replan_cooldown_turns: u64,
    pub max_replay_per_turn: usize,
    pub min_turns_before_guardrail: u64,
    pub profile_window: usize,
    pub model_priors: HashMap<String, f64>,
    pub fallback_default: f64,
}

impl Default for CapacityControllerConfig {
    fn default() -> Self {
        let mut model_priors = HashMap::new();
        model_priors.insert("deepseek_v3_2_chat".to_string(), 3.9);
        model_priors.insert("deepseek_v3_2_reasoner".to_string(), 4.1);
        model_priors.insert("deepseek_v4_pro".to_string(), 3.5);
        model_priors.insert("deepseek_v4_flash".to_string(), 4.2);

        Self {
            // OFF BY DEFAULT since v0.8.11. The capacity controller's
            // interventions (TargetedContextRefresh, VerifyAndReplan)
            // silently rewrite or clear the session message log, which
            // surprises the user and destroys V4's prefix cache. v0.8.11
            // committed to "trust the model with the full 1M-token
            // context, only compact on explicit user `/compact`."
            // Auto-managing the prefix on the user's behalf works against
            // that posture. Power users who want the controller can opt
            // in via `capacity.enabled = true` in
            // `~/.deepseek/config.toml`.
            enabled: false,
            // Thresholds retained for the opt-in path; tuning notes live
            // in git history (#63 follow-up).
            low_risk_max: 0.50,
            medium_risk_max: 0.62,
            severe_min_slack: -0.25,
            severe_violation_ratio: 0.40,
            refresh_cooldown_turns: 6,
            replan_cooldown_turns: 5,
            max_replay_per_turn: 1,
            min_turns_before_guardrail: 4,
            profile_window: 8,
            model_priors,
            fallback_default: 3.8,
        }
    }
}

impl CapacityControllerConfig {
    /// Build effective capacity config from app config.
    #[must_use]
    pub fn from_app_config(config: &crate::config::Config) -> Self {
        let mut out = Self::default();
        let Some(capacity) = config.capacity.as_ref() else {
            return out;
        };

        if let Some(v) = capacity.enabled {
            out.enabled = v;
        }
        if let Some(v) = capacity.low_risk_max {
            out.low_risk_max = v;
        }
        if let Some(v) = capacity.medium_risk_max {
            out.medium_risk_max = v;
        }
        if let Some(v) = capacity.severe_min_slack {
            out.severe_min_slack = v;
        }
        if let Some(v) = capacity.severe_violation_ratio {
            out.severe_violation_ratio = v;
        }
        if let Some(v) = capacity.refresh_cooldown_turns {
            out.refresh_cooldown_turns = v;
        }
        if let Some(v) = capacity.replan_cooldown_turns {
            out.replan_cooldown_turns = v;
        }
        if let Some(v) = capacity.max_replay_per_turn {
            out.max_replay_per_turn = v;
        }
        if let Some(v) = capacity.min_turns_before_guardrail {
            out.min_turns_before_guardrail = v;
        }
        if let Some(v) = capacity.profile_window {
            out.profile_window = v.max(2);
        }

        if let Some(v) = capacity.deepseek_v3_2_chat_prior {
            out.model_priors.insert("deepseek_v3_2_chat".to_string(), v);
        }
        if let Some(v) = capacity.deepseek_v3_2_reasoner_prior {
            out.model_priors
                .insert("deepseek_v3_2_reasoner".to_string(), v);
        }
        if let Some(v) = capacity.deepseek_v4_pro_prior {
            out.model_priors.insert("deepseek_v4_pro".to_string(), v);
        }
        if let Some(v) = capacity.deepseek_v4_flash_prior {
            out.model_priors.insert("deepseek_v4_flash".to_string(), v);
        }
        if let Some(v) = capacity.fallback_default_prior {
            out.fallback_default = v;
        }

        out
    }
}

/// Guardrail decision output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailAction {
    NoIntervention,
    TargetedContextRefresh,
    VerifyWithToolReplay,
    VerifyAndReplan,
}

impl GuardrailAction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            GuardrailAction::NoIntervention => "no_intervention",
            GuardrailAction::TargetedContextRefresh => "targeted_context_refresh",
            GuardrailAction::VerifyWithToolReplay => "verify_with_tool_replay",
            GuardrailAction::VerifyAndReplan => "verify_and_replan",
        }
    }
}

/// Coarse failure risk band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskBand {
    Low,
    Medium,
    High,
}

impl RiskBand {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RiskBand::Low => "low",
            RiskBand::Medium => "medium",
            RiskBand::High => "high",
        }
    }
}

/// Input used to observe current turn pressure.
#[derive(Debug, Clone)]
pub struct CapacityObservationInput {
    pub turn_index: u64,
    pub model: String,
    pub action_count_this_turn: usize,
    pub tool_calls_recent_window: usize,
    pub unique_reference_ids_recent_window: usize,
    pub context_used_ratio: f64,
}

/// Rolling slack profile.
#[derive(Debug, Clone, Copy, Default)]
pub struct DynamicSlackProfile {
    pub final_slack: f64,
    pub min_slack: f64,
    pub violation_ratio: f64,
    pub slack_volatility: f64,
    pub slack_drop: f64,
}

/// Per-checkpoint capacity snapshot.
#[derive(Debug, Clone)]
pub struct CapacitySnapshot {
    pub turn_index: u64,
    pub h_hat: f64,
    pub c_hat: f64,
    pub slack: f64,
    pub profile: DynamicSlackProfile,
    pub p_fail: f64,
    pub risk_band: RiskBand,
    pub severe: bool,
}

/// Full controller decision including reason and block flags.
#[derive(Debug, Clone)]
pub struct CapacityDecision {
    pub action: GuardrailAction,
    pub reason: String,
    pub cooldown_blocked: bool,
}

#[derive(Debug, Clone, Default)]
struct GuardrailRuntimeState {
    last_refresh_turn: Option<u64>,
    last_replan_turn: Option<u64>,
    replay_count_this_turn: usize,
    replay_disabled_turn: Option<u64>,
    intervention_applied_turn: Option<u64>,
}

/// Capacity controller.
#[derive(Debug, Clone)]
pub struct CapacityController {
    config: CapacityControllerConfig,
    slack_window: VecDeque<f64>,
    recent_tool_counts: VecDeque<usize>,
    recent_ref_counts: VecDeque<usize>,
    state: GuardrailRuntimeState,
    last_snapshot: Option<CapacitySnapshot>,
}

impl CapacityController {
    #[must_use]
    pub fn new(config: CapacityControllerConfig) -> Self {
        Self {
            config,
            slack_window: VecDeque::new(),
            recent_tool_counts: VecDeque::new(),
            recent_ref_counts: VecDeque::new(),
            state: GuardrailRuntimeState::default(),
            last_snapshot: None,
        }
    }

    pub fn observe_pre_turn(
        &mut self,
        input: CapacityObservationInput,
    ) -> Option<CapacitySnapshot> {
        self.observe(input)
    }

    pub fn observe_post_tool(
        &mut self,
        input: CapacityObservationInput,
    ) -> Option<CapacitySnapshot> {
        self.observe(input)
    }

    /// Decide intervention from the latest snapshot, with cooldown and safety gates.
    #[must_use]
    pub fn decide(
        &mut self,
        turn_index: u64,
        snapshot: Option<&CapacitySnapshot>,
    ) -> CapacityDecision {
        if !self.config.enabled {
            return CapacityDecision {
                action: GuardrailAction::NoIntervention,
                reason: "capacity_controller_disabled".to_string(),
                cooldown_blocked: false,
            };
        }

        let Some(snapshot) = snapshot else {
            return CapacityDecision {
                action: GuardrailAction::NoIntervention,
                reason: "missing_capacity_data_fail_open".to_string(),
                cooldown_blocked: false,
            };
        };

        if turn_index < self.config.min_turns_before_guardrail {
            return CapacityDecision {
                action: GuardrailAction::NoIntervention,
                reason: "min_turns_before_guardrail_not_reached".to_string(),
                cooldown_blocked: false,
            };
        }

        let proposed = decide_policy(&self.config, snapshot);
        if proposed == GuardrailAction::NoIntervention {
            return CapacityDecision {
                action: proposed,
                reason: "low_risk_no_intervention".to_string(),
                cooldown_blocked: false,
            };
        }

        if self
            .state
            .intervention_applied_turn
            .is_some_and(|t| t == turn_index)
        {
            return CapacityDecision {
                action: GuardrailAction::NoIntervention,
                reason: "intervention_already_applied_this_turn".to_string(),
                cooldown_blocked: true,
            };
        }

        match proposed {
            GuardrailAction::TargetedContextRefresh => {
                if self
                    .state
                    .last_refresh_turn
                    .is_some_and(|last| turn_index <= last + self.config.refresh_cooldown_turns)
                {
                    return CapacityDecision {
                        action: GuardrailAction::NoIntervention,
                        reason: "refresh_cooldown_active".to_string(),
                        cooldown_blocked: true,
                    };
                }
            }
            GuardrailAction::VerifyWithToolReplay => {
                if self
                    .state
                    .replay_disabled_turn
                    .is_some_and(|t| t == turn_index)
                {
                    return CapacityDecision {
                        action: GuardrailAction::NoIntervention,
                        reason: "replay_disabled_for_turn".to_string(),
                        cooldown_blocked: true,
                    };
                }
                if self.state.replay_count_this_turn >= self.config.max_replay_per_turn {
                    return CapacityDecision {
                        action: GuardrailAction::NoIntervention,
                        reason: "max_replay_per_turn_reached".to_string(),
                        cooldown_blocked: true,
                    };
                }
            }
            GuardrailAction::VerifyAndReplan => {
                if self
                    .state
                    .last_replan_turn
                    .is_some_and(|last| turn_index <= last + self.config.replan_cooldown_turns)
                {
                    return CapacityDecision {
                        action: GuardrailAction::NoIntervention,
                        reason: "replan_cooldown_active".to_string(),
                        cooldown_blocked: true,
                    };
                }
            }
            GuardrailAction::NoIntervention => {}
        }

        CapacityDecision {
            action: proposed,
            reason: "policy_selected_action".to_string(),
            cooldown_blocked: false,
        }
    }

    pub fn mark_turn_start(&mut self, turn_index: u64) {
        let new_turn = match self.last_snapshot.as_ref() {
            None => true,
            Some(snapshot) => snapshot.turn_index != turn_index,
        };
        if new_turn {
            self.state.replay_count_this_turn = 0;
            self.state.replay_disabled_turn = None;
            self.state.intervention_applied_turn = None;
        }
    }

    pub fn mark_intervention_applied(&mut self, turn_index: u64, action: GuardrailAction) {
        self.state.intervention_applied_turn = Some(turn_index);
        match action {
            GuardrailAction::TargetedContextRefresh => {
                self.state.last_refresh_turn = Some(turn_index);
            }
            GuardrailAction::VerifyWithToolReplay => {
                self.state.replay_count_this_turn =
                    self.state.replay_count_this_turn.saturating_add(1);
            }
            GuardrailAction::VerifyAndReplan => {
                self.state.last_replan_turn = Some(turn_index);
            }
            GuardrailAction::NoIntervention => {}
        }
    }

    pub fn mark_replay_failed(&mut self, turn_index: u64) {
        self.state.replay_disabled_turn = Some(turn_index);
    }

    #[must_use]
    pub fn last_snapshot(&self) -> Option<&CapacitySnapshot> {
        self.last_snapshot.as_ref()
    }

    fn observe(&mut self, input: CapacityObservationInput) -> Option<CapacitySnapshot> {
        if !self.config.enabled {
            return None;
        }

        let context_used_ratio = input.context_used_ratio.clamp(0.0, 2.0);
        let action_complexity_bits = log2_1p(input.action_count_this_turn);
        let tool_complexity_bits = log2_1p(input.tool_calls_recent_window);
        let ref_complexity_bits = log2_1p(input.unique_reference_ids_recent_window);
        let context_pressure_bits = 6.0 * context_used_ratio;

        let h_hat = (0.35 * action_complexity_bits)
            + (0.30 * tool_complexity_bits)
            + (0.20 * ref_complexity_bits)
            + (0.15 * context_pressure_bits);
        let c_hat = self.model_prior(&input.model);
        let slack = c_hat - h_hat;

        push_window(&mut self.slack_window, slack, self.config.profile_window);
        push_window(
            &mut self.recent_tool_counts,
            input.tool_calls_recent_window,
            self.config.profile_window,
        );
        push_window(
            &mut self.recent_ref_counts,
            input.unique_reference_ids_recent_window,
            self.config.profile_window,
        );

        let profile = compute_profile(&self.slack_window);
        let z = (-1.65 * profile.final_slack)
            + (-0.85 * profile.min_slack)
            + (1.35 * profile.violation_ratio)
            + (0.70 * profile.slack_volatility)
            + (0.28 * profile.slack_drop)
            - 0.12;
        let p_fail = sigmoid(z).clamp(0.0, 1.0);
        let risk_band = if p_fail <= self.config.low_risk_max {
            RiskBand::Low
        } else if p_fail <= self.config.medium_risk_max {
            RiskBand::Medium
        } else {
            RiskBand::High
        };
        let severe = profile.min_slack <= self.config.severe_min_slack
            || profile.violation_ratio >= self.config.severe_violation_ratio;

        let snapshot = CapacitySnapshot {
            turn_index: input.turn_index,
            h_hat,
            c_hat,
            slack,
            profile,
            p_fail,
            risk_band,
            severe,
        };
        self.last_snapshot = Some(snapshot.clone());
        Some(snapshot)
    }

    fn model_prior(&self, model: &str) -> f64 {
        let normalized = normalize_model_prior_key(model);
        self.config
            .model_priors
            .get(normalized)
            .copied()
            .unwrap_or(self.config.fallback_default)
    }
}

/// Pure policy mapping for snapshot -> action.
#[must_use]
pub fn decide_policy(
    _config: &CapacityControllerConfig,
    snapshot: &CapacitySnapshot,
) -> GuardrailAction {
    match snapshot.risk_band {
        RiskBand::Low => GuardrailAction::NoIntervention,
        RiskBand::Medium => GuardrailAction::TargetedContextRefresh,
        RiskBand::High if snapshot.severe => GuardrailAction::VerifyAndReplan,
        RiskBand::High => GuardrailAction::VerifyWithToolReplay,
    }
}

fn normalize_model_prior_key(model: &str) -> &str {
    // Strip optional "deepseek-ai/" NIM namespace prefix before pattern matching.
    let model = model.strip_prefix("deepseek-ai/").unwrap_or(model);
    let lower = model.to_ascii_lowercase();
    // V4 variants must be checked before the generic V3/chat/reasoner branches
    // because those branches do not contain "v4" tokens and the ordering prevents
    // accidental cross-matches.
    if lower.contains("v4-pro") || lower.contains("v4_pro") {
        "deepseek_v4_pro"
    } else if lower.contains("v4-flash") || lower.contains("v4_flash") {
        "deepseek_v4_flash"
    } else if lower.contains("reasoner") || lower.contains("r1") {
        "deepseek_v3_2_reasoner"
    } else if lower.contains("chat") || lower.contains("v3") {
        "deepseek_v3_2_chat"
    } else {
        "fallback_default"
    }
}

fn log2_1p(v: usize) -> f64 {
    (1.0 + (v as f64)).log2()
}

fn push_window<T>(window: &mut VecDeque<T>, value: T, max_len: usize) {
    window.push_back(value);
    while window.len() > max_len {
        window.pop_front();
    }
}

fn compute_profile(window: &VecDeque<f64>) -> DynamicSlackProfile {
    if window.is_empty() {
        return DynamicSlackProfile::default();
    }

    let values: Vec<f64> = window.iter().copied().collect();
    let final_slack = *values.last().unwrap_or(&0.0);
    let min_slack = values.iter().copied().fold(f64::INFINITY, f64::min);
    let violations = values.iter().filter(|v| **v <= 0.0).count() as f64;
    let violation_ratio = violations / (values.len() as f64);

    let deltas: Vec<f64> = values.windows(2).map(|w| w[1] - w[0]).collect();
    let slack_drop = if values.len() >= 2 {
        (values[values.len() - 2] - values[values.len() - 1]).max(0.0)
    } else {
        0.0
    };

    let slack_volatility = if deltas.is_empty() {
        0.0
    } else {
        let mean = deltas.iter().sum::<f64>() / (deltas.len() as f64);
        let var = deltas
            .iter()
            .map(|delta| {
                let centered = *delta - mean;
                centered * centered
            })
            .sum::<f64>()
            / (deltas.len() as f64);
        var.sqrt()
    };

    DynamicSlackProfile {
        final_slack,
        min_slack,
        violation_ratio,
        slack_volatility,
        slack_drop,
    }
}

fn sigmoid(z: f64) -> f64 {
    if z >= 0.0 {
        let ez = (-z).exp();
        1.0 / (1.0 + ez)
    } else {
        let ez = z.exp();
        ez / (1.0 + ez)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(p_fail: f64, severe: bool, risk_band: RiskBand) -> CapacitySnapshot {
        CapacitySnapshot {
            turn_index: 3,
            h_hat: 1.0,
            c_hat: 3.8,
            slack: 2.8,
            profile: DynamicSlackProfile {
                final_slack: 2.8,
                min_slack: if severe { -0.5 } else { 0.2 },
                violation_ratio: if severe { 0.6 } else { 0.1 },
                slack_volatility: 0.2,
                slack_drop: 0.1,
            },
            p_fail,
            risk_band,
            severe,
        }
    }

    #[test]
    fn low_risk_maps_to_no_intervention() {
        let cfg = CapacityControllerConfig::default();
        let snap = make_snapshot(0.2, false, RiskBand::Low);
        assert_eq!(decide_policy(&cfg, &snap), GuardrailAction::NoIntervention);
    }

    #[test]
    fn medium_risk_maps_to_refresh() {
        let cfg = CapacityControllerConfig::default();
        let snap = make_snapshot(0.5, false, RiskBand::Medium);
        assert_eq!(
            decide_policy(&cfg, &snap),
            GuardrailAction::TargetedContextRefresh
        );
    }

    #[test]
    fn high_non_severe_maps_to_replay() {
        let cfg = CapacityControllerConfig::default();
        let snap = make_snapshot(0.8, false, RiskBand::High);
        assert_eq!(
            decide_policy(&cfg, &snap),
            GuardrailAction::VerifyWithToolReplay
        );
    }

    #[test]
    fn high_severe_maps_to_replan() {
        let cfg = CapacityControllerConfig::default();
        let snap = make_snapshot(0.9, true, RiskBand::High);
        assert_eq!(decide_policy(&cfg, &snap), GuardrailAction::VerifyAndReplan);
    }

    /// v0.8.11 flipped the default to `enabled = false`. The controller's
    /// observe / decide methods early-return when disabled — opt-in only.
    #[test]
    fn default_controller_is_disabled_and_skips_observations() {
        let cfg = CapacityControllerConfig::default();
        assert!(!cfg.enabled);

        let mut controller = CapacityController::new(cfg);
        let snapshot = controller.observe_pre_turn(CapacityObservationInput {
            turn_index: 1,
            model: "deepseek-v4-pro".to_string(),
            action_count_this_turn: 10,
            tool_calls_recent_window: 10,
            unique_reference_ids_recent_window: 10,
            context_used_ratio: 0.95,
        });

        // With enabled=false, observe_pre_turn returns None.
        assert!(snapshot.is_none());
    }

    /// Opting in via `capacity.enabled = true` re-arms the controller —
    /// observations produce snapshots, decisions can fire interventions.
    #[test]
    fn opt_in_controller_observes_and_decides() {
        let cfg = CapacityControllerConfig {
            enabled: true,
            ..Default::default()
        };

        let mut controller = CapacityController::new(cfg);
        let snapshot = controller.observe_pre_turn(CapacityObservationInput {
            turn_index: 1,
            model: "deepseek-v4-pro".to_string(),
            action_count_this_turn: 10,
            tool_calls_recent_window: 10,
            unique_reference_ids_recent_window: 10,
            context_used_ratio: 0.95,
        });

        assert!(snapshot.is_some());
        let snap = snapshot.unwrap();
        assert_eq!(snap.turn_index, 1);
        assert!(snap.p_fail > 0.0);
    }

    #[test]
    fn app_config_without_capacity_uses_default_disabled() {
        let cfg = CapacityControllerConfig::from_app_config(&crate::config::Config::default());
        // v0.8.11: default is disabled. No capacity section in config
        // means the controller stays inert; users opt in deliberately.
        assert!(!cfg.enabled);
        assert_eq!(cfg.low_risk_max, 0.50);
        assert_eq!(cfg.refresh_cooldown_turns, 6);
        assert_eq!(cfg.min_turns_before_guardrail, 4);
        assert_eq!(cfg.model_priors.get("deepseek_v4_pro"), Some(&3.5));
        assert_eq!(cfg.model_priors.get("deepseek_v4_flash"), Some(&4.2));
    }

    #[test]
    fn normalize_v4_pro_variants() {
        assert_eq!(
            normalize_model_prior_key("deepseek-v4-pro"),
            "deepseek_v4_pro"
        );
        assert_eq!(
            normalize_model_prior_key("deepseek-v4_pro"),
            "deepseek_v4_pro"
        );
        assert_eq!(
            normalize_model_prior_key("deepseek-ai/deepseek-v4-pro"),
            "deepseek_v4_pro"
        );
        assert_eq!(
            normalize_model_prior_key("deepseek-ai/deepseek-v4_pro"),
            "deepseek_v4_pro"
        );
    }

    #[test]
    fn normalize_v4_flash_variants() {
        assert_eq!(
            normalize_model_prior_key("deepseek-v4-flash"),
            "deepseek_v4_flash"
        );
        assert_eq!(
            normalize_model_prior_key("deepseek-v4_flash"),
            "deepseek_v4_flash"
        );
        assert_eq!(
            normalize_model_prior_key("deepseek-ai/deepseek-v4-flash"),
            "deepseek_v4_flash"
        );
        assert_eq!(
            normalize_model_prior_key("deepseek-ai/deepseek-v4_flash"),
            "deepseek_v4_flash"
        );
    }

    #[test]
    fn normalize_v4_and_fallback_prior_keys() {
        assert_eq!(
            normalize_model_prior_key("deepseek-v4-pro"),
            "deepseek_v4_pro"
        );
        assert_eq!(
            normalize_model_prior_key("deepseek-v4-flash"),
            "deepseek_v4_flash"
        );
        assert_eq!(
            normalize_model_prior_key("unknown-model"),
            "fallback_default"
        );
    }

    #[test]
    fn v4_priors_loaded_into_default_config() {
        let cfg = CapacityControllerConfig::default();
        assert_eq!(cfg.model_priors.get("deepseek_v4_pro").copied(), Some(3.5));
        assert_eq!(
            cfg.model_priors.get("deepseek_v4_flash").copied(),
            Some(4.2)
        );
    }

    #[test]
    fn cooldown_blocks_repeated_action() {
        // Capacity controller is opt-in (off by default since v0.6.2). This
        // test exercises the cooldown logic, so explicitly enable it.
        let config = CapacityControllerConfig {
            enabled: true,
            ..CapacityControllerConfig::default()
        };
        let mut controller = CapacityController::new(config);
        let turn_index = 5;
        controller.mark_turn_start(turn_index);
        controller.mark_intervention_applied(turn_index, GuardrailAction::TargetedContextRefresh);

        let snapshot = make_snapshot(0.5, false, RiskBand::Medium);
        let decision = controller.decide(turn_index + 1, Some(&snapshot));
        assert_eq!(decision.action, GuardrailAction::NoIntervention);
        assert!(decision.cooldown_blocked);
    }

    /// Hot-path microbench for `compute_profile`. Run with:
    ///
    /// ```text
    /// cargo test -p deepseek-tui --release capacity::tests::bench_compute_profile -- --ignored --nocapture
    /// ```
    ///
    /// Establishes a baseline cost so we can detect regressions when the
    /// observation cadence is high (50+ message turns × per-step calls). Adds
    /// no dev-deps; we measure with `Instant` and print rather than gating CI.
    // Perf bench prints per-call timing — runs in `cargo test`, never
    // inside the TUI alt-screen.
    #[allow(clippy::print_stdout)]
    #[test]
    #[ignore]
    fn bench_compute_profile() {
        use std::time::Instant;

        for &window_len in &[16usize, 64, 256, 1024] {
            let mut window: VecDeque<f64> = VecDeque::with_capacity(window_len);
            for i in 0..window_len {
                #[allow(clippy::cast_precision_loss)]
                window.push_back((i as f64).sin() * 0.5);
            }

            let iters = 100_000usize;
            let start = Instant::now();
            for _ in 0..iters {
                let profile = compute_profile(&window);
                std::hint::black_box(profile);
            }
            let elapsed = start.elapsed();
            let per_call_ns = elapsed.as_nanos() as f64 / iters as f64;
            println!(
                "compute_profile window={window_len:>4}  total={:?}  per-call={per_call_ns:>8.0}ns",
                elapsed
            );
        }
    }
}
