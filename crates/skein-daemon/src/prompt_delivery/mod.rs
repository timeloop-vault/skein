//! Daemon-side prompt delivery (#733): the pure building blocks for
//! pasting and submitting a prompt into a harness's PTY from the daemon,
//! namely the send gate (a port of the webview's `canSendPrompt`), the
//! bytes a paste becomes, and a tracker of the terminal's bracketed-paste
//! mode. Those are pure; `service` is the one worker thread that drives
//! them and `daemon_env` the real environment it reads and writes.

#[cfg(test)]
mod accept_tests;
pub mod core;
pub mod daemon_env;
pub mod env;
pub mod facts;
pub mod gate;
pub mod mail;
pub mod mail_nudge;
pub mod mail_retry;
pub mod mail_settle;
pub mod paste;
pub mod paste_mode;
pub mod runner_pty;
pub mod send;
pub mod service;
#[cfg(test)]
mod service_tests;
pub mod shared;
pub mod submit_retry;
#[cfg(test)]
pub(crate) mod test_support;

pub use self::core::{DeliveryCore, DeliveryEvent, Reply};
pub use daemon_env::{
    DELIVERY_SEAM_EVENT, DaemonEnv, DeliverySeam, MAIL_HELD_EVENT, MailHeld, PtyPort,
};
pub use env::{DeliveryEnv, DraftState, UnreadMail};
pub use facts::{GateFacts, PhaseFacts};
pub use gate::{Refusal, RefusalCode, at_safe_stopping_point, can_send};
pub use paste::{SUBMIT, paste_bytes};
pub use paste_mode::PasteModeTracker;
pub use runner_pty::RunnerPty;
pub use send::{SendEvent, SendOpts, SendStart, Sender};
pub use service::{DeliveryHandle, DeliveryService, event_for_notice};
pub use shared::Shared;
