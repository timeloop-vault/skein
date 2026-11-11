//! The Tauri implementations of the agent API's client seams
//! (`agent_api/client.rs`, #582): the main webview's screenshot and the
//! agent-request event.

use tauri::{AppHandle, Manager as _};

use crate::agent_api::client::{ClientRequests, Rect, Shot, WebviewCapture};
use crate::agent_api::state::{AGENT_REQUEST_EVENT, AgentRequest};

/// Emits [`AGENT_REQUEST_EVENT`] to the webview.
pub struct TauriClientRequests(crate::tauri_event_sink::TauriEventSink);

impl TauriClientRequests {
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self(crate::tauri_event_sink::TauriEventSink::new(app))
    }
}

impl ClientRequests for TauriClientRequests {
    fn send(&self, request: &AgentRequest) -> Result<(), String> {
        crate::event_sink::emit(&self.0, AGENT_REQUEST_EVENT, request)
    }
}

/// Captures the main window's webview with `skein-webshot`.
pub struct TauriWebviewCapture(AppHandle);

impl TauriWebviewCapture {
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self(app)
    }
}

fn to_webshot(r: Rect) -> skein_webshot::Rect {
    skein_webshot::Rect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

fn from_webshot(s: skein_webshot::Shot) -> Shot {
    Shot {
        png: s.png,
        width: s.width,
        height: s.height,
    }
}

/// Start the platform capture. Must run on the main thread (inside
/// `with_webview`).
#[allow(clippy::needless_pass_by_value)] // consumed on macOS and Windows only
fn start_capture(
    pw: tauri::webview::PlatformWebview,
    rect: Rect,
    scale: f64,
    done: impl FnOnce(Result<Shot, String>) + Send + 'static,
) {
    let done = move |r: Result<skein_webshot::Shot, String>| done(r.map(from_webshot));
    let rect = to_webshot(rect);
    #[cfg(windows)]
    skein_webshot::capture(&pw.controller(), rect, scale, done);
    #[cfg(target_os = "macos")]
    skein_webshot::capture(pw.inner(), rect, scale, done);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = &pw;
        skein_webshot::capture(std::ptr::null_mut(), rect, scale, done);
    }
}

impl WebviewCapture for TauriWebviewCapture {
    fn capture(
        &self,
        rect: Rect,
        scale: f64,
        done: Box<dyn FnOnce(Result<Shot, String>) + Send>,
    ) -> Result<(), String> {
        let window = self
            .0
            .get_webview_window(crate::setup::MAIN_LABEL)
            .ok_or_else(|| "the main window is gone".to_owned())?;
        window
            .with_webview(move |pw| start_capture(pw, rect, scale, done))
            .map_err(|e| format!("could not reach the webview: {e}"))
    }
}
