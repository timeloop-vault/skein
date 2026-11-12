//! Shadow comparator (#732): pairing, expiry, skips, the cap.

// `assert!(x.is_empty())` reads better than comparing against a typed empty array.
#![allow(clippy::assert_is_empty)]

use super::shadow::{
    SHADOW_PAIR_WINDOW_MS as W, SHADOW_PENDING_CAP, SHADOW_SUMMARY_EVERY_MS, ShadowComparator,
    ShadowStats, Side,
};
use super::types::{ActivityPhase, Transition, TransitionSource};

fn rust(id: &str, source: TransitionSource, at_ms: i64) -> Transition {
    Transition {
        harness_id: id.to_owned(),
        from: ActivityPhase::Running,
        to: ActivityPhase::Waiting,
        source,
        at_ms,
        has_user_input: false,
    }
}

fn client(c: &mut ShadowComparator, id: &str, source: &str, at_ms: i64, now: i64) {
    let d = c.client_transition(id, "running", "waiting", Some(source), at_ms, now);
    assert!(d.is_empty());
}

const END: TransitionSource = TransitionSource::L2c1ClaudeEndTurn;

#[test]
fn pairs_rust_then_client() {
    let mut c = ShadowComparator::new();
    c.register("h");
    assert!(c.rust_transition(&rust("h", END, 1_000), 1_000).is_empty());
    client(&mut c, "h", "l2c1-claude-end-turn", 1_400, 1_400);
    assert_eq!(c.stats().paired, 1);
    assert!(c.sweep(1_000 + 2 * W).is_empty());
    assert_eq!(c.stats().rust_only + c.stats().client_only, 0);
}

#[test]
fn pairs_client_then_rust() {
    let mut c = ShadowComparator::new();
    c.register("h");
    client(&mut c, "h", "l2c1-claude-end-turn", 1_000, 1_000);
    assert!(c.rust_transition(&rust("h", END, 1_900), 1_900).is_empty());
    assert_eq!(c.stats().paired, 1);
}

#[test]
fn pairs_as_a_multiset_not_in_order() {
    let mut c = ShadowComparator::new();
    c.register("h");
    let tool = TransitionSource::L2c1ClaudeToolResult;
    let _ = c.rust_transition(&rust("h", END, 1_000), 1_000);
    let _ = c.rust_transition(&rust("h", tool, 1_100), 1_100);
    let _ = c.rust_transition(&rust("h", END, 1_200), 1_200);
    // The client reports them in a different order.
    client(&mut c, "h", "l2c1-claude-end-turn", 1_050, 1_300);
    client(&mut c, "h", "l2c1-claude-tool-result", 1_150, 1_300);
    client(&mut c, "h", "l2c1-claude-end-turn", 1_250, 1_300);
    assert_eq!(c.stats().paired, 3);
    assert!(c.sweep(1_300 + 2 * W).is_empty());
}

#[test]
fn pairs_with_the_closest_twin() {
    let mut c = ShadowComparator::new();
    c.register("h");
    let _ = c.rust_transition(&rust("h", END, 1_000), 1_000);
    let _ = c.rust_transition(&rust("h", END, 4_000), 4_000);
    client(&mut c, "h", "l2c1-claude-end-turn", 3_900, 4_100);
    // The 4_000 one was taken, so the 1_000 one is what is left.
    let d = c.sweep(1_000 + W + 1);
    assert_eq!(d.len(), 1);
    assert!(d[0].nearby == "none" && d[0].side == Side::RustOnly);
}

#[test]
fn timestamps_further_apart_than_the_window_do_not_pair() {
    let mut c = ShadowComparator::new();
    c.register("h");
    let _ = c.rust_transition(&rust("h", END, 1_000), 1_000);
    client(&mut c, "h", "l2c1-claude-end-turn", 1_000 + W + 1, 1_100);
    assert_eq!(c.stats().paired, 0);
    assert_eq!(c.sweep(1_100 + W + 1).len(), 2);
}

#[test]
fn a_different_source_is_a_divergence_naming_the_other_side() {
    let mut c = ShadowComparator::new();
    c.register("h");
    let _ = c.rust_transition(&rust("h", END, 1_000), 1_000);
    client(&mut c, "h", "delegation-settled", 1_200, 1_200);
    let d = c.sweep(1_200 + W + 1);
    assert_eq!(d.len(), 2);
    let r = d.iter().find(|d| d.side == Side::RustOnly).unwrap();
    assert_eq!(r.source, "l2c1-claude-end-turn");
    assert_eq!(r.nearby, "running->waiting via delegation-settled @1200");
    let k = d.iter().find(|d| d.side == Side::ClientOnly).unwrap();
    assert_eq!(k.nearby, "running->waiting via l2c1-claude-end-turn @1000");
    assert_eq!(
        c.stats(),
        ShadowStats {
            paired: 0,
            rust_only: 1,
            client_only: 1,
            skipped: 0,
            ..ShadowStats::default()
        }
    );
}

