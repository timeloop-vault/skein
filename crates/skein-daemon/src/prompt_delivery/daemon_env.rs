//! The real [`DeliveryEnv`] (#733): the delivery worker's eyes and hands on
//! the daemon. Everything but the PTY is the real component; the PTY sits
//! behind [`PtyPort`] so a test can record what would be written.
//!
//! Lock order: the worker is the only caller. It reads the phase service
//! (its lock, briefly), the room store (its lock, briefly), the database and
//! this module's [`Shared`] mutexes, one at a time; it never holds one while
//! taking another. The reverse direction (phase hooks, the PTY coalescer, the
//! client) only ever sends on a channel or takes a single [`Shared`] mutex.

use std::sync::Arc;

use serde::Serialize;
use skein_proto::harness_kind::HarnessKind;

use super::context_nudge::Mark;
use super::env::{ContextSettings, DeliveryEnv, DraftState, UnreadMail};
use super::facts::{GateFacts, PhaseFacts};
use super::paste::SUBMIT;
use super::shared::Shared;
use crate::agent_api::verbs::unread_mail_impl;
use crate::db::Database;
use crate::event_sink::{self, SharedEventSink};
use crate::harness_argv::runs_its_tool;
use crate::harness_phase::PhaseService;
use crate::room_store::RoomStore;
use crate::spawn_env::state::SpawnEnvState;

/// Broadcast when a harness's mail starts or stops being held by the draft
/// guard; payload [`MailHeld`].
pub const MAIL_HELD_EVENT: &str = "skein://mail-held";

