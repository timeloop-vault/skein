//! Loopback test of the v2 connection loop against a hand-rolled std TCP
//! server (no framework: the test must control how the stream dies).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use super::super::OpencodeEvent;
use super::connection::{Target, run_with};

/// base64("opencode:secret")
const EXPECTED_AUTH: &str = "Basic b3BlbmNvZGU6c2VjcmV0";
const HEAD: &str =
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";

fn read_request(sock: &mut TcpStream) -> String {
    let mut req = Vec::new();
    let mut byte = [0u8; 1];
    while !req.ends_with(b"\r\n\r\n") {
        if sock.read(&mut byte).unwrap_or(0) == 0 {
            break;
        }
        req.push(byte[0]);
    }
    String::from_utf8_lossy(&req).into_owned()
}

fn write_connected_chunk(sock: &mut TcpStream) {
    let frame = "data: {\"type\":\"server.connected\",\"data\":{}}\n\n";
    write!(sock, "{HEAD}{:x}\r\n{frame}\r\n", frame.len()).unwrap();
    sock.flush().unwrap();
}

#[tokio::test]
async fn sends_basic_auth_announces_connects_and_reports_a_drop() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = Arc::clone(&requests);
    let server = std::thread::spawn(move || {
        // Connection 1: connect, then die mid-body (no terminating chunk).
        let (mut a, _) = listener.accept().unwrap();
        seen.lock().push(read_request(&mut a));
        write_connected_chunk(&mut a);
        drop(a);
        // Connection 2: connect and stay open until the test is done.
        let (mut b, _) = listener.accept().unwrap();
        seen.lock().push(read_request(&mut b));
        write_connected_chunk(&mut b);
        std::thread::sleep(Duration::from_secs(3));
    });

    crate::setup::install_rustls_provider();
    let events: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&events);
    let cancel = Arc::new(Notify::new());
    let cancel_task = Arc::clone(&cancel);
    let task = tokio::spawn(async move {
        let target = Target {
            port,
            password: "secret".to_owned(),
        };
        let on_event = move |e: OpencodeEvent| {
            sink.lock().push(match e {
                OpencodeEvent::Connected => "connected".to_owned(),
                OpencodeEvent::SessionBaselineIdle => "baseline".to_owned(),
                OpencodeEvent::SessionEnd => "end".to_owned(),
                other => format!("{other:?}"),
            });
        };
        // Short backoff so the reconnect does not cost a real second.
        run_with(&target, &cancel_task, &on_event, None, &[0]).await;
    });

    let want = ["connected", "baseline", "end", "connected", "baseline"];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while events.lock().len() < want.len() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cancel.notify_one();
    task.await.unwrap();
    assert_eq!(*events.lock(), want);

    let reqs = requests.lock().clone();
    assert_eq!(reqs.len(), 2);
    for r in &reqs {
        assert!(r.starts_with("GET /api/event "), "{r}");
        assert!(
            r.lines()
                .any(|l| l.eq_ignore_ascii_case(&format!("authorization: {EXPECTED_AUTH}"))),
            "missing basic auth in {r}"
        );
    }
    drop(server);
}
