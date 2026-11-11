//! What the agent API needs from the client it serves, without naming the
//! client's toolkit (#582): a screenshot of the main webview and a way to
//! deliver a request the client must answer. The Tauri implementations
//! live in `app/src-tauri/src/tauri_client.rs`.

use super::state::AgentRequest;

/// A rectangle of the client's main view, in CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A captured PNG and its pixel size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Native capture of a rectangle of the client's main view.
pub trait WebviewCapture: Send + Sync {
    /// Start capturing `rect` at `scale` device pixels per CSS pixel and
    /// call `done` with the result, possibly from another thread.
    ///
    /// # Errors
    /// The capture could not be started; `done` is then never called.
    fn capture(
        &self,
        rect: Rect,
        scale: f64,
        done: Box<dyn FnOnce(Result<Shot, String>) + Send>,
    ) -> Result<(), String>;
}

/// Delivery of a request only the client can answer.
pub trait ClientRequests: Send + Sync {
    /// Deliver one request to the client. Err = it could not be delivered.
    ///
    /// # Errors
    /// The request could not be delivered.
    fn send(&self, request: &AgentRequest) -> Result<(), String>;
}
