#![allow(clippy::expect_used)]
pub use codex_protocol::error;

// core_test_support installs the same argument dispatch as the upstream suite.
#[path = "suite/code_mode_completion.rs"]
mod code_mode_completion;
#[path = "suite/exec_completion.rs"]
mod exec_completion;
