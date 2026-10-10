//! The login-shell `PATH` probe, run once at startup and on demand.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, LazyLock, Mutex as StdMutex, PoisonError};
#[cfg(not(target_os = "windows"))]
use std::thread;
use std::time::Duration;
// Used only by `run_probe`, which only exists off Windows.
#[cfg(not(target_os = "windows"))]
use std::time::Instant;

use crate::spawn_settings::SpawnSettings;
// Only the probe reads the capture mode.
#[cfg(not(target_os = "windows"))]
use crate::spawn_env;
#[cfg(not(target_os = "windows"))]
use crate::spawn_env::login_env;
#[cfg(not(target_os = "windows"))]
use crate::spawn_settings::CaptureMode;

/// How long the probe shell may run before we kill it.
///
/// Healthy cost measured on this machine is 20-30 ms; 5 s is a hang, not
/// a slow machine.
///
/// Windows never starts a probe (`prewarm_probe` takes the
/// `NotApplicable` branch), so this is unreachable there rather than
/// wrong — the mirror of the `allow` on `ProbeFailure::NotApplicable`,
/// which is unreachable everywhere else.
#[cfg_attr(target_os = "windows", allow(dead_code))]
const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// How long a spawn will wait for a probe that hasn't finished yet.
/// Strictly longer than `PROBE_DEADLINE` so the probe thread always
/// reaches a terminal state first and this bound never actually binds.
const PROBE_WAIT: Duration = Duration::from_secs(6);

/// What the login-shell probe found, if anything.
#[derive(Clone)]
pub(crate) enum ProbeOutcome {
    /// Still running (or never started).
    Pending,
    /// Only the probe constructs this, so it is unreachable on Windows
    /// (see `PROBE_DEADLINE`).
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    Captured {
        path: String,
        /// The login shell's whole environment. `None` = the full capture
        /// failed but `PATH` worked (the PATH-only fallback).
        env: Option<Vec<(String, String)>>,
        shell: String,
        elapsed_ms: u64,
    },
    Failed {
        reason: ProbeFailure,
        shell: String,
        elapsed_ms: u64,
    },
}

/// Every variant but `NotApplicable` is produced by the probe, which
/// Windows never runs (see `PROBE_DEADLINE`) — hence the blanket
/// `allow` there, and the inverted one on `NotApplicable` below.
#[cfg_attr(target_os = "windows", allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeFailure {
    /// No probe on this platform — Windows uses portable-pty's live
    /// registry PATH merge instead. Only ever constructed there.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    NotApplicable,
    /// The user chose "don't ask a shell". Distinct from
    /// `UnsupportedShell` so the UI can report a deliberate setting as a
    /// setting rather than as a shell-compatibility failure.
    Disabled,
    /// We have no verified flag set for this shell (nu, xonsh, elvish).
    UnsupportedShell,
    /// The shell didn't finish inside `PROBE_DEADLINE` and was killed.
    Timeout,
    /// The shell couldn't be started at all.
    SpawnFailed,
    /// The shell ran but printed no usable payload.
    NoPayload,
}

// Manual `Debug`: `env` holds full values (API keys), so `{:?}` must only
// ever show a count.
struct VarCount(usize);

impl std::fmt::Debug for VarCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} vars>", self.0)
    }
}

impl std::fmt::Debug for ProbeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => f.write_str("Pending"),
            Self::Captured {
                path,
                env,
                shell,
                elapsed_ms,
            } => f
                .debug_struct("Captured")
                .field("path", path)
                .field("env", &env.as_ref().map(|e| VarCount(e.len())))
                .field("shell", shell)
                .field("elapsed_ms", elapsed_ms)
                .finish(),
            Self::Failed {
                reason,
                shell,
                elapsed_ms,
            } => f
                .debug_struct("Failed")
                .field("reason", reason)
                .field("shell", shell)
                .field("elapsed_ms", elapsed_ms)
                .finish(),
        }
    }
}

