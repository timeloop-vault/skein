//! Recording Skein's own hook approvals in a profile's Hermes allowlist
//! (#625), after the user consented. Hermes keys an approval on the exact
//! `(event, command)` pair; we add only our pairs, as the entry Hermes
//! itself writes (`agent/shell_hooks.py` `_record_approval`), and leave
//! every existing entry and unknown key as the JSON value it was.

use std::path::Path;
use std::time::SystemTime;

use serde_json::{Map, Value, json};

use super::yaml_hooks::{HOOK_EVENTS, HookSpec};
use super::{read_existing, write_config_atomic};

/// Hermes' per-profile allowlist of approved `(event, command)` pairs
/// (`agent/shell_hooks.py`: `ALLOWLIST_FILENAME`, under `get_hermes_home()`,
/// which `-p <profile>` points at the profile directory).
pub(crate) const ALLOWLIST_FILE: &str = "shell-hooks-allowlist.json";

/// `YYYY-MM-DDTHH:MM:SS[.ffffff]Z`, the shape of Python's
/// `datetime.isoformat()` with `+00:00` replaced by `Z`.
fn iso_utc(t: SystemTime) -> String {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = i64::try_from(d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let frac = match d.subsec_micros() {
        0 => String::new(),
        us => format!(".{us:06}"),
    };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{frac}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Add Skein's `(event, command)` pairs from `spec` to
/// `<profile_home>/shell-hooks-allowlist.json`; pairs already there are
/// left alone. A missing file is created. A file that is not a JSON object
/// with an array `approvals` is refused untouched. `script` supplies
/// `script_mtime_at_approval`. Returns whether anything was written.
pub(crate) fn record_approvals(
    profile_home: &Path,
    spec: &HookSpec,
    script: &Path,
) -> Result<bool, String> {
    let path = profile_home.join(ALLOWLIST_FILE);
    let old = read_existing(&path)?;
    let mut root = match &old {
        None => Map::new(),
        Some(bytes) => {
            let body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
            match serde_json::from_slice::<Value>(body) {
                Ok(Value::Object(m)) => m,
                _ => {
                    return Err(format!(
                        "{} is not a JSON object; not editing it",
                        path.display()
                    ));
                }
            }
        }
    };
    let approvals = root
        .entry("approvals")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(approvals) = approvals else {
        return Err(format!(
            "{} has an \"approvals\" that is not a list; not editing it",
            path.display()
        ));
    };
    let now = iso_utc(SystemTime::now());
    let mtime = std::fs::metadata(script)
        .and_then(|m| m.modified())
        .ok()
        .map_or(Value::Null, |t| Value::String(iso_utc(t)));
    let mut added = false;
    for ev in HOOK_EVENTS {
        let command = spec.command_for(ev);
        let known = approvals.iter().any(|a| {
            a.get("event").and_then(Value::as_str) == Some(ev)
                && a.get("command").and_then(Value::as_str) == Some(command.as_str())
        });
        if !known {
            approvals.push(json!({
                "event": ev,
                "command": command,
                "approved_at": now,
                "script_mtime_at_approval": mtime,
            }));
            added = true;
        }
    }
    if !added && old.is_some() {
        return Ok(false);
    }
    let mut out = serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|e| format!("cannot encode the allowlist: {e}"))?;
    out.push(b'\n');
    write_config_atomic(&path, old.as_deref(), &out)?;
    // Hermes creates its allowlist 0600; a new one of ours matches. An
    // existing file keeps its mode (the atomic write copies it).
    #[cfg(unix)]
    if old.is_none() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn iso_matches_pythons_isoformat() {
        let t = |s: u64, us: u32| SystemTime::UNIX_EPOCH + Duration::new(s, us * 1000);
        assert_eq!(iso_utc(t(0, 0)), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(t(1_709_208_000, 5)), "2024-02-29T12:00:00.000005Z");
        assert_eq!(
            iso_utc(t(1_792_584_123, 250_000)),
            "2026-10-21T12:02:03.250000Z"
        );
    }
}
