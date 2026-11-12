//! The harness phase machine (#732): which phase each harness is in
//! (spawning / running / idle / waiting / permission / exited), ported
//! from the webview's `harnessActivity` store.
//!
//! Pure and synchronous: no threads, no I/O, no globals. Every call takes
//! the current time, so the tables in the sibling `*_tests.rs` files drive
//! it deterministically.
//!
//! `machine.rs` holds [`PhaseMachine`] and the tick; `lifecycle.rs`, `io.rs`
//! and `permission.rs` are `impl PhaseMachine` blocks split the way the TS
//! files are. The deferral/work rules, the Hermes screen phase and the
//! supervisor repairs are later ports and enter at the `SEAM(#732 ...)`
//! markers in `machine.rs`.

pub mod background_tasks;
pub mod client_input;
pub mod constants;
mod deferral;
pub mod invariants;
mod io;
mod lifecycle;
pub mod machine;
pub mod patterns;
mod permission;
pub mod screen;
pub mod service;
pub mod shadow;
pub mod sinks;
pub mod subagents;
mod supervisor;
mod supervisor_state;
pub mod translate;
pub mod types;
mod work;

#[cfg(test)]
mod background_tests;
#[cfg(test)]
mod core_helpers_tests;
#[cfg(test)]
mod core_tests;
#[cfg(test)]
mod deferral_tests;
#[cfg(test)]
mod delegation_tests;
#[cfg(test)]
mod invariants_runtime_tests;
#[cfg(test)]
mod invariants_state_tests;
#[cfg(test)]
mod invariants_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod patterns_tests;
#[cfg(test)]
mod screen_tests;
#[cfg(test)]
mod service_fix_tests;
#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod shadow_resync_tests;
#[cfg(test)]
mod shadow_tests;
#[cfg(test)]
mod supervisor_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod watchdog_recover_tests;
#[cfg(test)]
mod watchdog_tests;

pub use client_input::ClientPhaseInput;
pub use invariants::{
    Effect, HarnessFacts, LogLevel, MailState, SupervisorCode, SupervisorFacts, TranscriptSample,
    Violation, ViolationRecord,
};
pub use lifecycle::SessionSwitch;
pub use machine::{PhaseMachine, at_safe_stopping_point, is_decisive_input};
pub use screen::ScreenReading;
pub use service::PhaseService;
pub use shadow::{ShadowComparator, ShadowStats};
pub use types::{
    ActivityPhase, AdapterEvent, DegradedBy, HarnessActivity, Transition, TransitionSource,
    TurnSignal, TurnSignalKind,
};
pub use work::WorkCounts;
