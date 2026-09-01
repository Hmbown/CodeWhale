//! Status surface state: toast queue, sticky status, and the
//! `status_message` -> toast synchronization helpers.
//!
//! `StatusToast` / `StatusToastLevel` live here with the `impl App`
//! extension block that owns toast/status/message behavior; state fields
//! (`status_toasts`, `sticky_status`, `status_message`,
//! `last_status_message_seen`) remain on `App` in `app.rs`.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct StatusToast {
    pub text: String,
    pub level: StatusToastLevel,
    pub created_at: Instant,
    pub ttl_ms: Option<u64>,
    pub(crate) kind: StatusToastKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusToastKind {
    Ordinary,
    ContextPressure(crate::context_budget::PressureLevel),
}

impl StatusToast {
    #[must_use]
    pub fn new(text: impl Into<String>, level: StatusToastLevel, ttl_ms: Option<u64>) -> Self {
        Self {
            text: text.into(),
            level,
            created_at: Instant::now(),
            ttl_ms,
            kind: StatusToastKind::Ordinary,
        }
    }

    #[must_use]
    pub(crate) fn context_pressure(
        text: impl Into<String>,
        level: crate::context_budget::PressureLevel,
    ) -> Self {
        Self {
            text: text.into(),
            level: StatusToastLevel::Warning,
            created_at: Instant::now(),
            ttl_ms: None,
            kind: StatusToastKind::ContextPressure(level),
        }
    }

    #[must_use]
    pub fn is_expired(&self, now: Instant) -> bool {
        self.ttl_ms
            .is_some_and(|ttl| now.duration_since(self.created_at).as_millis() >= u128::from(ttl))
    }
}

impl App {
    pub fn push_status_toast(
        &mut self,
        text: impl Into<String>,
        level: StatusToastLevel,
        ttl_ms: Option<u64>,
    ) {
        let toast = StatusToast::new(text, level, ttl_ms);
        self.status_toasts.push_back(toast);
        while self.status_toasts.len() > 24 {
            self.status_toasts.pop_front();
        }
        self.needs_redraw = true;
    }

    /// Default lifetime for sticky error toasts. Long enough to read, short
    /// enough that a failed workflow does not permanently occupy footer chrome.
    pub const STICKY_ERROR_TTL_MS: u64 = 8_000;

    pub fn set_sticky_status(
        &mut self,
        text: impl Into<String>,
        level: StatusToastLevel,
        ttl_ms: Option<u64>,
    ) {
        // Cap sticky errors so a missing TTL never becomes permanent chrome.
        // Explicit shorter TTLs still win; longer/None fall back to the default.
        let ttl_ms = match level {
            StatusToastLevel::Error => Some(
                ttl_ms
                    .unwrap_or(Self::STICKY_ERROR_TTL_MS)
                    .min(Self::STICKY_ERROR_TTL_MS),
            ),
            _ => ttl_ms,
        };
        self.sticky_status = Some(StatusToast::new(text, level, ttl_ms));
        self.needs_redraw = true;
    }

    pub fn clear_sticky_status(&mut self) {
        if self.sticky_status.take().is_some() {
            self.needs_redraw = true;
        }
    }

    /// Dismiss the persistent context-pressure warning without dismissing
    /// unrelated error/status chrome. Returns whether anything was cleared.
    pub fn dismiss_context_pressure_warning(&mut self) -> bool {
        let is_context_pressure = self
            .sticky_status
            .as_ref()
            .is_some_and(|status| matches!(status.kind, StatusToastKind::ContextPressure(_)));
        if is_context_pressure {
            if let Some(StatusToastKind::ContextPressure(level)) =
                self.sticky_status.as_ref().map(|status| status.kind)
            {
                self.context_pressure_warning_dismissed = Some(level);
            }
            self.clear_sticky_status();
            return true;
        }
        false
    }

