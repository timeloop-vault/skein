//! The shadow comparator (#732): pairs the Rust machine's transitions with
//! the ones the webview's TypeScript machine reports, and names every one
//! that has no partner.
//!
//! Why a window and a multiset, not an ordered stream: the two machines tick
//! at different offsets inside the second and the client's rows arrive over
//! IPC, so the same decision shows up on each side a few hundred ms apart and
//! in a different order relative to its neighbours. A transition is
//! "the same" when `(to, source)` match and the timestamps are within
//! [`SHADOW_PAIR_WINDOW_MS`]; whichever side's twin never arrives within that
//! window is a divergence, logged once. `from` is not part of the key: a
//! differing `from` is the echo of an earlier divergence that was already
//! reported, and requiring it made one divergence cascade into a warning per
//! transition. It is counted (`from_mismatch`) and logged at debug.
//!
//! Pure: time is injected and nothing here reads a clock. Logging is the
//! only side effect; every method also returns what it reported so tests
//! assert on values instead of on log output.

use std::collections::{HashMap, HashSet, VecDeque};

use super::types::Transition;

/// How far apart two transitions may be, and how long an unmatched one waits
/// for its twin, before it counts as a divergence.
pub const SHADOW_PAIR_WINDOW_MS: i64 = 5_000;
/// Per harness and per side. A burst past this drops (and reports) the
/// oldest, so a client that stops answering cannot grow memory.
pub const SHADOW_PENDING_CAP: usize = 256;
/// How often the one-line summary is logged, if anything was compared.
pub const SHADOW_SUMMARY_EVERY_MS: i64 = 5 * 60 * 1_000;
/// After a [`ShadowComparator::view_reset`], client rows whose `from` is
/// `spawning` are skipped for at most this long.
pub const SHADOW_RESYNC_MS: i64 = 15_000;
/// A [`ShadowComparator::view_reset`] this soon after a real spawn is the
/// unmount's late echo of that restart, not a rebuild, and is ignored.
pub const SHADOW_RESET_AFTER_SPAWN_MS: i64 = 2_000;
/// At most this many divergence WARN lines per harness per rolling minute.
pub const SHADOW_WARN_PER_MIN: usize = 20;
const WARN_WINDOW_MS: i64 = 60_000;
/// How many of the other side's pending transitions a divergence line names.
const NEARBY: usize = 3;

/// Sources the machine never produces: the frontend writes these straight to
/// `harness_events` (`restartHarness`, the version auto-restart), so the Rust
/// side can never have a twin for them.
const CLIENT_ONLY_SOURCES: [&str; 2] = ["user-restart", "version-auto-restart"];

/// Counters since the service started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShadowStats {
    pub paired: u64,
    pub rust_only: u64,
    pub client_only: u64,
    /// Client rows ignored on purpose (never-produced sources, unknown
    /// harness). Not divergences.
    pub skipped: u64,
    /// Pairs whose `from` differed (counted as paired too).
    pub from_mismatch: u64,
    /// Divergences counted but not logged, over [`SHADOW_WARN_PER_MIN`].
    pub suppressed: u64,
}

/// Which side had a transition the other never matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    RustOnly,
    ClientOnly,
}

impl Side {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RustOnly => "rust_only",
            Self::ClientOnly => "client_only",
        }
    }
}

/// One unmatched transition, as reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub harness_id: String,
    pub side: Side,
    pub from: String,
    pub to: String,
    pub source: String,
    pub age_ms: i64,
    /// The other side's nearest pending transitions for this harness.
    pub nearby: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    from: String,
    to: String,
    source: String,
    /// The transition's own timestamp: what pairing compares.
    at_ms: i64,
    /// When this side's row reached the comparator: what expiry measures, so
    /// a row that arrives late over IPC still gets its full window.
    seen_ms: i64,
}

impl Entry {
    fn same(&self, other: &Entry) -> bool {
        self.to == other.to && self.source == other.source
    }

    fn describe(&self) -> String {
        format!(
            "{}->{} via {} @{}",
            self.from, self.to, self.source, self.at_ms
        )
    }
}

#[derive(Debug, Default)]
struct Pending {
    rust: VecDeque<Entry>,
    client: VecDeque<Entry>,
}

#[derive(Debug, Default)]
pub struct ShadowComparator {
    pending: HashMap<String, Pending>,
    /// Harnesses the service has seen; a client row for any other is ignored.
    known: HashSet<String>,
    /// Per harness: until when client rows from `spawning` are skipped.
    resync_until: HashMap<String, i64>,
    /// Per harness: when the service last really spawned it.
    spawned_at: HashMap<String, i64>,
    /// Per harness: when the recent WARN lines were logged.
    warned: HashMap<String, VecDeque<i64>>,
    stats: ShadowStats,
    last_summary_ms: Option<i64>,
}

