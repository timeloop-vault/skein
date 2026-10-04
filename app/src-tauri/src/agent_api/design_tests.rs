//! The design-pane verbs (#512): scoping, harness pick, frontend error
//! mapping, and the guards on the two write verbs.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::tests::{Fixture, agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, DesignTargetArgs, OpenDesignEntryArgs, ShowElementArgs, VerbError};
use crate::agent_api::state::AgentApiState;
use crate::db::Room;

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// A room folder holding two html entries.
fn folder() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.html"), "<p>a</p>").unwrap();
    std::fs::write(dir.path().join("b.html"), "<p>b</p>").unwrap();
    dir
}

fn design_room(id: &str, dir: &TempDir, design_ids: &[&str]) -> Room {
    let mut hs = vec![harness(&format!("{id}-claude"), "claude", "main")];
    hs.extend(design_ids.iter().map(|d| harness(d, "design", d)));
    let mut r = room(id, hs);
    r.cwd = Some(dir.path().to_string_lossy().into_owned());
    r
}

/// A state whose frontend records every call and answers with `answer`.
fn recording(
    f: &Fixture,
    answer: impl Fn(&str, &Value) -> Result<Value, String> + Send + Sync + 'static,
) -> (AgentApiState, Calls) {
    let state = agent_api_state(f);
    let calls: Calls = Arc::default();
    let sink = Arc::clone(&calls);
    state.set_test_frontend(move |kind, args| {
        sink.lock().unwrap().push((kind.to_owned(), args.clone()));
        answer(kind, args)
    });
    (state, calls)
}

fn ok_everywhere(kind: &str, args: &Value) -> Result<Value, String> {
    match kind {
        "design.panes" => Ok(json!({ "panes": [
            { "harnessId": "d1", "mounted": true, "ready": true },
            { "harnessId": "d2", "mounted": true, "ready": false },
        ] })),
        "design.state" => Ok(json!({ "entry": "a.html", "ready": true })),
        "design.open_entry" => Ok(json!({ "entry": args["entry"], "previous": "a.html" })),
        "design.show_element" => Ok(json!({ "tier": "exact", "highlighted": true })),
        other => Err(format!("unexpected kind {other:?}")),
    }
}

fn target(h: Option<&str>) -> DesignTargetArgs {
    DesignTargetArgs {
        harness: h.map(str::to_owned),
    }
}

fn open_args(h: Option<&str>, entry: &str) -> OpenDesignEntryArgs {
    OpenDesignEntryArgs {
        harness: h.map(str::to_owned),
        entry: entry.to_owned(),
    }
}

fn show_args(h: Option<&str>, selector: Option<&str>, anchor: Option<Value>) -> ShowElementArgs {
    ShowElementArgs {
        harness: h.map(str::to_owned),
        selector: selector.map(str::to_owned),
        anchor,
    }
}

fn is_refused(err: &VerbError, prefix: &str) -> bool {
    matches!(err, VerbError::Refused(m) if m.starts_with(prefix))
}

// ── scoping ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn another_rooms_design_harness_is_not_found_and_never_reaches_the_frontend() {
    let f = fixture();
    let dir = folder();
    save(
        &f.db,
        &[design_room("A", &dir, &[]), design_room("B", &dir, &["dB"])],
    );
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);

    let e = verbs::get_design_state(&state, &caller, &target(Some("dB")))
        .await
        .unwrap_err();
    assert!(matches!(e, VerbError::NotFound(_)), "{e:?}");
    let e = verbs::open_design_entry(&state, &caller, &open_args(Some("dB"), "a.html"), true)
        .await
        .unwrap_err();
    assert!(matches!(e, VerbError::NotFound(_)), "{e:?}");
    let e = verbs::show_element(
        &state,
        &caller,
        &show_args(Some("dB"), Some("p"), None),
        true,
    )
    .await
    .unwrap_err();
    assert!(matches!(e, VerbError::NotFound(_)), "{e:?}");
    assert!(
        calls.lock().unwrap().is_empty(),
        "frontend must not be asked"
    );
}

#[tokio::test]
async fn list_never_shows_another_rooms_harness() {
    let f = fixture();
    let dir = folder();
    save(
        &f.db,
        &[
            design_room("A", &dir, &["dA"]),
            design_room("B", &dir, &["dB"]),
        ],
    );
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    let out = verbs::list_design_harnesses(&state, &caller).await.unwrap();
    let ids: Vec<&str> = out
        .harnesses
        .iter()
        .map(|h| h.harness_id.as_str())
        .collect();
    assert_eq!(ids, ["dA"]);
    let calls = calls.lock().unwrap();
    assert!(calls.iter().all(|(_, a)| a["roomId"] == json!("A")));
}

