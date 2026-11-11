//! The daemon-to-client half of the in-process transport: events, requests
//! the client must answer, and the screenshot capture.
//!
//! The client-to-daemon half is direct calls on the [`Daemon`]'s state from
//! the app's command wrappers. There is deliberately no transport trait yet,
//! because in-process is the only transport (sockets are #585).
//!
//! [`Daemon`]: crate::daemon::Daemon

use std::sync::Arc;

use crate::agent_api::client::{ClientRequests, WebviewCapture};
use crate::event_sink::SharedEventSink;

/// What the daemon needs from its embedder to talk back to the client.
pub struct InProcessTransport {
    pub events: SharedEventSink,
    pub client: Arc<dyn ClientRequests>,
    pub capture: Arc<dyn WebviewCapture>,
}