impl ShadowComparator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn stats(&self) -> ShadowStats {
        self.stats
    }

    /// The service has a harness by this id, so client rows for it are
    /// comparable.
    pub fn register(&mut self, harness_id: &str) {
        self.known.insert(harness_id.to_owned());
    }

    /// Drop everything pending for a harness (it was forgotten).
    pub fn forget(&mut self, harness_id: &str) {
        self.known.remove(harness_id);
        self.pending.remove(harness_id);
        self.resync_until.remove(harness_id);
        self.spawned_at.remove(harness_id);
        self.warned.remove(harness_id);
    }

    /// The service really spawned this harness: the webview's record of it
    /// is not a rebuild of a running one, so an open resync ends, and a
    /// `view_reset` within [`SHADOW_RESET_AFTER_SPAWN_MS`] is ignored.
    pub fn end_resync(&mut self, harness_id: &str, now_ms: i64) {
        self.resync_until.remove(harness_id);
        self.spawned_at.insert(harness_id.to_owned(), now_ms);
    }

    /// The webview rebuilt its own record for a harness that keeps running:
    /// its machine will replay `spawning -> ...` rows the steady Rust machine
    /// never makes. Drop what is pending for the id and skip client rows from
    /// `spawning` until the first one that is not, or [`SHADOW_RESYNC_MS`].
    pub fn view_reset(&mut self, harness_id: &str, now_ms: i64) {
        if !self.known.contains(harness_id) {
            return;
        }
        if self
            .spawned_at
            .get(harness_id)
            .is_some_and(|&at| now_ms > at && now_ms - at < SHADOW_RESET_AFTER_SPAWN_MS)
        {
            return;
        }
        self.pending.remove(harness_id);
        self.resync_until
            .insert(harness_id.to_owned(), now_ms + SHADOW_RESYNC_MS);
    }

    /// The Rust machine made a transition.
    pub fn rust_transition(&mut self, t: &Transition, now_ms: i64) -> Vec<Divergence> {
        self.known.insert(t.harness_id.clone());
        let entry = Entry {
            from: t.from.as_str().to_owned(),
            to: t.to.as_str().to_owned(),
            source: t.source.as_str().to_owned(),
            at_ms: t.at_ms,
            seen_ms: now_ms,
        };
        self.offer(&t.harness_id, Side::RustOnly, entry, now_ms)
    }

    /// The webview's machine reported a transition (one `harness_events`
    /// row as the TS side wrote it).
    pub fn client_transition(
        &mut self,
        harness_id: &str,
        from: &str,
        to: &str,
        source: Option<&str>,
        at_ms: i64,
        now_ms: i64,
    ) -> Vec<Divergence> {
        let source = source.unwrap_or("none");
        if CLIENT_ONLY_SOURCES.contains(&source) || !self.known.contains(harness_id) {
            self.stats.skipped += 1;
            return Vec::new();
        }
        if let Some(&until) = self.resync_until.get(harness_id) {
            if now_ms >= until || from != "spawning" {
                self.resync_until.remove(harness_id);
            } else {
                self.stats.skipped += 1;
                return Vec::new();
            }
        }
        let entry = Entry {
            from: from.to_owned(),
            to: to.to_owned(),
            source: source.to_owned(),
            at_ms,
            seen_ms: now_ms,
        };
        self.offer(harness_id, Side::ClientOnly, entry, now_ms)
    }

    /// `side` is the side `entry` came from: pair it with the closest twin
    /// on the other side, else queue it.
    fn offer(&mut self, id: &str, side: Side, entry: Entry, now_ms: i64) -> Vec<Divergence> {
        let p = self.pending.entry(id.to_owned()).or_default();
        let (mine, theirs) = match side {
            Side::RustOnly => (&mut p.rust, &mut p.client),
            Side::ClientOnly => (&mut p.client, &mut p.rust),
        };
        let twin = theirs
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                o.same(&entry) && (o.at_ms - entry.at_ms).abs() <= SHADOW_PAIR_WINDOW_MS
            })
            .min_by_key(|(_, o)| (o.at_ms - entry.at_ms).abs())
            .map(|(i, _)| i);
        if let Some(i) = twin {
            let other = theirs.remove(i);
            self.stats.paired += 1;
            if let Some(o) = other {
                if o.from == entry.from {
                    tracing::debug!(
                        target: "skein::phase_shadow",
                        harness = id,
                        from = entry.from,
                        to = entry.to,
                        source = entry.source,
                        "paired"
                    );
                } else {
                    self.stats.from_mismatch += 1;
                    tracing::debug!(
                        target: "skein::phase_shadow",
                        harness = id,
                        this_from = entry.from,
                        other_from = o.from,
                        to = entry.to,
                        source = entry.source,
                        "paired with a different from"
                    );
                }
            }
            return Vec::new();
        }
        mine.push_back(entry);
        let mut out = Vec::new();
        if mine.len() > SHADOW_PENDING_CAP
            && let Some(old) = mine.pop_front()
        {
            let nearby = Self::nearby(p, side, &old);
            out.push(self.report(id, side, &old, nearby, now_ms));
        }
        out
    }

    /// Report every pending entry older than the window. Call once per tick.
    pub fn sweep(&mut self, now_ms: i64) -> Vec<Divergence> {
        let is_old = |e: &Entry| now_ms - e.seen_ms > SHADOW_PAIR_WINDOW_MS;
        // Name the other side's neighbours BEFORE removing anything: the twin
        // a reader wants to see ("client said ... via delegation-settled") is
        // often expiring in this same sweep.
        let mut expired: Vec<(String, Side, Entry, String)> = Vec::new();
        for (id, p) in &self.pending {
            for (side, list) in [(Side::RustOnly, &p.rust), (Side::ClientOnly, &p.client)] {
                for e in list.iter().filter(|e| is_old(e)) {
                    expired.push((id.clone(), side, e.clone(), Self::nearby(p, side, e)));
                }
            }
        }
        for p in self.pending.values_mut() {
            p.rust.retain(|e| !is_old(e));
            p.client.retain(|e| !is_old(e));
        }
        self.pending
            .retain(|_, p| !(p.rust.is_empty() && p.client.is_empty()));
        // HashMap order is arbitrary; report oldest first for a readable log.
        expired.sort_by_key(|(_, _, e, _)| e.at_ms);
        let out: Vec<Divergence> = expired
            .into_iter()
            .map(|(id, side, e, nearby)| self.report(&id, side, &e, nearby, now_ms))
            .collect();
        if let Some(line) = self.take_summary(now_ms) {
            tracing::info!(target: "skein::phase_shadow", "{line}");
        }
        out
    }

    /// The periodic one-line summary, if one is due and anything was compared.
    pub fn take_summary(&mut self, now_ms: i64) -> Option<String> {
        let last = *self.last_summary_ms.get_or_insert(now_ms);
        if now_ms - last < SHADOW_SUMMARY_EVERY_MS {
            return None;
        }
        self.last_summary_ms = Some(now_ms);
        let s = self.stats;
        if s.paired + s.rust_only + s.client_only == 0 {
            return None;
        }
        Some(format!(
            "shadow summary: paired={} rust_only={} client_only={} skipped={} \
             from_mismatch={} suppressed={}",
            s.paired, s.rust_only, s.client_only, s.skipped, s.from_mismatch, s.suppressed
        ))
    }

    fn report(
        &mut self,
        id: &str,
        side: Side,
        e: &Entry,
        nearby: String,
        now_ms: i64,
    ) -> Divergence {
        match side {
            Side::RustOnly => self.stats.rust_only += 1,
            Side::ClientOnly => self.stats.client_only += 1,
        }
        let d = Divergence {
            harness_id: id.to_owned(),
            side,
            from: e.from.clone(),
            to: e.to.clone(),
            source: e.source.clone(),
            age_ms: now_ms - e.seen_ms,
            nearby,
        };
        if self.may_warn(id, now_ms) {
            tracing::warn!(
                target: "skein::phase_shadow",
                harness = %d.harness_id,
                side = d.side.as_str(),
                from = %d.from,
                to = %d.to,
                source = %d.source,
                age_ms = d.age_ms,
                other_side = %d.nearby,
                "phase shadow divergence"
            );
        } else {
            self.stats.suppressed += 1;
        }
        d
    }

    /// The rolling-minute WARN budget of one harness; true when a line may
    /// be logged (and books it).
    fn may_warn(&mut self, id: &str, now_ms: i64) -> bool {
        let log = self.warned.entry(id.to_owned()).or_default();
        while log.front().is_some_and(|&t| now_ms - t >= WARN_WINDOW_MS) {
            log.pop_front();
        }
        if log.len() >= SHADOW_WARN_PER_MIN {
            return false;
        }
        log.push_back(now_ms);
        true
    }

    /// The other side's pending transitions nearest in time to `e`.
    fn nearby(p: &Pending, side: Side, e: &Entry) -> String {
        let others = match side {
            Side::RustOnly => &p.client,
            Side::ClientOnly => &p.rust,
        };
        let mut near: Vec<&Entry> = others.iter().collect();
        near.sort_by_key(|o| (o.at_ms - e.at_ms).abs());
        if near.is_empty() {
            return "none".to_owned();
        }
        near.iter()
            .take(NEARBY)
            .map(|o| o.describe())
            .collect::<Vec<_>>()
            .join("; ")
    }
}
