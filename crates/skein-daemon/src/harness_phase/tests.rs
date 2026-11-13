//! Tests for the phase machine and its driver. Each file is named for the
//! concern it covers. `background_tasks.rs` and `subagents.rs` are declared
//! from their owning module (`#[path]`) because they need its private items.
mod background;
mod client_input;
mod core;
mod core_helpers;
mod deferral;
mod delegation;
mod invariants;
mod invariants_runtime;
mod invariants_state;
mod lifecycle;
mod machine;
mod output_panic;
mod patterns;
mod screen;
mod service;
mod service_fix;
mod stream;
mod supervisor;
mod watchdog;
mod watchdog_recover;
