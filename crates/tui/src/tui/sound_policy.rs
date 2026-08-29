//! Opt-in, deterministic event-sound policy for TUI notification events
//! (#4817).
//!
//! This is terminal-bell-level only: every cue is one or two BEL (`\x07`)
//! bytes written to stdout, exactly the bytes the existing
//! `bell_sound`/`beep_sound` helpers in [`super::notifications`] emit.
//! The cues are functional signals (a fixed, documented event → byte
//! mapping), not designed-for-pleasantness audio — there are no audio
//! assets and no new dependencies. BEL is inert on terminals that ignore
//! it and on platforms where the byte is a no-op, and the whole policy is
//! **off by default**, so a platform with no sound support falls back to
//! doing nothing.
//!
//! The decision function is pure with respect to wall-clock time: the
//! caller supplies `now_ms`, so rate limiting is deterministic and
//! testable. The runtime clock used by the wiring in
//! [`super::notifications::notify_done_to`] is
//! [`std::time::SystemTime`] epoch millis ([`epoch_millis_now`]) — not
//! strictly monotonic, but fine for rate limiting, and documented here
//! rather than implied.

use std::io::{self, Write};
use std::sync::{OnceLock, RwLock};

use super::notification_payload::NotificationKind;

/// The closed set of events that can produce a sound cue. Mirrors
/// [`NotificationKind`] one-to-one; kept as a separate type so the sound
/// policy's surface (parsing, allow-lists, docs) is independent of the
/// notification payload taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEvent {
    /// An agent turn finished successfully.
    TurnComplete,
    /// A sub-agent reached a terminal status.
    SubagentTerminal,
    /// A tool call is blocked waiting for approval.
    ApprovalNeeded,
    /// The agent is blocked on a user answer.
    InputNeeded,
    /// The sandbox denied an operation; the user must elevate.
    ElevationNeeded,
    /// The model called the `notify` tool.
    ModelNotify,
}

impl SoundEvent {
    /// Total mapping from the notification taxonomy. Every
    /// [`NotificationKind`] has exactly one sound event.
    #[must_use]
    pub fn from_notification_kind(kind: NotificationKind) -> SoundEvent {
        match kind {
            NotificationKind::TurnComplete => SoundEvent::TurnComplete,
            NotificationKind::SubagentTerminal => SoundEvent::SubagentTerminal,
            NotificationKind::ApprovalNeeded => SoundEvent::ApprovalNeeded,
            NotificationKind::InputNeeded => SoundEvent::InputNeeded,
            NotificationKind::ElevationNeeded => SoundEvent::ElevationNeeded,
            NotificationKind::ModelNotify => SoundEvent::ModelNotify,
        }
    }

    /// Parse the kebab-case config/docs spelling, e.g. `"turn-complete"`. Unknown strings return `None`;
    /// callers skip them rather than failing.
    #[must_use]
    pub fn parse(s: &str) -> Option<SoundEvent> {
        match s {
            "turn-complete" => Some(SoundEvent::TurnComplete),
            "subagent-terminal" => Some(SoundEvent::SubagentTerminal),
            "approval-needed" => Some(SoundEvent::ApprovalNeeded),
            "input-needed" => Some(SoundEvent::InputNeeded),
            "elevation-needed" => Some(SoundEvent::ElevationNeeded),
            "model-notify" => Some(SoundEvent::ModelNotify),
            _ => None,
        }
    }

    /// Slot index into [`EventSoundPolicy::last_played_ms`].
    fn index(self) -> usize {
        match self {
            SoundEvent::TurnComplete => 0,
            SoundEvent::SubagentTerminal => 1,
            SoundEvent::ApprovalNeeded => 2,
            SoundEvent::InputNeeded => 3,
            SoundEvent::ElevationNeeded => 4,
            SoundEvent::ModelNotify => 5,
        }
    }
}

/// Terminal-safe cues. `Bell` and `Beep` emit the same single BEL byte as
/// the existing `bell_sound`/`beep_sound` helpers; the distinction is
/// semantic (which event fired), not a different sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundCue {
    /// One BEL byte (`\x07`).
    Bell,
    /// Two BEL bytes (`\x07\x07`).
    DoubleBell,
    /// One BEL byte (`\x07`) — the same bytes `beep_sound` writes.
    Beep,
}

