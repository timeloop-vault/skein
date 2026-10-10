//! The full login-shell environment, as captured by the spawn probe.
//!
//! The probe (`PROBE_SCRIPT`) prints `PATH` and then the whole
//! environment between the `ENV_PROBE_*` sentinels. Everything here is
//! pure: parse the bytes, drop shell-session noise, merge `NO_PROXY`.
//! Windows never runs the probe, hence the `dead_code` allowances on
//! the parse side.

/// Sentinels wrapping the NUL-separated environment payload.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) const ENV_PROBE_START: &str = "___SKEIN_ENV_BEGIN___";
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) const ENV_PROBE_END: &str = "___SKEIN_ENV_END___";

/// Loopback hosts that must never go through a proxy.
pub(crate) const LOOPBACK_NO_PROXY: &[&str] = &["127.0.0.1", "localhost", "::1"];

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// Pull the environment out of the probe's raw stdout.
///
/// Takes the first `ENV_PROBE_START` and the LAST `ENV_PROBE_END` after
/// it (an rc-file trap may print garbage after the payload). `None` when
/// either sentinel is missing (a truncated, killed-on-deadline payload),
/// the payload is empty, or no entry parses. Entries are NUL-separated
/// `KEY=VALUE`, split at the first `=` (values may hold `=` and
/// newlines; an empty value is valid). Entries that are empty, lack
/// `=`, have an empty key or are not UTF-8 are skipped individually.
/// Duplicate keys: last wins. Output is sorted by key.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) fn extract_probe_env(stdout: &[u8]) -> Option<Vec<(String, String)>> {
    let start = find_bytes(stdout, ENV_PROBE_START.as_bytes())? + ENV_PROBE_START.len();
    let rest = &stdout[start..];
    let end = rfind_bytes(rest, ENV_PROBE_END.as_bytes())?;
    let payload = &rest[..end];

    let mut map = std::collections::BTreeMap::new();
    for seg in payload.split(|&b| b == 0) {
        if seg.is_empty() {
            continue;
        }
        let Ok(seg) = std::str::from_utf8(seg) else {
            continue;
        };
        let Some((key, value)) = seg.split_once('=') else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        map.insert(key.to_owned(), value.to_owned());
    }
    if map.is_empty() {
        None
    } else {
        Some(map.into_iter().collect())
    }
}

/// Variables of the probe's own shell session that must not leak into a
/// harness. Case-sensitive, exact.
pub(crate) fn is_login_env_noise(key: &str) -> bool {
    matches!(
        key,
        // The probe shell incremented it; the harness shell starts fresh.
        "SHLVL"
        // The probe's cwd, not the harness's; a stale PWD misleads shells.
        | "PWD"
        | "OLDPWD"
        // The last command the probe ran, not anything of the user's.
        | "_"
        // Markers run_probe sets on the probe itself (pty/probe.rs).
        | "SKEIN_PROBE"
        | "DISABLE_AUTO_UPDATE"
        // Handled by the PATH merge; never overlaid.
        | "PATH"
    )
}

/// The captured environment minus noise keys (and nothing else), in order.
pub(crate) fn login_env_overlay(captured: &[(String, String)]) -> Vec<(String, String)> {
    captured
        .iter()
        .filter(|(k, _)| !is_login_env_noise(k))
        .cloned()
        .collect()
}

/// Comma-separated union of `upper`, `lower` and the loopback hosts, for
/// `NO_PROXY` / `no_proxy`.
///
/// A union rather than "just loopback": curl reads `no_proxy` before
/// `NO_PROXY`, so setting only the lowercase one to loopback would hide
/// a user's existing uppercase corporate bypass list. The caller sets
/// BOTH names to this value (on Windows env keys are case-insensitive,
/// so there it sets one). Entries are trimmed, empties dropped, and
/// duplicates removed case-insensitively, keeping the first spelling.
pub(crate) fn merge_no_proxy(upper: Option<&str>, lower: Option<&str>) -> String {
    let mut out: Vec<&str> = Vec::new();
    let sources = upper
        .into_iter()
        .chain(lower)
        .flat_map(|s| s.split(','))
        .chain(LOOPBACK_NO_PROXY.iter().copied());
    for entry in sources {
        let entry = entry.trim();
        if entry.is_empty() || out.iter().any(|e| e.eq_ignore_ascii_case(entry)) {
            continue;
        }
        out.push(entry);
    }
    out.join(",")
}

#[cfg(test)]
#[path = "login_env_tests.rs"]
mod tests;
