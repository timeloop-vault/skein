//! The harness phase machine (#732): which phase each harness is in
//! (spawning / running / idle / waiting / permission / exited), ported
//! from the webview's `harnessActivity` store.
//!
//! Pure and synchronous: no threads, no I/O, no globals. Every call takes
//! the current time, so the tables in the `tests/` files drive
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
pub mod sinks;
pub mod stream;
pub mod subagents;
mod supervisor;
mod supervisor_state;
pub mod transcript_sampler;
pub mod translate;
pub mod types;
mod work;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use client_input::ClientPhaseInput;
pub use invariants::{
    Effect, HarnessFacts, LogLevel, MailState, SupervisorCode, SupervisorFacts, TranscriptSample,
    Violation, ViolationRecord,
};
pub use lifecycle::SessionSwitch;
pub use machine::{PhaseMachine, at_safe_stopping_point, is_decisive_input};
pub use screen::ScreenReading;
pub use service::PhaseService;
pub use stream::{HARNESS_PHASE_EVENT, PhaseUpdate, PhaseView};
pub use transcript_sampler::{TranscriptSampler, now_ms};
pub use types::{
    ActivityPhase, AdapterEvent, DegradedBy, HarnessActivity, Transition, TransitionSource,
    TurnSignal, TurnSignalKind,
};
pub use work::WorkCounts;