#[tokio::test]
async fn every_request_carries_the_callers_own_room_id() {
    let f = fixture();
    let dir = folder();
    save(
        &f.db,
        &[
            design_room("A", &dir, &["d1"]),
            design_room("B", &dir, &["d1b"]),
        ],
    );
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    verbs::list_design_harnesses(&state, &caller).await.unwrap();
    verbs::get_design_state(&state, &caller, &target(None))
        .await
        .unwrap();
    verbs::open_design_entry(&state, &caller, &open_args(None, "b.html"), true)
        .await
        .unwrap();
    verbs::show_element(&state, &caller, &show_args(None, Some("p"), None), true)
        .await
        .unwrap();
    let calls = calls.lock().unwrap();
    let kinds: Vec<&str> = calls.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "design.panes",
            "design.state",
            "design.open_entry",
            "design.show_element"
        ]
    );
    for (k, a) in calls.iter() {
        assert_eq!(a["roomId"], json!("A"), "{k}");
    }
}

// ── harness pick ────────────────────────────────────────────────────────

#[tokio::test]
async fn no_design_harness_is_refused_and_lists_empty() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &[])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    let e = verbs::get_design_state(&state, &caller, &target(None))
        .await
        .unwrap_err();
    assert!(is_refused(&e, "no_design_harness:"), "{e:?}");
    let out = verbs::list_design_harnesses(&state, &caller).await.unwrap();
    assert!(out.harnesses.is_empty());
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn two_design_harnesses_require_an_explicit_pick() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1", "d2"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, ok_everywhere);
    let e = verbs::get_design_state(&state, &caller, &target(None))
        .await
        .unwrap_err();
    assert!(is_refused(&e, "harness_required:"), "{e:?}");
    let out = verbs::get_design_state(&state, &caller, &target(Some("d2")))
        .await
        .unwrap();
    assert_eq!(out["harnessId"], json!("d2"));
}

#[tokio::test]
async fn a_single_design_harness_is_picked_implicitly() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    let out = verbs::get_design_state(&state, &caller, &target(None))
        .await
        .unwrap();
    assert_eq!(out["harnessId"], json!("d1"));
    assert_eq!(out["entry"], json!("a.html"));
    assert_eq!(calls.lock().unwrap()[0].1["harnessId"], json!("d1"));
}

#[tokio::test]
async fn a_non_design_harness_in_the_callers_room_is_not_found() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    let e = verbs::get_design_state(&state, &caller, &target(Some("A-claude")))
        .await
        .unwrap_err();
    assert!(matches!(e, VerbError::NotFound(_)), "{e:?}");
    assert!(calls.lock().unwrap().is_empty());
}

// ── frontend errors ─────────────────────────────────────────────────────

#[tokio::test]
async fn not_mounted_and_not_ready_surface_as_refusals_with_their_code() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    for code in ["not_mounted", "not_ready"] {
        let msg = format!("{code}: pane says no");
        let (state, _) = recording(&f, move |_, _| Err(msg.clone()));
        let e = verbs::get_design_state(&state, &caller, &target(None))
            .await
            .unwrap_err();
        assert!(is_refused(&e, &format!("{code}:")), "{e:?}");
        let e = verbs::open_design_entry(&state, &caller, &open_args(None, "a.html"), true)
            .await
            .unwrap_err();
        assert!(is_refused(&e, &format!("{code}:")), "{e:?}");
        let e = verbs::show_element(&state, &caller, &show_args(None, Some("p"), None), true)
            .await
            .unwrap_err();
        assert!(is_refused(&e, &format!("{code}:")), "{e:?}");
    }
}

#[tokio::test]
async fn a_missing_webview_is_unavailable() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let state = agent_api_state(&f); // no test frontend, no AppHandle
    let e = verbs::get_design_state(&state, &caller, &target(None))
        .await
        .unwrap_err();
    assert!(matches!(e, VerbError::Unavailable(_)), "{e:?}");
    let e = verbs::show_element(&state, &caller, &show_args(None, Some("p"), None), true)
        .await
        .unwrap_err();
    assert!(matches!(e, VerbError::Unavailable(_)), "{e:?}");
}

// ── list ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_reports_entries_and_pane_status() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1", "d2", "d3"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, ok_everywhere);
    let out = verbs::list_design_harnesses(&state, &caller).await.unwrap();
    assert_eq!(out.harnesses.len(), 3);
    assert_eq!(out.harnesses[0].entries, ["a.html", "b.html"]);
    assert_eq!(
        (out.harnesses[0].mounted, out.harnesses[0].ready),
        (Some(true), Some(true))
    );
    assert_eq!(
        (out.harnesses[1].mounted, out.harnesses[1].ready),
        (Some(true), Some(false))
    );
    // d3 is absent from the panes answer: false, not null.
    assert_eq!(
        (out.harnesses[2].mounted, out.harnesses[2].ready),
        (Some(false), Some(false))
    );
}

#[tokio::test]
async fn list_degrades_to_null_when_the_panes_request_fails() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, |_, _| Err("boom".to_owned()));
    let out = verbs::list_design_harnesses(&state, &caller).await.unwrap();
    assert_eq!(out.harnesses.len(), 1);
    assert_eq!(out.harnesses[0].mounted, None);
    assert_eq!(out.harnesses[0].ready, None);
    assert_eq!(out.harnesses[0].entries, ["a.html", "b.html"]);
}

