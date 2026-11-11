//! The review routes: what the reviewer said, the diff under review, and
//! the sign-off gate — plus the two routes that exist only to refuse.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;

use super::super::state::AgentApiState;
use super::super::verbs::{self, AddressedArgs, DiffArgs, GetCommentArgs, ListArgs, ReplyArgs};
use super::{Notify, authenticate, error_body, json_of, refuse, with_caller};

#[derive(Debug, Deserialize)]
pub(super) struct ListQuery {
    status: Option<String>,
    file: Option<String>,
}

pub(super) async fn api_list(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |caller| {
        verbs::list_comments(
            &state.db,
            caller,
            &ListArgs {
                status: q.status.clone(),
                file: q.file.clone(),
            },
        )
        .and_then(json_of)
    })
}

pub(super) async fn api_get(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |caller| {
        verbs::get_comment(
            &state.db,
            caller,
            &GetCommentArgs {
                thread_id: thread_id.clone(),
            },
        )
        .and_then(json_of)
    })
}

pub(super) async fn api_diff(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<DiffQuery>,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |caller| {
        verbs::get_diff(
            &state.db,
            caller,
            &DiffArgs {
                file: q.file.clone(),
                scope: q.scope.clone(),
                commit_sha: q.commit_sha.clone(),
            },
        )
        .and_then(json_of)
    })
}

#[derive(Debug, Deserialize)]
pub(super) struct DiffQuery {
    file: Option<String>,
    scope: Option<String>,
    commit_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReplyBody {
    body: String,
}

pub(super) async fn api_reply(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    axum::Json(input): axum::Json<ReplyBody>,
) -> Response {
    with_caller(&state, &headers, Notify::OnSuccess, |caller| {
        verbs::reply(
            &state.db,
            caller,
            &ReplyArgs {
                thread_id: thread_id.clone(),
                body: input.body.clone(),
            },
        )
        .and_then(json_of)
    })
}

#[derive(Debug, Deserialize)]
pub(super) struct AddressedBody {
    commit_sha: Option<String>,
    note: Option<String>,
}

pub(super) async fn api_addressed(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    axum::Json(input): axum::Json<AddressedBody>,
) -> Response {
    with_caller(&state, &headers, Notify::OnSuccess, |caller| {
        verbs::mark_addressed(
            &state.db,
            caller,
            &AddressedArgs {
                thread_id: thread_id.clone(),
                commit_sha: input.commit_sha.clone(),
                note: input.note.clone(),
            },
        )
        .and_then(json_of)
    })
}

/// Whether the reviewer has signed off (#214) — the gate to read
/// before landing.
pub(super) async fn api_status(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |caller| {
        verbs::review_status(&state.db, caller).and_then(json_of)
    })
}

/// Present, and always refuses.
///
/// The write half of the sign-off does not exist for anyone but the
/// reviewer. Same reasoning as `api_resolve` one level up: an agent
/// that could approve its own work would remove the gate the review
/// exists to be.
pub(super) async fn api_signoff(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = authenticate(&state, &headers) {
        return refuse(&e);
    }
    error_body(
        StatusCode::FORBIDDEN,
        "signing off is the reviewer's decision — GET this endpoint to \
         see whether they have",
    )
}

/// Present, and always refuses.
///
/// A 404 here would read as "not implemented yet" and invite a retry
/// after the next release. The route exists so the answer is the
/// design: resolve belongs to the reviewer (D8).
pub(super) async fn api_resolve(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
) -> Response {
    // Still authenticate: an unauthenticated caller should learn it has
    // no token, not read our policy.
    let _ = thread_id;
    if let Err(e) = authenticate(&state, &headers) {
        return refuse(&e);
    }
    error_body(
        StatusCode::FORBIDDEN,
        "resolving a thread is the reviewer's decision — reply and call \
         mark_addressed instead",
    )
}
