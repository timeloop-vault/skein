//! PTY layer — wraps `portable-pty` so the rest of the app sees an
//! event-stream handle keyed by an opaque id.
//!
//! Each spawn runs two OS threads:
//!   - The reader thread streams `PtyEvent::Data` chunks until the
//!     master pipe sees EOF.
//!   - The waiter thread blocks on the OS process handle via
//!     `child.wait()` and emits `PtyEvent::Exit` the moment the child
//!     dies — naturally (Claude `/exit`) or via `kill`.
//!
//! Two threads is load-bearing on Windows: `ConPTY` keeps the reader
//! pipe open after the child exits, so the read loop alone would never
//! see EOF on a natural exit. Watching the process handle independently
//! gets us the exit signal regardless.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex as StdMutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;

use crate::spawn_env;

/// Events the PTY reader thread streams to the frontend. Tagged so the
/// JS side can branch on `kind`: `data` chunks become terminal output,
/// `exit` triggers the "Press Enter for shell, R to retry" UX.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PtyEvent {
    Data { chunk: String },
    Exit { code: Option<u32> },
}

/// Errors a PTY operation can produce. Stringly-typed because they all
/// flow back to the frontend as `Result<_, String>` anyway.
///
/// `portable-pty` uses `anyhow::Error` (which does not implement
/// `std::error::Error` due to its blanket impl), so we collapse via
/// `to_string` at the call site rather than impl `From<E: Error>`.
#[derive(Debug)]
pub struct PtyError(pub String);

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PtyError {}

impl PtyError {
    fn from_err<E: std::fmt::Display>(e: E) -> Self {
        Self(e.to_string())
    }
}