/// The deterministic event mapping. Fixed table, documented here and in
/// `docs/CONFIGURATION.md`; changing it is a behavior change, not a tune.
/// The cues are functional signals, not designed-for-pleasantness audio.
#[must_use]
pub fn cue_for(event: SoundEvent) -> SoundCue {
    match event {
        SoundEvent::TurnComplete => SoundCue::Bell,
        SoundEvent::SubagentTerminal => SoundCue::Bell,
        SoundEvent::ApprovalNeeded => SoundCue::DoubleBell,
        SoundEvent::InputNeeded => SoundCue::Beep,
        SoundEvent::ElevationNeeded => SoundCue::DoubleBell,
        SoundEvent::ModelNotify => SoundCue::Beep,
    }
}

/// Why a sound was not played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressReason {
    /// The policy is disabled (the default — platform-safe no-op).
    Disabled,
    /// Quiet mode is on.
    QuietMode,
    /// The event is not in the allow-list.
    NotListed,
    /// The event fired within `min_interval_ms` of its previous play.
    RateLimited,
    /// `turn-complete` is left to the existing `completion_sound` channel
    /// so the two never double-ding.
    TurnCompleteHandledByCompletionSound,
}

/// The outcome of a policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundDecision {
    /// Emit this cue.
    Play(SoundCue),
    /// Do not emit; this is why.
    Suppress(SuppressReason),
}

/// Deterministic, opt-in event-sound policy.
///
/// `decide` is pure apart from recording the play timestamp: the caller
/// supplies `now_ms`, so there is no wall-clock dependency in the rules.
#[derive(Debug, Clone)]
pub struct EventSoundPolicy {
    /// Master switch. Default `false` — nothing is emitted unless the
    /// user opts in via `[notifications.event_sound]`.
    pub enabled: bool,
    /// Allow-list of events that may play.
    pub events: Vec<SoundEvent>,
    /// Minimum milliseconds between two plays of the *same* event.
    pub min_interval_ms: u64,
    /// Quiet mode: suppress everything without editing the allow-list.
    pub quiet: bool,
    /// Whether the separate `[notifications].completion_sound` channel is
    /// active. When it is, `turn-complete` is suppressed here (see
    /// [`SuppressReason::TurnCompleteHandledByCompletionSound`]) to avoid
    /// a double ding. Stored on the policy (rather than passed to
    /// `decide`) because it is configuration, not per-call state.
    pub completion_sound_active: bool,
    /// Last play timestamp (caller-supplied millis) per event slot.
    last_played_ms: [Option<u64>; 6],
}

impl Default for EventSoundPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            events: vec![SoundEvent::TurnComplete, SoundEvent::ApprovalNeeded],
            min_interval_ms: 2000,
            quiet: false,
            completion_sound_active: false,
            last_played_ms: [None; 6],
        }
    }
}

impl EventSoundPolicy {
    /// Build a policy from `[notifications.event_sound]`. Unknown event
    /// strings are ignored (`parse` → `None` → skip); this never panics.
    ///
    /// This direction (tui consumes config) matches how
    /// `CompletionSound` is plumbed and keeps `config.rs` free of tui
    /// imports.
    #[must_use]
    pub fn from_config(
        config: &crate::config::EventSoundConfig,
        completion_sound_active: bool,
    ) -> Self {
        let events = config
            .events
            .iter()
            .filter_map(|s| SoundEvent::parse(s))
            .collect();
        Self {
            enabled: config.enabled,
            events,
            min_interval_ms: config.min_interval_ms,
            quiet: config.quiet,
            completion_sound_active,
            last_played_ms: [None; 6],
        }
    }

    /// Decide whether `event` plays at `now_ms`. Rules, in order:
    ///
    /// 1. `!enabled` → `Suppress(Disabled)`
    /// 2. `quiet` → `Suppress(QuietMode)`
    /// 3. event not in the allow-list → `Suppress(NotListed)`
    /// 4. `turn-complete` while `completion_sound` is active →
    ///    `Suppress(TurnCompleteHandledByCompletionSound)`
    /// 5. within `min_interval_ms` of the last play of this event →
    ///    `Suppress(RateLimited)`
    /// 6. otherwise record the timestamp and `Play(cue_for(event))`.
    pub fn decide(&mut self, event: SoundEvent, now_ms: u64) -> SoundDecision {
        if !self.enabled {
            return SoundDecision::Suppress(SuppressReason::Disabled);
        }
        if self.quiet {
            return SoundDecision::Suppress(SuppressReason::QuietMode);
        }
        if !self.events.contains(&event) {
            return SoundDecision::Suppress(SuppressReason::NotListed);
        }
        if event == SoundEvent::TurnComplete && self.completion_sound_active {
            return SoundDecision::Suppress(SuppressReason::TurnCompleteHandledByCompletionSound);
        }
        let slot = &mut self.last_played_ms[event.index()];
        if let Some(last) = *slot
            && now_ms.saturating_sub(last) < self.min_interval_ms
        {
            return SoundDecision::Suppress(SuppressReason::RateLimited);
        }
        *slot = Some(now_ms);
        SoundDecision::Play(cue_for(event))
    }
}