    /// Drop sticky error chrome when the user resumes typing so a prior
    /// workflow/provider failure does not linger over the next draft.
    pub fn acknowledge_sticky_on_composer_activity(&mut self) {
        if self
            .sticky_status
            .as_ref()
            .is_some_and(|toast| matches!(toast.level, StatusToastLevel::Error))
        {
            self.clear_sticky_status();
        }
    }

    pub(super) fn classify_status_text(text: &str) -> (StatusToastLevel, Option<u64>, bool) {
        let lower = text.to_ascii_lowercase();
        let has = |needle: &str| lower.contains(needle);

        if has("offline mode") || has("context critical") {
            return (StatusToastLevel::Warning, None, true);
        }
        if has("error")
            || has("failed")
            || has("denied")
            || has("timeout")
            || has("aborted")
            || has("critical")
        {
            return (
                StatusToastLevel::Error,
                Some(Self::STICKY_ERROR_TTL_MS),
                true,
            );
        }
        // A success keyword under a negation ("not saved", "no longer
        // found", "could not enable") is a failure the coarse keyword match
        // would otherwise paint green. Guard it: negated success degrades to
        // a neutral Info toast rather than a misleading Success.
        let negated = has("not ")
            || has("no longer")
            || has("no ")
            || has("could not")
            || has("couldn't")
            || has("cannot")
            || has("can't")
            || has("unable");
        if !negated
            && (has("saved")
                || has("loaded")
                || has("queued")
                || has("found")
                || has("enabled")
                || has("completed"))
        {
            return (StatusToastLevel::Success, Some(5_000), false);
        }
        if has("cancelled") || has("canceled") || has("warning") {
            return (StatusToastLevel::Warning, Some(5_000), false);
        }
        (StatusToastLevel::Info, Some(4_000), false)
    }

    fn is_mode_switch_status_message(message: &str) -> bool {
        message.starts_with("Switched to ") && message.ends_with(" mode")
    }

    pub fn sync_status_message_to_toasts(&mut self) {
        let current = self.status_message.clone();
        if self.last_status_message_seen == current {
            return;
        }
        self.last_status_message_seen = current.clone();

        let Some(message) = current else {
            return;
        };
        if message.trim().is_empty() {
            return;
        }
        if Self::is_mode_switch_status_message(&message) {
            return;
        }

        let (level, ttl_ms, sticky) = Self::classify_status_text(&message);
        if sticky {
            self.set_sticky_status(message, level, ttl_ms);
        } else {
            if matches!(level, StatusToastLevel::Success)
                && self
                    .sticky_status
                    .as_ref()
                    .is_some_and(|toast| matches!(toast.level, StatusToastLevel::Error))
            {
                self.clear_sticky_status();
            }
            self.push_status_toast(message, level, ttl_ms);
        }
    }

    fn prune_expired_status_toasts(&mut self, now: Instant) {
        let queued_before = self.status_toasts.len();
        self.status_toasts.retain(|toast| !toast.is_expired(now));
        let queued_removed = self.status_toasts.len() != queued_before;
        let sticky_removed = self
            .sticky_status
            .as_ref()
            .is_some_and(|toast| toast.is_expired(now));
        if sticky_removed {
            self.sticky_status = None;
        }
        if queued_removed || sticky_removed {
            self.needs_redraw = true;
        }
    }

    pub fn active_status_toast(&mut self) -> Option<StatusToast> {
        self.sync_status_message_to_toasts();
        let now = Instant::now();
        self.prune_expired_status_toasts(now);

        let sticky = self.sticky_status.clone();
        let latest = self.status_toasts.back().cloned();
        match (sticky, latest) {
            (Some(sticky), Some(latest))
                if matches!(
                    sticky.kind,
                    StatusToastKind::ContextPressure(crate::context_budget::PressureLevel::High)
                        | StatusToastKind::ContextPressure(
                            crate::context_budget::PressureLevel::Medium
                        )
                ) =>
            {
                Some(latest)
            }
            (Some(sticky), _) => Some(sticky),
            (None, latest) => latest,
        }
    }
}
