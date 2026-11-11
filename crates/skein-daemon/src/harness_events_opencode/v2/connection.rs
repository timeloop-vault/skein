//! The v2 reconnect / read loop (#677): `GET /api/event` on the per-harness
//! sidecar, Basic-auth'd. Same shape and timeouts as v1's loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use skein_harness::opencode::v2::{drain_sse_data, parse_event};
use tokio::sync::Notify;

use super::super::OpencodeEvent;
use super::super::connection::{
    BACKOFF_SCHEDULE_SECS, COLD_CONNECT_WARN_AFTER, CONNECT_TIMEOUT, RESPONSE_TIMEOUT, StreamError,
    emit_connect_events,
};
use super::super::session::{SESSION_LOOKUP_TIMEOUT, is_valid_session_id};
use super::translate::{Translation, V2Translator};

type OnEvent = dyn Fn(OpencodeEvent) + Send + Sync;

/// The sidecar's basic-auth user name; only the password is secret.
const USER: &str = "opencode";

/// Where and how to reach one sidecar. Deliberately no `Debug`/`Serialize`:
/// the password must never reach a log.
pub(super) struct Target {
    pub(super) port: u16,
    pub(super) password: String,
}

impl Target {
    fn get(&self, client: &reqwest::Client, path: &str) -> reqwest::RequestBuilder {
        client
            .get(format!("http://127.0.0.1:{}{path}", self.port))
            .basic_auth(USER, Some(&self.password))
    }
}

/// Loops until `cancel` fires, with the v1 backoff between attempts.
pub(in crate::harness_events_opencode) async fn run_adapter_v2(
    port: u16,
    password: String,
    cancel: Arc<Notify>,
    on_event: Arc<OnEvent>,
    session_id: Option<String>,
) {
    run_with(
        &Target { port, password },
        &cancel,
        on_event.as_ref(),
        session_id.as_deref(),
        BACKOFF_SCHEDULE_SECS,
    )
    .await;
}

pub(super) async fn run_with(
    target: &Target,
    cancel: &Notify,
    on_event: &OnEvent,
    session_id: Option<&str>,
    backoff: &[u64],
) {
    let port = target.port;
    let client = match reqwest::Client::builder()
        // reqwest has no implicit loopback bypass of HTTP(S)_PROXY.
        .no_proxy()
        // `connect_timeout`, never `timeout`: the stream is infinite (see v1).
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(port, error = %e, "opencode_events_v2: client build failed");
            on_event(OpencodeEvent::SessionEnd);
            return;
        }
    };
    let connected_once = AtomicBool::new(false);
    let mut translator = V2Translator::new(session_id);
    let mut attempt: usize = 0;
    let started = tokio::time::Instant::now();
    let mut warned_cold = false;
    loop {
        let connect = stream_events(
            &client,
            target,
            RESPONSE_TIMEOUT,
            cancel,
            on_event,
            &connected_once,
            &mut translator,
        );
        tokio::select! {
            biased;
            () = cancel.notified() => return,
            result = connect => match result {
                Ok(()) => attempt = 0,
                Err(e) => {
                    if connected_once.load(Ordering::Acquire) {
                        tracing::warn!(port, attempt, error = %e, "opencode_events_v2: stream error after connect");
                        on_event(OpencodeEvent::SessionEnd);
                    } else if !warned_cold && started.elapsed() >= COLD_CONNECT_WARN_AFTER {
                        warned_cold = true;
                        tracing::warn!(port, attempt, error = %e, "opencode_events_v2: still not connected; retrying");
                    } else {
                        tracing::debug!(port, attempt, error = %e, "opencode_events_v2: cold connect refused");
                    }
                }
            },
        }
        let secs = backoff
            .get(attempt)
            .or(backoff.last())
            .copied()
            .unwrap_or(30);
        tokio::select! {
            biased;
            () = cancel.notified() => return,
            () = tokio::time::sleep(Duration::from_secs(secs)) => {}
        }
        attempt = attempt.saturating_add(1);
    }
}

/// One attempt: open the stream, announce it, translate frames until it ends.
async fn stream_events(
    client: &reqwest::Client,
    target: &Target,
    response_timeout: Duration,
    cancel: &Notify,
    on_event: &OnEvent,
    connected_once: &AtomicBool,
    translator: &mut V2Translator,
) -> Result<(), StreamError> {
    use futures_util::StreamExt;

    let response = tokio::time::timeout(response_timeout, target.get(client, "/api/event").send())
        .await
        .map_err(|_| StreamError::NoResponse(response_timeout))??;
    if !response.status().is_success() {
        return Err(response.error_for_status().unwrap_err().into());
    }
    connected_once.store(true, Ordering::Release);
    tracing::info!(port = target.port, "opencode_events_v2: connected");
    emit_connect_events(on_event);

    let mut byte_stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            biased;
            () = cancel.notified() => return Ok(()),
            chunk = byte_stream.next() => {
                let Some(chunk) = chunk else { return Ok(()) };
                buf.extend_from_slice(&chunk?);
                for payload in drain_sse_data(&mut buf) {
                    let Some(event) = parse_event(&payload) else { continue };
                    let out = match translator.translate(event.clone()) {
                        Translation::Emit(out) => out,
                        Translation::NeedsLookup(sid) => {
                            // Sequential and order-preserving, raced against
                            // cancel so a detach never waits on a lookup.
                            let lookup = lookup_is_root(client, target, &sid);
                            tokio::select! {
                                biased;
                                () = cancel.notified() => return Ok(()),
                                is_root = lookup => match is_root {
                                    Some(root) => {
                                        translator.record(&sid, root);
                                        match translator.translate(event) {
                                            Translation::Emit(out) => out,
                                            Translation::NeedsLookup(_) => Vec::new(),
                                        }
                                    }
                                    None => translator.translate_as_child(event),
                                },
                            }
                        }
                    };
                    for e in out {
                        on_event(e);
                    }
                }
            }
        }
    }
}

/// Root iff `data.parentID` is absent or null. `None` when the body has no
/// `data` object: that is a different shape, not a root.
pub(super) fn is_root_from_body(body: &Value) -> Option<bool> {
    let data = body.get("data")?.as_object()?;
    Some(data.get("parentID").is_none_or(Value::is_null))
}

/// `GET /api/session/{id}`; `None` on any failure (the caller treats the
/// session as a child).
async fn lookup_is_root(
    client: &reqwest::Client,
    target: &Target,
    session_id: &str,
) -> Option<bool> {
    if !is_valid_session_id(session_id) {
        return None;
    }
    let path = format!("/api/session/{session_id}");
    let response = tokio::time::timeout(SESSION_LOOKUP_TIMEOUT, target.get(client, &path).send())
        .await
        .ok()?
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: Value = response.json().await.ok()?;
    is_root_from_body(&body)
}