/// One live PTY. We hold the master so we can resize, the writer for
/// stdin, and the killer so closing the harness doesn't leak the child.
struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Default)]
pub struct PtyManager {
    inner: Mutex<HashMap<String, Pty>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn `cmd` (argv-style) inside `cwd` with the given terminal
    /// dimensions. `on_event` is called from the reader and waiter
    /// threads — once per output chunk and once on child exit. Must be
    /// `Send + Sync` because both threads share access via an `Arc`.
    ///
    /// Returns the id you should pass to `write` / `resize` / `kill`.
    pub fn spawn<F>(
        &self,
        id: String,
        cmd: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        on_event: F,
    ) -> Result<(), PtyError>
    where
        F: Fn(PtyEvent) + Send + Sync + 'static,
    {
        let Some((program, args)) = cmd.split_first() else {
            return Err(PtyError("pty_spawn: empty cmd".into()));
        };
        tracing::info!(
            id = %id,
            cmd = ?cmd,
            cwd = %cwd.display(),
            rows,
            cols,
            "pty_spawn"
        );

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(PtyError::from_err)?;

        let mut builder = CommandBuilder::new(program);
        for arg in args {
            builder.arg(arg);
        }
        builder.cwd(cwd);

        // `CommandBuilder::new` already seeds the child env from this
        // process's environment — and on Windows it additionally merges
        // the *live* `HKLM` + `HKCU` registry PATH, i.e. the user's
        // current PATH rather than whatever stale block Skein happened
        // to inherit at launch. We used to copy `std::env::vars()` over
        // the top of that, which on Windows overwrote the registry
        // merge with the stale value (portable-pty lowercases env keys
        // there, so our `PATH` collided with its `Path`) — the one
        // platform where nothing else compensated. There is nothing to
        // re-copy: let the base env stand and override only what we own.

        // #192: strip host-terminal identity. Skein inherits markers
        // like TERM_PROGRAM / TMUX / VSCODE_* when it is itself launched
        // from a terminal, and the agent CLIs sniff them — adopting the
        // *host* terminal's key and clipboard quirks instead of
        // xterm.js's. `iter_full_env_as_str` borrows the builder, so
        // collect the keys before removing any.
        let doomed: Vec<String> = builder
            .iter_full_env_as_str()
            .map(|(k, _)| k.to_owned())
            .filter(|k| spawn_env::is_host_terminal_var(k))
            .collect();
        for key in &doomed {
            builder.env_remove(key);
        }

        // PATH: the login-shell probe when we have one, otherwise the
        // inherited/registry PATH — and Skein's additions on top either
        // way. Applying the additions only on the probe's success path
        // (which is what the code did before) meant a probe failure
        // silently took `~/.local/bin` with it, and on this machine
        // that is where `claude` itself lives.
        let probe = probe_result();
        let base: OsString = builder
            .get_env("PATH")
            .map(OsString::from)
            .unwrap_or_default();
        let probed = probe.path().map_or(base, OsString::from);
        let resolved_path = spawn_env::merge_path(
            &probed,
            &default_path_prepend(),
            crate::home_dir().as_deref(),
            &|k| std::env::var(k).ok(),
            &|p| p.is_dir(),
        );
        // The resolved PATH was previously observable nowhere: the old
        // log line printed the *probe's* output, i.e. the value before
        // Skein's own additions, so "what did this harness actually
        // get" could not be answered from the logs at all.
        tracing::info!(
            id = %id,
            probe = %probe.describe(),
            stripped = doomed.len(),
            path = %resolved_path.to_string_lossy(),
            "pty_spawn resolved environment"
        );
        builder.env("PATH", resolved_path);

        // Unix only: portable-pty otherwise fills SHELL from the passwd
        // database, which can disagree with the shell we actually probed
        // and with the one a shell harness runs. Windows has no
        // meaningful $SHELL, and setting one confuses Git-Bash-aware
        // tools that read it.
        #[cfg(not(target_os = "windows"))]
        builder.env("SHELL", probe_shell());

        // TERM / COLORTERM are about how *we* render, not about what the
        // user configured, so they are forced last and unconditionally.
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");

        let mut child = pair.slave.spawn_command(builder).map_err(|e| {
            tracing::error!(id = %id, cmd = ?cmd, error = %e, "pty_spawn child spawn failed");
            PtyError::from_err(e)
        })?;
        let killer = child.clone_killer();

        // Drop the slave handle so EOF reaches the read end correctly
        // when the child exits *and* the master is closed. (On Windows
        // `ConPTY` the reader still won't see EOF on a natural exit until
        // the master is dropped, which is why we have a separate waiter
        // thread below.)
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(PtyError::from_err)?;
        let writer = pair.master.take_writer().map_err(PtyError::from_err)?;

        let on_event = Arc::new(on_event);
        let on_event_reader = Arc::clone(&on_event);
        let on_event_waiter = on_event;

        let reader_id = id.clone();
        thread::spawn(move || {
            // Issue #23: keep a small carry buffer so a multi-byte
            // UTF-8 sequence split across two reads (em-dash, box-
            // drawing chars, emoji — all 3 or 4 bytes) doesn't get
            // replaced with U+FFFD on each side of the boundary.
            // We append each read into `pending`, slice off the
            // longest valid UTF-8 prefix, emit it, and carry the
            // trailing partial bytes into the next iteration.
            //
            // For genuinely malformed bytes (not just incomplete),
            // we still drop them via lossy conversion — same as
            // before — but only for bytes we're *certain* are
            // invalid (Utf8Error::error_len() is Some).
            let mut pending: Vec<u8> = Vec::with_capacity(8192);
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        pending.extend_from_slice(&buf[..n]);
                        match std::str::from_utf8(&pending) {
                            Ok(s) => {
                                on_event_reader(PtyEvent::Data {
                                    chunk: s.to_owned(),
                                });
                                pending.clear();
                            }
                            Err(e) => {
                                let valid_up_to = e.valid_up_to();
                                if valid_up_to > 0 {
                                    // Safe by construction: bytes
                                    // [..valid_up_to] are valid UTF-8.
                                    let valid =
                                        std::str::from_utf8(&pending[..valid_up_to]).unwrap_or("");
                                    on_event_reader(PtyEvent::Data {
                                        chunk: valid.to_owned(),
                                    });
                                }
                                if let Some(invalid_len) = e.error_len() {
                                    // Definitely-malformed bytes — drop
                                    // them (same as the old lossy path).
                                    let drain_to = valid_up_to + invalid_len;
                                    pending.drain(..drain_to);
                                } else {
                                    // Trailing bytes are an incomplete
                                    // sequence — wait for the next read.
                                    pending.drain(..valid_up_to);
                                }
                            }
                        }
                    }
                }
            }
            tracing::info!(id = %reader_id, "pty reader exit");
        });

        let exit_id = id.clone();
        thread::spawn(move || {
            // Blocks on the OS process handle — returns the moment the
            // child dies, regardless of pipe state. This is the *only*
            // reliable way to detect a natural exit on Windows `ConPTY`.
            let code = child.wait().ok().map(|s| s.exit_code());
            tracing::info!(id = %exit_id, code = ?code, "pty exit");
            // Chapter 7 phase 2: data-flush timeout. The reader thread
            // can still deliver trailing bytes after the child has
            // exited — on Windows ConPTY especially, the read pipe
            // stays open until the master is dropped, so a TUI's last
            // frame can lag the wait() return by a few ms. Sleeping a
            // beat before firing Exit lets the reader drain so the
            // user actually sees that final frame instead of a
            // truncated viewport. Mirrors VS Code's
            // ShutdownConstants.DataFlushTimeout (250 ms) — see
            // microsoft/node-pty#72 for the original bug. Skein-side
            // the latency is on the natural-exit path only, never
            // during running output.
            thread::sleep(Duration::from_millis(250));
            on_event_waiter(PtyEvent::Exit { code });
        });

        let pty = Pty {
            master: pair.master,
            writer,
            killer,
        };
        self.inner.lock().insert(id, pty);
        Ok(())
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), PtyError> {
        let mut map = self.inner.lock();
        let pty = map
            .get_mut(id)
            .ok_or_else(|| PtyError(format!("pty_write: no pty with id {id}")))?;
        pty.writer.write_all(data).map_err(PtyError::from_err)?;
        pty.writer.flush().map_err(PtyError::from_err)?;
        Ok(())
    }

    pub fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), PtyError> {
        let map = self.inner.lock();
        let pty = map
            .get(id)
            .ok_or_else(|| PtyError(format!("pty_resize: no pty with id {id}")))?;
        pty.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(PtyError::from_err)?;
        Ok(())
    }

    /// Best-effort: if the child has already exited, this is a no-op.
    pub fn kill(&self, id: &str) {
        let mut map = self.inner.lock();
        if let Some(mut pty) = map.remove(id) {
            let _ = pty.killer.kill();
        }
    }
}