#[test]
fn rust_only_expires_after_the_window_not_before() {
    let mut c = ShadowComparator::new();
    let _ = c.rust_transition(&rust("h", END, 1_000), 1_000);
    assert!(c.sweep(1_000 + W).is_empty());
    let d = c.sweep(1_000 + W + 1);
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].side, d[0].age_ms), (Side::RustOnly, W + 1));
    // Reported once.
    assert!(c.sweep(1_000 + 3 * W).is_empty());
    assert_eq!(c.stats().rust_only, 1);
}

#[test]
fn client_only_expires_after_the_window() {
    let mut c = ShadowComparator::new();
    c.register("h");
    client(&mut c, "h", "l2a-idle", 1_000, 1_000);
    assert!(c.sweep(1_000 + W).is_empty());
    let d = c.sweep(1_000 + W + 1);
    assert_eq!((d.len(), d[0].side), (1, Side::ClientOnly));
    assert_eq!(c.stats().client_only, 1);
}

#[test]
fn expiry_counts_from_arrival_not_from_the_row_timestamp() {
    let mut c = ShadowComparator::new();
    c.register("h");
    // A client row stamped long ago that only just arrived still waits.
    client(&mut c, "h", "l2a-idle", 0, 10 * W);
    assert!(c.sweep(10 * W + W).is_empty());
}

#[test]
fn rows_the_machine_never_produces_are_skipped() {
    let mut c = ShadowComparator::new();
    c.register("h");
    for s in ["user-restart", "version-auto-restart"] {
        assert!(
            c.client_transition("h", "running", "spawning", Some(s), 1, 1)
                .is_empty()
        );
    }
    assert!(c.sweep(10 * W).is_empty());
    assert_eq!(c.stats().skipped, 2);
    assert_eq!(c.stats().client_only, 0);
}

#[test]
fn client_rows_for_an_unknown_harness_are_skipped() {
    let mut c = ShadowComparator::new();
    assert!(
        c.client_transition("ghost", "running", "waiting", Some("l2a-idle"), 1, 1)
            .is_empty()
    );
    assert!(c.sweep(10 * W).is_empty());
    assert_eq!(c.stats().skipped, 1);
    // A Rust transition makes the id known.
    let _ = c.rust_transition(&rust("ghost", END, 5), 5);
    client(&mut c, "ghost", "l2c1-claude-end-turn", 5, 5);
    assert_eq!(c.stats().paired, 1);
}

#[test]
fn forget_drops_pending_and_the_known_id() {
    let mut c = ShadowComparator::new();
    c.register("h");
    let _ = c.rust_transition(&rust("h", END, 1_000), 1_000);
    c.forget("h");
    assert!(c.sweep(10 * W).is_empty());
    assert_eq!(c.stats().rust_only, 0);
    let skipped = c.client_transition("h", "a", "b", Some("x"), 1, 1);
    assert!(skipped.is_empty());
    assert_eq!(c.stats().skipped, 1);
}

#[test]
fn the_cap_drops_and_reports_the_oldest() {
    let mut c = ShadowComparator::new();
    let mut reported = Vec::new();
    for i in 0..=i64::try_from(SHADOW_PENDING_CAP).unwrap() {
        reported.extend(c.rust_transition(&rust("h", END, i), i));
    }
    assert_eq!(reported.len(), 1);
    assert_eq!((reported[0].side, c.stats().rust_only), (Side::RustOnly, 1));
    // The oldest (at 0) was dropped; the rest still wait for their twin.
    client(&mut c, "h", "l2c1-claude-end-turn", 1, 300);
    assert_eq!(c.stats().paired, 1);
}

#[test]
fn summary_is_logged_every_five_minutes_if_anything_was_compared() {
    let mut c = ShadowComparator::new();
    c.register("h");
    assert!(c.take_summary(0).is_none());
    let _ = c.rust_transition(&rust("h", END, 10), 10);
    client(&mut c, "h", "l2c1-claude-end-turn", 10, 10);
    assert!(c.take_summary(SHADOW_SUMMARY_EVERY_MS - 1).is_none());
    let line = c.take_summary(SHADOW_SUMMARY_EVERY_MS).unwrap();
    assert!(line.contains("paired=1"));
    assert!(c.take_summary(SHADOW_SUMMARY_EVERY_MS + 1).is_none());
}

#[test]
fn no_summary_when_nothing_was_compared() {
    let mut c = ShadowComparator::new();
    assert!(c.take_summary(0).is_none());
    assert!(c.take_summary(SHADOW_SUMMARY_EVERY_MS).is_none());
}
