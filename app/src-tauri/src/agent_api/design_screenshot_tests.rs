//! `get_design_screenshot` / `show_design_pane` (#552): the scale math,
//! the not-visible refusal, the guards, and the MCP image block. The
//! native capture itself needs a window and is not exercised here.

use serde_json::{Value, json};
use skein_webshot::{Rect, Shot};

use super::design_tests::{design_room, folder, is_refused, recording};
use super::mcp;
use super::tests::{caller_for, fixture, save};
use super::verbs::{
    self, DesignTargetArgs, MAX_PNG_BYTES, ScreenshotArgs, VerbError, build_output, capture_scale,
    parse_target,
};

fn args(v: Value) -> ScreenshotArgs {
    serde_json::from_value(v).unwrap()
}

#[test]
fn scale_is_the_screen_ratio_when_the_image_fits() {
    assert!((capture_scale(2.0, 400.0, 300.0, None) - 2.0).abs() < 1e-9);
}

#[test]
fn scale_shrinks_so_the_longest_edge_meets_max_edge() {
    // 1600 css px at dpr 2 would be 3200; default cap 1568.
    let s = capture_scale(2.0, 1600.0, 900.0, None);
    assert!((s - 1568.0 / 1600.0).abs() < 1e-9, "{s}");
    // A tall rect is bound by its height.
    let s = capture_scale(2.0, 400.0, 2000.0, Some(1000));
    assert!((s - 0.5).abs() < 1e-9, "{s}");
}

#[test]
fn max_edge_is_clamped_and_a_bad_ratio_falls_back_to_one() {
    let low = capture_scale(4.0, 1000.0, 1000.0, Some(1));
    assert!((low - 0.256).abs() < 1e-9, "{low}");
    let high = capture_scale(4.0, 3000.0, 100.0, Some(99_999));
    assert!((high - 2576.0 / 3000.0).abs() < 1e-9, "{high}");
    assert!((capture_scale(0.0, 100.0, 100.0, None) - 1.0).abs() < 1e-9);
    assert!((capture_scale(f64::NAN, 100.0, 100.0, None) - 1.0).abs() < 1e-9);
}

#[test]
fn a_hidden_target_is_not_visible_with_its_reason() {
    for reason in ["other_room", "hidden_tab", "not_laid_out", "no_pane"] {
        let e = parse_target(&json!({ "visible": false, "reason": reason })).unwrap_err();
        assert!(is_refused(&e, "not_visible:"), "{e:?}");
        assert!(e.message().contains(reason), "{}", e.message());
        assert!(e.message().contains("show_design_pane"), "{}", e.message());
    }
}

#[test]
fn a_visible_target_without_a_usable_rect_is_not_laid_out() {
    for a in [
        json!({ "visible": true }),
        json!({ "visible": true, "rect": { "x": 0, "y": 0, "width": 0, "height": 10 } }),
        json!({ "visible": true, "rect": { "x": 0, "y": 0 } }),
    ] {
        let e = parse_target(&a).unwrap_err();
        assert!(is_refused(&e, "not_visible:") && e.message().contains("not_laid_out"));
    }
}

#[test]
fn a_visible_target_parses() {
    let t = parse_target(&json!({
        "visible": true, "devicePixelRatio": 2,
        "rect": { "x": 10, "y": 20, "width": 300, "height": 200 }, "entry": "a.html",
    }))
    .unwrap();
    assert_eq!(t.rect.width, 300.0);
    assert_eq!(t.device_pixel_ratio, 2.0);
    assert_eq!(t.entry.as_deref(), Some("a.html"));
}

fn target() -> verbs::Target {
    verbs::Target {
        rect: Rect {
            x: 1.0,
            y: 2.0,
            width: 300.0,
            height: 200.0,
        },
        device_pixel_ratio: 2.0,
        entry: Some("a.html".into()),
    }
}

fn shot(len: usize) -> Shot {
    Shot {
        png: vec![7; len],
        width: 600,
        height: 400,
    }
}

