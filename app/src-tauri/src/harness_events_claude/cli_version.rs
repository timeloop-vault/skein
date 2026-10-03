//! Which Claude Code version wrote the main transcript's rows (#491): the
//! attach-time seed and the live change detector. Row parsing itself
//! lives in `skein_harness::claude::cli_version`.

use super::ClaudeEvent;
use skein_harness::claude::cli_version::row_cli_version;

/// The `CliVersion` event for the newest row of `content` that carries a
/// version, for the attach seed. Walks from the end and stops at the
/// first hit, so a normal transcript parses one or two lines rather than
/// the whole file a second time.
pub(super) fn latest_cli_version_event(content: &str) -> Option<ClaudeEvent> {
    content.lines().rev().find_map(|line| {
        let line = line.trim();
        if !line.contains("\"version\"") {
            return None;
        }
        let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
        main_row_event(&value)
    })
}

/// Live detector: an event when `value` (a main-transcript row) names a
/// version other than `last`, which it then updates. The attach seed never
/// touches `last`, so the first live row with a version always emits.
pub(super) fn live_cli_version_event(
    value: &serde_json::Value,
    last: &mut Option<String>,
) -> Option<ClaudeEvent> {
    let event = main_row_event(value)?;
    let ClaudeEvent::CliVersion { version, .. } = &event else {
        return None;
    };
    if last.as_deref() == Some(version.as_str()) {
        return None;
    }
    *last = Some(version.clone());
    Some(event)
}

fn main_row_event(value: &serde_json::Value) -> Option<ClaudeEvent> {
    if skein_harness::claude::is_sidechain(value) {
        return None;
    }
    let row = row_cli_version(value)?;
    Some(ClaudeEvent::CliVersion {
        version: row.version,
        timestamp_ms: row.timestamp_ms,
    })
}
