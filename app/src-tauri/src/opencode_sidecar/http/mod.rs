//! The two HTTP calls Skein makes to its own sidecar (#676): wait until it
//! answers, then create a session. Both take `(base_url, password)` so a
//! mock server can stand in for opencode.

use std::time::{Duration, Instant};

const USER: &str = "opencode";
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const READY_POLL: Duration = Duration::from_millis(100);
pub(crate) const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// A client for the loopback sidecar. `no_proxy`: reqwest honours
/// `HTTP(S)_PROXY` and has no implicit loopback bypass.
pub(crate) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))
}

/// Poll `GET /api/agent` (opencode has no health endpoint; this is the
/// cheapest authenticated route) until it answers 200. `abort` is asked
/// before every attempt; `Some(reason)` ends the wait with that error.
pub(crate) async fn wait_ready(
    client: &reqwest::Client,
    base_url: &str,
    password: &str,
    interval: Duration,
    timeout: Duration,
    mut abort: impl FnMut() -> Option<String>,
) -> Result<(), String> {
    let url = format!("{base_url}/api/agent");
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(reason) = abort() {
            return Err(reason);
        }
        // Errors (connection refused while it boots) are just "not yet".
        if let Ok(resp) = client
            .get(&url)
            .basic_auth(USER, Some(password))
            .send()
            .await
            && resp.status().is_success()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "opencode server not ready after {}s",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

/// The `POST /api/session` body. `agent` is sent only when non-blank.
pub(crate) fn session_create_body(cwd: &str, agent: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({ "location": { "directory": cwd } });
    if let Some(agent) = agent.map(str::trim).filter(|a| !a.is_empty()) {
        body["agent"] = serde_json::Value::String(agent.to_owned());
    }
    body
}

/// The session id at `data.id` of a create response.
pub(crate) fn parse_session_id(body: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("session create: not JSON: {e}"))?;
    v.pointer("/data/id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "session create: no data.id in the response".to_owned())
}

/// Create a session in `cwd` and return its id.
pub(crate) async fn create_session(
    client: &reqwest::Client,
    base_url: &str,
    password: &str,
    cwd: &str,
    agent: Option<&str>,
) -> Result<String, String> {
    let resp = client
        .post(format!("{base_url}/api/session"))
        .basic_auth(USER, Some(password))
        .header("content-type", "application/json")
        .body(session_create_body(cwd, agent).to_string())
        .send()
        .await
        .map_err(|e| format!("session create: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("session create: reading the response: {e}"))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(200).collect();
        return Err(format!("session create: HTTP {status}: {snippet}"));
    }
    parse_session_id(&text)
}

#[cfg(test)]
mod tests;
