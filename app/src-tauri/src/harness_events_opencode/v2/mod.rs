//! opencode v2 (2.x) telemetry adapter (#677): reads the per-harness
//! sidecar's `GET /api/event` and emits the same `OpencodeEvent`s v1 does, so
//! the frontend translator is unchanged.

mod connection;
mod translate;

pub(super) use connection::run_adapter_v2;

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod loopback_tests;
#[cfg(test)]
mod tests;
