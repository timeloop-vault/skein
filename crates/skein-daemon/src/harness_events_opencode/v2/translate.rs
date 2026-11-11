//! Pure v2 → `OpencodeEvent` translation (#677). No I/O: the one thing it
//! cannot decide alone, whether a session is a root or a subagent child, is
//! handed back to the connection as `Translation::NeedsLookup`.

use std::collections::HashMap;

use skein_harness::opencode::v2::{FormOutcome, SessionStatus, V2Event};

use super::super::OpencodeEvent;

/// What translating one event yields.
#[derive(Debug)]
pub(super) enum Translation {
    /// Zero or more events to emit, in order.
    Emit(Vec<OpencodeEvent>),
    /// The event is root-sensitive and this session's root/child status is
    /// unknown. Look it up, `record` the answer, then translate again (or
    /// `translate_as_child` if the lookup failed).
    NeedsLookup(String),
}

/// Per-adapter root/child knowledge. Lives across reconnects: a
/// `session.created` is seen once, so a reconnect must not forget it.
#[derive(Debug, Default)]
pub(super) struct V2Translator {
    roots: HashMap<String, bool>,
}

impl V2Translator {
    /// `own_session` is the harness's own attached session: Skein only ever
    /// captures or resumes a root, so it is known without a lookup.
    pub(super) fn new(own_session: Option<&str>) -> Self {
        let mut t = Self::default();
        if let Some(sid) = own_session.filter(|s| !s.is_empty()) {
            t.roots.insert(sid.to_owned(), true);
        }
        t
    }

    pub(super) fn record(&mut self, session_id: &str, is_root: bool) {
        self.roots.insert(session_id.to_owned(), is_root);
    }

    pub(super) fn translate(&mut self, event: V2Event) -> Translation {
        if let V2Event::SessionCreated {
            session_id,
            parent_id,
        } = &event
        {
            self.roots.insert(session_id.clone(), parent_id.is_none());
        }
        match root_sensitive_session(&event) {
            None => Translation::Emit(emit(event, false)),
            Some(sid) => match self.roots.get(sid) {
                Some(&is_root) => Translation::Emit(emit(event, is_root)),
                None => Translation::NeedsLookup(sid.to_owned()),
            },
        }
    }

    /// A failed lookup: the safe reading, since a child never emits idle.
    /// Deliberately not cached, so a later event can try the lookup again.
    /// An event treated as a child after a failed lookup can never produce `SessionIdle`,
    /// so a root that loses its idle this way stays running until its next idle: the
    /// safe direction (a false "your turn" is worse).
    #[allow(clippy::unused_self)]
    pub(super) fn translate_as_child(&self, event: V2Event) -> Vec<OpencodeEvent> {
        emit(event, false)
    }
}

/// The session whose root/child status decides this event's meaning.
fn root_sensitive_session(event: &V2Event) -> Option<&str> {
    match event {
        V2Event::ExecutionStarted { session_id }
        | V2Event::ExecutionEnded { session_id, .. }
        | V2Event::SessionStatus { session_id, .. }
        | V2Event::UserPrompt { session_id, .. } => Some(session_id),
        _ => None,
    }
}

fn emit(event: V2Event, is_root: bool) -> Vec<OpencodeEvent> {
    let one = |e| vec![e];
    match event {
        V2Event::FormCreated {
            form_id,
            session_id: Some(session_id),
            kind: Some(kind),
        } if kind == "question" => one(OpencodeEvent::QuestionAsked {
            request_id: form_id,
            session_id,
        }),
        V2Event::SessionCreated {
            session_id,
            parent_id,
        } => one(OpencodeEvent::SessionCreated {
            session_id,
            parent_id,
        }),
        V2Event::ExecutionStarted { .. } => one(if is_root {
            OpencodeEvent::SessionBusy
        } else {
            OpencodeEvent::MessageDelta
        }),
        V2Event::ExecutionEnded { .. } if is_root => one(OpencodeEvent::SessionIdle),
        V2Event::SessionStatus { status, .. } => match (status, is_root) {
            (SessionStatus::Idle, true) => one(OpencodeEvent::SessionIdle),
            (SessionStatus::Idle, false) => Vec::new(),
            (SessionStatus::Busy | SessionStatus::Retry, true) => one(OpencodeEvent::SessionBusy),
            (SessionStatus::Busy | SessionStatus::Retry, false) => one(OpencodeEvent::MessageDelta),
        },
        V2Event::UserPrompt { session_id, .. } if is_root => {
            one(OpencodeEvent::RootSessionPrompted { session_id })
        }
        V2Event::Streaming { .. } => one(OpencodeEvent::MessageDelta),
        V2Event::ToolInputStarted { name, .. } => one(OpencodeEvent::ToolUseStart { name }),
        V2Event::PermissionAsked {
            request_id,
            session_id,
        } => one(OpencodeEvent::PermissionAsked {
            request_id,
            session_id,
        }),
        V2Event::PermissionReplied {
            request_id,
            session_id,
        } => one(OpencodeEvent::PermissionReplied {
            request_id,
            session_id,
        }),
        V2Event::FormResolved {
            form_id,
            session_id: Some(session_id),
            outcome: FormOutcome::Replied | FormOutcome::Cancelled,
        } => one(OpencodeEvent::QuestionResolved {
            request_id: form_id,
            session_id,
        }),
        V2Event::AgentSelected { session_id, agent } => {
            one(OpencodeEvent::UserMessageAgent { session_id, agent })
        }
        // Everything else says nothing: tool calls/ends, a non-question or
        // sessionless form, a child's end/prompt, the connect banner.
        V2Event::ServerConnected
        | V2Event::ToolCalled { .. }
        | V2Event::ToolEnded { .. }
        | V2Event::FormCreated { .. }
        | V2Event::FormResolved { .. }
        | V2Event::ExecutionEnded { .. }
        | V2Event::UserPrompt { .. } => Vec::new(),
    }
}
