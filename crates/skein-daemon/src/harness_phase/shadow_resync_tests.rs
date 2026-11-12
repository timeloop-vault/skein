//! Shadow comparator (#732): pairing on `(to, source)`, view resync, the
//! WARN rate cap.

#![allow(clippy::assert_is_empty)]

use super::shadow::{
    SHADOW_PAIR_WINDOW_MS as W, SHADOW_RESET_AFTER_SPAWN_MS, SHADOW_RESYNC_MS,
    SHADOW_SUMMARY_EVERY_MS, SHADOW_WARN_PER_MIN, ShadowComparator,
};
use super::types::{ActivityPhase, Transition, TransitionSource};

fn rust(id: &str, from: ActivityPhase, to: ActivityPhase, at_ms: i64) -> Transition {
    Transition {
        harness_id: id.to_owned(),
        from,
        to,
        source: TransitionSource::L2c1ClaudeEndTurn,
        at_ms,
        has_user_input: false,
    }
}

const END: &str = "l2c1-claude-end-turn";

#[test]
fn pairs_on_to_and_source_and_counts_a_differing_from() {
    let mut c = ShadowComparator::new();
    c.register("h");
    let t = rust("h", ActivityPhase::Running, ActivityPhase::Waiting, 1_000);
    assert!(c.rust_transition(&t, 1_000).is_empty());
    assert!(
        c.client_transition("h", "idle", "waiting", Some(END), 1_100, 1_100)
            .is_empty()
    );
    let s = c.stats();
    assert_eq!((s.paired, s.from_mismatch), (1, 1));
    assert!(c.sweep(1_000 + 2 * W).is_empty());
    assert_eq!(c.stats().rust_only + c.stats().client_only, 0);
}

#[test]
fn view_reset_skips_spawning_rows_until_the_first_other_one() {
    let mut c = ShadowComparator::new();
    c.register("h");
    let t = rust("h", ActivityPhase::Running, ActivityPhase::Waiting, 1_000);
    let _ = c.rust_transition(&t, 1_000);
    c.view_reset("h", 2_000);
    // The pending Rust row was dropped with the reset.
    assert!(c.sweep(2_000 + 2 * W).is_empty());
    assert_eq!(c.stats().rust_only, 0);
    for at in [2_100, 2_200] {
        assert!(
            c.client_transition("h", "spawning", "running", Some("pty-output"), at, at)
                .is_empty()
        );
    }
    assert_eq!(c.stats().skipped, 2);
    // The first row that is not from `spawning` ends the resync and is compared.
    let _ = c.client_transition("h", "running", "waiting", Some(END), 2_300, 2_300);
    let _ = c.client_transition("h", "spawning", "running", Some("pty-output"), 2_400, 2_400);
    assert_eq!(c.stats().skipped, 2);
    assert_eq!(c.sweep(2_400 + 2 * W).len(), 2);
}

#[test]
fn view_reset_resync_ends_after_the_period() {
    let mut c = ShadowComparator::new();
    c.register("h");
    c.view_reset("h", 1_000);
    let at = 1_000 + SHADOW_RESYNC_MS;
    let _ = c.client_transition("h", "spawning", "running", Some("pty-output"), at, at);
    assert_eq!(c.stats().skipped, 0);
    assert_eq!(c.sweep(at + 2 * W).len(), 1);
}

#[test]
fn rust_transitions_are_compared_normally_during_resync() {
    let mut c = ShadowComparator::new();
    c.register("h");
    c.view_reset("h", 1_000);
    let t = rust("h", ActivityPhase::Running, ActivityPhase::Waiting, 1_100);
    let _ = c.rust_transition(&t, 1_100);
    assert!(
        c.client_transition("h", "running", "waiting", Some(END), 1_200, 1_200)
            .is_empty()
    );
    assert_eq!(c.stats().paired, 1);
}

#[test]
fn divergence_warns_are_capped_per_harness_per_minute_and_summarised() {
    let mut cmp = ShadowComparator::new();
    cmp.register("h");
    cmp.register("other");
    let n = SHADOW_WARN_PER_MIN + 5;
    for i in 0..n {
        let at = i64::try_from(i).unwrap();
        let t = rust("h", ActivityPhase::Running, ActivityPhase::Waiting, at);
        let _ = cmp.rust_transition(&t, at);
    }
    // All reported as divergences, the extra ones only counted.
    let div = cmp.sweep(10 * W);
    assert_eq!(div.len(), n);
    let st = cmp.stats();
    assert_eq!(
        (st.rust_only, st.suppressed),
        (u64::try_from(n).unwrap(), 5)
    );
    // Another harness has its own budget.
    let t = rust("other", ActivityPhase::Running, ActivityPhase::Waiting, 1);
    let _ = cmp.rust_transition(&t, 1);
    let _ = cmp.sweep(20 * W);
    assert_eq!(cmp.stats().suppressed, 5);
    let line = cmp.take_summary(50_000 + SHADOW_SUMMARY_EVERY_MS).unwrap();
    assert!(line.contains("suppressed=5") && line.contains("from_mismatch=0"));
}

fn spawning_waiting(c: &mut ShadowComparator, at: i64) {
    let t = Transition {
        harness_id: "h".to_owned(),
        from: ActivityPhase::Spawning,
        to: ActivityPhase::Waiting,
        source: TransitionSource::L2c1ClaudeEndTurn,
        at_ms: at,
        has_user_input: false,
    };
    let _ = c.rust_transition(&t, at);
    let _ = c.client_transition("h", "spawning", "waiting", Some(END), at + 50, at + 50);
}

#[test]
fn a_real_spawn_after_a_view_reset_ends_the_resync() {
    let mut c = ShadowComparator::new();
    c.register("h");
    c.view_reset("h", 1_000);
    c.end_resync("h", 1_100);
    spawning_waiting(&mut c, 1_200);
    assert_eq!((c.stats().paired, c.stats().skipped), (1, 0));
    assert!(c.sweep(1_200 + 2 * W).is_empty());
}

#[test]
fn a_view_reset_just_after_a_real_spawn_is_ignored() {
    let mut c = ShadowComparator::new();
    c.register("h");
    c.end_resync("h", 1_000);
    c.view_reset("h", 1_500);
    spawning_waiting(&mut c, 1_600);
    assert_eq!((c.stats().paired, c.stats().skipped), (1, 0));
    // Later than the grace period it resyncs as usual.
    c.view_reset("h", 1_000 + SHADOW_RESET_AFTER_SPAWN_MS);
    let _ = c.client_transition("h", "spawning", "running", Some("pty-output"), 4_000, 4_000);
    assert_eq!(c.stats().skipped, 1);
}

#[test]
fn a_view_reset_with_no_spawn_still_skips_spawning_rows() {
    let mut c = ShadowComparator::new();
    c.register("h");
    c.view_reset("h", 1_000);
    let _ = c.client_transition("h", "spawning", "waiting", Some(END), 1_100, 1_100);
    assert_eq!(c.stats().skipped, 1);
}
