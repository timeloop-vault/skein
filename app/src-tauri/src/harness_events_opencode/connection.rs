//! The reconnect / read loop: connect to `/event`, announce the
//! connection, and read frames until the stream closes or errors.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use super::OpencodeEvent;
use super::frames::process_buffer;
use super::session::{decide_root_session_prompted, lookup_session_is_root, seed_own_session_root};
use crate::db::Database;
use crate::harness_actions_opencode;

/// Reconnect backoff schedule (seconds), capped at the last value
/// for any further attempts. 1 → 2 → 4 → 8 → 16 → 30 → 30 … gets us
/// from "opencode hasn't bound yet" to "opencode is dead" with
/// minimal wasted polling.
pub(super) const BACKOFF_SCHEDULE_SECS: &[u64] = &[1, 2, 4, 8, 16, 30];

/// TCP connect timeout. Localhost is sub-millisecond when opencode is
/// alive; we keep a small budget so a wedged opencode (port bound but
/// listener dead) doesn't hang the reconnect loop forever.
///
/// Critical: this is `connect_timeout`, NOT `timeout`. `timeout` is
/// the overall request budget — SSE responses are infinite by
/// design, so a `timeout` would abort the stream after the budget
/// expires with "error decoding response body" (observed against a
/// live opencode session in v1: 5 s timeout, stream healthy, error
/// fired after 5 s every reconnect cycle).
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Budget for `/event` to answer with its response headers once the TCP
/// connect has succeeded. opencode writes them immediately, followed by a
/// `server.connected` frame, so 10 s is generous. Without it, a request
/// opencode accepted but never answered waited forever: no retry, no log,
/// and the harness sat on `running` with an empty Live Context.
/// `CONNECT_TIMEOUT` doesn't cover this, and `timeout` can't (see above).
pub(super) const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long cold-connect attempts may keep failing before the adapter
/// logs a warning. Those failures are normal for the first second or two
/// while opencode binds its port, so each one logs at `debug`. That level
/// is off by default, so an adapter that never connects would otherwise
/// leave nothing in the log.
pub(super) const COLD_CONNECT_WARN_AFTER: Duration = Duration::from_secs(30);

/// The reconnect / read loop. Lives in a tokio task. Loops until
/// `cancel` fires, with exponential backoff between attempts.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_adapter(
    port: u16,
    cancel: Arc<Notify>,
    on_event: Arc<dyn Fn(OpencodeEvent) + Send + Sync>,
    db: Arc<Database>,
    app: Option<tauri::AppHandle>,
    harness_id: String,
    room_id: String,
    cwd: String,
    session_id: Option<String>,
) {
    let url = format!("http://127.0.0.1:{port}/event");
    let client = match reqwest::Client::builder()
        // reqwest honours HTTP(S)_PROXY / ALL_PROXY and has no implicit loopback bypass.
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(port, error = %e, "opencode_events: client build failed");
            on_event(OpencodeEvent::SessionEnd);
            return;
        }
    };

    // Shared with stream_events so it can flip the flag the moment
    // a 200 response opens — used to decide whether a later error
    // gets a `SessionEnd` signal (yes, real disconnect) or stays
    // quiet (no, this is still the initial cold-connect race while
    // opencode hasn't bound yet).
    let connected_once = Arc::new(AtomicBool::new(false));
    // Per-adapter root/child cache (#116), keyed by opencode session
    // id. Lives across reconnects deliberately: `session.created` only
    // fires once, at session birth, so a reconnect that misses it must
    // not forget what an earlier connection already learned.
    let root_cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
    // Seed with the harness's own attached/resumed session: Skein only
    // ever captures or resumes a ROOT session for a harness, never a
    // subagent child, so this is safe to assume without a lookup — and
    // it means a boot resume's first user message doesn't pay a
    // network round trip for something already known.
    if let Some((sid, is_root)) = seed_own_session_root(session_id.as_deref()) {
        root_cache.lock().insert(sid, is_root);
    }
    // Survives reconnects like `root_cache`: a user message and its text
    // part can straddle one.
    let prompts = Mutex::new(harness_actions_opencode::UserPromptTracker::default());
    let mut attempt: usize = 0;
    let started = tokio::time::Instant::now();
    let mut warned_cold = false;
    loop {
        // Race the SSE attempt against cancellation. If `cancel` ever
        // fires (Drop of Adapter), exit immediately.
        let connected_for_call = Arc::clone(&connected_once);
        let connect = stream_events(
            &client,
            &url,
            port,
            RESPONSE_TIMEOUT,
            &cancel,
            on_event.as_ref(),
            &connected_for_call,
            &db,
            app.as_ref(),
            &harness_id,
            &room_id,
            &cwd,
            &root_cache,
            &prompts,
        );
        tokio::select! {
            biased;
            () = cancel.notified() => {
                tracing::debug!(port, "opencode_events: cancelled");
                return;
            }
            result = connect => {
                match result {
                    Ok(()) => {
                        // Stream ended cleanly. Reset backoff so
                        // reconnect after an opencode restart is
                        // fast.
                        attempt = 0;
                    }
                    Err(e) => {
                        if connected_once.load(Ordering::Acquire) {
                            tracing::warn!(port, attempt, error = %e, "opencode_events: stream error after connect");
                            // Surface as SessionEnd so the frontend
                            // falls back to L2a until we reconnect.
                            on_event(OpencodeEvent::SessionEnd);
                        } else if !warned_cold && started.elapsed() >= COLD_CONNECT_WARN_AFTER {
                            warned_cold = true;
                            tracing::warn!(
                                port,
                                attempt,
                                harness_id,
                                error = %e,
                                "opencode_events: still not connected after {}s; retrying",
                                COLD_CONNECT_WARN_AFTER.as_secs()
                            );
                        } else {
                            tracing::debug!(port, attempt, error = %e, "opencode_events: cold connect refused");
                        }
                    }
                }
            }
        }

        // Pick the backoff for this attempt and sleep, but wake
        // early on cancel. Index saturates at the last entry of
        // the schedule so we plateau at 30 s.
        let secs = BACKOFF_SCHEDULE_SECS
            .get(attempt)
            .copied()
            .unwrap_or_else(|| BACKOFF_SCHEDULE_SECS.last().copied().unwrap_or(30));
        let sleep = tokio::time::sleep(Duration::from_secs(secs));
        tokio::select! {
            biased;
            () = cancel.notified() => {
                tracing::debug!(port, "opencode_events: cancelled during backoff");
                return;
            }
            () = sleep => {}
        }
        attempt = attempt.saturating_add(1);
    }
}

