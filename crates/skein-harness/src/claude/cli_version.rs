//! Which Claude Code version wrote a transcript row, and which one is
//! installed (#491).
//!
//! Every transcript row carries a top-level `"version": "2.1.288"` and an
//! ISO `timestamp`. A running `claude` keeps the version it started with,
//! so a row's version older than the installed one means the process
//! predates an upgrade. Both the row field and `claude --version`
//! (`2.1.288 (Claude Code)`) are undocumented upstream, so everything here
//! is lenient: anything unexpected is `None`, never an error.

use std::ffi::OsStr;
use std::io::Read;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::agents::clean_command;
use crate::time::parse_iso8601_ms;

/// The version a row names, with when the row was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowVersion {
    pub version: String,
    /// `None` when the row has no parseable `timestamp`.
    pub timestamp_ms: Option<i64>,
}

/// The `version` a transcript row carries. A missing, non-string or empty
/// field is `None`.
pub fn row_cli_version(row: &Value) -> Option<RowVersion> {
    let version = row.get("version")?.as_str()?.trim();
    if version.is_empty() {
        return None;
    }
    let timestamp_ms = row
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_iso8601_ms);
    Some(RowVersion {
        version: version.to_owned(),
        timestamp_ms,
    })
}

/// The version out of `claude --version` output: the first whitespace
/// token of the first non-empty line, accepted only as
/// `digits.digits.digits` with an optional `-prerelease` / `+build`.
pub fn parse_version_output(stdout: &str) -> Option<String> {
    let token = stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .split_whitespace()
        .next()?;
    looks_like_version(token).then(|| token.to_owned())
}

fn looks_like_version(token: &str) -> bool {
    let core_end = token.find(['-', '+']).unwrap_or(token.len());
    let (core, suffix) = token.split_at(core_end);
    let mut parts = core.split('.');
    let numeric =
        |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if !(numeric(parts.next()) && numeric(parts.next()) && numeric(parts.next())) {
        return false;
    }
    if parts.next().is_some() {
        return false;
    }
    // A suffix, when present, must say something after its marker.
    suffix.is_empty()
        || (suffix.len() > 1
            && suffix[1..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+')))
}

/// Run `<program> --version` and parse it. `None` for a program that will
/// not start, exits non-zero, prints something unrecognisable, or is still
/// running after `timeout` (it is killed then, so a wedged CLI cannot
/// hold the caller's thread).
pub fn probe_version(program: &OsStr, timeout: Duration) -> Option<String> {
    let mut child = clean_command(program)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse_version_output(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn row_version_table() {
        let ts = "2026-05-15T21:16:22.572Z";
        let ts_ms = parse_iso8601_ms(ts);
        let cases: Vec<(&str, Value, Option<RowVersion>)> = vec![
            (
                "full row",
                json!({"type": "user", "version": "2.1.288", "timestamp": ts}),
                Some(RowVersion {
                    version: "2.1.288".into(),
                    timestamp_ms: ts_ms,
                }),
            ),
            (
                "no timestamp",
                json!({"version": "2.1.288"}),
                Some(RowVersion {
                    version: "2.1.288".into(),
                    timestamp_ms: None,
                }),
            ),
            (
                "bad timestamp",
                json!({"version": "2.1.288", "timestamp": "yesterday"}),
                Some(RowVersion {
                    version: "2.1.288".into(),
                    timestamp_ms: None,
                }),
            ),
            ("missing version", json!({"type": "user"}), None),
            ("number version", json!({"version": 2.1}), None),
            ("null version", json!({"version": null}), None),
            ("empty version", json!({"version": ""}), None),
            ("blank version", json!({"version": "  "}), None),
            ("not an object", json!("2.1.288"), None),
        ];
        for (name, row, want) in cases {
            assert_eq!(row_cli_version(&row), want, "{name}");
        }
    }

    #[test]
    fn version_output_table() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("real", "2.1.288 (Claude Code)\n", Some("2.1.288")),
            ("bare", "2.1.288", Some("2.1.288")),
            ("crlf", "2.1.288 (Claude Code)\r\n", Some("2.1.288")),
            (
                "leading whitespace",
                "  \n  2.1.288 (Claude Code)\n",
                Some("2.1.288"),
            ),
            (
                "prerelease",
                "2.2.0-beta.1 (Claude Code)",
                Some("2.2.0-beta.1"),
            ),
            ("build", "2.2.0+abc123 (Claude Code)", Some("2.2.0+abc123")),
            ("both", "2.2.0-rc.1+abc", Some("2.2.0-rc.1+abc")),
            ("garbage", "command not found: claude", None),
            ("empty", "", None),
            ("blank lines", "\n \n", None),
            ("two parts", "2.1 (Claude Code)", None),
            ("four parts", "2.1.2.3", None),
            ("v prefix", "v2.1.288", None),
            ("empty prerelease", "2.1.288-", None),
            ("non digits", "2.x.288", None),
            ("only first line counts", "hello\n2.1.288", None),
        ];
        for (name, out, want) in cases {
            assert_eq!(
                parse_version_output(out).as_deref(),
                *want,
                "{name}: {out:?}"
            );
        }
    }

    #[test]
    fn probe_of_a_missing_program_is_none() {
        let got = probe_version(
            OsStr::new("definitely-not-a-real-program-491"),
            Duration::from_secs(2),
        );
        assert_eq!(got, None);
    }
}
