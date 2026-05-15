// Used by the deferred context-limit relay feature (#667). The implementation
// path is staged but not yet wired from the engine; suppress dead-code warnings
// rather than delete the table until the follow-up feature consumes it.
#[allow(dead_code)]
pub const THRESHOLDS: [(f32, &str); 3] = [
    (
        0.9,
        "Context at 90%: stop and write relay to .deepseek/handoff.md now",
    ),
    (
        0.8,
        "Context at 80%: urgent hard-limit pressure; compact or write relay now",
    ),
    (
        0.6,
        "Context at 60%: prepare relay or suggest /compact for sustained work",
    ),
];
#[allow(dead_code)]
pub fn threshold_message(ratio: f32) -> Option<&'static str> {
    THRESHOLDS
        .iter()
        .find(|(t, _)| ratio >= *t)
        .map(|(_, m)| *m)
}
