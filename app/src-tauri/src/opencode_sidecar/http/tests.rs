use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};

use super::*;

#[test]
fn parse_session_id_reads_data_id() {
    assert_eq!(
        parse_session_id(r#"{"data":{"id":"ses_abc","agent":"build"}}"#).unwrap(),
        "ses_abc"
    );
}

#[test]
fn parse_session_id_rejects_missing_empty_and_non_json() {
    assert!(parse_session_id(r#"{"data":{}}"#).is_err());
    assert!(parse_session_id(r#"{"data":{"id":""}}"#).is_err());
    assert!(parse_session_id(r#"{"id":"ses_x"}"#).is_err());
    assert!(
        parse_session_id("not json")
            .unwrap_err()
            .contains("not JSON")
    );
}

#[test]
fn session_body_includes_agent_only_when_non_blank() {
    let with = session_create_body("/w", Some("plan"));
    assert_eq!(
        with,
        serde_json::json!({"location": {"directory": "/w"}, "agent": "plan"})
    );
    let expected = serde_json::json!({"location": {"directory": "/w"}});
    assert_eq!(session_create_body("/w", Some("  ")), expected);
    assert_eq!(session_create_body("/w", None), expected);
}

const PW: &str = "s3cret";

fn authed(h: &HeaderMap) -> bool {
    use base64::Engine;
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("opencode:{PW}"))
    );
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(want.as_str())
}

/// `/api/agent` answers 503 for its first `warmup` calls, then follows the
/// auth rule; `/api/session` echoes an id built from the request body.
async fn mock(warmup: usize) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/api/agent",
            get(
                move |State(hits): State<Arc<AtomicUsize>>, h: HeaderMap| async move {
                    if hits.fetch_add(1, Ordering::SeqCst) < warmup {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else if authed(&h) {
                        StatusCode::OK
                    } else {
                        StatusCode::UNAUTHORIZED
                    }
                },
            ),
        )
        .route(
            "/api/session",
            post(|h: HeaderMap, body: String| async move {
                if !authed(&h) {
                    return (StatusCode::UNAUTHORIZED, String::new());
                }
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let agent = v["agent"].as_str().unwrap_or("none");
                let dir = v["location"]["directory"].as_str().unwrap_or("");
                (
                    StatusCode::OK,
                    serde_json::json!({"data": {"id": format!("ses_{agent}_{dir}")}}).to_string(),
                )
            }),
        )
        .with_state(Arc::clone(&hits));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{port}"), hits)
}

const FAST: Duration = Duration::from_millis(10);

#[tokio::test]
async fn ready_polls_through_warmup_and_needs_the_right_password() {
    let (url, hits) = mock(3).await;
    crate::install_rustls_provider();
    let c = client().unwrap();
    wait_ready(&c, &url, PW, FAST, Duration::from_secs(5), || None)
        .await
        .unwrap();
    assert!(hits.load(Ordering::SeqCst) >= 4);

    let err = wait_ready(&c, &url, "wrong", FAST, Duration::from_millis(150), || None)
        .await
        .unwrap_err();
    assert!(err.contains("not ready"), "{err}");
}

#[tokio::test]
async fn ready_stops_early_when_aborted_and_when_nothing_listens() {
    let (url, _) = mock(0).await;
    crate::install_rustls_provider();
    let c = client().unwrap();
    let err = wait_ready(&c, &url, PW, FAST, Duration::from_secs(5), || {
        Some("exited with code 1".to_owned())
    })
    .await
    .unwrap_err();
    assert_eq!(err, "exited with code 1");

    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://127.0.0.1:{}", l.local_addr().unwrap().port())
    };
    assert!(
        wait_ready(&c, &dead, PW, FAST, Duration::from_millis(200), || None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn create_session_sends_agent_and_directory_and_checks_auth() {
    let (url, _) = mock(0).await;
    crate::install_rustls_provider();
    let c = client().unwrap();
    assert_eq!(
        create_session(&c, &url, PW, "/w", Some("build"))
            .await
            .unwrap(),
        "ses_build_/w"
    );
    assert_eq!(
        create_session(&c, &url, PW, "/w", Some(" ")).await.unwrap(),
        "ses_none_/w"
    );
    let err = create_session(&c, &url, "wrong", "/w", None)
        .await
        .unwrap_err();
    assert!(err.contains("401"), "{err}");
}

struct StopOnDrop<'a>(&'a crate::opencode_sidecar::OpencodeSidecarManager);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.stop("live");
    }
}

/// Against a real opencode v2. `SKEIN_TEST_OPENCODE_V2_EXE` names the
/// binary; the run is isolated in a temp HOME/XDG tree.
#[tokio::test]
#[ignore = "needs SKEIN_TEST_OPENCODE_V2_EXE"]
async fn live_opencode_v2_serve_round_trip() {
    use crate::opencode_sidecar::{OpencodeSidecarManager, SidecarState, StartRequest};
    use std::ffi::OsString;

    let Some(exe) = std::env::var_os("SKEIN_TEST_OPENCODE_V2_EXE") else {
        eprintln!("SKEIN_TEST_OPENCODE_V2_EXE unset; skipping");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let h = home.path();
    let mut env: Vec<(OsString, OsString)> = vec![
        ("HOME".into(), h.into()),
        ("USERPROFILE".into(), h.into()),
        ("XDG_CONFIG_HOME".into(), h.join("config").into()),
        ("XDG_DATA_HOME".into(), h.join("data").into()),
        ("XDG_STATE_HOME".into(), h.join("state").into()),
        ("XDG_CACHE_HOME".into(), h.join("cache").into()),
    ];
    for key in ["PATH", "SystemRoot", "windir", "TEMP", "TMP"] {
        if let Some(v) = std::env::var_os(key) {
            env.push((key.into(), v));
        }
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let m = OpencodeSidecarManager::default();
    let info = m
        .start(StartRequest {
            harness_id: "live".into(),
            cwd: work.path().to_path_buf(),
            env,
            exe: exe.into(),
            port,
            lower_priority: false,
            on_exit: Arc::new(|_| {}),
            args_override: None,
        })
        .unwrap();
    // A failed assertion below must not leak the child.
    let _guard = StopOnDrop(&m);
    let pw = m.password_for("live", info.generation).unwrap();
    let url = format!("http://127.0.0.1:{port}");
    crate::install_rustls_provider();
    let c = client().unwrap();
    wait_ready(&c, &url, &pw, READY_POLL, READY_TIMEOUT, || {
        match m.exited("live", info.generation) {
            Some(SidecarState::Exited(code)) => {
                Some(format!("exited {code}: {:?}", m.stderr_tail("live")))
            }
            _ => None,
        }
    })
    .await
    .unwrap();
    let cwd = work.path().to_string_lossy().into_owned();
    let id = create_session(&c, &url, &pw, &cwd, Some("build"))
        .await
        .unwrap();
    let got: serde_json::Value = serde_json::from_str(
        &c.get(format!("{url}/api/session/{id}"))
            .basic_auth("opencode", Some(&pw))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(got["data"]["agent"], "build", "{got}");
    m.stop("live");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while crate::opencode_sidecar::record::observe(info.pid).is_some() {
        assert!(std::time::Instant::now() < deadline, "pid still alive");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