impl ProbeOutcome {
    /// One-line state for the spawn log, so a "command not found" in a
    /// harness can be traced to the probe without a rebuild.
    pub(super) fn describe(&self) -> String {
        match self {
            Self::Pending => "pending".to_owned(),
            Self::Captured {
                env: Some(env),
                shell,
                elapsed_ms,
                ..
            } => format!(
                "captured PATH + {} vars from {shell} in {elapsed_ms} ms",
                env.len()
            ),
            Self::Captured {
                env: None,
                shell,
                elapsed_ms,
                ..
            } => format!("captured PATH only (env capture failed) from {shell} in {elapsed_ms} ms"),
            Self::Failed {
                reason,
                shell,
                elapsed_ms,
            } => format!("failed ({reason:?}) shell={shell} after {elapsed_ms} ms"),
        }
    }

    pub(crate) fn path(&self) -> Option<&str> {
        match self {
            Self::Captured { path, .. } => Some(path),
            _ => None,
        }
    }

    /// The login shell's full environment, when the probe got it.
    pub(crate) fn login_env(&self) -> Option<&[(String, String)]> {
        match self {
            Self::Captured { env, .. } => env.as_deref(),
            _ => None,
        }
    }
}

/// The probe runs once per process, off the main thread, and every spawn
/// reads the result.
///
/// A `OnceLock` was wrong here for two reasons: it evaluated the probe
/// lazily *inside* the first spawn — which is a sync Tauri command, so a
/// hanging rc file froze the whole app's event loop with no timeout —
/// and it cannot be re-run, which the Settings "re-probe" action needs.
static PROBE: LazyLock<(StdMutex<ProbeOutcome>, Condvar)> =
    LazyLock::new(|| (StdMutex::new(ProbeOutcome::Pending), Condvar::new()));

/// Whether a probe was ever kicked off. Without this, a build that never
/// calls `prewarm_probe` — a unit test, or a future entry point that
/// forgets — would make every single spawn sit out the full wait for an
/// answer that is never coming.
static PROBE_STARTED: AtomicBool = AtomicBool::new(false);

fn set_probe(outcome: ProbeOutcome) {
    let (lock, cv) = &*PROBE;
    // A poisoned lock here means a previous probe panicked; the value is
    // still structurally fine and losing the PATH is worse than the
    // panic was.
    let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
    *guard = outcome;
    cv.notify_all();
}