/// Write the cue's bytes. These are exactly the bytes the existing
/// `bell_sound`/`beep_sound` helpers emit; BEL is inert on terminals and
/// platforms that ignore it, so this is a platform-safe no-op-safe write
/// everywhere.
pub fn emit(cue: SoundCue, out: &mut dyn Write) -> io::Result<()> {
    let bytes: &[u8] = match cue {
        SoundCue::Bell => b"\x07",
        SoundCue::DoubleBell => b"\x07\x07",
        SoundCue::Beep => b"\x07",
    };
    out.write_all(bytes)
}

/// Epoch millis for the runtime wiring. Not strictly monotonic (NTP can
/// step it backwards; `saturating_sub` in `decide` makes that harmless),
/// but fine for rate limiting.
#[must_use]
pub fn epoch_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

static POLICY: OnceLock<RwLock<EventSoundPolicy>> = OnceLock::new();

fn policy_cell() -> &'static RwLock<EventSoundPolicy> {
    POLICY.get_or_init(|| RwLock::new(EventSoundPolicy::default()))
}

/// Install a policy process-wide. Called at startup from
/// [`super::notifications::settings`] alongside `set_completion_sound`.
#[cfg(test)]
pub fn configure(policy: EventSoundPolicy) {
    if let Ok(mut slot) = policy_cell().write() {
        *slot = policy;
    }
}

/// Update runtime policy without erasing its per-event rate-limit history.
///
/// Notification Settings is consulted by several event producers. Rebuilding
/// the policy on each producer path used to clear `last_played_ms`, making the
/// documented minimum interval ineffective in real sessions.
pub fn reconfigure(mut policy: EventSoundPolicy) {
    if let Ok(mut slot) = policy_cell().write() {
        policy.last_played_ms = slot.last_played_ms;
        *slot = policy;
    }
}