/// The `PATH` additions Skein applies on top of whatever the shell or
/// the OS reported.
///
/// `~/.local/bin` is the de-facto install location for `pip install
/// --user`, `pipx`, `uv tool` and Claude Code's own installer — and, as
/// verified on this machine, it is routinely absent from the rc chain,
/// because it is normally put there by something further up (a display
/// manager, VS Code's terminal integration) that a Finder-launched
/// bundle never sees. `~/bin` is the same story for hand-rolled scripts.
///
/// Windows gets the same `~/.local/bin` because that is where Claude
/// Code's native Windows installer puts `claude.exe`. It deliberately
/// does *not* get `%LOCALAPPDATA%\Microsoft\WindowsApps`: that is
/// already in the registry PATH, and it holds the Store execution-alias
/// stubs — prepending it would put the stub `python.exe` (which opens
/// the Microsoft Store) ahead of a real Python for every harness.
///
/// Issue #3 replaces this with a user-editable list; these stay as the
/// defaults.
fn default_path_prepend() -> Vec<String> {
    if cfg!(windows) {
        vec![r"%USERPROFILE%\.local\bin".to_owned()]
    } else {
        vec!["~/.local/bin".to_owned(), "~/bin".to_owned()]
    }
}

/// How long the probe shell may run before we kill it.
///
/// Healthy cost measured on this machine is 20-30 ms; 5 s is a hang, not
/// a slow machine.
const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// How long a spawn will wait for a probe that hasn't finished yet.
/// Strictly longer than `PROBE_DEADLINE` so the probe thread always
/// reaches a terminal state first and this bound never actually binds.
const PROBE_WAIT: Duration = Duration::from_secs(6);