/// What every successful (re)connect announces: `Connected`, then the
/// assumed-idle baseline. opencode only broadcasts `session.status` on
/// *transitions*; subscribers don't get a baseline. A session sitting
/// at its prompt (fresh spawn or restart-of-idle-session) never emits
/// `session.status idle` until the user types something — so without
/// the baseline the dot stays in `spawning|running` forever for rooms
/// that started Skein already idle, and a turn that ended during an
/// SSE outage would stick on running.
///
/// It is its own variant, not `SessionIdle` (#175): on a mid-turn
/// reconnect the baseline is a guess, and the frontend must not treat
/// it as an observed "your turn". If opencode is actually mid-turn,
/// the next `session.status busy` arrives within ~100 ms and
/// overrides it.
pub(super) fn emit_connect_events(on_event: &(dyn Fn(OpencodeEvent) + Send + Sync)) {
    on_event(OpencodeEvent::Connected);
    on_event(OpencodeEvent::SessionBaselineIdle);
}

/// Why one `stream_events` attempt failed.
#[derive(Debug)]
pub(super) enum StreamError {
    /// Connect refused, HTTP non-2xx, or a transport error mid-stream.
    Http(reqwest::Error),
    /// Connected, but `/event` sent no response within the budget.
    NoResponse(Duration),
}

impl From<reqwest::Error> for StreamError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "{e}"),
            Self::NoResponse(d) => write!(f, "no response from /event within {}s", d.as_secs_f32()),
        }
    }
}

/// Open the SSE stream and dispatch events until it closes or errors.
/// Returns `Ok(())` on clean stream end (server closed), `Err` on
/// any failure (connect refused, HTTP non-200, no response within
/// `response_timeout`, transport error).
/// Mutates the `connected_once` signal indirectly by emitting
/// `Connected` on first message.
#[allow(clippy::too_many_arguments)]
pub(super) async fn stream_events(
    client: &reqwest::Client,
    url: &str,
    port: u16,
    response_timeout: Duration,
    cancel: &Notify,
    on_event: &(dyn Fn(OpencodeEvent) + Send + Sync),
    connected_once: &AtomicBool,
    db: &Database,
    app: Option<&tauri::AppHandle>,
    harness_id: &str,
    room_id: &str,
    cwd: &str,
    root_cache: &Mutex<HashMap<String, bool>>,
    prompts: &Mutex<harness_actions_opencode::UserPromptTracker>,
) -> Result<(), StreamError> {
    use futures_util::StreamExt;

    let response = tokio::time::timeout(response_timeout, client.get(url).send())
        .await
        .map_err(|_| StreamError::NoResponse(response_timeout))??;
    if !response.status().is_success() {
        // Coerce non-2xx into an error so the caller bumps backoff.
        return Err(response.error_for_status().unwrap_err().into());
    }
    // Flip the shared flag so the caller knows we made it past the
    // HTTP handshake. Any error from this point on is a real
    // disconnect, not a cold-connect race.
    connected_once.store(true, Ordering::Release);
    tracing::info!(port, harness_id, "opencode_events: connected");
    emit_connect_events(on_event);

    let mut byte_stream = response.bytes_stream();
    // Carries incomplete bytes across chunk boundaries. SSE framing
    // is `data: {json}\n\n` but TCP can split a frame anywhere; we
    // buffer and split on the `\n\n` separator.
    let mut buf: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            biased;
            () = cancel.notified() => return Ok(()),
            chunk = byte_stream.next() => {
                let Some(chunk) = chunk else {
                    // Stream ended cleanly.
                    return Ok(());
                };
                let chunk = chunk?;
                buf.extend_from_slice(&chunk);
                let unresolved =
                    process_buffer(&mut buf, on_event, db, app, harness_id, room_id, cwd, root_cache, prompts);
                // Resolve outside process_buffer: this is the only
                // point in the adapter allowed to do network I/O, and
                // process_buffer stays synchronous and unit-testable.
                // Sequential (order-preserving), but each lookup races
                // `cancel` too — up to SESSION_LOOKUP_TIMEOUT per id is
                // otherwise unobserved by the outer select!, so closing
                // a harness mid-lookup would hang the detach on it.
                for session_id in unresolved {
                    let lookup = lookup_session_is_root(client, port, &session_id);
                    tokio::select! {
                        biased;
                        () = cancel.notified() => {
                            tracing::debug!(port, "opencode_events: cancelled during root lookup");
                            return Ok(());
                        }
                        is_root = lookup => {
                            if let Some(is_root) = is_root {
                                root_cache.lock().insert(session_id.clone(), is_root);
                                if let Some(event) = decide_root_session_prompted(&session_id, Some(is_root)) {
                                    on_event(event);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
