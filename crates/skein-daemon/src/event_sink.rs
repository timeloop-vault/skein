//! Where harness events and live action rows are delivered. (#577, #575)
//!
//! A daemon has no `AppHandle`, so the code that produces events must not
//! name one. It talks to an [`EventSink`] instead: the Tauri implementation
//! lives in `tauri_event_sink.rs` and a network transport implements the
//! same trait later. This module deliberately imports nothing from `tauri`.

use std::sync::Arc;

use serde::Serialize;

/// Delivers one named event with an already-serialized JSON payload.
pub trait EventSink: Send + Sync {
    /// # Errors
    /// The transport's failure, flattened to a string.
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String>;

    /// The last seq this sink stamped; 0 for a sink that stamps none.
    fn current_seq(&self) -> u64 {
        0
    }

    /// Step one of a two-step emit: take the next seq and write it into the
    /// payload, so a producer can do it under its own lock and deliver later.
    /// The default stamps nothing (seq 0, payload unchanged).
    fn stamp_json(&self, _event: &str, payload: String) -> Stamped {
        Stamped { seq: 0, payload }
    }

    /// Step two: hand a stamped event on.
    ///
    /// # Errors
    /// The transport's failure, flattened to a string.
    fn deliver_json(&self, event: &str, stamped: Stamped) -> Result<(), String> {
        self.emit_json(event, stamped.payload)
    }
}

/// An event after stamping, ready for [`EventSink::deliver_json`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped {
    pub seq: u64,
    /// The payload JSON with `seq` written in.
    pub payload: String,
}

/// A sink shared between managers.
pub type SharedEventSink = Arc<dyn EventSink>;

/// Serialize `payload` with `serde_json` and deliver it through `sink`.
///
/// # Errors
/// A serialization failure, or whatever the sink reports.
pub fn emit<S: Serialize + ?Sized>(
    sink: &dyn EventSink,
    event: &str,
    payload: &S,
) -> Result<(), String> {
    let json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    sink.emit_json(event, json)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<(String, String)>>);

    impl EventSink for RecordingSink {
        fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
            self.0.lock().unwrap().push((event.to_string(), payload));
            Ok(())
        }
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Sample {
        room_id: &'static str,
        timestamp_ms: i64,
    }

    #[test]
    fn emit_serializes_like_serde_json() {
        let sink = RecordingSink::default();
        let s = Sample {
            room_id: "r",
            timestamp_ms: 7,
        };
        emit(&sink, "evt", &s).unwrap();
        let got = sink.0.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "evt");
        assert_eq!(got[0].1, serde_json::to_string(&s).unwrap());
        assert_eq!(got[0].1, r#"{"roomId":"r","timestampMs":7}"#);
    }
}
