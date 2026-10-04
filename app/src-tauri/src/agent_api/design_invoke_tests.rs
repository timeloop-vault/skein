//! `invoke_element` (#549): argument checks before any guard, the request
//! the frontend receives, and the shared guards.

use serde_json::{Value, json};

use super::design_tests::{design_room, folder, is_refused, recording};
use super::tests::{caller_for, fixture, save};
use super::verbs::{self, InvokeElementArgs, ShowElementArgs, VerbError};

fn args(v: Value) -> InvokeElementArgs {
    serde_json::from_value(v).unwrap()
}

#[allow(clippy::unnecessary_wraps)] // the recording frontend's answer shape
fn answer(_: &str, _: &Value) -> Result<Value, String> {
    Ok(json!({
        "tier": "selector", "invoked": true, "action": "tap",
        "element": { "tag": "button" }, "domChanged": true
    }))
}

#[tokio::test]
async fn bad_arguments_are_refused_before_any_guard_or_frontend_call() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let cases = [
        json!({}),
        json!({ "selector": "p", "anchor": { "tag": "p" } }),
        json!({ "selector": "p", "action": "swipe" }),
        json!({ "selector": "p", "action": "swipe", "direction": "diagonal" }),
        json!({ "selector": "p", "direction": "left" }),
        json!({ "selector": "p", "action": "tap", "direction": "left" }),
        json!({ "selector": "p", "distance": 50 }),
        json!({ "selector": "p", "action": "swipe", "direction": "up", "distance": 7 }),
        json!({ "selector": "p", "action": "swipe", "direction": "up", "distance": 2001 }),
        json!({ "selector": "p", "action": "swipe", "direction": "up", "distance": 12.5 }),
        json!({ "selector": "p", "action": "swipe", "direction": "up", "distance": "50" }),
        json!({ "selector": "p", "action": "swipe", "direction": "up", "distance": -3 }),
        json!({ "selector": "p", "action": "drag" }),
    ];
    // More refusals than the rate cap, with the kill switch off: neither
    // may be what answers, and none may have spent any budget.
    for _ in 0..2 {
        for c in &cases {
            let e = verbs::invoke_element(&state, &caller, &args(c.clone()), false)
                .await
                .unwrap_err();
            assert!(is_refused(&e, "bad_arguments:"), "{c} -> {e:?}");
        }
    }
    assert!(calls.lock().unwrap().is_empty());
    verbs::invoke_element(&state, &caller, &args(json!({ "selector": "p" })), true)
        .await
        .expect("no budget was spent by the refusals");
}

#[tokio::test]
async fn tap_is_the_default_and_nothing_swipe_shaped_is_forwarded() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let out = verbs::invoke_element(&state, &caller, &args(json!({ "selector": "#go" })), true)
        .await
        .unwrap();
    assert_eq!(out["harnessId"], json!("d1"));
    assert_eq!(out["domChanged"], json!(true));
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "design.invoke_element");
    assert_eq!(
        calls[0].1,
        json!({ "roomId": "A", "harnessId": "d1", "selector": "#go", "action": "tap" })
    );
}

#[tokio::test]
async fn swipe_forwards_direction_and_defaults_the_distance() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    verbs::invoke_element(
        &state,
        &caller,
        &args(json!({ "selector": "#c", "action": "swipe", "direction": "left" })),
        true,
    )
    .await
    .unwrap();
    verbs::invoke_element(
        &state,
        &caller,
        &args(json!({
            "anchor": { "tag": "li" }, "action": "swipe", "direction": "down", "distance": 2000
        })),
        true,
    )
    .await
    .unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls[0].1,
        json!({ "roomId": "A", "harnessId": "d1", "selector": "#c", "action": "swipe",
                "direction": "left", "distance": 120 })
    );
    assert_eq!(
        calls[1].1,
        json!({ "roomId": "A", "harnessId": "d1", "anchor": { "tag": "li" }, "action": "swipe",
                "direction": "down", "distance": 2000 })
    );
}

#[tokio::test]
async fn kill_switch_and_rate_cap_apply() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let a = args(json!({ "selector": "p" }));
    let e = verbs::invoke_element(&state, &caller, &a, false)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "disabled:"), "{e:?}");
    assert!(calls.lock().unwrap().is_empty());
    for n in 0..10 {
        verbs::invoke_element(&state, &caller, &a, true)
            .await
            .unwrap_or_else(|e| panic!("call {n}: {e:?}"));
    }
    let e = verbs::invoke_element(&state, &caller, &a, true)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "rate_limited:"), "{e:?}");
}

#[tokio::test]
async fn the_cap_is_shared_with_the_other_design_write_verbs() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, answer);
    for _ in 0..10 {
        let a = ShowElementArgs {
            harness: None,
            selector: Some("p".into()),
            anchor: None,
        };
        verbs::show_element(&state, &caller, &a, true)
            .await
            .unwrap();
    }
    let e = verbs::invoke_element(&state, &caller, &args(json!({ "selector": "p" })), true)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "rate_limited:"), "{e:?}");
}

#[tokio::test]
async fn foreign_and_missing_design_harnesses_are_refused_without_a_frontend_call() {
    let f = fixture();
    let dir = folder();
    save(
        &f.db,
        &[design_room("A", &dir, &[]), design_room("B", &dir, &["dB"])],
    );
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let e = verbs::invoke_element(
        &state,
        &caller,
        &args(json!({ "harness": "dB", "selector": "p" })),
        true,
    )
    .await
    .unwrap_err();
    assert!(matches!(e, VerbError::NotFound(_)), "{e:?}");
    // The caller's own non-design harness is no design harness either.
    let e = verbs::invoke_element(
        &state,
        &caller,
        &args(json!({ "harness": "A-claude", "selector": "p" })),
        true,
    )
    .await
    .unwrap_err();
    assert!(matches!(e, VerbError::NotFound(_)), "{e:?}");
    let e = verbs::invoke_element(&state, &caller, &args(json!({ "selector": "p" })), true)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "no_design_harness:"), "{e:?}");
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn frontend_refusals_surface_with_their_codes() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    for (msg, code) in [
        (
            "busy: the user is picking an element in this design pane; try again once they finish",
            "busy:",
        ),
        ("not_mounted: the pane is not open", "not_mounted:"),
        ("not_ready: the preview has not loaded", "not_ready:"),
    ] {
        let (state, _) = recording(&f, move |_, _| Err(msg.to_owned()));
        let e = verbs::invoke_element(&state, &caller, &args(json!({ "selector": "p" })), true)
            .await
            .unwrap_err();
        assert!(is_refused(&e, code), "{msg} -> {e:?}");
    }
}

#[tokio::test]
async fn a_non_invoking_answer_passes_through_untouched() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, |_, _| {
        Ok(
            json!({ "tier": "ambiguous", "invoked": false, "action": "tap",
                   "element": null, "count": 4 }),
        )
    });
    let out = verbs::invoke_element(&state, &caller, &args(json!({ "selector": "li" })), true)
        .await
        .unwrap();
    assert_eq!(out["tier"], json!("ambiguous"));
    assert_eq!(out["invoked"], json!(false));
    assert_eq!(out["count"], json!(4));
    assert!(out.get("domChanged").is_none());
    assert_eq!(out["harnessId"], json!("d1"));
}
