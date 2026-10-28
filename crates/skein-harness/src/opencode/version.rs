//! Which opencode major version a spawn will run (#675).
//!
//! opencode v1 prints a bare `1.18.35` for `--version`; v2 prints
//! `opencode v2.2.0`. Both are undocumented, so parsing is lenient: the
//! first `N.N.N` anywhere in the output. The v2 binary is a ~250 MB native
//! executable, so a probe is cached per (path, size, mtime).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::agents::clean_command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpencodeVersion {
    pub major: u32,
    /// `major.minor.patch` as matched; any prerelease suffix is dropped.
    pub version: String,
}

/// The first `digits.digits.digits` in `stdout`. `None` when there is none
/// or a component does not fit a `u32`.
pub fn parse_version_output(stdout: &str) -> Option<OpencodeVersion> {
    let bytes = stdout.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // A run of digits starts here; try it as `N.N.N`.
        let mut end = i;
        let mut parts: Vec<&str> = Vec::new();
        loop {
            let start = end;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if start == end {
                break;
            }
            parts.push(&stdout[start..end]);
            if parts.len() == 3 {
                break;
            }
            if end + 1 < bytes.len() && bytes[end] == b'.' && bytes[end + 1].is_ascii_digit() {
                end += 1;
            } else {
                break;
            }
        }
        if parts.len() == 3 {
            let nums: Option<Vec<u32>> = parts.iter().map(|p| p.parse().ok()).collect();
            return nums.map(|n| OpencodeVersion {
                major: n[0],
                version: format!("{}.{}.{}", n[0], n[1], n[2]),
            });
        }
        // Skip the whole digit run we just consumed.
        i = end.max(i + 1);
    }
    None
}

/// Cache key: the resolved program and what identifies its file contents.
pub type Key = (PathBuf, Option<u64>, Option<SystemTime>);

/// A failed probe is not retried for this long: a broken or slow binary
/// would otherwise cost the full timeout on every spawn.
pub const FAILURE_TTL: Duration = Duration::from_secs(60);

/// A success is also re-probed after this long. On Windows the resolved
/// path is usually npm's tiny `opencode.cmd` shim, which keeps its size and
/// mtime when the package behind it is upgraded (v1 to v2), so the key alone
/// would serve a stale answer until Skein restarts.
pub const SUCCESS_TTL: Duration = Duration::from_mins(5);

type Cache = HashMap<Key, (OpencodeVersion, Instant)>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
static FAILED: Mutex<Option<HashMap<Key, Instant>>> = Mutex::new(None);

/// The cache key for `program`: its path plus file size and mtime, so an
/// upgrade in place re-probes.
pub fn key_for(program: &OsStr) -> Key {
    let meta = std::fs::metadata(program).ok();
    (
        PathBuf::from(program),
        meta.as_ref().map(std::fs::Metadata::len),
        meta.and_then(|m| m.modified().ok()),
    )
}

/// Cached lookup: a success is kept until the key changes or
/// [`SUCCESS_TTL`] passes, a failure for [`FAILURE_TTL`]; expired entries
/// are evicted on lookup. A key with no file metadata (nothing resolved) is
/// never cached either way. `run` only executes on a miss.
pub fn probe_with(
    key: &Key,
    now: Instant,
    run: impl FnOnce() -> Option<OpencodeVersion>,
) -> Option<OpencodeVersion> {
    if key.1.is_none() && key.2.is_none() {
        return run();
    }
    {
        let mut guard = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cache) = guard.as_mut() {
            match cache.get(key) {
                Some((v, at)) if now.saturating_duration_since(*at) < SUCCESS_TTL => {
                    return Some(v.clone());
                }
                Some(_) => {
                    cache.remove(key);
                }
                None => {}
            }
        }
    }
    {
        let mut guard = FAILED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(failed) = guard.as_mut() {
            match failed.get(key) {
                Some(at) if now.saturating_duration_since(*at) < FAILURE_TTL => return None,
                Some(_) => {
                    failed.remove(key);
                }
                None => {}
            }
        }
    }
    let got = run();
    if let Some(v) = &got {
        CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(key.clone(), (v.clone(), now));
    } else {
        FAILED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(key.clone(), now);
    }
    got
}

