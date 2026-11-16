//! Every fact the send machine reads and every effect it has, behind one
//! trait: tests fake it, the delivery driver implements it.

use super::facts::GateFacts;

/// The client's last reported composer draft (TS `checkDraft`'s input,
/// already judged by the client).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftState {
    Clean,
    /// The client's `checkDraft` refusal text, passed through verbatim.
    Held {
        reason: String,
    },
}

/// What a harness's mailbox holds unread, in the shape `mailNudgeText`
/// needs (the daemon's `agent_api::verbs::mail::MailUnread`, with the
/// count narrowed: `count` as `u32::try_from(i64).unwrap_or(u32::MAX)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadMail {
    pub count: u32,
    /// Distinct sender room names, first-seen order (`from_room_names`).
    pub from: Vec<String>,
}

pub trait DeliveryEnv {
    fn gate_facts(&self, harness_id: &str, now_ms: i64) -> GateFacts;
    /// Per-spawn identity (the PTY id); `None` when no live PTY. Replaces
    /// the TS "target identity".
    fn spawn_token(&self, harness_id: &str) -> Option<String>;
    /// Monotonic count of user input the client reported (TS
    /// `userInputCount`); 0 with no client.
    fn user_input_seq(&self, harness_id: &str) -> u64;
    /// The client's last reported composer draft; `Clean` when no client
    /// ever reported.
    fn draft(&self, harness_id: &str) -> DraftState;
    /// Write to the PTY. `false` when the write failed.
    fn write_pty(&mut self, harness_id: &str, bytes: &[u8]) -> bool;
    /// The daemon records its own submit (`PromptSubmitted`; TS
    /// `notePromptSubmitted`).
    fn note_prompt_submitted(&mut self, harness_id: &str);
    /// TS `harnessActivity.recoverUnheardSilence` (#404). The send machine
    /// never calls it (the TS send flow doesn't either: the mail layer
    /// does, after a settlement rollback); it is here for that layer.
    fn recover_unheard_silence(&mut self, harness_id: &str);
    /// The harness's unread mail (a read-only lookup; never marks read).
    /// `None` when the harness is unknown or the lookup failed.
    fn unread(&self, harness_id: &str) -> Option<UnreadMail>;
    /// The UI's "mail held by the draft guard" marker. Called only when
    /// the held flag changes.
    fn publish_held(&mut self, harness_id: &str, held: bool);
}
