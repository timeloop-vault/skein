//! The shared fake `DeliveryEnv` for the prompt-delivery tests.

use std::sync::Arc;

use skein_proto::harness_kind::HarnessKind;

use super::context_nudge::Mark;
use super::env::{ContextSettings, DeliveryEnv, DraftState, MailFacts, UnreadMail};
use super::facts::{GateFacts, PhaseFacts};
use super::shared::Shared;
use crate::harness_phase::ActivityPhase;

pub const T0: i64 = 1_000_000;

pub struct Fake {
    pub facts: GateFacts,
    pub token: Option<String>,
    pub seq: u64,
    pub draft: DraftState,
    pub writes: Vec<Vec<u8>>,
    pub write_ok: bool,
    pub submitted: u32,
    pub recovered: u32,
    pub unread: Option<UnreadMail>,
    /// Every `publish_held` call, in order.
    pub held_log: Vec<bool>,
    /// What `publish_mail_facts` wrote, as the real env does.
    pub shared: Arc<Shared>,
    /// The context-limit settings (#689).
    pub context: ContextSettings,
    pub context_row: Option<(i64, u64)>,
    /// The persisted mark; survives a new `DeliveryCore`.
    pub mark: Option<Mark>,
}

pub fn sendable_phase() -> PhaseFacts {
    PhaseFacts {
        phase: ActivityPhase::Waiting,
        question_open: false,
        delegation_deferred_at: None,
        authoritative: true,
        adapter_heard: true,
        adapter_silent: false,
        launch_signal_at: None,
        injected: true,
    }
}

pub fn fake(kind: HarnessKind) -> Fake {
    Fake {
        facts: GateFacts {
            kind,
            pty_live: true,
            runs_its_tool: true,
            phase: Some(sendable_phase()),
            last_output_at: None,
            bracketed_paste: false,
            now_ms: T0,
        },
        token: Some("pty-1".into()),
        seq: 0,
        draft: DraftState::Clean,
        writes: Vec::new(),
        write_ok: true,
        submitted: 0,
        recovered: 0,
        unread: None,
        held_log: Vec::new(),
        shared: Arc::default(),
        context: ContextSettings {
            limit: None,
            enabled: true,
            body: None,
        },
        context_row: None,
        mark: None,
    }
}

impl Fake {
    pub fn set_phase(&mut self, f: impl FnOnce(&mut PhaseFacts)) {
        f(self.facts.phase.as_mut().unwrap());
    }
    pub fn enters(&self) -> usize {
        self.writes.iter().filter(|w| w.as_slice() == b"\r").count()
    }
}

impl DeliveryEnv for Fake {
    fn gate_facts(&self, _: &str, now_ms: i64) -> GateFacts {
        GateFacts {
            now_ms,
            ..self.facts
        }
    }
    fn spawn_token(&self, _: &str) -> Option<String> {
        self.token.clone()
    }
    fn user_input_seq(&self, _: &str) -> u64 {
        self.seq
    }
    fn draft(&self, _: &str) -> DraftState {
        self.draft.clone()
    }
    fn write_pty(&mut self, _: &str, bytes: &[u8]) -> bool {
        if self.write_ok {
            self.writes.push(bytes.to_vec());
        }
        self.write_ok
    }
    fn note_prompt_submitted(&mut self, _: &str) {
        self.submitted += 1;
    }
    fn recover_unheard_silence(&mut self, _: &str) {
        self.recovered += 1;
        self.facts.phase = Some(sendable_phase());
    }
    fn unread(&self, _: &str) -> Option<UnreadMail> {
        self.unread.clone()
    }
    fn publish_held(&mut self, _: &str, held: bool) {
        self.held_log.push(held);
    }
    fn publish_mail_facts(&mut self, id: &str, facts: MailFacts) {
        self.shared.set_mail_facts(id, facts);
    }
    fn context_settings(&self) -> ContextSettings {
        self.context.clone()
    }
    fn newest_context(&self, _: &str) -> Result<Option<(i64, u64)>, String> {
        Ok(self.context_row)
    }
    fn context_mark(&self, _: &str) -> Result<Option<Mark>, String> {
        Ok(self.mark)
    }
    fn set_context_mark(&mut self, _: &str, mark: Mark) -> Result<(), String> {
        self.mark = Some(mark);
        Ok(())
    }
}
