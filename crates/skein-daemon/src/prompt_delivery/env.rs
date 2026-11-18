//! Every fact the send machine reads and every effect it has, behind one
//! trait: tests fake it, the delivery driver implements it.

use super::context_nudge::Mark;
use super::facts::GateFacts;
pub use super::shared::MailFacts;

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

/// The user's context-limit settings (#689), read live on every check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextSettings {
    /// The limit in tokens; `None` = off.
    pub limit: Option<u64>,
    pub enabled: bool,
    /// `None` = the built-in text.
    pub body: Option<String>,
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
    /// The daemon records its own submit (`PhaseService::prompt_submitted`;
    /// formerly TS `notePromptSubmitted`).
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
    /// What the daemon's supervisor reads about this harness's mail (#733
    /// part B): the time of the last nudge pasted and the refusal that
    /// holds pending mail. All-`None` means the worker forgot the harness.
    fn publish_mail_facts(&mut self, harness_id: &str, facts: MailFacts);
    /// The context-limit settings, read live.
    fn context_settings(&self) -> ContextSettings;
    /// The newest `(context row id, context tokens)` of the harness.
    /// `Ok(None)`: no row, or a kind without a context size.
    ///
    /// # Errors
    /// The lookup failed; the caller does nothing.
    fn newest_context(&self, harness_id: &str) -> Result<Option<(i64, u64)>, String>;
    /// The harness's persisted context-nudge mark.
    ///
    /// # Errors
    /// The lookup failed; the caller does nothing.
    fn context_mark(&self, harness_id: &str) -> Result<Option<Mark>, String>;
    /// Persist the mark.
    ///
    /// # Errors
    /// The write failed (or the harness is unknown to the store).
    fn set_context_mark(&mut self, harness_id: &str, mark: Mark) -> Result<(), String>;
}
