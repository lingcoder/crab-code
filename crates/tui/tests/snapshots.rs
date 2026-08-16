//! Integration tests for TUI cell snapshot baselines.
//!
//! Each fixture builds a preset `App`-cell state, renders it into a
//! fixed-size buffer, and compares against `tests/snaps/<name>.snap`.
//!
//! First run: snap file does not exist -> baseline is written and the
//! test passes.
//! Subsequent runs: any rendering difference fails with a printed diff.
//!
//! Updating a baseline: delete the corresponding `.snap` file and rerun
//! (only after verifying the rendering change is intentional).
//!
//! Layout note: fixture sources live in `tests/snapshots/` so they do
//! not pollute the top-level `tests/` directory. Cargo only treats
//! top-level `tests/*.rs` files as separate test binaries; sub-directory
//! files are silent. We use `#[path = "..."]` attributes to point the
//! `mod` declarations at the sub-directory.

#[path = "snapshots/helpers.rs"]
mod helpers;

#[path = "snapshots/s01_cold_start.rs"]
mod s01_cold_start;
#[path = "snapshots/s02_text_only.rs"]
mod s02_text_only;
#[path = "snapshots/s03_thinking.rs"]
mod s03_thinking;
#[path = "snapshots/s04_single_read.rs"]
mod s04_single_read;
#[path = "snapshots/s05_parallel_reads.rs"]
mod s05_parallel_reads;
#[path = "snapshots/s06_long_bash.rs"]
mod s06_long_bash;
#[path = "snapshots/s07_tool_error.rs"]
mod s07_tool_error;
#[path = "snapshots/s08_compact.rs"]
mod s08_compact;
#[path = "snapshots/s09_interrupt.rs"]
mod s09_interrupt;
#[path = "snapshots/s10_spinner_status.rs"]
mod s10_spinner_status;
#[path = "snapshots/s11_resume_replay.rs"]
mod s11_resume_replay;