/// Broadcast after the daemon wrote a paste (`"paste"`) or an Enter
/// (`"submit"`) into a harness's PTY; payload [`DeliverySeam`]. The
/// webview's composer-draft tracker needs it to know text it did not type
/// is in the composer.
pub const DELIVERY_SEAM_EVENT: &str = "skein://delivery-seam";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverySeam {
    pub harness_id: String,
    pub event: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailHeld {
    pub harness_id: String,
    pub held: bool,
}

/// What the worker needs of a PTY.
pub trait PtyPort: Send + Sync {
    /// The id of the harness's live PTY (the spawn token); `None` when it
    /// has no process or the process has exited.
    fn live_pty(&self, harness_id: &str) -> Option<String>;
    /// `false` when the write failed.
    fn write(&self, pty_id: &str, bytes: &[u8]) -> bool;
    /// The terminal engine's bracketed-paste flag for the harness's live
    /// PTY (#809). `None` when there is no live PTY or it has no engine
    /// (raw mode); the caller then uses its own tracker.
    fn bracketed_paste(&self, harness_id: &str) -> Option<bool>;
}

/// A harness's unread mail, read fresh from the database (never marks
/// read). The delivery env and the supervisor's mail facts (#733 part B)
/// read it through this one function. `None` when the harness is unknown
/// or the lookup failed.
#[must_use]
pub fn unread_for(store: &RoomStore, db: &Database, harness_id: &str) -> Option<UnreadMail> {
    let (room_id, _) = store.find_harness(harness_id)?;
    let m = unread_mail_impl(db, &room_id, harness_id).ok()?;
    Some(UnreadMail {
        count: u32::try_from(m.count).unwrap_or(u32::MAX),
        from: m.from_room_names,
    })
}

pub struct DaemonEnv {
    pub store: Arc<RoomStore>,
    pub phase: Arc<PhaseService>,
    pub db: Arc<Database>,
    pub events: SharedEventSink,
    pub pty: Arc<dyn PtyPort>,
    pub shared: Arc<Shared>,
    /// The live spawn settings: the context limit and its nudge (#689).
    pub spawn_env: Arc<SpawnEnvState>,
}

impl DeliveryEnv for DaemonEnv {
    fn gate_facts(&self, harness_id: &str, now_ms: i64) -> GateFacts {
        let record = self.store.find_harness(harness_id);
        let kind = record
            .as_ref()
            .and_then(|(_, h)| HarnessKind::from_name(&h.kind));
        let (last_output_at, tracked_paste) = self.shared.output_facts(harness_id);
        // Frames mode: the engine knows; raw mode: the tracker (#809).
        let bracketed_paste = self
            .pty
            .bracketed_paste(harness_id)
            .unwrap_or(tracked_paste);
        GateFacts {
            // Without a stored record of a known kind nothing is live: the
            // placeholder kind is never reached past `pty_live`.
            kind: kind.unwrap_or(HarnessKind::Byoh),
            pty_live: kind.is_some() && self.pty.live_pty(harness_id).is_some(),
            // From the STORED record, so a post-exit shell gets nothing.
            runs_its_tool: record.as_ref().is_some_and(|(_, h)| runs_its_tool(h)),
            phase: self.phase.get(harness_id).as_ref().map(PhaseFacts::from),
            last_output_at,
            bracketed_paste,
            now_ms,
        }
    }

    fn spawn_token(&self, harness_id: &str) -> Option<String> {
        self.pty.live_pty(harness_id)
    }

    fn user_input_seq(&self, harness_id: &str) -> u64 {
        self.shared.client_facts(harness_id).1
    }

    fn draft(&self, harness_id: &str) -> DraftState {
        self.shared.client_facts(harness_id).0
    }

    fn write_pty(&mut self, harness_id: &str, bytes: &[u8]) -> bool {
        let ok = self
            .pty
            .live_pty(harness_id)
            .is_some_and(|id| self.pty.write(&id, bytes));
        if ok {
            let event = if bytes == SUBMIT { "submit" } else { "paste" };
            let payload = DeliverySeam {
                harness_id: harness_id.to_owned(),
                event,
            };
            if let Err(e) = event_sink::emit(self.events.as_ref(), DELIVERY_SEAM_EVENT, &payload) {
                tracing::warn!(harness = harness_id, error = %e, "prompt delivery: delivery-seam emit failed");
            }
        }
        ok
    }

    fn note_prompt_submitted(&mut self, harness_id: &str) {
        self.phase.prompt_submitted(harness_id);
    }

    fn recover_unheard_silence(&mut self, harness_id: &str) {
        self.phase.recover_unheard_silence(harness_id);
    }

    fn unread(&self, harness_id: &str) -> Option<UnreadMail> {
        unread_for(&self.store, &self.db, harness_id)
    }

    fn publish_mail_facts(&mut self, harness_id: &str, facts: super::env::MailFacts) {
        self.shared.set_mail_facts(harness_id, facts);
    }

    fn publish_held(&mut self, harness_id: &str, held: bool) {
        self.shared.set_held(harness_id, held);
        let payload = MailHeld {
            harness_id: harness_id.to_owned(),
            held,
        };
        if let Err(e) = event_sink::emit(self.events.as_ref(), MAIL_HELD_EVENT, &payload) {
            tracing::warn!(harness = harness_id, error = %e, "prompt delivery: mail-held emit failed");
        }
    }

    fn context_settings(&self) -> ContextSettings {
        let s = self.spawn_env.snapshot();
        ContextSettings {
            limit: s.context_threshold,
            enabled: s.context_nudge.enabled,
            body: s.context_nudge.body,
        }
    }

    fn newest_context(&self, harness_id: &str) -> Result<Option<(i64, u64)>, String> {
        self.db.newest_context_tokens(harness_id)
    }

    fn context_mark(&self, harness_id: &str) -> Result<Option<Mark>, String> {
        Ok(self.db.context_nudge_mark(harness_id)?.map(|row| Mark {
            last_row_id: row.last_row_id,
            armed: row.armed,
        }))
    }

    fn set_context_mark(&mut self, harness_id: &str, mark: Mark) -> Result<(), String> {
        let (room_id, _) = self
            .store
            .find_harness(harness_id)
            .ok_or_else(|| "harness is not in the room store".to_owned())?;
        self.db
            .set_context_nudge_mark(harness_id, &room_id, mark.last_row_id, mark.armed)
    }
}