// ── open_design_entry ───────────────────────────────────────────────────

#[tokio::test]
async fn an_unlisted_entry_is_refused_before_any_frontend_call() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    let e = verbs::open_design_entry(&state, &caller, &open_args(None, "nope.html"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, VerbError::Refused(m) if m.starts_with("unknown_entry:") && m.contains("a.html")),
        "{e:?}"
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_listed_entry_is_forwarded_with_room_harness_and_entry() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    let out = verbs::open_design_entry(&state, &caller, &open_args(None, "b.html"), true)
        .await
        .unwrap();
    assert_eq!(out["entry"], json!("b.html"));
    assert_eq!(out["previous"], json!("a.html"));
    assert_eq!(out["harnessId"], json!("d1"));
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "design.open_entry");
    assert_eq!(
        calls[0].1,
        json!({ "roomId": "A", "harnessId": "d1", "entry": "b.html" })
    );
}

#[tokio::test]
async fn write_verbs_are_disabled_by_the_kill_switch() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    let e = verbs::open_design_entry(&state, &caller, &open_args(None, "a.html"), false)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "disabled:"), "{e:?}");
    let e = verbs::show_element(&state, &caller, &show_args(None, Some("p"), None), false)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "disabled:"), "{e:?}");
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn write_verbs_share_a_rate_cap_of_ten_a_minute() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, ok_everywhere);
    for n in 0..5 {
        verbs::open_design_entry(&state, &caller, &open_args(None, "a.html"), true)
            .await
            .unwrap_or_else(|e| panic!("open {n}: {e:?}"));
        verbs::show_element(&state, &caller, &show_args(None, Some("p"), None), true)
            .await
            .unwrap_or_else(|e| panic!("show {n}: {e:?}"));
    }
    let e = verbs::open_design_entry(&state, &caller, &open_args(None, "a.html"), true)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "rate_limited:"), "{e:?}");
    let e = verbs::show_element(&state, &caller, &show_args(None, Some("p"), None), true)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "rate_limited:"), "{e:?}");
}

#[tokio::test]
async fn an_archived_caller_room_is_refused() {
    let f = fixture();
    let dir = folder();
    // Auth itself refuses an archived room, so archive it after the
    // caller was minted (the verb's own check is the second line).
    let mut r = design_room("A", &dir, &["d1"]);
    save(&f.db, &[r.clone()]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    r.archived = Some(1);
    save(&f.db, &[r]);
    let (state, calls) = recording(&f, ok_everywhere);
    let e = verbs::open_design_entry(&state, &caller, &open_args(None, "a.html"), true)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "archived:"), "{e:?}");
    let e = verbs::get_design_state(&state, &caller, &target(None))
        .await
        .unwrap_err();
    assert!(is_refused(&e, "archived:"), "{e:?}");
    assert!(calls.lock().unwrap().is_empty());
}

// ── show_element ────────────────────────────────────────────────────────

#[tokio::test]
async fn show_element_needs_exactly_one_of_selector_and_anchor() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    let bad = [
        show_args(None, None, None),
        show_args(None, Some("p"), Some(json!({ "selector": "p" }))),
        show_args(None, None, Some(json!("p"))),
        show_args(None, None, Some(json!([1]))),
        show_args(None, Some("   "), None),
    ];
    for args in &bad {
        let e = verbs::show_element(&state, &caller, args, true)
            .await
            .unwrap_err();
        assert!(is_refused(&e, "bad_arguments:"), "{args:?} -> {e:?}");
    }
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn show_element_forwards_exactly_one_of_selector_or_anchor() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, ok_everywhere);
    verbs::show_element(
        &state,
        &caller,
        &show_args(None, Some("h1.title"), None),
        true,
    )
    .await
    .unwrap();
    let anchor = json!({ "tag": "button", "text": "Go" });
    verbs::show_element(
        &state,
        &caller,
        &show_args(None, None, Some(anchor.clone())),
        true,
    )
    .await
    .unwrap();
    let calls = calls.lock().unwrap();
    let (k0, a0) = &calls[0];
    assert_eq!(k0, "design.show_element");
    assert_eq!(a0["selector"], json!("h1.title"));
    assert!(a0.get("anchor").is_none());
    let a1 = &calls[1].1;
    assert_eq!(a1["anchor"], anchor);
    assert!(a1.get("selector").is_none());
    assert_eq!(a1["harnessId"], json!("d1"));
}

#[tokio::test]
async fn show_element_passes_the_frontend_result_through() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, |_, _| {
        Ok(json!({ "tier": "max-overlap", "highlighted": true, "count": 3, "score": 0.8 }))
    });
    let out = verbs::show_element(&state, &caller, &show_args(None, Some("p"), None), true)
        .await
        .unwrap();
    assert_eq!(out["tier"], json!("max-overlap"));
    assert_eq!(out["count"], json!(3));
    assert_eq!(out["score"], json!(0.8));
    assert_eq!(out["harnessId"], json!("d1"));
}
