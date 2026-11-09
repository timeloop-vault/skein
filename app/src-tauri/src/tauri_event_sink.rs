//! The Tauri [`EventSink`]: emits to the webview through an `AppHandle`.
//! The only place that holds an `AppHandle` for event delivery. (#577)

use tauri::{AppHandle, Emitter};

use crate::event_sink::EventSink;

pub struct TauriEventSink(AppHandle);

impl TauriEventSink {
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self(app)
    }
}

impl EventSink for TauriEventSink {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        self.0.emit_str(event, payload).map_err(|e| e.to_string())
    }
}
