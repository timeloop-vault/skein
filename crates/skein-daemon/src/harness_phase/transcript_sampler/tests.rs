//! The sampler's rules (`runtime.ts`) with an injected stat and clock.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use skein_proto::resume::TranscriptStatDto;

use super::{TRANSCRIPT_SAMPLE_MS, TRANSCRIPT_SAMPLES_CAP, TranscriptSampler};
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::{ActivityPhase, SupervisorCode, SupervisorFacts};
use crate::harness_phase::{HarnessFacts, TranscriptSample};

#[derive(Clone, Copy)]
enum Reply {
    Fail,
    Missing,
}

struct Rig {
    sampler: TranscriptSampler,
    calls: Arc<AtomicUsize>,
    size: Arc<AtomicU64>,
    reply: Arc<Mutex<Option<Reply>>>,
}

/// A stat that reports a file of `size` bytes, or `reply` when one is set.
fn rig() -> Rig {
    let calls = Arc::new(AtomicUsize::new(0));
    let size = Arc::new(AtomicU64::new(100));
    let reply: Arc<Mutex<Option<Reply>>> = Arc::new(Mutex::new(None));
    let (c, s, r) = (Arc::clone(&calls), Arc::clone(&size), Arc::clone(&reply));
    let sampler = TranscriptSampler::new(Arc::new(move |_, _| {
        c.fetch_add(1, Ordering::SeqCst);
        match *r.lock().unwrap() {
            Some(Reply::Fail) => return Err("boom".into()),
            Some(Reply::Missing) => return Ok(None),
            None => {}
        }
        let n = s.load(Ordering::SeqCst);
        Ok(Some(TranscriptStatDto {
            size: n,
            mtime_ms: n,
        }))
    }));
    Rig {
        sampler,
        calls,
        size,
        reply,
    }
}

/// The harness record's `spawned_at` in tests that do not respawn.
const SPAWN: i64 = 1;

fn claude(r: &Rig, now: i64) -> Option<Vec<TranscriptSample>> {
    r.sampler.sample("h", SPAWN, Some(("sid", "/w")), now)
}

fn calls(r: &Rig) -> usize {
    r.calls.load(Ordering::SeqCst)
}

#[test]
fn first_tick_samples_and_stamps_at_with_the_tick_time() {
    let r = rig();
    let s = claude(&r, 5_000).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!((s[0].at, s[0].size), (5_000, 100));
}

#[test]
fn samples_at_most_once_per_interval() {
    let r = rig();
    claude(&r, 0);
    claude(&r, TRANSCRIPT_SAMPLE_MS - 1);
    assert_eq!(calls(&r), 1);
    let s = claude(&r, TRANSCRIPT_SAMPLE_MS).unwrap();
    assert_eq!((calls(&r), s.len()), (2, 2));
}

#[test]
fn keeps_the_newest_eight_oldest_first() {
    let r = rig();
    for i in 0..12 {
        claude(&r, i * TRANSCRIPT_SAMPLE_MS);
    }
    let s = claude(&r, 11 * TRANSCRIPT_SAMPLE_MS).unwrap();
    assert_eq!(s.len(), TRANSCRIPT_SAMPLES_CAP);
    assert_eq!(s[0].at, 4 * TRANSCRIPT_SAMPLE_MS);
    assert_eq!(s[7].at, 11 * TRANSCRIPT_SAMPLE_MS);
}

#[test]
fn a_new_session_id_drops_the_old_samples() {
    let r = rig();
    claude(&r, 0);
    claude(&r, TRANSCRIPT_SAMPLE_MS);
    // Same tick as a throttled one: the reset still happens, nothing is added.
    let s = r
        .sampler
        .sample("h", SPAWN, Some(("other", "/w")), TRANSCRIPT_SAMPLE_MS + 1);
    assert_eq!(s.unwrap().len(), 0);
    let s = r
        .sampler
        .sample("h", SPAWN, Some(("other", "/w")), 2 * TRANSCRIPT_SAMPLE_MS);
    assert_eq!(s.unwrap().len(), 1);
}

