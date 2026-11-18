//! The per-daemon event sequence (#757, P2b "The event stream"). The
//! daemon's sink stamps every broadcast event with one monotonic `seq`
//! as a top-level key of its JSON object, and keeps a bounded replay log.
//! Pure data; the stamping and the log live in `skein-daemon`.

use serde::{Deserialize, Serialize};

/// A stamped event payload decoded together with its seq: the payload's own
/// fields sit beside `seq` in one JSON object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sequenced<T> {
    pub seq: u64,
    #[serde(flatten)]
    pub payload: T,
}

/// One event from the replay log. `payload` is the stamped JSON text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggedEvent {
    pub seq: u64,
    pub event: String,
    pub payload: String,
}

/// The answer to `subscribe(since)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Replay {
    /// Every logged event after `since`, in seq order; `seq` is the current
    /// seq when the answer was made.
    Events { events: Vec<LoggedEvent>, seq: u64 },
    /// `since` is older than the log, from another daemon instance, or
    /// ahead of the daemon: re-read the snapshots and continue from `seq`
    /// (the current seq).
    Gap { seq: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sequenced_flattens_payload() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct P {
            origin: String,
        }
        let v = json!({"seq": 7, "origin": "c1"});
        let s: Sequenced<P> = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(s.seq, 7);
        assert_eq!(
            s.payload,
            P {
                origin: "c1".into()
            }
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), v);
    }

    #[test]
    fn replay_round_trips_camel_case() {
        let e = Replay::Events {
            events: vec![LoggedEvent {
                seq: 3,
                event: "a".into(),
                payload: "{\"seq\":3}".into(),
            }],
            seq: 3,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            json!({"type":"events","events":[{"seq":3,"event":"a","payload":"{\"seq\":3}"}],"seq":3})
        );
        assert_eq!(serde_json::from_value::<Replay>(v).unwrap(), e);
        let g = Replay::Gap { seq: 9 };
        let v = serde_json::to_value(&g).unwrap();
        assert_eq!(v, json!({"type":"gap","seq":9}));
        assert_eq!(serde_json::from_value::<Replay>(v).unwrap(), g);
    }
}
