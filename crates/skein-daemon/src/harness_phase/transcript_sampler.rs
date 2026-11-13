//! The Claude transcript samples `tail_silent_file_growing` reads (#732), a
//! port of the sampler half of `app/src/supervisor/runtime.ts`: one stat per
//! harness every [`TRANSCRIPT_SAMPLE_MS`], the newest [`TRANSCRIPT_SAMPLES_CAP`]
//! kept oldest first, reset when the session id changes, the attach stops
//! being Claude, or the harness is respawned (`spawned_at` changes, which also
//! clears the throttle stamp). Time and the stat are injected; the state has its own lock,
//! which is never held across the stat.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use skein_proto::resume::TranscriptStatDto;

use super::invariants::TranscriptSample;

pub const TRANSCRIPT_SAMPLE_MS: i64 = 15_000;
pub const TRANSCRIPT_SAMPLES_CAP: usize = 8;

/// `(session id, cwd)` to the transcript's size and mtime; `Ok(None)` = no file.
pub type StatFn =
    Arc<dyn Fn(&str, &str) -> Result<Option<TranscriptStatDto>, String> + Send + Sync>;

/// Epoch milliseconds, the clock the ticker stamps samples with.
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[derive(Default)]
struct Entry {
    spawned_at: Option<i64>,
    session_id: Option<String>,
    samples: Vec<TranscriptSample>,
    last_sample_at: Option<i64>,
}

pub struct TranscriptSampler {
    stat: StatFn,
    entries: Mutex<HashMap<String, Entry>>,
}

impl TranscriptSampler {
    #[must_use]
    pub fn new(stat: StatFn) -> Self {
        Self {
            stat,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The real stat, `resume::claude_transcript_stat`.
    #[must_use]
    pub fn system() -> Self {
        Self::new(Arc::new(crate::resume::claude_transcript_stat))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One tick for one harness. `spawned_at` is the record's; a change means
    /// a new process, whose samples and throttle start over. `claude` is `Some((session id, cwd))` only for
    /// a live Claude attach. Returns the samples (oldest first) for a Claude
    /// attach, `None` otherwise, as `runtime.ts` builds `transcript`. `at` of a
    /// sample is `now` as of the sampling decision, not the stat's completion.
    pub fn sample(
        &self,
        id: &str,
        spawned_at: i64,
        claude: Option<(&str, &str)>,
        now: i64,
    ) -> Option<Vec<TranscriptSample>> {
        let Some((session_id, cwd)) = claude else {
            let mut map = self.lock();
            if let Some(e) = map.get_mut(id) {
                if e.spawned_at != Some(spawned_at) {
                    *e = Entry::default();
                }
                e.spawned_at = Some(spawned_at);
                e.session_id = None;
                e.samples.clear();
            }
            return None;
        };
        let due = {
            let mut map = self.lock();
            let e = map.entry(id.to_owned()).or_default();
            if e.spawned_at != Some(spawned_at) {
                *e = Entry {
                    spawned_at: Some(spawned_at),
                    ..Entry::default()
                };
            }
            if e.session_id.as_deref() != Some(session_id) {
                e.session_id = Some(session_id.to_owned());
                e.samples.clear();
            }
            let due = e
                .last_sample_at
                .is_none_or(|t| now - t >= TRANSCRIPT_SAMPLE_MS);
            if due {
                e.last_sample_at = Some(now);
            }
            due
        };
        if due {
            // No lock held: this touches the file system.
            match (self.stat)(session_id, cwd) {
                Ok(Some(st)) => self.push(id, session_id, now, &st),
                Ok(None) => {}
                Err(err) => {
                    tracing::debug!(harness = id, error = %err, "supervisor transcript stat failed");
                }
            }
        }
        self.lock().get(id).map(|e| e.samples.clone())
    }

    fn push(&self, id: &str, session_id: &str, now: i64, st: &TranscriptStatDto) {
        let mut map = self.lock();
        // Dropped or re-pointed while the stat ran.
        let Some(e) = map.get_mut(id) else { return };
        if e.session_id.as_deref() != Some(session_id) {
            return;
        }
        e.samples.push(TranscriptSample {
            at: now,
            size: i64::try_from(st.size).unwrap_or(i64::MAX),
            #[allow(clippy::cast_precision_loss)]
            mtime_ms: st.mtime_ms as f64,
        });
        if e.samples.len() > TRANSCRIPT_SAMPLES_CAP {
            e.samples.remove(0);
        }
    }

    /// Forget every harness not in `live` (`entries.delete` in `runtime.ts`).
    pub fn prune(&self, live: &HashSet<String>) {
        self.lock().retain(|id, _| live.contains(id));
    }
}

#[cfg(test)]
mod tests;
