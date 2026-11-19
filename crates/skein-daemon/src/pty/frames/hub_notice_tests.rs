//! Pane notices after exit (#809, T-5): appended to the engine only once the
//! child's output has ended, cut to every viewer before `Exit`, kept for a
//! later attach. The hub knows no wording: the observer supplies it.

use std::sync::Arc;

use super::test_support::{Fx, Obs};

fn with_notice(text: &str) -> (Arc<Obs>, Fx) {
    let obs = Arc::new(Obs::default());
    *obs.notice.lock() = Some(text.as_bytes().to_vec());
    let fx = Fx::observed(&obs);
    (obs, fx)
}

fn screen_has(fx: &Fx, needle: &str) -> bool {
    fx.hub.screen_text().iter().any(|r| r.contains(needle))
}

#[test]
fn the_exit_notice_is_in_the_screen_and_in_the_frame_before_exit() {
    let (_obs, fx) = with_notice("\r\nBYE-NOTICE\r\n");
    let (_, rec) = fx.viewer(None);
    fx.hub.feed(b"hello");
    fx.hub.exit(Some(0));
    assert!(screen_has(&fx, "BYE-NOTICE"));
    let events = rec.events();
    let notice_at = events
        .iter()
        .position(|e| e.starts_with("data:") && e.contains("BYE-NOTICE"))
        .expect("a frame carries the notice");
    assert_eq!(events.last().map(String::as_str), Some("exit:Some(0)"));
    assert!(notice_at < events.len() - 1, "{events:?}");
}

#[test]
fn the_notice_follows_the_last_output_in_a_frame_of_its_own() {
    let (_obs, fx) = with_notice("\r\nNOTICE");
    let (_, rec) = fx.viewer(None);
    fx.hub.feed(b"output-line");
    fx.hub.exit(None);
    let datas = rec.datas();
    // The full redraw at attach, the output's final frame, then the notice.
    assert!(datas.len() >= 3, "{datas:?}");
    let last = datas.last().unwrap();
    assert!(
        last.contains("NOTICE") && !last.contains("output-line"),
        "{last:?}"
    );
    assert!(datas[datas.len() - 2].contains("output-line"));
}

#[test]
fn an_open_synchronized_update_is_ended_before_the_notice() {
    let (_obs, fx) = with_notice("\r\nAFTER-SYNC");
    let (_, rec) = fx.viewer(None);
    fx.hub.feed(b"\x1b[?2026hhalf-drawn");
    fx.hub.exit(Some(1));
    assert!(screen_has(&fx, "half-drawn"));
    assert!(screen_has(&fx, "AFTER-SYNC"));
    assert!(!fx.hub.modes().sync_pending);
    assert!(rec.events().last().unwrap().starts_with("exit:"));
}

#[test]
fn a_notice_asked_for_on_a_live_pty_waits_for_the_exit() {
    let (_obs, fx) = with_notice("\r\nEXIT-LINE\r\n");
    let (_, rec) = fx.viewer(None);
    fx.hub.append_notice(b"QUEUED-ONE\r\n");
    fx.hub.append_notice(b"QUEUED-TWO\r\n");
    fx.hub.cut();
    assert!(!screen_has(&fx, "QUEUED-ONE"), "reached the live parser");
    assert!(rec.datas().iter().all(|d| !d.contains("QUEUED")));

    fx.hub.exit(Some(0));
    let screen = fx.hub.screen_text().join("\n");
    let (exit_at, one, two) = (
        screen.find("EXIT-LINE").unwrap(),
        screen.find("QUEUED-ONE").unwrap(),
        screen.find("QUEUED-TWO").unwrap(),
    );
    assert!(exit_at < one && one < two, "{screen}");
    assert!(rec.events().last().unwrap().starts_with("exit:"));
}

#[test]
fn a_notice_after_exit_is_appended_and_cut_at_once() {
    let (_obs, fx) = with_notice("\r\nEXIT-LINE\r\n");
    let (_, rec) = fx.viewer(None);
    fx.hub.exit(Some(0));
    let before = rec.events().len();
    fx.hub.append_notice(b"LATE-NOTICE\r\n");
    assert!(screen_has(&fx, "LATE-NOTICE"));
    let events = rec.events();
    assert!(events.len() > before);
    assert!(events.last().unwrap().contains("LATE-NOTICE"), "{events:?}");
}

#[test]
fn a_viewer_attaching_later_sees_the_notices_in_its_redraw() {
    let (_obs, fx) = with_notice("\r\nEXIT-LINE\r\n");
    fx.hub.exit(Some(0));
    fx.hub.append_notice(b"LATE-NOTICE\r\n");
    let (_, rec) = fx.viewer(None);
    let redraw = rec.datas().join("");
    assert!(
        redraw.contains("EXIT-LINE") && redraw.contains("LATE-NOTICE"),
        "{redraw:?}"
    );
    assert_eq!(
        rec.events().last().map(String::as_str),
        Some("exit:Some(0)")
    );
}

#[test]
fn a_hub_whose_observer_supplies_no_notice_adds_nothing() {
    let obs = Arc::new(Obs::default());
    let fx = Fx::observed(&obs);
    let (_, rec) = fx.viewer(None);
    fx.hub.exit(Some(0));
    assert_eq!(rec.kinds(), ["grid", "data", "exit"]);
}

#[test]
fn a_second_exit_appends_no_second_notice() {
    let (_obs, fx) = with_notice("\r\nONCE\r\n");
    fx.hub.exit(Some(0));
    fx.hub.exit(Some(0));
    let screen = fx.hub.screen_text().join("\n");
    assert_eq!(screen.matches("ONCE").count(), 1, "{screen}");
}
