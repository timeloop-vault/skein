//! The mailbox routes (#327, #364): send, read, and page back through
//! history.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::super::state::AgentApiState;
use super::super::verbs::{
    self, HistoryDirection, HistorySince, MessageHistoryArgs, ReadMessagesArgs, SendMessageArgs,
};
use super::{authenticate, error_body, json_of, mail_context, refuse};

#[derive(Debug, Deserialize)]
pub(super) struct SendMessageBody {
    to: String,
    body: String,
}

/// `POST /api/messages` — the plain-JSON mirror of `send_message`.
pub(super) async fn api_send_message(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<SendMessageBody>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let out = verbs::send_message(
        &state.db,
        &caller,
        &SendMessageArgs {
            to: input.to,
            body: input.body,
        },
        mail.policy,
        mail.agent_sees_mcp,
        mail.app.as_ref(),
    );
    match out {
        Ok(v) => {
            state.notify_mail_changed(&v.to_room_id, &v.to_harness_id);
            match json_of(v) {
                Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
                Err(e) => error_body(StatusCode::INTERNAL_SERVER_ERROR, e.message()),
            }
        }
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReadMessagesQuery {
    /// `includeRead` on the wire — unlike every other query param on
    /// this API (`commit_sha` and friends stay `snake_case`, matching
    /// the MCP `arguments` shape), because the brief for #327 names
    /// this exact spelling and there is no MCP argument here for it to
    /// mirror.
    #[serde(default)]
    include_read: Option<bool>,
}

/// `GET /api/messages?includeRead=true` — the plain-JSON mirror of
/// `read_messages`.
pub(super) async fn api_read_messages(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<ReadMessagesQuery>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let out = verbs::read_messages(
        &state.db,
        &caller,
        &ReadMessagesArgs {
            include_read: q.include_read,
        },
        mail.policy,
    );
    match out {
        Ok(v) => {
            if v.newly_marked_read > 0 {
                if let Some(harness_id) = caller.harness_id.as_deref() {
                    state.notify_mail_changed(&caller.room_id, harness_id);
                }
            }
            match json_of(v) {
                Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
                Err(e) => error_body(StatusCode::INTERNAL_SERVER_ERROR, e.message()),
            }
        }
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageHistoryQuery {
    with: Option<String>,
    since: Option<String>,
    limit: Option<u32>,
    direction: Option<String>,
}

/// `GET /api/messages/history` — the plain-JSON mirror of the MCP
/// `message_history` tool (#364). Never marks anything read, unlike
/// `api_read_messages` above — there is nothing to notify after a call
/// that changes nothing.
///
/// Query params arrive as plain strings, so `since` is resolved by hand
/// here the way MCP's untagged `HistorySince` is resolved for free by
/// `serde_json`: a value that parses as an integer is a millisecond
/// timestamp, anything else is a message id — never ambiguous, since a
/// message id is a UUID and never looks numeric.
pub(super) async fn api_message_history(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<MessageHistoryQuery>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let direction = match q.direction.as_deref() {
        None => None,
        Some("in") => Some(HistoryDirection::In),
        Some("out") => Some(HistoryDirection::Out),
        Some("both") => Some(HistoryDirection::Both),
        Some(other) => {
            return error_body(
                StatusCode::BAD_REQUEST,
                &format!("unknown direction {other:?} — use in, out or both"),
            );
        }
    };
    let since = q.since.as_deref().map(|s| match s.parse::<i64>() {
        Ok(ms) => HistorySince::Ms(ms),
        Err(_) => HistorySince::MessageId(s.to_owned()),
    });
    let out = verbs::message_history(
        &state.db,
        &caller,
        &MessageHistoryArgs {
            with: q.with,
            since,
            limit: q.limit,
            direction,
        },
        mail.policy,
    );
    match out.and_then(json_of) {
        Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}
