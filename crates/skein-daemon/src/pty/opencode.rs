//! The Tauri-free parts of the opencode PTY commands: the process-scan
//! wire shape and the free-port picker.

/// Wire shape for `pty_scan_opencode` (#517).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeScanDto {
    pid: u32,
    session_id: Option<String>,
    port: Option<u16>,
    port_confirmed: bool,
    continue_last: bool,
}

/// Scan the descendants of `pid` for an opencode TUI and report it as the
/// wire shape.
#[must_use]
pub fn scan_opencode_dto(pid: u32) -> Option<OpencodeScanDto> {
    super::procscan::scan_opencode(pid).map(|f| OpencodeScanDto {
        pid: f.pid,
        session_id: f.session_id,
        port: f.port,
        port_confirmed: f.port_confirmed,
        continue_last: f.continue_last,
    })
}

/// Allocate a free TCP port on `127.0.0.1` for opencode's embedded
/// HTTP server. Epic #50 L2c-2.
///
/// Implementation: bind a `TcpListener` to port 0, read the OS-assigned
/// port, drop the listener. There's a small race window between drop
/// and the next process binding it (~microseconds typically), but it
/// only matters if another process simultaneously asks for a free
/// port and beats opencode to bind. In practice we've never observed
/// it; if it ever happens opencode reports the bind error in the PTY
/// and the user can restart the harness.
///
/// # Errors
/// The bind or address lookup failure, as a string.
pub fn pick_free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("pick_free_port: {e}"))
}