/// Run `<program> --version` and parse it, uncached. `None` for a program
/// that will not start, exits non-zero, prints nothing recognisable, or is
/// still running after `timeout` (killed then). `path_env`, when given,
/// becomes `PATH` (npm's `opencode.cmd` shim needs `node` on it).
pub fn run_version(
    program: &OsStr,
    timeout: Duration,
    path_env: Option<&OsStr>,
) -> Option<OpencodeVersion> {
    let mut cmd = clean_command(program);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Some(path) = path_env {
        cmd.env("PATH", path);
    }
    let mut child = cmd.spawn().ok()?;
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

/// [`run_version`] behind the (path, size, mtime) cache.
pub fn probe_version(
    program: &OsStr,
    timeout: Duration,
    path_env: Option<&OsStr>,
) -> Option<OpencodeVersion> {
    let key = key_for(program);
    probe_with(&key, Instant::now(), || {
        run_version(program, timeout, path_env)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unnecessary_wraps)]
    fn v(major: u32, version: &str) -> Option<OpencodeVersion> {
        Some(OpencodeVersion {
            major,
            version: version.into(),
        })
    }

    #[test]
    fn version_output_table() {
        let cases: &[(&str, &str, Option<OpencodeVersion>)] = &[
            ("v1 bare", "1.18.35", v(1, "1.18.35")),
            ("v1 newline", "1.18.35\n", v(1, "1.18.35")),
            ("v2", "opencode v2.2.0", v(2, "2.2.0")),
            ("v2 crlf", "opencode v2.2.0\r\n", v(2, "2.2.0")),
            ("v1 other", "1.19.5", v(1, "1.19.5")),
            ("prerelease", "2.3.0-beta.1", v(2, "2.3.0")),
            (
                "leading noise",
                "warming up\nloading 12\nopencode v2.2.0\n",
                v(2, "2.2.0"),
            ),
            ("garbage", "command not found", None),
            ("empty", "", None),
            ("two parts", "1.2", None),
            ("two parts then three", "1.2 then 3.4.5", v(3, "3.4.5")),
            ("overflow", "99999999999.1.1", None),
            ("trailing dot", "1.2.", None),
        ];
        for (name, out, want) in cases {
            assert_eq!(&parse_version_output(out), want, "{name}: {out:?}");
        }
    }

    fn key(name: &str, size: u64, secs: u64) -> Key {
        (
            PathBuf::from(name),
            Some(size),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        )
    }

    #[test]
    fn a_cached_success_is_not_rerun_until_the_key_changes() {
        let k = key("opencode-cache-test", 100, 1);
        let t0 = Instant::now();
        let runs = std::cell::Cell::new(0);
        let go = |k: &Key| {
            probe_with(k, t0, || {
                runs.set(runs.get() + 1);
                v(2, "2.2.0")
            })
        };
        assert_eq!(go(&k), v(2, "2.2.0"));
        assert_eq!(go(&k), v(2, "2.2.0"));
        assert_eq!(runs.get(), 1, "hit must not re-run");
        go(&key("opencode-cache-test", 101, 1));
        assert_eq!(runs.get(), 2, "size change re-runs");
        go(&key("opencode-cache-test", 100, 2));
        assert_eq!(runs.get(), 3, "mtime change re-runs");
    }

    #[test]
    fn a_failed_probe_is_not_rerun_until_the_ttl_passes() {
        let k = key("opencode-fail-test", 1, 1);
        let t0 = Instant::now();
        let mut runs = 0;
        for at in [
            t0,
            t0 + Duration::from_secs(30),
            t0 + FAILURE_TTL + Duration::from_secs(1),
        ] {
            assert!(
                probe_with(&k, at, || {
                    runs += 1;
                    None
                })
                .is_none()
            );
        }
        assert_eq!(runs, 2, "within the TTL no re-run, after it a re-run");
    }

    #[test]
    fn a_success_is_rerun_after_the_success_ttl() {
        let k = key("opencode-success-ttl-test", 7, 7);
        let t0 = Instant::now();
        let runs = std::cell::Cell::new(0);
        let go = |at: Instant| {
            probe_with(&k, at, || {
                runs.set(runs.get() + 1);
                v(1, "1.18.35")
            })
        };
        go(t0);
        go(t0 + SUCCESS_TTL / 2);
        assert_eq!(runs.get(), 1);
        go(t0 + SUCCESS_TTL + Duration::from_secs(1));
        assert_eq!(runs.get(), 2, "expired success re-probes");
    }

    #[test]
    fn a_key_without_metadata_is_never_cached() {
        let k: Key = (PathBuf::from("opencode-no-meta-test"), None, None);
        let t0 = Instant::now();
        let runs = std::cell::Cell::new(0);
        for ok in [true, true, false, false] {
            probe_with(&k, t0, || {
                runs.set(runs.get() + 1);
                if ok { v(2, "2.2.0") } else { None }
            });
        }
        assert_eq!(runs.get(), 4);
    }

    #[test]
    fn probe_of_a_missing_program_is_none() {
        let got = probe_version(
            OsStr::new("definitely-not-a-real-program-675"),
            Duration::from_secs(2),
            None,
        );
        assert_eq!(got, None);
    }
}
