//! Recursive Language Model (RLM) loop — paper-spec Algorithm 1.
//!
//! Implements Zhang, Kraska & Khattab (arXiv:2512.24601, §2 Algorithm 1):
//!
//! ```text
//! state ← InitREPL(prompt=P)
//! state ← AddFunction(state, sub_RLM)
//! hist ← [Metadata(state)]
//! while True:
//!     code ← LLM(hist)
//!     (state, stdout) ← REPL(state, code)
//!     hist ← hist ∥ code ∥ Metadata(stdout)
//!     if state[Final] is set:
//!         return state[Final]
//! ```
//!
//! Invariants:
//! - `P` is held only as a REPL variable (`context` / `ctx`); never
//!   appears in the root LLM's window.
//! - The root LLM receives small metadata messages — length, preview,
//!   helper list, prior-round summary.
//! - Code rounds and sub-LLM calls travel over a single stdin/stdout
//!   pipe to a long-lived Python subprocess. No HTTP sidecar.

use crate::models::Usage;

pub mod bridge;
pub mod prompt;
pub mod session;
pub mod turn;

pub use bridge::RlmBridge;
pub use prompt::rlm_system_prompt;
pub use turn::{RlmTermination, RlmTurnResult, run_rlm_turn, run_rlm_turn_with_root};

fn add_usage_with_prompt_cache(total: &mut Usage, delta: &Usage) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.prompt_cache_hit_tokens =
        add_optional_usage(total.prompt_cache_hit_tokens, delta.prompt_cache_hit_tokens);
    total.prompt_cache_miss_tokens = add_optional_usage(
        total.prompt_cache_miss_tokens,
        delta.prompt_cache_miss_tokens,
    );
}

fn add_optional_usage(total: Option<u32>, delta: Option<u32>) -> Option<u32> {
    match (total, delta) {
        (Some(total), Some(delta)) => Some(total.saturating_add(delta)),
        (None, Some(delta)) => Some(delta),
        (Some(total), None) => Some(total),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_usage_with_prompt_cache_preserves_cache_counts() {
        let mut total = Usage {
            input_tokens: 100,
            output_tokens: 10,
            prompt_cache_hit_tokens: Some(80),
            prompt_cache_miss_tokens: Some(20),
            ..Usage::default()
        };
        let delta = Usage {
            input_tokens: 50,
            output_tokens: 5,
            prompt_cache_hit_tokens: Some(30),
            prompt_cache_miss_tokens: Some(20),
            ..Usage::default()
        };

        add_usage_with_prompt_cache(&mut total, &delta);

        assert_eq!(total.input_tokens, 150);
        assert_eq!(total.output_tokens, 15);
        assert_eq!(total.prompt_cache_hit_tokens, Some(110));
        assert_eq!(total.prompt_cache_miss_tokens, Some(40));
    }
}