/// `runtime.ts` rebuilds the entry when `spawnedAt` changes: the samples go
/// and the throttle stamp with them, so the new process is sampled at once.
#[test]
fn a_respawn_drops_the_samples_and_the_throttle() {
    let r = rig();
    claude(&r, 0);
    assert_eq!(calls(&r), 1);
    // Inside the throttle window, same session: only the new spawn time differs.
    let s = r
        .sampler
        .sample("h", SPAWN + 1, Some(("sid", "/w")), 1)
        .unwrap();
    assert_eq!(calls(&r), 2, "the throttle stamp was reset");
    assert_eq!(s.len(), 1, "the old process's sample is gone");
    assert_eq!(s[0].at, 1);
    // The same spawn time again is throttled and keeps its sample.
    let s = r
        .sampler
        .sample("h", SPAWN + 1, Some(("sid", "/w")), 2)
        .unwrap();
    assert_eq!(calls(&r), 2);
    assert_eq!(s.len(), 1);
}

#[test]
fn a_non_claude_attach_has_no_samples_and_clears_them() {
    let r = rig();
    claude(&r, 0);
    assert_eq!(r.sampler.sample("h", SPAWN, None, 1), None);
    let s = claude(&r, TRANSCRIPT_SAMPLE_MS).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].at, TRANSCRIPT_SAMPLE_MS);
}

#[test]
fn a_stat_error_adds_nothing_and_still_throttles() {
    let r = rig();
    *r.reply.lock().unwrap() = Some(Reply::Fail);
    assert_eq!(claude(&r, 0).unwrap().len(), 0);
    assert_eq!(claude(&r, 1).unwrap().len(), 0);
    assert_eq!(calls(&r), 1);
}

#[test]
fn a_missing_file_adds_nothing() {
    let r = rig();
    *r.reply.lock().unwrap() = Some(Reply::Missing);
    assert_eq!(claude(&r, 0).unwrap().len(), 0);
    *r.reply.lock().unwrap() = None;
    assert_eq!(claude(&r, TRANSCRIPT_SAMPLE_MS).unwrap().len(), 1);
}

#[test]
fn prune_forgets_harnesses_that_are_gone() {
    let r = rig();
    claude(&r, 0);
    r.sampler.prune(&HashSet::new());
    // Forgotten: not throttled, starts with no samples.
    let s = claude(&r, 1).unwrap();
    assert_eq!((calls(&r), s.len()), (2, 1));
    r.sampler.prune(&HashSet::from(["h".to_owned()]));
    assert_eq!(claude(&r, 2).unwrap().len(), 1);
}

#[test]
fn growth_over_the_minimum_window_fires_tail_silent_file_growing() {
    let now = Arc::new(std::sync::atomic::AtomicI64::new(1_000_000));
    let c = Arc::clone(&now);
    let svc = PhaseService::test_with_clock(Arc::new(move || c.load(Ordering::SeqCst)));
    svc.spawned("h", "room", false);
    svc.claude_attached("h");
    assert_ne!(svc.get("h").unwrap().phase, ActivityPhase::Exited);
    let r = rig();
    // Samples 15 s apart; the file grows once, between the first and second.
    for step in 0..6 {
        if step == 1 {
            r.size.store(500, Ordering::SeqCst);
        }
        let t = 1_000_000 + step * TRANSCRIPT_SAMPLE_MS;
        now.store(t, Ordering::SeqCst);
        let mut facts = SupervisorFacts::default();
        facts.harnesses.insert(
            "h".to_owned(),
            HarnessFacts {
                live_attach: Some(true),
                transcript: r.sampler.sample("h", SPAWN, Some(("sid", "/w")), t),
                ..HarnessFacts::default()
            },
        );
        svc.tick_once(&facts);
        let fired = svc
            .with(|i, _| i.machine.supervisor_violations("h").to_vec())
            .iter()
            .any(|v| v.code == SupervisorCode::TailSilentFileGrowing);
        // Silent for 60 s since the first sample: only the last step reaches it
        // (the growth is visible against the first sample from step 1 on).
        assert_eq!(fired, step >= 4, "step {step}");
    }
}