/// What the login-shell probe found, if anything.
#[derive(Debug, Clone)]
pub(crate) enum ProbeOutcome {
    /// Still running (or never started).
    Pending,
    Captured {
        path: String,
        shell: String,
        elapsed_ms: u64,
    },
    Failed {
        reason: ProbeFailure,
        shell: String,
        elapsed_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeFailure {
    /// No probe on this platform — Windows uses portable-pty's live
    /// registry PATH merge instead. Only ever constructed there.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    NotApplicable,
    /// We have no verified flag set for this shell (nu, xonsh, elvish).
    UnsupportedShell,
    /// The shell didn't finish inside `PROBE_DEADLINE` and was killed.
    Timeout,
    /// The shell couldn't be started at all.
    SpawnFailed,
    /// The shell ran but printed no usable payload.
    NoPayload,
}

impl ProbeOutcome {
    /// One-line state for the spawn log, so a "command not found" in a
    /// harness can be traced to the probe without a rebuild.
    fn describe(&self) -> String {
        match self {
            Self::Pending => "pending".to_owned(),
            Self::Captured {
                shell, elapsed_ms, ..
            } => format!("captured from {shell} in {elapsed_ms} ms"),
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
fn probe_result() -> ProbeOutcome {
    let (lock, cv) = &*PROBE;
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
pub(crate) fn prewarm_probe(data_dir: PathBuf) {
    #[cfg(target_os = "windows")]
    {
        let _ = data_dir;
        set_probe(ProbeOutcome::Failed {
            reason: ProbeFailure::NotApplicable,
            shell: String::new(),
            elapsed_ms: 0,
        });
    }
    #[cfg(not(target_os = "windows"))]
    {
        thread::spawn(move || {
            let shell = probe_shell();
            set_probe(run_probe(&shell, PROBE_DEADLINE, &data_dir));
        });
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
pub(crate) fn probe_shell() -> String {
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
fn run_probe(shell: &str, deadline: Duration, data_dir: &Path) -> ProbeOutcome {
    use std::process::{Command, Stdio};

    let started = Instant::now();
    let ms = |t: Instant| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX);

    let Some(args) = spawn_env::probe_args(shell) else {
        tracing::info!(shell = %shell, "probe: unsupported shell, using inherited PATH");
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

    let stdout = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    let elapsed_ms = ms(started);

    if let Some(path) = spawn_env::extract_probe_path(&stdout) {
        tracing::info!(shell = %shell, elapsed_ms, path = %path, "probe: captured user PATH");
        return ProbeOutcome::Captured {
            path,
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
        raw_stdout = %stdout,
        "probe: no usable PATH; falling back to the inherited PATH plus additions"
    );
    ProbeOutcome::Failed {
        reason,
        shell: shell.to_owned(),
        elapsed_ms,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    /// Write an executable stand-in shell. `probe_args` dispatches on the
    /// file name, so naming the script `bash` makes `run_probe` drive it
    /// exactly as it would drive a real one — which lets us pin the
    /// behaviour that matters without depending on the developer's own
    /// rc files.
    fn fake_shell(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("create fake shell");
        write!(f, "#!/bin/sh\n{body}\n").expect("write fake shell");
        drop(f);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake shell");
        path
    }

    const PAYLOAD: &str =
        "printf '%s%s%s' '___SKEIN_PATH_BEGIN___' '/probe/bin:/usr/bin' '___SKEIN_PATH_END___'";

    #[test]
    fn probe_survives_an_rc_file_that_backgrounds_a_daemon() {
        // The bug this whole rewrite exists for. `Command::output()`
        // drains stdout to EOF before reaping, so a grandchild holding
        // the write end blocks the read forever even though the shell
        // itself exited immediately. Measured: the pipe form hangs past
        // 8 s here, the spooled-file form returns in ~0.02 s.
        //
        // `sleep 45` outlives the 5 s deadline by design — if this test
        // ever starts taking 5 s, the file redirection has regressed
        // into a pipe.
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = fake_shell(dir.path(), "bash", &format!("( sleep 45 ) &\n{PAYLOAD}"));

        let started = Instant::now();
        let outcome = run_probe(
            shell.to_str().expect("utf-8 path"),
            Duration::from_secs(5),
            dir.path(),
        );

        assert_eq!(outcome.path(), Some("/probe/bin:/usr/bin"));
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "probe waited on the backgrounded grandchild ({:?})",
            started.elapsed()
        );
    }

    #[test]
    fn probe_kills_a_shell_that_overruns_the_deadline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = fake_shell(dir.path(), "zsh", "sleep 30");

        let started = Instant::now();
        let outcome = run_probe(
            shell.to_str().expect("utf-8 path"),
            Duration::from_millis(300),
            dir.path(),
        );

        assert!(
            matches!(
                outcome,
                ProbeOutcome::Failed {
                    reason: ProbeFailure::Timeout,
                    ..
                }
            ),
            "expected a timeout, got {outcome:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn probe_accepts_a_good_payload_from_a_shell_that_exits_non_zero() {
        // `bash -l -i -c` prints "no job control in this shell" and can
        // exit non-zero while having produced a perfectly usable PATH.
        // The old code threw those away.
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = fake_shell(dir.path(), "bash", &format!("{PAYLOAD}\nexit 3"));

        let outcome = run_probe(
            shell.to_str().expect("utf-8 path"),
            Duration::from_secs(5),
            dir.path(),
        );
        assert_eq!(outcome.path(), Some("/probe/bin:/usr/bin"));
    }

    #[test]
    fn probe_reports_no_payload_rather_than_inventing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = fake_shell(dir.path(), "bash", "echo 'welcome to your shell'");

        let outcome = run_probe(
            shell.to_str().expect("utf-8 path"),
            Duration::from_secs(5),
            dir.path(),
        );
        assert!(matches!(
            outcome,
            ProbeOutcome::Failed {
                reason: ProbeFailure::NoPayload,
                ..
            }
        ));
        assert_eq!(outcome.path(), None);
    }

    #[test]
    fn probe_skips_shells_it_cannot_drive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = fake_shell(dir.path(), "nu", PAYLOAD);

        let outcome = run_probe(
            shell.to_str().expect("utf-8 path"),
            Duration::from_secs(5),
            dir.path(),
        );
        assert!(matches!(
            outcome,
            ProbeOutcome::Failed {
                reason: ProbeFailure::UnsupportedShell,
                ..
            }
        ));
    }

    #[test]
    fn probe_leaves_no_spool_files_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = fake_shell(dir.path(), "bash", PAYLOAD);
        run_probe(
            shell.to_str().expect("utf-8 path"),
            Duration::from_secs(5),
            dir.path(),
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read tempdir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("probe-"))
            .collect();
        assert!(leftovers.is_empty(), "spool files left: {leftovers:?}");
    }

    #[test]
    fn path_additions_survive_a_failed_probe() {
        // The live one-condition-away outage: `augment_path` used to sit
        // inside `if let Some(path) = login_shell_path()`, so any probe
        // failure silently dropped `~/.local/bin` — which on this
        // machine is where `claude` itself lives.
        let failed = ProbeOutcome::Failed {
            reason: ProbeFailure::Timeout,
            shell: "/bin/zsh".to_owned(),
            elapsed_ms: 5000,
        };
        let base = OsString::from("/usr/bin:/bin");
        let probed = failed.path().map_or(base, OsString::from);
        let home = PathBuf::from("/home/tester");
        let merged = spawn_env::merge_path(
            &probed,
            &["~/.local/bin".to_owned()],
            Some(&home),
            &|_| None,
            &|_| true,
        );
        assert_eq!(
            std::env::split_paths(&merged)
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["/home/tester/.local/bin", "/usr/bin", "/bin"]
        );
    }
}
