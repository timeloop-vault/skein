//! opencode v2 (2.x) `GET /api/event` telemetry: SSE framing plus a parser
//! from one frame's JSON to the events Skein surfaces (#677).
//!
//! A frame is `{id, created, type, location?, data, durable?, metadata?}`; the
//! payload is under `data`, not v1's `properties`. Upstream types the stream
//! as an opaque string, so the captured fixtures in `v2/fixtures/` are the
//! contract. Shapes that were never seen live are marked "schema-derived"
//! where they are parsed. Tool shapes are reusable by the action-row issue
//! (#678). Pure: no I/O, no Tauri.

use serde_json::Value;

#[cfg(test)]
mod fixture_tests;
mod sse;
#[cfg(test)]
mod tests;

pub use sse::drain_sse_data;

/// How a session execution ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionOutcome {
    Succeeded,
    Failed,
    Interrupted,
}

/// `session.status` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Busy,
    Retry,
}

/// How a form was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormOutcome {
    Replied,
    Cancelled,
}

/// One v2 event Skein surfaces. Unknown types never become one.
#[derive(Debug, Clone, PartialEq)]
pub enum V2Event {
    ServerConnected,
    SessionCreated {
        session_id: String,
        parent_id: Option<String>,
    },
    ExecutionStarted {
        session_id: String,
    },
    ExecutionEnded {
        session_id: String,
        outcome: ExecutionOutcome,
    },
    SessionStatus {
        session_id: String,
        status: SessionStatus,
    },
    /// A user prompt entered the session's inbox.
    UserPrompt {
        session_id: String,
        inbox_id: String,
    },
    /// Model output (text or reasoning) is still flowing.
    Streaming {
        session_id: String,
    },
    ToolInputStarted {
        session_id: String,
        call_id: String,
        name: String,
    },
    ToolCalled {
        session_id: String,
        call_id: String,
        input: Value,
    },
    ToolEnded {
        session_id: String,
        call_id: String,
        error: bool,
    },
    PermissionAsked {
        request_id: String,
        session_id: String,
    },
    PermissionReplied {
        request_id: String,
        session_id: String,
    },
    FormCreated {
        form_id: String,
        /// Absent for an MCP elicitation form.
        session_id: Option<String>,
        /// `metadata.kind`, e.g. `"question"`.
        kind: Option<String>,
    },
    FormResolved {
        form_id: String,
        session_id: Option<String>,
        outcome: FormOutcome,
    },
    AgentSelected {
        session_id: String,
        agent: String,
    },
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn field(data: &Value, key: &str) -> Option<String> {
    data.get(key).and_then(text)
}

/// Parses one frame's JSON payload. `None` for malformed JSON, for types we do
/// not surface, and for a session-scoped frame with no `sessionID`.
#[must_use]
pub fn parse_event(payload: &str) -> Option<V2Event> {
    let frame: Value = serde_json::from_str(payload).ok()?;
    let ty = frame.get("type")?.as_str()?;
    let empty = Value::Null;
    let data = frame.get("data").unwrap_or(&empty);
    if ty == "server.connected" {
        return Some(V2Event::ServerConnected);
    }
    if let Some(kind) = ty.strip_prefix("form.") {
        return parse_form(kind, data);
    }
    let session_id = field(data, "sessionID")?;
    let call_id = || field(data, "id");
    Some(match ty {
        "session.created" => V2Event::SessionCreated {
            session_id,
            // schema-derived, not captured: the 2.2.0 capture is a root session.
            parent_id: field(data, "parentID"),
        },
        "session.execution.started" => V2Event::ExecutionStarted { session_id },
        "session.execution.succeeded" => ended(session_id, ExecutionOutcome::Succeeded),
        "session.execution.failed" => ended(session_id, ExecutionOutcome::Failed),
        "session.execution.interrupted" => ended(session_id, ExecutionOutcome::Interrupted),
        "session.status" => V2Event::SessionStatus {
            session_id,
            status: parse_status(data.get("status")?)?,
        },
        "session.inbox.enqueued" => {
            if data.pointer("/item/type").and_then(Value::as_str) != Some("user") {
                return None;
            }
            V2Event::UserPrompt {
                session_id,
                inbox_id: field(data, "inboxID")?,
            }
        }
        "session.text.delta" | "session.reasoning.delta" => V2Event::Streaming { session_id },
        "session.tool.input.started" => V2Event::ToolInputStarted {
            session_id,
            call_id: call_id()?,
            name: field(data, "name")?,
        },
        "session.tool.called" => V2Event::ToolCalled {
            session_id,
            call_id: call_id()?,
            input: data.get("input").cloned().unwrap_or(Value::Null),
        },
        "session.tool.success" => V2Event::ToolEnded {
            session_id,
            call_id: call_id()?,
            error: data.pointer("/metadata/error").and_then(Value::as_bool) == Some(true),
        },
        "session.tool.failed" => V2Event::ToolEnded {
            session_id,
            call_id: call_id()?,
            error: true,
        },
        "permission.asked" => V2Event::PermissionAsked {
            request_id: call_id()?,
            session_id,
        },
        "permission.replied" => V2Event::PermissionReplied {
            request_id: field(data, "requestID")?,
            session_id,
        },
        "session.agent.selected" => V2Event::AgentSelected {
            session_id,
            agent: parse_agent(data.get("agent")?)?,
        },
        _ => return None,
    })
}

fn ended(session_id: String, outcome: ExecutionOutcome) -> V2Event {
    V2Event::ExecutionEnded {
        session_id,
        outcome,
    }
}

/// Schema-derived, not captured: `{type: idle|busy|retry}` per the upstream
/// `SessionStatus` union; a bare string is accepted too.
fn parse_status(v: &Value) -> Option<SessionStatus> {
    let s = v.as_str().or_else(|| v.get("type")?.as_str())?;
    match s {
        "idle" => Some(SessionStatus::Idle),
        "busy" => Some(SessionStatus::Busy),
        "retry" => Some(SessionStatus::Retry),
        _ => None,
    }
}

/// Schema-derived, not captured (the agent event schema was not among the
/// recon copies): `data.agent` as a string, or an object with `id`/`name`.
fn parse_agent(v: &Value) -> Option<String> {
    text(v)
        .or_else(|| field(v, "id"))
        .or_else(|| field(v, "name"))
}

/// `form.created` carries the form under `data.form`; `form.replied` and
/// `form.cancelled` are keyed on `data.id` (not `requestID`). The session is
/// optional: an MCP elicitation form has none.
fn parse_form(kind: &str, data: &Value) -> Option<V2Event> {
    match kind {
        "created" => {
            let form = data.get("form")?;
            Some(V2Event::FormCreated {
                form_id: field(form, "id")?,
                session_id: field(form, "sessionID"),
                kind: form.pointer("/metadata/kind").and_then(text),
            })
        }
        "replied" | "cancelled" => Some(V2Event::FormResolved {
            form_id: field(data, "id")?,
            session_id: field(data, "sessionID"),
            // `cancelled` is schema-derived, not captured.
            outcome: if kind == "replied" {
                FormOutcome::Replied
            } else {
                FormOutcome::Cancelled
            },
        }),
        _ => None,
    }
}