/// Read the probe result, waiting only if it is still in flight.
///
/// In practice this never waits: the probe is kicked off during `setup()`
/// and completes in tens of milliseconds, long before the webview has
/// hydrated rooms and asked for a PTY. The bound exists so that the
/// pathological case costs 6 s once instead of freezing the app forever.
/// Read the probe result without waiting. Used by anything that must
/// not block the main thread; callers have to handle `Pending`.
pub(super) fn probe_snapshot() -> ProbeOutcome {
    PROBE
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The PATH the user's login shell reported, if the probe has captured
/// one yet. Never waits. For questions about what the user's *terminal*
/// can run — a GUI app's own PATH is launchd's on macOS, which knows
/// nothing of `~/.local/bin` (epic #255's `skein` command).
pub(crate) fn login_shell_path() -> Option<String> {
    match probe_snapshot() {
        ProbeOutcome::Captured { path, .. } => Some(path),
        ProbeOutcome::Pending | ProbeOutcome::Failed { .. } => None,
    }
}

pub(super) fn probe_result() -> ProbeOutcome {
    let (lock, cv) = &*PROBE;
    if !PROBE_STARTED.load(Ordering::Acquire) {
        return lock.lock().unwrap_or_else(PoisonError::into_inner).clone();
    }
    let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
    let (guard, timed_out) = cv
        .wait_timeout_while(guard, PROBE_WAIT, |o| matches!(o, ProbeOutcome::Pending))
        .unwrap_or_else(PoisonError::into_inner);
    if timed_out.timed_out() {
        tracing::warn!("probe_result: gave up waiting for the login-shell probe");
    }
    guard.clone()
}

/// Start the login-shell probe on a helper thread.
///
/// Called from `setup()`. `data_dir` is where the probe's stdout is
/// spooled — a file we own rather than a world-writable temp dir, and it
/// survives long enough to be worth reading in a post-mortem.
pub(crate) fn prewarm_probe(data_dir: PathBuf, settings: SpawnSettings) {
    PROBE_STARTED.store(true, Ordering::Release);
    sweep_spool_files(&data_dir);
    #[cfg(target_os = "windows")]
    {
        let _ = (data_dir, settings);
        set_probe(ProbeOutcome::Failed {
            reason: ProbeFailure::NotApplicable,
            shell: String::new(),
            elapsed_ms: 0,
        });
    }
    #[cfg(not(target_os = "windows"))]
    {
        thread::spawn(move || {
            let shell = probe_shell(&settings);
            set_probe(run_probe(
                &shell,
                settings.capture,
                PROBE_DEADLINE,
                &data_dir,
            ));
        });
    }
}

/// Re-run the probe after a settings change (the "Re-probe" action).
///
/// Resets to `Pending` first so a spawn racing the change waits for the
/// new answer rather than using the old shell's `PATH`.
pub(crate) fn reprobe(data_dir: PathBuf, settings: SpawnSettings) {
    set_probe(ProbeOutcome::Pending);
    prewarm_probe(data_dir, settings);
}

/// Remove spool files an earlier run left behind.
///
/// `run_probe` unlinks its own file, but the probe thread is detached
/// and never joined — quitting Skein mid-probe, or a panic between
/// create and unlink, orphans one permanently. The names carry a uuid,
/// so they would accumulate rather than being reused.
fn sweep_spool_files(data_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_spool = name.starts_with("probe-")
            && Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("out"));
        if is_spool {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The shell to ask for a `PATH`.
///
/// `$SHELL` *is* present in a Finder-launched bundle — verified with
/// `ps eww` against a running Skein.app: `PATH` is stripped to
/// `/usr/bin:/bin:/usr/sbin:/sbin`, but `SHELL=/bin/zsh` is there. The
/// fallback therefore almost never fires; when it does, `/bin/zsh` is
/// right on macOS (the OS default since Catalina). Falling back to
/// `/bin/bash` there — which is what this used to do — reads bash rc
/// files that a zsh user has never written.
#[cfg(not(target_os = "windows"))]
pub(crate) fn probe_shell(settings: &SpawnSettings) -> String {
    if let Some(configured) = settings.valid_shell() {
        return configured.to_owned();
    }
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/bash".into()
            }
        })
}

