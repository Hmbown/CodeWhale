//! The first-run notice copy.
//!
//! One string, owned by the crate that owns what is collected, so the TUI and
//! the CLI cannot drift into describing two different products. Every claim
//! below is checked against [`crate::event`] by a test: if the schema grows a
//! field this text does not cover, that test fails.
//!
//! Two properties of the wording are deliberate and load-bearing:
//!
//! 1. **The declining option is pre-selected and Enter takes it.** Enabling
//!    requires a deliberate keystroke. There is no third option, no "improve
//!    the product" checkbox, and nothing pre-checked.
//! 2. **The red lines are stated as "not collected", not as "anonymized".**
//!    Sampling and hashing are not the same promise, and a notice that implies
//!    them when neither is true is worse than no notice.

/// Headline shown above [`NOTICE_BODY`].
pub const NOTICE_HEADLINE: &str = "Help improve CodeWhale?";

/// The notice itself.
///
/// Wrapped at 72 columns so it renders unchanged in a modal, in a pipe, and in
/// an 80-column terminal.
pub const NOTICE_BODY: &str = "\
CodeWhale can send anonymous usage counts: which version you run, your
OS and CPU family, which features you used, how long sessions ran, and
how they ended.

It never sends prompts, code, file names, paths, repo or branch names,
model output, model names, or credentials. Not sampled, not hashed —
not collected.

You are identified only by a random ID stored on this machine. It is
deleted the moment you turn this off, and it is replaced every 90 days.

Full schema, field by field:  docs/TELEMETRY.md
Turn it off any time:         codewhale config set telemetry false
                              or CODEWHALE_TELEMETRY=0";

/// The question, with the declining answer capitalised as the default.
pub const NOTICE_PROMPT: &str = "Enable telemetry? [y/N]";

/// The line printed once a decision is recorded, so the user has a receipt.
#[must_use]
pub fn decision_receipt(opt_in: bool) -> &'static str {
    if opt_in {
        "Telemetry is on. Turn it off any time with `codewhale config set telemetry false`."
    } else {
        "Telemetry stays off. You will not be asked again."
    }
}

/// Whether a typed answer means yes.
///
/// Everything else — an empty line, EOF, a closed pipe, `n`, a typo — means no.
/// That asymmetry is the point: only an affirmative answer is an affirmative
/// answer.
#[must_use]
pub fn answer_is_yes(input: &str) -> bool {
    matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
