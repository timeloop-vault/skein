//! The facts the delivery worker reads that other threads write (#733),
//! each behind its own short mutex: nothing here is held across a call
//! into another component, so no lock order exists to get wrong.
//!
//! - the PTY output tap (paste mode, last output), written from the PTY's
//!   coalescer thread, never through the worker's channel;
//! - the client's last reported draft and input count;
//! - the set of harnesses whose mail is held by the draft guard.

use std::collections::{BTreeSet, HashMap};

use parking_lot::Mutex;

use super::env::DraftState;
use super::paste_mode::PasteModeTracker;

#[derive(Debug, Default)]
struct OutputEntry {
    paste: PasteModeTracker,
    last_output_at: Option<i64>,
}

#[derive(Debug)]
struct ClientEntry {
    draft: DraftState,
    user_input_seq: u64,
    /// The client's own report number: only a greater one replaces this.
    report: u64,
}

/// What the daemon's supervisor reads about a harness's mail (#733 part B):
/// the worker publishes it, the supervisor thread reads it without ever
/// asking the worker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailFacts {
    /// Epoch ms of the last mail nudge actually pasted (a rolled-back one
    /// is taken back out).
    pub last_nudge_at: Option<i64>,
    /// Why mail is still undelivered (the gate or draft refusal); `None`
    /// once nothing is pending.
    pub last_refusal: Option<String>,
}

#[derive(Debug, Default)]
pub struct Shared {
    output: Mutex<HashMap<String, OutputEntry>>,
    client: Mutex<HashMap<String, ClientEntry>>,
    held: Mutex<BTreeSet<String>>,
    mail: Mutex<HashMap<String, MailFacts>>,
}

impl Shared {
    pub fn note_output(&self, harness_id: &str, chunk: &str, now_ms: i64) {
        let mut out = self.output.lock();
        let e = out.entry(harness_id.to_owned()).or_default();
        e.paste.feed(chunk);
        e.last_output_at = Some(now_ms);
    }

    /// `(last output epoch ms, bracketed paste on)`; `(None, false)` for a
    /// harness that has produced nothing since its spawn.
    pub fn output_facts(&self, harness_id: &str) -> (Option<i64>, bool) {
        self.output
            .lock()
            .get(harness_id)
            .map_or((None, false), |e| (e.last_output_at, e.paste.is_on()))
    }

    /// Stores a client report unless it is older than the stored one (the
    /// webview's `report` numbers rise per harness; invokes can still arrive
    /// out of order). Returns whether the mail check is worth running: the
    /// first report since the spawn, or a draft going Held -> Clean.
    pub fn set_client(
        &self,
        harness_id: &str,
        draft: DraftState,
        user_input_seq: u64,
        report: u64,
    ) -> bool {
        let mut map = self.client.lock();
        let notify = match map.get(harness_id) {
            Some(prev) if prev.report >= report => return false,
            Some(prev) => {
                matches!(prev.draft, DraftState::Held { .. }) && draft == DraftState::Clean
            }
            None => true,
        };
        map.insert(
            harness_id.to_owned(),
            ClientEntry {
                draft,
                user_input_seq,
                report,
            },
        );
        notify
    }

    /// Never reported = `Clean` / 0.
    pub fn client_facts(&self, harness_id: &str) -> (DraftState, u64) {
        self.client
            .lock()
            .get(harness_id)
            .map_or((DraftState::Clean, 0), |e| {
                (e.draft.clone(), e.user_input_seq)
            })
    }

    /// A new process or a removed harness: the old process's output state
    /// and the client's reports about it are stale.
    pub fn reset(&self, harness_id: &str) {
        self.output.lock().remove(harness_id);
        self.client.lock().remove(harness_id);
    }

    /// Returns whether the set changed.
    pub fn set_held(&self, harness_id: &str, held: bool) -> bool {
        let mut set = self.held.lock();
        if held {
            set.insert(harness_id.to_owned())
        } else {
            set.remove(harness_id)
        }
    }

    pub fn held_snapshot(&self) -> Vec<String> {
        self.held.lock().iter().cloned().collect()
    }

    /// Stores a harness's mail facts; an all-empty value removes the entry,
    /// so the map holds only harnesses the worker still tracks.
    pub fn set_mail_facts(&self, harness_id: &str, facts: MailFacts) {
        let mut map = self.mail.lock();
        if facts == MailFacts::default() {
            map.remove(harness_id);
        } else {
            map.insert(harness_id.to_owned(), facts);
        }
    }

    /// Never published (or forgotten) = all `None`.
    pub fn mail_facts(&self, harness_id: &str) -> MailFacts {
        self.mail
            .lock()
            .get(harness_id)
            .cloned()
            .unwrap_or_default()
    }

    /// How many harnesses have a mail entry (a bound check for tests).
    #[cfg(test)]
    pub fn mail_entries(&self) -> usize {
        self.mail.lock().len()
    }
}