#[test]
fn an_oversized_png_is_refused_not_sent() {
    let e = build_output("d1", &target(), 2.0, &shot(MAX_PNG_BYTES + 1)).unwrap_err();
    assert!(is_refused(&e, "too_large:"), "{e:?}");
}

#[test]
fn the_mcp_result_is_a_text_block_then_an_image_block() {
    let out = build_output("d1", &target(), 2.0, &shot(4)).unwrap();
    let result = mcp::image_content(&out);
    assert_eq!(result["isError"], json!(false));
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], json!("text"));
    let meta: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(meta["harnessId"], json!("d1"));
    assert_eq!(meta["entry"], json!("a.html"));
    assert_eq!(meta["width"], json!(600));
    assert_eq!(meta["cssRect"]["width"], json!(300.0));
    assert!(
        meta.get("png").is_none(),
        "the image is not repeated as text"
    );
    assert_eq!(content[1]["type"], json!("image"));
    assert_eq!(content[1]["mimeType"], json!("image/png"));
    assert_eq!(content[1]["data"], json!("BwcHBw=="));
}

fn answer(kind: &str, _: &Value) -> Result<Value, String> {
    match kind {
        "design.capture_target" => Ok(json!({ "visible": false, "reason": "hidden_tab" })),
        "design.show_pane" => {
            Ok(json!({ "shown": true, "switchedRoom": false, "revealed": "show_docked" }))
        }
        other => Err(format!("unexpected kind {other:?}")),
    }
}

#[tokio::test]
async fn a_hidden_pane_refuses_without_capturing_or_changing_the_ui() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let e = verbs::get_design_screenshot(&state, &caller, &args(json!({})), true)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "not_visible:"), "{e:?}");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "design.capture_target");
    assert_eq!(calls[0].1["harnessId"], json!("d1"));
}

#[tokio::test]
async fn a_visible_pane_without_a_window_fails_the_capture() {
    // The test state has no AppHandle, so the native step reports it.
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, |_, _| {
        Ok(json!({ "visible": true, "devicePixelRatio": 1,
            "rect": { "x": 0, "y": 0, "width": 10, "height": 10 } }))
    });
    let e = verbs::get_design_screenshot(&state, &caller, &args(json!({})), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, VerbError::Unavailable(m) if m.starts_with("capture_failed:")),
        "{e:?}"
    );
}

#[tokio::test]
async fn both_verbs_honour_the_kill_switch_and_scoping() {
    let f = fixture();
    let dir = folder();
    save(
        &f.db,
        &[
            design_room("A", &dir, &["d1"]),
            design_room("B", &dir, &["dB"]),
        ],
    );
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let e = verbs::get_design_screenshot(&state, &caller, &args(json!({})), false)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "disabled:"), "{e:?}");
    let e = verbs::show_design_pane(&state, &caller, &DesignTargetArgs::default(), false)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "disabled:"), "{e:?}");
    let e = verbs::show_design_pane(
        &state,
        &caller,
        &DesignTargetArgs {
            harness: Some("dB".into()),
        },
        true,
    )
    .await
    .unwrap_err();
    assert!(matches!(e, VerbError::NotFound(_)), "{e:?}");
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn show_design_pane_forwards_the_answer_with_the_harness_id() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let out = verbs::show_design_pane(&state, &caller, &DesignTargetArgs::default(), true)
        .await
        .unwrap();
    assert_eq!(out["harnessId"], json!("d1"));
    assert_eq!(out["revealed"], json!("show_docked"));
    assert_eq!(calls.lock().unwrap()[0].0, "design.show_pane");
}

#[test]
fn the_specs_carry_the_annotations_and_the_exception_wording() {
    let specs = mcp::tool_specs();
    let find = |n: &str| specs.iter().find(|t| t["name"] == n).unwrap().clone();
    assert_eq!(
        find("get_design_screenshot")["annotations"]["readOnlyHint"],
        json!(true)
    );
    let pane = find("show_design_pane");
    assert_eq!(pane["annotations"]["readOnlyHint"], json!(false));
    let d = pane["description"].as_str().unwrap();
    assert!(d.contains("SWITCH THE ACTIVE ROOM") && d.contains("permission"));
}
