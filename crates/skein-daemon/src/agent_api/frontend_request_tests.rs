//! The frontend request round-trip (#328): the pending-request map.

use std::time::Duration;

use serde_json::{Value, json};

use super::tests::{agent_api_state, fixture};

// ── frontend request round-trip (#328) ───────────────────────────
//
// `request_frontend` has no `AppHandle` in any of these — `fixture()`
// only ever builds `AgentApiState::for_test`, which is the "no webview"
// case on its own. So most of these exercise the pending-map machinery
// directly via `register`/`await_response`/`complete_request`, the way
// the brief's split was meant to allow.

#[tokio::test]
async fn completing_a_pending_request_delivers_the_value() {
    let f = fixture();
    let state = agent_api_state(&f);
    let (id, rx) = state.register();

    let value = json!({ "picked": "yes" });
    state.complete_request(&id, Ok(value.clone())).unwrap();

    let got = state
        .await_response(&id, rx, Duration::from_secs(1), "test")
        .await;
    assert_eq!(got, Ok(value));
    assert!(state.pending_requests.lock().is_empty());
}

#[tokio::test]
async fn completing_a_pending_request_with_an_error_delivers_the_error() {
    let f = fixture();
    let state = agent_api_state(&f);
    let (id, rx) = state.register();

    state
        .complete_request(&id, Err("the user cancelled".to_owned()))
        .unwrap();

    let got = state
        .await_response(&id, rx, Duration::from_secs(1), "test")
        .await;
    assert_eq!(got, Err("the user cancelled".to_owned()));
    assert!(state.pending_requests.lock().is_empty());
}

#[tokio::test]
async fn an_uncompleted_request_times_out_and_leaves_the_map_empty() {
    let f = fixture();
    let state = agent_api_state(&f);
    let (id, rx) = state.register();

    let got = state
        .await_response(&id, rx, Duration::from_millis(20), "test")
        .await;
    assert!(got.is_err());
    assert!(state.pending_requests.lock().is_empty());

    // And the id is now unknown — a late completion attempt fails
    // rather than silently succeeding into nothing.
    assert!(state.complete_request(&id, Ok(Value::Null)).is_err());
}

#[tokio::test]
async fn dropping_the_awaiter_before_it_resolves_still_frees_the_pending_id() {
    // Outer cancellation: the future `await_response` returns is itself
    // dropped before it resolves — exactly what happens to an axum
    // handler when the client disconnects mid-request (#330, the route
    // that will drive `request_frontend`). `tokio::time::timeout` here
    // plays the part of that cancellation: its 20ms outer bound is far
    // shorter than `await_response`'s own 1s one, so it always wins and
    // drops the inner future while it is still pending.
    let f = fixture();
    let state = agent_api_state(&f);
    let (id, rx) = state.register();

    let outer = tokio::time::timeout(
        Duration::from_millis(20),
        state.await_response(&id, rx, Duration::from_secs(1), "test"),
    )
    .await;
    assert!(outer.is_err(), "the outer cancellation should have won");

    // A manual `remove()` placed after the inner `.await` would never
    // have run here — this only passes because of the drop guard.
    assert!(state.pending_requests.lock().is_empty());
    assert!(state.complete_request(&id, Ok(Value::Null)).is_err());
}

#[tokio::test]
async fn completing_an_unknown_id_is_an_error() {
    let f = fixture();
    let state = agent_api_state(&f);
    let err = state
        .complete_request("never-issued", Ok(Value::Null))
        .unwrap_err();
    assert!(err.contains("never-issued"));
}

#[tokio::test]
async fn completing_the_same_request_twice_fails_the_second_time() {
    let f = fixture();
    let state = agent_api_state(&f);
    let (id, _rx) = state.register();

    assert!(state.complete_request(&id, Ok(Value::Null)).is_ok());
    assert!(state.complete_request(&id, Ok(Value::Null)).is_err());
}

#[tokio::test]
async fn completing_after_the_awaiter_already_timed_out_fails() {
    let f = fixture();
    let state = agent_api_state(&f);
    let (id, rx) = state.register();

    let _ = state
        .await_response(&id, rx, Duration::from_millis(10), "test")
        .await;

    // The timeout already removed the entry, so this is the same case
    // as completing an id that was never issued.
    assert!(state.complete_request(&id, Ok(Value::Null)).is_err());
}

#[tokio::test]
async fn request_frontend_with_no_app_handle_fails_immediately_and_leaves_the_map_empty() {
    let f = fixture();
    let state = agent_api_state(&f);

    let got = state
        .request_frontend("pick-folder", json!({}), Duration::from_secs(1))
        .await;
    assert!(got.is_err());
    assert!(state.pending_requests.lock().is_empty());
}