/// Ask a login + interactive shell what `PATH` it ends up with.
///
/// Two things here are load-bearing and easy to "simplify" wrongly:
///
/// 1. **stdout goes to a file, not a pipe.** `Command::output()` drains
///    both pipes to EOF *before* reaping the child, so an rc file that
///    backgrounds anything (`ssh-agent`, `gpg-agent`, a `mise`/`direnv`
///    daemon, `tmux new-session -d`) leaves a grandchild holding the
///    write end and EOF never comes — the read blocks forever even
///    though the shell itself exited. Reproduced: an rc file containing
///    `( sleep 45 ) &` hangs the pipe form indefinitely and completes
///    the file form in 0.02 s. A timeout alone does *not* fix this,
///    because killing the shell doesn't close a grandchild's fd.
/// 2. **Parse first, judge exit status second.** `bash -l -i -c` prints
///    "no job control in this shell" and can exit non-zero while having
///    produced a perfectly good payload. The old code rejected those.
#[cfg(not(target_os = "windows"))]
pub(super) fn run_probe(
    shell: &str,
    mode: CaptureMode,
    deadline: Duration,
    data_dir: &Path,
) -> ProbeOutcome {
    use std::process::{Command, Stdio};

    let started = Instant::now();
    let ms = |t: Instant| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX);

    if mode == CaptureMode::None {
        tracing::info!("probe: capture disabled by setting, using inherited PATH");
        return ProbeOutcome::Failed {
            reason: ProbeFailure::Disabled,
            shell: shell.to_owned(),
            elapsed_ms: 0,
        };
    }

    let Some(args) = spawn_env::probe_args(shell, mode) else {
        tracing::info!(
            shell = %shell,
            ?mode,
            "probe: no probe for this shell/mode, using inherited PATH"
        );
        return ProbeOutcome::Failed {
            reason: ProbeFailure::UnsupportedShell,
            shell: shell.to_owned(),
            elapsed_ms: ms(started),
        };
    };

    let out_path = data_dir.join(format!("probe-{}.out", uuid::Uuid::new_v4()));
    let spooled = match std::fs::File::create(&out_path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(path = %out_path.display(), error = %e, "probe: cannot spool stdout");
            return ProbeOutcome::Failed {
                reason: ProbeFailure::SpawnFailed,
                shell: shell.to_owned(),
                elapsed_ms: ms(started),
            };
        }
    };

    let mut cmd = Command::new(shell);
    cmd.args(args)
        .arg(spawn_env::PROBE_SCRIPT)
        .stdin(Stdio::null())
        .stdout(spooled)
        .stderr(Stdio::null());
    // rc files branch on these ("am I inside tmux / VS Code?"), so the
    // probe must see the same environment a harness will.
    for (key, _) in std::env::vars_os() {
        if spawn_env::is_host_terminal_var(&key.to_string_lossy()) {
            cmd.env_remove(&key);
        }
    }
    // A documented escape hatch: `[[ -n $SKEIN_PROBE ]] && return` at the
    // top of an rc file makes the probe cheap without affecting the
    // user's real shells.
    cmd.env("SKEIN_PROBE", "1");
    cmd.env("DISABLE_AUTO_UPDATE", "true");

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(shell = %shell, error = %e, "probe: spawn failed");
            let _ = std::fs::remove_file(&out_path);
            return ProbeOutcome::Failed {
                reason: ProbeFailure::SpawnFailed,
                shell: shell.to_owned(),
                elapsed_ms: ms(started),
            };
        }
    };

    let mut killed = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() >= deadline {
                    tracing::warn!(shell = %shell, "probe: deadline exceeded, killing shell");
                    let _ = child.kill();
                    let _ = child.wait();
                    killed = true;
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                tracing::warn!(shell = %shell, error = %e, "probe: wait failed");
                break;
            }
        }
    }

    // Bytes, not a String: one non-UTF-8 byte anywhere (an rc banner)
    // must not cost the whole probe.
    let bytes = std::fs::read(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    let elapsed_ms = ms(started);

    if let Some(path) = spawn_env::extract_probe_path(&String::from_utf8_lossy(&bytes)) {
        tracing::info!(shell = %shell, elapsed_ms, path = %path, "probe: captured user PATH");
        let env = login_env::extract_probe_env(&bytes);
        if let Some(env) = &env {
            // Names only: values are secrets (tokens, API keys).
            let overlay = login_env::login_env_overlay(env);
            let mut names: Vec<&str> = overlay.iter().map(|(k, _)| k.as_str()).collect();
            names.sort_unstable();
            tracing::info!(
                shell = %shell,
                elapsed_ms,
                vars = names.len(),
                names = %names.join(","),
                "probe: captured login environment"
            );
        } else {
            tracing::warn!(
                shell = %shell,
                elapsed_ms,
                "probe: env capture failed; falling back to PATH only"
            );
        }
        return ProbeOutcome::Captured {
            path,
            env,
            shell: shell.to_owned(),
            elapsed_ms,
        };
    }

    let reason = if killed {
        ProbeFailure::Timeout
    } else {
        ProbeFailure::NoPayload
    };
    tracing::warn!(
        shell = %shell,
        elapsed_ms,
        ?reason,
        stdout_bytes = bytes.len(),
        "probe: no usable PATH; falling back to the inherited PATH plus additions"
    );
    ProbeOutcome::Failed {
        reason,
        shell: shell.to_owned(),
        elapsed_ms,
    }
}