/// Run the installed policy for one notification event, writing any cue to
/// `out`. The sink is injected (matching `notify_done_to`) so no test path
/// can BEL a real terminal. Best-effort: lock poisoning and write errors
/// are swallowed, matching the no-op-safe style of the notification module.
pub fn handle_notification_kind_to(kind: NotificationKind, now_ms: u64, out: &mut dyn Write) {
    let event = SoundEvent::from_notification_kind(kind);
    let decision = policy_cell()
        .write()
        .map(|mut policy| policy.decide(event, now_ms));
    if let Ok(SoundDecision::Play(cue)) = decision {
        let _ = emit(cue, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EventSoundConfig;

    fn all_events() -> [SoundEvent; 6] {
        [
            SoundEvent::TurnComplete,
            SoundEvent::SubagentTerminal,
            SoundEvent::ApprovalNeeded,
            SoundEvent::InputNeeded,
            SoundEvent::ElevationNeeded,
            SoundEvent::ModelNotify,
        ]
    }

    fn enabled_policy(events: Vec<SoundEvent>) -> EventSoundPolicy {
        EventSoundPolicy {
            enabled: true,
            events,
            ..EventSoundPolicy::default()
        }
    }

    /// The deterministic mapping, pinned to the documented table.
    #[test]
    fn cue_table_is_the_documented_mapping() {
        assert_eq!(cue_for(SoundEvent::TurnComplete), SoundCue::Bell);
        assert_eq!(cue_for(SoundEvent::SubagentTerminal), SoundCue::Bell);
        assert_eq!(cue_for(SoundEvent::ApprovalNeeded), SoundCue::DoubleBell);
        assert_eq!(cue_for(SoundEvent::InputNeeded), SoundCue::Beep);
        assert_eq!(cue_for(SoundEvent::ElevationNeeded), SoundCue::DoubleBell);
        assert_eq!(cue_for(SoundEvent::ModelNotify), SoundCue::Beep);
    }

    /// The kebab-case spellings are the documented config vocabulary.
    #[test]
    fn parse_covers_the_documented_event_names() {
        let names = [
            ("turn-complete", SoundEvent::TurnComplete),
            ("subagent-terminal", SoundEvent::SubagentTerminal),
            ("approval-needed", SoundEvent::ApprovalNeeded),
            ("input-needed", SoundEvent::InputNeeded),
            ("elevation-needed", SoundEvent::ElevationNeeded),
            ("model-notify", SoundEvent::ModelNotify),
        ];
        assert_eq!(
            names.iter().map(|(_, event)| *event).collect::<Vec<_>>(),
            all_events(),
            "the documented names must cover every event"
        );
        for (name, event) in names {
            assert_eq!(SoundEvent::parse(name), Some(event));
        }
        assert_eq!(SoundEvent::parse("not-an-event"), None);
        assert_eq!(SoundEvent::parse(""), None);
    }

    /// The mapping from the notification taxonomy is total: all six
    /// kinds map, and the match in `from_notification_kind` stops
    /// compiling if a kind is added without a mapping.
    #[test]
    fn from_notification_kind_is_total() {
        let kinds = [
            NotificationKind::TurnComplete,
            NotificationKind::SubagentTerminal,
            NotificationKind::ApprovalNeeded,
            NotificationKind::InputNeeded,
            NotificationKind::ElevationNeeded,
            NotificationKind::ModelNotify,
        ];
        let events: Vec<SoundEvent> = kinds
            .into_iter()
            .map(SoundEvent::from_notification_kind)
            .collect();
        assert_eq!(events, all_events());
    }

    /// Platform-safe no-op fallback: a fresh default policy is disabled,
    /// so every event is suppressed and nothing is ever emitted.
    #[test]
    fn default_policy_suppresses_everything_as_disabled() {
        let mut policy = EventSoundPolicy::default();
        assert!(!policy.enabled);
        for event in all_events() {
            assert_eq!(
                policy.decide(event, 0),
                SoundDecision::Suppress(SuppressReason::Disabled)
            );
            assert_eq!(
                policy.decide(event, 10_000),
                SoundDecision::Suppress(SuppressReason::Disabled)
            );
        }
    }

    #[test]
    fn quiet_mode_suppresses_everything() {
        let mut policy = EventSoundPolicy {
            quiet: true,
            ..enabled_policy(all_events().to_vec())
        };
        for event in all_events() {
            assert_eq!(
                policy.decide(event, 0),
                SoundDecision::Suppress(SuppressReason::QuietMode)
            );
        }
    }

    #[test]
    fn unlisted_event_is_suppressed_as_not_listed() {
        let mut policy = enabled_policy(vec![SoundEvent::TurnComplete]);
        for event in all_events() {
            let expected = if event == SoundEvent::TurnComplete {
                SoundDecision::Play(SoundCue::Bell)
            } else {
                SoundDecision::Suppress(SuppressReason::NotListed)
            };
            assert_eq!(policy.decide(event, 0), expected, "{event:?}");
        }
    }

    /// Rate-limit property, exercised over several event kinds and
    /// timestamp sequences with a caller-supplied clock: within
    /// `min_interval_ms` of a play the same event is rate-limited; at
    /// exactly `min_interval_ms` it plays again.
    #[test]
    fn rate_limit_allows_play_only_after_min_interval() {
        for event in all_events() {
            for start in [0u64, 1_000, 123_456] {
                let mut policy = enabled_policy(vec![event]);
                let cue = cue_for(event);
                assert_eq!(policy.decide(event, start), SoundDecision::Play(cue));
                for t in [start + 500, start + 1999] {
                    assert_eq!(
                        policy.decide(event, t),
                        SoundDecision::Suppress(SuppressReason::RateLimited),
                        "{event:?} at t={t} (start={start})"
                    );
                }
                assert_eq!(
                    policy.decide(event, start + 2000),
                    SoundDecision::Play(cue),
                    "{event:?} at min_interval boundary"
                );
            }
        }
    }

    /// Rate limiting is per-event: playing one event does not throttle
    /// another.
    #[test]
    fn rate_limit_is_per_event() {
        let mut policy = enabled_policy(all_events().to_vec());
        assert_eq!(
            policy.decide(SoundEvent::ApprovalNeeded, 0),
            SoundDecision::Play(SoundCue::DoubleBell)
        );
        assert_eq!(
            policy.decide(SoundEvent::InputNeeded, 1),
            SoundDecision::Play(SoundCue::Beep)
        );
    }

    /// A backwards clock step (NTP) must not panic or double-play.
    #[test]
    fn backwards_clock_is_treated_as_rate_limited() {
        let mut policy = enabled_policy(vec![SoundEvent::ApprovalNeeded]);
        assert_eq!(
            policy.decide(SoundEvent::ApprovalNeeded, 5_000),
            SoundDecision::Play(SoundCue::DoubleBell)
        );
        assert_eq!(
            policy.decide(SoundEvent::ApprovalNeeded, 1_000),
            SoundDecision::Suppress(SuppressReason::RateLimited)
        );
    }

    /// No double-ding with the existing `completion_sound` channel.
    #[test]
    fn turn_complete_defers_to_active_completion_sound() {
        let mut active = EventSoundPolicy {
            completion_sound_active: true,
            ..enabled_policy(vec![SoundEvent::TurnComplete])
        };
        assert_eq!(
            active.decide(SoundEvent::TurnComplete, 0),
            SoundDecision::Suppress(SuppressReason::TurnCompleteHandledByCompletionSound)
        );

        let mut inactive = enabled_policy(vec![SoundEvent::TurnComplete]);
        assert_eq!(
            inactive.decide(SoundEvent::TurnComplete, 0),
            SoundDecision::Play(SoundCue::Bell)
        );
    }

    /// Exact emitted bytes, following the `capture()` byte-assertion
    /// pattern in `notifications.rs`.
    #[test]
    fn emit_writes_exact_bel_bytes() {
        let mut buf = Vec::new();
        emit(SoundCue::Bell, &mut buf).unwrap();
        assert_eq!(buf, b"\x07");

        let mut buf = Vec::new();
        emit(SoundCue::DoubleBell, &mut buf).unwrap();
        assert_eq!(buf, b"\x07\x07");

        let mut buf = Vec::new();
        emit(SoundCue::Beep, &mut buf).unwrap();
        assert_eq!(buf, b"\x07");
    }

    #[test]
    fn from_config_ignores_unknown_events_and_parses_kebab_case() {
        let config = EventSoundConfig {
            enabled: true,
            events: vec![
                "turn-complete".to_string(),
                "bogus".to_string(),
                "ApprovalNeeded".to_string(),
                "approval-needed".to_string(),
            ],
            min_interval_ms: 500,
            quiet: false,
        };
        let policy = EventSoundPolicy::from_config(&config, true);
        assert!(policy.enabled);
        assert_eq!(
            policy.events,
            vec![SoundEvent::TurnComplete, SoundEvent::ApprovalNeeded]
        );
        assert_eq!(policy.min_interval_ms, 500);
        assert!(!policy.quiet);
        assert!(policy.completion_sound_active);
    }

    /// The installed policy is process-global (`POLICY` is a `OnceLock`), so
    /// any test that touches it must serialize on the crate's test-env lock
    /// and put the default back before releasing it. The cue goes to an
    /// injected sink, never the real stdout.
    #[test]
    fn installed_policy_drives_the_global_handler() {
        let _lock = crate::test_support::lock_test_env();
        configure(enabled_policy(vec![SoundEvent::ApprovalNeeded]));

        let mut out = Vec::new();
        handle_notification_kind_to(NotificationKind::ApprovalNeeded, 0, &mut out);
        assert_eq!(out, b"\x07\x07", "the installed allow-list plays");

        let mut out = Vec::new();
        handle_notification_kind_to(NotificationKind::ApprovalNeeded, 1, &mut out);
        assert!(
            out.is_empty(),
            "the rate limit carries across handler calls"
        );

        let mut out = Vec::new();
        handle_notification_kind_to(NotificationKind::InputNeeded, 0, &mut out);
        assert!(out.is_empty(), "an unlisted event stays silent");

        // Restore the default so no later test inherits an enabled policy.
        configure(EventSoundPolicy::default());
        let mut out = Vec::new();
        handle_notification_kind_to(NotificationKind::ApprovalNeeded, 100_000, &mut out);
        assert!(out.is_empty(), "the default policy is disabled");
    }

    #[test]
    fn runtime_reconfigure_preserves_rate_limit_history() {
        let _lock = crate::test_support::lock_test_env();
        let policy = enabled_policy(vec![SoundEvent::ApprovalNeeded]);
        configure(policy.clone());

        let mut out = Vec::new();
        handle_notification_kind_to(NotificationKind::ApprovalNeeded, 0, &mut out);
        assert_eq!(out, b"\x07\x07");

        reconfigure(policy);
        let mut out = Vec::new();
        handle_notification_kind_to(NotificationKind::ApprovalNeeded, 1, &mut out);
        assert!(
            out.is_empty(),
            "re-reading Settings must not erase the per-event rate limit"
        );

        configure(EventSoundPolicy::default());
    }

    #[test]
    fn from_config_matches_config_defaults() {
        let policy = EventSoundPolicy::from_config(&EventSoundConfig::default(), false);
        assert!(!policy.enabled);
        assert_eq!(
            policy.events,
            vec![SoundEvent::TurnComplete, SoundEvent::ApprovalNeeded]
        );
        assert_eq!(policy.min_interval_ms, 2000);
        assert!(!policy.quiet);
    }
}
