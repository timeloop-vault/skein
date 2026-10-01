//! The harness routes: open, close and list a room's harnesses.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::super::state::AgentApiState;
use super::super::verbs::{self, CloseHarnessArgs, ListHarnessesArgs, OpenHarnessArgs};
use super::{authenticate, error_body, json_of, mail_context, refuse};

/// `POST /api/harnesses` — the plain-JSON mirror of the MCP
/// `open_harness` tool (#411). Same shape as `api_create_room`:
/// `OpenHarnessArgs` already carries the camelCase wire shape and names
/// its own target room, so the body is taken directly rather than
/// through a separate struct.
pub(super) async fn api_open_harness(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    axum::Json(args): axum::Json<OpenHarnessArgs>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let harness_control_enabled = state.spawn_settings().allow_agent_harness_control;
    match verbs::open_harness(&state, &caller, &args, &mail, harness_control_enabled).await {
        Ok(v) => match json_of(v) {
            Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
            Err(e) => error_body(StatusCode::INTERNAL_SERVER_ERROR, e.message()),
        },
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}

/// `POST /api/harnesses/{harness_id}/close` — the plain-JSON mirror of
/// the MCP `close_harness` tool (#411). `harness_id` comes from the
/// path, exactly like `api_close_room`'s `room_id`; `close_harness`
/// finds the harness's own room by searching, so there is no separate
/// room id to take.
pub(super) async fn api_close_harness(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(harness_id): Path<String>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let harness_control_enabled = state.spawn_settings().allow_agent_harness_control;
    let out = verbs::close_harness(
        &state,
        &caller,
        &CloseHarnessArgs {
            harness: harness_id,
        },
        harness_control_enabled,
    )
    .await;
    match out.and_then(json_of) {
        Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct ListHarnessesQuery {
    room: Option<String>,
}

/// `GET /api/harnesses?room=<id>` — the plain-JSON mirror of the MCP
/// `list_harnesses` tool (#356). Same cross-room exception as
/// `api_get_room` above.
pub(super) async fn api_list_harnesses(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<ListHarnessesQuery>,
) -> Response {
    if let Err(e) = authenticate(&state, &headers) {
        return refuse(&e);
    }
    let out = verbs::list_harnesses(&state, &ListHarnessesArgs { room: q.room }).await;
    match out.and_then(json_of) {
        Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}
