//! Note command.
//!
//! This module separates the command handler from the implementation.

pub mod note_command;
pub mod note_impl;
pub use note_command::Note;
pub use note_impl::note;
