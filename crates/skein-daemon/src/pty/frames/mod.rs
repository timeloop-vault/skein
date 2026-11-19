//! Frame fan-out for the daemon-owned terminal (#760, T-3): the terminal
//! mode flag (`SKEIN_TERMINAL`), the pure frame scheduler that decides when
//! to cut a frame from the engine, and the per-PTY hub that owns the engine
//! and its viewers. A PTY uses the hub only in `TerminalMode::Frames`. Design:
//! `docs/skeind-terminal-design.md`, "The frame" and "T4".

mod driver;
mod hub;
mod mode;
mod scheduler;
mod state;
#[cfg(test)]
mod test_support;

#[cfg(test)]
mod driver_tests;
#[cfg(test)]
mod hub_notice_tests;
#[cfg(test)]
mod hub_screen_tests;
#[cfg(test)]
mod hub_size_tests;
#[cfg(test)]
mod hub_tests;

pub(in crate::pty) use driver::HubDriver;
pub use hub::{FrameHub, HubIo};
pub use mode::TerminalMode;
pub(in crate::pty) use scheduler::run_frame_scheduler;
