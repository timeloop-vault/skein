//! What an agent can actually do.
//!
//! Mostly plain functions over a [`Database`] and a [`Caller`]. Nothing
//! here knows about HTTP or MCP, which is what makes the interesting
//! properties — room scoping, the resolve prohibition, attribution —
//! testable without a server or a Tauri runtime. [`create_room`] (#330)
//! is the one exception that needs more than that: opening a room means
//! asking the webview to do it (#328's `request_frontend`), so it also
//! takes an [`super::state::AgentApiState`] — itself usable with no
//! Tauri runtime via `AgentApiState::for_test`, so the "no server
//! needed" property still holds in tests.
//!
//! Two rules run through all of them:
//!
//! * **The caller's room is the only room.** Every verb that names a
//!   thread re-reads that thread and compares its `room_id`. A thread
//!   id guessed from another room is [`VerbError::NotFound`], not a
//!   silent read of someone else's review.
//! * **Anchoring is not reimplemented here.** Where a comment sits
//!   *now* comes from [`crate::review_surface::query::file_impl`], the
//!   same call the pane makes, so the agent and the user are never
//!   looking at two different answers to the same question.
//!
//! # Where things are
//!
//! | File | The question it answers |
//! |---|---|
//! | `verbs/review.rs` | What the reviewer said, what the diff under review is, and has it been signed off. |
//! | `verbs/review_ctx.rs` | How a thread is looked up in the caller's room, and the per-room reads a comment listing needs. |
//! | `verbs/find_rooms.rs` | Which rooms own, contain or sit under a path (#354). |
//! | `verbs/mail.rs` | How a harness sends, reads and pages back through mail (#327, #364). |
//! | `verbs/mail_routing.rs` | Where mail goes: recipient resolution, refusals, and recording and emitting a send. |
//! | `verbs/room_create.rs` | How an agent opens a whole new room (#330). |
//! | `verbs/room_close.rs` | How an agent archives a room it created (#411). |
//! | `verbs/harness_control.rs` | How an agent opens or closes a harness in a room (#411). |
//! | `verbs/design.rs` | How an agent reads and drives a room's design pane (#512). |
//! | `verbs/design_changes.rs` | `show_changes`: outline the elements a diff scope touches (#547). |
//! | `verbs/room_listing.rs` | The cross-room reads: rooms, one room, a room's harnesses (#356). |
//! | `verbs/info.rs` | Which Skein build the agent runs under (#535). |
//! | `verbs/shared.rs` | Constants and helpers more than one family uses. |
//!
//! This file keeps [`VerbError`] and re-exports every verb, argument
//! and result type, so callers keep writing `verbs::X`.

mod design;
mod design_changes;
mod find_rooms;
mod harness_control;
mod info;
mod mail;
mod mail_routing;
mod review;
mod review_ctx;
mod room_close;
mod room_create;
mod room_listing;
mod shared;

pub use design::*;
pub use design_changes::*;
pub use find_rooms::*;
pub use harness_control::*;
pub use info::*;
pub use mail::*;
pub use review::*;
pub use room_close::*;
pub use room_create::*;
pub use room_listing::*;

/// Why a verb refused. The HTTP and MCP layers each map these into
/// their own vocabulary; the verbs themselves only say what went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerbError {
    /// No such thread *in this room* — the two cases are deliberately
    /// indistinguishable to the caller, so a token cannot be used to
    /// probe another room for thread ids.
    NotFound(String),
    /// The request was understood and refused.
    Refused(String),
    /// The room has no worktree, or git could not answer.
    Unavailable(String),
    /// Something below us failed.
    Internal(String),
}

impl VerbError {
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(m) | Self::Refused(m) | Self::Unavailable(m) | Self::Internal(m) => m,
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::Refused(_) => 403,
            Self::Unavailable(_) => 409,
            Self::Internal(_) => 500,
        }
    }
}

type VerbResult<T> = Result<T, VerbError>;

pub(super) fn internal(e: impl std::fmt::Display) -> VerbError {
    VerbError::Internal(e.to_string())
}
