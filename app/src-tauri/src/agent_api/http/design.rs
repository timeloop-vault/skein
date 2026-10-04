//! The design-pane routes (#512): the plain-JSON mirror of the four
//! design verbs.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::super::state::AgentApiState;
use super::super::verbs::{
    self, DesignTargetArgs, OpenDesignEntryArgs, ShowElementArgs, VerbError,
};
use super::{authenticate, error_body, json_of, refuse};

fn respond(out: Result<serde_json::Value, VerbError>) -> Response {
    match out {
        Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}

/// `GET /api/design/harnesses` — `list_design_harnesses`.
pub(super) async fn api_design_harnesses(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    respond(
        verbs::list_design_harnesses(&state, &caller)
            .await
            .and_then(json_of),
    )
}

#[derive(Debug, Deserialize)]
pub(super) struct DesignStateQuery {
    harness: Option<String>,
}

/// `GET /api/design/state?harness=` — `get_design_state`.
pub(super) async fn api_design_state(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<DesignStateQuery>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    respond(
        verbs::get_design_state(&state, &caller, &DesignTargetArgs { harness: q.harness }).await,
    )
}

/// `POST /api/design/entry` — `open_design_entry`.
pub(super) async fn api_design_entry(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    axum::Json(args): axum::Json<OpenDesignEntryArgs>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let enabled = state.spawn_settings().allow_agent_harness_control;
    respond(verbs::open_design_entry(&state, &caller, &args, enabled).await)
}

/// `POST /api/design/show` — `show_element`.
pub(super) async fn api_design_show(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    axum::Json(args): axum::Json<ShowElementArgs>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let enabled = state.spawn_settings().allow_agent_harness_control;
    respond(verbs::show_element(&state, &caller, &args, enabled).await)
}
