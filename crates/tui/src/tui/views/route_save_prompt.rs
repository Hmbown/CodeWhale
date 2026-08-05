//! Route-save decisions.
//!
//! A `/model` or `/provider` change is temporary by default. The explicit
//! persistence choices are offered as a NON-BLOCKING band in the status area
//! (u = update this Fleet, n = save as a new Fleet, d = remember as my
//! default, k = keep for this session only). Nothing is written until the
//! user presses one of those keys — a scripted or automated terminal is never
//! interrupted by a modal.

/// The explicit persistence choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSaveChoice {
    /// Rewrite the selected Fleet's operator route to the session route.
    UpdateFleet,
    /// Save the session route as a brand-new Fleet (user-global) and select it.
    SaveAsNewFleet,
    /// Remember the session route as the startup default (settings; only
    /// offered when no Fleet is selected).
    SaveAsDefault,
    /// Write nothing; the change lives for this session only. (Implemented
    /// directly by the key loop's `k`/Esc handling; kept as the named choice
    /// so receipts and tests speak the same vocabulary.)
    #[allow(dead_code)]
    SessionOnly,
}
