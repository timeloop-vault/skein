//! `hermes --version` for the hook-telemetry gate (#625): run with the
//! spawn's own environment, bounded, and cached per (program, mtime) so a
//! boot that restores several Hermes harnesses probes once.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use skein_harness::hermes::version::{HermesVersion, parse_version, telemetry_supported};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

type Key = (PathBuf, Option<SystemTime>);
type Cache = HashMap<Key, HermesVersion>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
/// When a probe of this key last failed; see [`FAILURE_TTL`].
static FAILED: Mutex<Option<HashMap<Key, Instant>>> = Mutex::new(None);

/// A failed or unparsable probe is not retried for this long, so a broken or
/// slow `hermes --version` does not cost up to [`PROBE_TIMEOUT`] on every
/// spawn and every profile listing.
const FAILURE_TTL: Duration = Duration::from_secs(60);

/// Run `<program> --version` with exactly `env`. `None` for anything that
/// stops a parsed answer: not runnable, timeout, unrecognised output.
fn run_version<'a>(
    program: &str,
    env: impl Iterator<Item = (&'a str, &'a str)>,
) -> Option<HermesVersion> {
    let mut cmd = Command::new(program);
    cmd.arg("--version")
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    // Detached: a grandchild holding the pipe open must not hold us.
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let got = rx.recv_timeout(PROBE_TIMEOUT).ok();
    let _ = child.kill();
    let _ = child.wait();
    parse_version(&String::from_utf8_lossy(&got?))
}

/// The installed Hermes' version, cached per (program path, file mtime).
/// A failure is remembered for [`FAILURE_TTL`] under the same key (a bare
/// name nothing resolved has no mtime and keys on `None`); a success is
/// kept until the file changes.
pub(crate) fn probe<'a>(
    program: &str,
    env: impl Iterator<Item = (&'a str, &'a str)>,
) -> Option<HermesVersion> {
    let key: Key = (
        PathBuf::from(program),
        std::fs::metadata(program).and_then(|m| m.modified()).ok(),
    );
    probe_with(&key, Instant::now(), || run_version(program, env))
}

fn probe_with(
    key: &Key,
    now: Instant,
    run: impl FnOnce() -> Option<HermesVersion>,
) -> Option<HermesVersion> {
    if let Some(v) = CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|c| c.get(key))
    {
        return Some(*v);
    }
    if FAILED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|f| f.get(key))
        .is_some_and(|at| now.saturating_duration_since(*at) < FAILURE_TTL)
    {
        return None;
    }
    let got = run();
    match got {
        Some(v) => {
            CACHE
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert_with(HashMap::new)
                .insert(key.clone(), v);
        }
        None => {
            FAILED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert_with(HashMap::new)
                .insert(key.clone(), now);
        }
    }
    got
}

/// Whether `program`'s Hermes has verified hook payloads. Unknown,
/// unparsable and unsupported all read `false`.
pub(crate) fn telemetry_ok<'a>(
    program: &str,
    env: impl Iterator<Item = (&'a str, &'a str)>,
) -> bool {
    let v = probe(program, env);
    if v.is_none() {
        tracing::info!(
            program,
            "hermes --version gave no usable version; hook telemetry off"
        );
    }
    v.as_ref().is_some_and(telemetry_supported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_probe_is_not_rerun_until_the_ttl_passes() {
        let key: Key = (PathBuf::from("hermes-probe-test-never-exists"), None);
        let t0 = Instant::now();
        let mut runs = 0;
        assert!(
            probe_with(&key, t0, || {
                runs += 1;
                None
            })
            .is_none()
        );
        assert!(
            probe_with(&key, t0 + Duration::from_secs(30), || {
                runs += 1;
                None
            })
            .is_none()
        );
        assert_eq!(runs, 1, "second call within the TTL must not re-run");
        probe_with(&key, t0 + FAILURE_TTL + Duration::from_secs(1), || {
            runs += 1;
            None
        });
        assert_eq!(runs, 2, "after the TTL it probes again");
    }
}
