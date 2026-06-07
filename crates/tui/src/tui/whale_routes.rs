//! Whale-size route taxonomy for model + thinking-effort combinations (#2026).
//!
//! Maps each `(model, reasoning_effort)` pair to a friendly whale-species label,
//! sorted from largest/deepest to smallest/fastest. The labels share the same
//! species pool as sub-agent nicknames (#2016). These labels are kept as an
//! internal taxonomy for sub-agent routing receipts and related affordances; the
//! main `/model` picker stays neutral and lets users choose model and thinking
//! independently.
//!
//! ## Route ordering (size → speed)
//!
//! 1. Blue Whale   — Pro + max thinking (largest, deepest)
//! 2. Fin Whale    — Pro + high thinking
//! 3. Sperm Whale  — Pro + no thinking
//! 4. Humpback     — Flash + max thinking
//! 5. Minke Whale  — Flash + high thinking
//! 6. Beluga       — Flash + no thinking (smallest, fastest)
//!
//! Unknown or non-DeepSeek models fall back to the raw model id without
//! fake whale labeling.

use crate::tui::app::ReasoningEffort;

/// One whale-sized route: a model + thinking-effort combination with
/// a friendly label, sort order, and descriptive hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhaleRoute {
    /// Whale-species label, e.g. "Blue Whale".
    pub label: &'static str,
    /// Model id, e.g. "deepseek-v4-pro".
    pub model: &'static str,
    /// Reasoning effort tier.
    pub effort: ReasoningEffort,
    /// Sort index (0 = largest / deepest).
    pub sort_order: usize,
    /// Short inline hint, e.g. "Pro + max thinking".
    pub hint: &'static str,
    /// Longer description for tooltips / route receipts.
    pub description: &'static str,
}

/// Six canonical routes, sorted largest → smallest.
pub const WHALE_ROUTES: &[WhaleRoute] = &[
    WhaleRoute {
        label: "Blue Whale",
        model: "deepseek-v4-pro",
        effort: ReasoningEffort::Max,
        sort_order: 0,
        hint: "Pro + max thinking",
        description: "Flagship reasoning at maximum depth — architecture, debugging, security reviews",
    },
    WhaleRoute {
        label: "Fin Whale",
        model: "deepseek-v4-pro",
        effort: ReasoningEffort::High,
        sort_order: 1,
        hint: "Pro + high thinking",
        description: "Deep reasoning for complex tasks — multi-file refactors, careful planning",
    },
    WhaleRoute {
        label: "Sperm Whale",
        model: "deepseek-v4-pro",
        effort: ReasoningEffort::Off,
        sort_order: 2,
        hint: "Pro + no thinking",
        description: "Full model power without reasoning overhead — straightforward code generation",
    },
    WhaleRoute {
        label: "Humpback",
        model: "deepseek-v4-flash",
        effort: ReasoningEffort::Max,
        sort_order: 3,
        hint: "Flash + max thinking",
        description: "Fast model with reasoning depth — lightweight analysis, first-pass reviews",
    },
    WhaleRoute {
        label: "Minke Whale",
        model: "deepseek-v4-flash",
        effort: ReasoningEffort::High,
        sort_order: 4,
        hint: "Flash + high thinking",
        description: "Fast model, moderate reasoning — tool execution, read-only scouting",
    },
    WhaleRoute {
        label: "Beluga",
        model: "deepseek-v4-flash",
        effort: ReasoningEffort::Off,
        sort_order: 5,
        hint: "Flash + no thinking",
        description: "Fastest and cheapest — lookups, searches, simple edits",
    },
];

impl WhaleRoute {
    /// Look up the whale route for a given model id and reasoning effort.
    /// Returns `None` for non-DeepSeek models or unrecognized combinations.
    #[must_use]
    #[allow(dead_code)]
    pub fn for_model_effort(model: &str, effort: ReasoningEffort) -> Option<&'static WhaleRoute> {
        WHALE_ROUTES
            .iter()
            .find(|r| r.model.eq_ignore_ascii_case(model) && r.effort == effort)
    }

    /// Look up a whale route by its sort-order index.
    #[must_use]
    #[allow(dead_code)]
    pub fn by_sort_order(index: usize) -> Option<&'static WhaleRoute> {
        WHALE_ROUTES.iter().find(|r| r.sort_order == index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_sorted_by_size() {
        for window in WHALE_ROUTES.windows(2) {
            assert!(
                window[0].sort_order < window[1].sort_order,
                "{} should sort before {}",
                window[0].label,
                window[1].label
            );
        }
    }

    #[test]
    fn lookup_blue_whale_for_pro_max() {
        let route = WhaleRoute::for_model_effort("deepseek-v4-pro", ReasoningEffort::Max)
            .expect("blue whale route exists");
        assert_eq!(route.label, "Blue Whale");
        assert_eq!(route.model, "deepseek-v4-pro");
        assert_eq!(route.effort, ReasoningEffort::Max);
        assert_eq!(route.sort_order, 0);
    }

    #[test]
    fn lookup_beluga_for_flash_off() {
        let route = WhaleRoute::for_model_effort("deepseek-v4-flash", ReasoningEffort::Off)
            .expect("beluga route exists");
        assert_eq!(route.label, "Beluga");
        assert_eq!(route.sort_order, 5);
    }

    #[test]
    fn lookup_case_insensitive_model() {
        let route = WhaleRoute::for_model_effort("DeepSeek-V4-Pro", ReasoningEffort::High)
            .expect("case-insensitive match");
        assert_eq!(route.label, "Fin Whale");
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(WhaleRoute::for_model_effort("gpt-4o", ReasoningEffort::High).is_none());
    }

    #[test]
    fn unknown_effort_with_valid_model_returns_none() {
        // ReasoningEffort::Auto is not in any whale route
        assert!(WhaleRoute::for_model_effort("deepseek-v4-pro", ReasoningEffort::Auto).is_none());
    }

    #[test]
    fn by_sort_order_finds_correct_routes() {
        assert_eq!(WhaleRoute::by_sort_order(0).unwrap().label, "Blue Whale");
        assert_eq!(WhaleRoute::by_sort_order(5).unwrap().label, "Beluga");
        assert!(WhaleRoute::by_sort_order(99).is_none());
    }

    #[test]
    fn every_route_has_unique_sort_order() {
        let orders: Vec<usize> = WHALE_ROUTES.iter().map(|r| r.sort_order).collect();
        let mut sorted = orders.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(orders.len(), sorted.len(), "duplicate sort orders detected");
    }

    #[test]
    fn every_route_has_unique_label() {
        let labels: Vec<&str> = WHALE_ROUTES.iter().map(|r| r.label).collect();
        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(labels.len(), sorted.len(), "duplicate labels detected");
    }
}
