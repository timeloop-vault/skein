//! Room persistence — sqlite, one row per room, JSON blob.
//!
//! The schema is deliberately minimal: we don't query individual fields
//! today, and storing the Room as a JSON blob lets the TS shape evolve
//! without schema migrations. We pay for that with no SQL-level queries
//! over fields like `repo` or `branch` — when a phase needs that, we can
//! split the blob into proper columns.
//!
//! Save semantics: `save_all` is a wipe + re-insert inside one transaction.
//! Cheap at prototype scale (a dozen rows of <1 KB each) and frees the
//! frontend from tracking which rooms changed.
//!
//! The sqlite table is still called `sessions` for legacy reasons —
//! pre-chapter-6 the Skein concept was called "session" and renaming
//! the table would need a migration for cosmetic gain. JSON blobs
//! inside don't carry the table name.
//!
//! Epic #50 L6 adds a second table — `harness_events` — that keeps an
//! append-only log of every harness phase transition. Foundation for
//! L7 (cross-harness activity feed) and a longer-term "since last
//! visit" surface. The TS side writes per transition via
//! `db_record_harness_event`; reads come back via the `recent_*`
//! query commands.
//!
//! Issue #80 ("Live Context") adds a third table — `harness_actions` —
//! that keeps the richer per-tool-call / per-plan-change / per-patch
//! log feeding the right-pane card stack. Phase transitions stay in
//! `harness_events`; everything else lands here with a `kind`
//! discriminator and a JSON `payload`. Rationale and the v1 kind set
//! are in `docs/live-context-recon.md` §4 and the design brief.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use rusqlite::Connection;

mod agent_tokens;
mod element_anchors;
mod element_proposals;
mod harness_log;
mod mail;
mod review_baselines;
mod review_threads;
mod rooms;
mod schema;
mod snapshots;
mod sweep;
#[cfg(test)]
mod test_support;

pub use agent_tokens::TokenLookup;
pub use element_anchors::ReviewElementAnchorRow;
pub use harness_log::{HarnessAction, HarnessEvent, NewHarnessAction, action_kind};
pub use mail::HarnessMessageRow;
pub use review_threads::{ReviewAddressedRow, ReviewCommentRow, ReviewThreadRow};
pub use rooms::{CreatedBy, Harness, LoadOutcome, RepoIdentity, Room};
pub(crate) use snapshots::replace_file;

/// Per-room ceiling on the total size of `review_baseline_images` rows
/// (#409 follow-up). A mirror is a preview convenience, not the review
/// record itself, so a room that keeps generating large "before" images
/// must not be allowed to grow `skein.db` without bound — a write that
/// would push the room over this cap drops the mirror for that path
/// instead of storing it (see `Database::set_review_baseline_image`).
pub(crate) const MAX_ROOM_IMAGE_MIRROR_BYTES: u64 = 128 * 1024 * 1024;

pub struct Database {
    conn: Mutex<Connection>,
    path: PathBuf,
    /// Set once `load_all` has completed successfully in this process.
    /// `save_all` refuses to commit an empty room list before that —
    /// the frontend only legitimately saves `[]` after a good load
    /// (issue #167: a failed boot load must never wipe the table).
    loaded_ok: AtomicBool,
    /// Ticket source for `save_all_seq` (#171). `db_save_rooms` became
    /// an async command so it could move sqlite's wipe-and-reinsert
    /// off the main thread — but the frontend fires it un-debounced on
    /// every `rooms` state change without awaiting the previous call,
    /// so two saves can now reach the connection lock out of order.
    /// Each save mints a ticket before its blocking work starts; see
    /// `save_all_seq`.
    save_seq: AtomicU64,
    /// The ticket of the last save that actually committed, held for
    /// the whole check-then-write so it can't race a concurrent save's
    /// own check (#171). `Mutex<u64>` rather than an atomic: the guard
    /// serializes seq-aware saves against each other for their full
    /// duration, not just the compare.
    last_saved_seq: Mutex<u64>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        // WAL keeps readers and the (single) writer from blocking each
        // other and survives crash-mid-write without a hot journal;
        // busy_timeout papers over transient contention instead of
        // surfacing SQLITE_BUSY to the user. (#167 belt-and-braces;
        // #178 tunes the rest of the write path.)
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        // Fold any WAL left by an unclean previous exit back into the
        // main file and truncate the sidecar. Keeps skein.db-wal
        // near-empty at rest, so hand-restoring `.bak` over skein.db
        // (or deleting skein.db alone) can't pair a stale hot WAL
        // with the wrong database file (#167 review).
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
            loaded_ok: AtomicBool::new(false),
            save_seq: AtomicU64::new(0),
            last_saved_seq: Mutex::new(0),
        })
    }

    /// Mint the next ticket for a `save_all_seq` call (#171). Callers
    /// mint this *before* the blocking work starts, which is what lets
    /// `save_all_seq` detect a save that reaches the connection lock
    /// after a newer one already committed — the realistic case, since
    /// two overlapping saves racing sqlite is far more likely than
    /// their tickets minting out of order. Minting itself happens at
    /// the top of the async command body, not at dispatch time, so a
    /// work-stealing runtime gives no guarantee that two concurrently
    /// dispatched saves mint in the order the frontend fired them —
    /// that residual window is a few instructions wide and accepted.
    pub fn next_save_seq(&self) -> u64 {
        self.save_seq.fetch_add(1, Ordering::Relaxed)
    }

    // ── harness event log (epic #50 L6) ──────────────────────────

    // ── harness action log (issue #80) ────────────────────────────

    // ── review baselines (issue #211) ─────────────────────────────

    // ── review comments (issue #212) ──────────────────────────────

    // ── the agent API (issue #213) ────────────────────────────────

    // ── viewed markers and settings (issue #212) ──────────────────

    // ── the reviewer's sign-off (#214) ────────────────────────────

    // ── the mailbox (issue #327) ───────────────────────────────────
}
