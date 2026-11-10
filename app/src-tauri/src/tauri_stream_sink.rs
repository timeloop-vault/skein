//! The Tauri [`StreamSink`]: sends over the per-call `tauri::ipc::Channel`
//! the frontend passed in. The only place that holds a `Channel` for
//! stream delivery. (#578)

use tauri::ipc::{Channel, IpcResponse};

use crate::stream_sink::StreamSink;

pub struct TauriChannelSink<T>(Channel<T>);

impl<T> TauriChannelSink<T> {
    #[must_use]
    pub fn new(channel: Channel<T>) -> Self {
        Self(channel)
    }
}

impl<T: IpcResponse + Send + Sync + 'static> StreamSink<T> for TauriChannelSink<T> {
    fn send(&self, item: T) -> Result<(), String> {
        self.0.send(item).map_err(|e| e.to_string())
    }
}
