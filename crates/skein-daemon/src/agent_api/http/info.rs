//! `GET /api/info`: which Skein build the agent runs under (#535).

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

use super::super::state::AgentApiState;
use super::super::verbs;
use super::{Notify, json_of, with_caller};

pub(super) async fn api_info(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |_caller| {
        json_of(verbs::skein_info(&state))
    })
}
