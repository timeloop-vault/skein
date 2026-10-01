//! The room routes: open, close, find, list and read rooms.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::super::state::AgentApiState;
use super::super::verbs::{
    self, CloseRoomArgs, CreateRoomArgs, FindRoomsForPathArgs, GetRoomArgs, ListRoomsArgs,
};
use super::{Notify, authenticate, error_body, json_of, mail_context, refuse, with_caller};

/// `POST /api/rooms` — the plain-JSON mirror of the MCP `create_room`
/// tool (#330). Takes `CreateRoomArgs` directly as the request body: it
/// already carries the camelCase wire shape the tool's own
/// `arguments` object does, so there is no separate body struct to keep
/// in sync the way `SendMessageBody` mirrors `SendMessageArgs`.
pub(super) async fn api_create_room(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    axum::Json(args): axum::Json<CreateRoomArgs>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let room_creation_enabled = state.spawn_settings().allow_agent_room_creation;
    match verbs::create_room(&state, &caller, &args, &mail, room_creation_enabled).await {
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

/// `POST /api/rooms/{room_id}/close` — the plain-JSON mirror of the MCP
/// `close_room` tool (#411). `room_id` comes from the path, exactly like
/// `api_get_room`; unlike that route this one is scoped to the caller's
/// own room the normal way — `close_room` requires the caller to be the
/// TARGET's creator, so there is no cross-room read to protect here the
/// way `find_rooms_for_path`/`get_room`/`list_harnesses` need to.
pub(super) async fn api_close_room(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let closing_enabled = state.spawn_settings().allow_agent_room_closing;
    let out = verbs::close_room(
        &state,
        &caller,
        &CloseRoomArgs { room: room_id },
        closing_enabled,
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
pub(super) struct FindRoomsQuery {
    path: Option<String>,
}

/// `GET /api/rooms/find?path=<p>` — the plain-JSON mirror of the MCP
/// `find_rooms_for_path` tool (#354). Unlike every route above it, the
/// verb underneath is not scoped to the caller's own room — see the
/// doc comment on `verbs::find_rooms_for_path` for why — so the
/// closure below ignores the `Caller` it is handed beyond the
/// authentication `with_caller` already did.
pub(super) async fn api_find_rooms_for_path(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<FindRoomsQuery>,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |_caller| {
        verbs::find_rooms_for_path(
            &state.db,
            &FindRoomsForPathArgs {
                path: q.path.clone().unwrap_or_default(),
            },
        )
        .and_then(json_of)
    })
}

#[derive(Debug, Deserialize)]
pub(super) struct ListRoomsQuery {
    created_by: Option<String>,
}

/// `GET /api/rooms?created_by=me` — the plain-JSON mirror of the MCP
/// `list_rooms` tool (#356). Async, unlike every synchronous route
/// above that goes through `with_caller`: the verb underneath makes its
/// own `"harness_phases"` round trip to the webview.
pub(super) async fn api_list_rooms(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<ListRoomsQuery>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let out = verbs::list_rooms(
        &state,
        &caller,
        &ListRoomsArgs {
            created_by: q.created_by,
        },
        &mail,
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

/// `GET /api/rooms/{room_id}` — the plain-JSON mirror of the MCP
/// `get_room` tool (#356). Not scoped to the caller's own room — see
/// the doc comment on `verbs::get_room` for why — so authentication
/// here is only "does this token exist at all", same as
/// `api_find_rooms_for_path`.
pub(super) async fn api_get_room(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
) -> Response {
    if let Err(e) = authenticate(&state, &headers) {
        return refuse(&e);
    }
    let out = verbs::get_room(&state, &GetRoomArgs { room: room_id }).await;
    match out.and_then(json_of) {
        Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}
