//! PTY layer — wraps `portable-pty` so the rest of the app sees an
//! event-stream handle keyed by an opaque id.
//!
//! Each spawn runs four OS threads:
//!   - The reader thread does the blocking PTY `read()` and forwards
//!     raw bytes over an mpsc channel; it never calls `on_event`
//!     itself, so `recv_timeout` elsewhere can do the batching.
//!   - The coalescer thread drains that channel and batches bytes into
//!     `PtyEvent::Data` chunks (#171 slice b): a chunk arriving after a
//!     quiet spell goes out immediately, so keystroke echo isn't
//!     delayed, but a sustained firehose (`cargo build` and friends)
//!     accumulates and flushes at most once per tick, or sooner past a
//!     byte cap, instead of one IPC message per up-to-8-KB `read()`. It
//!     flushes whatever is pending when the reader disconnects, before
//!     it exits — see `run_coalescer`.
//!   - The writer thread (#171 slice d) owns the PTY's stdin and drains
//!     a `Vec<u8>` channel that `PtyManager::write` only ever sends on.
//!     The blocking `write_all`/`flush` used to happen under the
//!     manager-wide lock, so a child that stopped reading stdin (a full
//!     tty input queue) blocked that write forever and froze
//!     resize/kill/spawn for every *other* PTY too — see
//!     `spawn_pty_writer`.
//!   - The waiter thread blocks on the OS process handle via
//!     `child.wait()` and emits `PtyEvent::Exit` the moment the child
//!     dies — naturally (Claude `/exit`) or via `kill`.
//!
//! The waiter is load-bearing on Windows: `ConPTY` keeps the reader
//! pipe open after the child exits, so the read loop alone would never
//! see EOF on a natural exit. Watching the process handle independently
//! gets us the exit signal regardless. Ordering the coalescer's
//! trailing `PtyEvent::Data` ahead of `PtyEvent::Exit` is **guaranteed**
//! when the coalescer finishes draining within 250 ms of `child.wait()`
//! returning — it drops a `Sender<()>` on exit, and the waiter's
//! `recv_timeout` on the paired receiver returns the instant that
//! happens, almost always far under the bound. Past 250 ms it degrades
//! to **best-effort**, same as before: a background grandchild can hold
//! the PTY open on Unix, and on Windows `ConPTY` may not have EOF'd the
//! reader yet, so the waiter gives up waiting and emits `Exit` anyway
//! rather than hang forever.

mod env;
mod output;
mod preview;
mod probe;
pub(crate) mod procscan;

use env::apply_env;
use output::{COALESCE_BYTE_CAP, COALESCE_WINDOW, run_coalescer, spawn_pty_writer};
use preview::windows_resolved_program;
use probe::probe_result;

pub(crate) use preview::{EnvPreview, env_preview, harness_program_lookup};
#[cfg(not(target_os = "windows"))]
pub(crate) use probe::probe_shell;
pub(crate) use probe::{login_shell_path, prewarm_probe, reprobe};

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;

use crate::agent_api::state::HarnessIdentity;
use crate::harness_config::HarnessConfig;
use crate::harness_kind::HarnessKind;
use crate::spawn_settings::SpawnSettings;

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

/// One live PTY. We hold the master so we can resize, a channel to the
/// dedicated writer thread for stdin (#171d — `write` never touches the
/// PTY's own I/O), and the killer so closing the harness doesn't leak
/// the child.
struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer_tx: mpsc::Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// The child's OS pid, kept so #517 can scan its descendants.
    pid: Option<u32>,
}

/// What `spawn` reports back about the environment it built.
pub struct SpawnOutcome {
    /// Whether #215's config injection was non-empty.
    pub injected: bool,
    /// Whether `CLAUDE_CODE_DISABLE_MOUSE_CLICKS` is truthy in the final
    /// spawn environment (#401).
    pub mouse_clicks_disabled: bool,
}

/// One spawn's inputs. A struct rather than six positional parameters
/// because `cmd`/`cwd` and `rows`/`cols` are trivially swappable at a
/// call site and the compiler would not notice.
pub struct SpawnRequest<'a> {
    pub id: String,
    pub cmd: &'a [String],
    pub cwd: &'a Path,
    pub rows: u16,
    pub cols: u16,
    pub settings: &'a SpawnSettings,
    /// How this harness reaches its room's review (#213). `None` when
    /// the agent API failed to bind — better no variables at all than
    /// four pointing at a dead port.
    pub agent: Option<&'a HarnessIdentity>,
    /// The harness kind, so #215 knows which CLI's configuration
    /// mechanism to use. Taken from the harness record rather than
    /// inferred from `cmd`, because the two can legitimately disagree:
    /// a `claude` harness whose command the user swapped for a shell is
    /// still a `claude` harness.
    pub kind: HarnessKind,
    /// The shipped config bundle that teaches the agent CLIs about the
    /// review API (#215). `None` outside the app (tests, preview).
    pub harness_config: Option<&'a HarnessConfig>,
}

/// #164: refuse a spawn whose cwd doesn't exist (or isn't a directory)
/// rather than let the OS silently fall back to some ancestor — the
/// symptom that sent a harness to `C:\` when its worktree had been
/// removed out from under it.
fn check_cwd(cwd: &Path) -> Result<(), PtyError> {
    match std::fs::metadata(cwd) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(PtyError(format!(
            "working directory is not a directory: {}",
            cwd.display()
        ))),
        Err(_) => Err(PtyError(format!(
            "working directory does not exist: {}",
            cwd.display()
        ))),
    }
}

#[derive(Default)]
pub struct PtyManager {
    inner: Mutex<HashMap<String, Pty>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn `req.cmd` (argv-style) inside `req.cwd` with the given
    /// terminal dimensions. `on_event` is called from the reader and
    /// waiter threads — once per output chunk and once on child exit.
    /// Must be `Send + Sync` because both threads share access via an
    /// `Arc`.
    ///
    /// `req.cwd` is enforced, never an OS fallback (#164): a cwd that no
    /// longer exists (a removed worktree, say) fails the spawn instead
    /// of silently landing the child in whatever ancestor directory the
    /// OS falls back to.
    ///
    /// The id in `req` is what you pass to `write` / `resize` / `kill`.
    /// Returns whether #215's config injection was non-empty for this
    /// spawn — the frontend's #238 nudge gate needs to know, and
    /// computing it a second time on the caller's side would risk
    /// drifting from what this function actually injected.
    pub fn spawn<F>(&self, req: SpawnRequest<'_>, on_event: F) -> Result<SpawnOutcome, PtyError>
    where
        F: Fn(PtyEvent) + Send + Sync + 'static,
    {
        let SpawnRequest {
            id,
            cmd,
            cwd,
            rows,
            cols,
            settings,
            agent,
            kind,
            harness_config,
        } = req;
        check_cwd(cwd)?;
        let Some((program, stored_args)) = cmd.split_first() else {
            return Err(PtyError("pty_spawn: empty cmd".into()));
        };

        // #215: what Skein appends so this harness's agent can reach the
        // review API without the user configuring anything. Computed
        // once — the Windows re-resolution below rebuilds the command
        // and must not recompute it into something different.
        let injection =
            crate::harness_config::injection_for(kind, program, harness_config, settings, agent);
        let injected = !injection.is_empty();
        let args: Vec<String> = stored_args
            .iter()
            .cloned()
            .chain(injection.args.iter().cloned())
            .collect();

        tracing::info!(
            id = %id,
            cmd = ?cmd,
            cwd = %cwd.display(),
            rows,
            cols,
            kind = %kind,
            "pty_spawn"
        );
        // Separate line, and only when there is something to say: this
        // is what you grep for when an agent turns out to have no
        // review tools. `cmd` above is the *stored* argv, so without
        // this the log would describe a command the child never got.
        if !injection.is_empty() {
            tracing::info!(
                id = %id,
                kind = %kind,
                args = ?injection.args,
                env = ?injection.env,
                "pty_spawn harness config injected"
            );
        }

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
        for arg in &args {
            builder.arg(arg);
        }
        builder.cwd(cwd);

        let mut applied = apply_env(&mut builder, settings, probe_result(), agent, &injection);

        // #207: `portable-pty` resolves the program itself, and on
        // Windows it prefers an extensionless `dir\name` over every
        // PATHEXT candidate — for an npm-installed harness that is the
        // `#!/bin/sh` shim, which `CreateProcessW` rejects outright.
        // Redo the lookup our way against the `PATH` the child will
        // actually get, then re-apply the environment to the rebuilt
        // command so the two can't drift. The probe is reused rather
        // than re-read: it is the same spawn, and `probe_result` can
        // block.
        if let Some(exe) = windows_resolved_program(program, &applied.path) {
            tracing::info!(id = %id, program, exe = %exe, "pty_spawn resolved program");
            let mut rebuilt = CommandBuilder::new(&exe);
            for arg in &args {
                rebuilt.arg(arg);
            }
            rebuilt.cwd(cwd);
            applied = apply_env(
                &mut rebuilt,
                settings,
                applied.probe.clone(),
                agent,
                &injection,
            );
            builder = rebuilt;
        }

        tracing::info!(
            id = %id,
            probe = %applied.probe.describe(),
            stripped = applied.stripped.len(),
            login_env = applied.login_env_keys.len(),
            path = %applied.path.to_string_lossy(),
            "pty_spawn resolved environment"
        );

        let mut child = pair.slave.spawn_command(builder).map_err(|e| {
            tracing::error!(id = %id, cmd = ?cmd, error = %e, "pty_spawn child spawn failed");
            PtyError::from_err(e)
        })?;
        let killer = child.clone_killer();
        let pid = child.process_id();

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

        // #171b: the raw reader only moves bytes off the PTY as fast as
        // the OS delivers them; it never calls `on_event` itself. The
        // coalescer thread below is what decides when a `PtyEvent::Data`
        // actually goes out, via `recv_timeout` — something a blocking
        // `reader.read()` loop can't do on its own.
        let (raw_tx, raw_rx) = mpsc::channel::<Vec<u8>>();

        let reader_id = id.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if raw_tx.send(buf[..n].to_vec()).is_err() {
                            // Coalescer already gone; nothing more to do.
                            break;
                        }
                    }
                }
            }
            // Dropping `raw_tx` here is the EOF signal the coalescer
            // acts on to flush and exit.
            tracing::info!(id = %reader_id, "pty reader exit");
        });

        // #171: the coalescer drops this the moment it has flushed
        // everything pending and is about to exit, so the waiter thread
        // below can hear "drained" without a fixed sleep.
        let (done_tx, done_rx) = mpsc::channel::<()>();

        let coalescer_id = id.clone();
        thread::spawn(move || {
            // Held only so it is dropped when this thread ends, after
            // `run_coalescer`'s final flush — that drop is the signal.
            let _done_tx = done_tx;
            // Issue #23: keep a small carry buffer so a multi-byte
            // UTF-8 sequence split across two coalesced chunks (em-
            // dash, box-drawing chars, emoji — all 3 or 4 bytes)
            // doesn't get replaced with U+FFFD on each side of the
            // boundary. Append each chunk into `carry`, slice off the
            // longest valid UTF-8 prefix, emit it, and carry the
            // trailing partial bytes into the next chunk.
            //
            // For genuinely malformed bytes (not just incomplete), we
            // still drop them — same as before — but only for bytes
            // we're *certain* are invalid (`Utf8Error::error_len()` is
            // `Some`).
            let mut carry: Vec<u8> = Vec::with_capacity(8192);
            run_coalescer(&raw_rx, COALESCE_WINDOW, COALESCE_BYTE_CAP, |chunk| {
                carry.extend_from_slice(&chunk);
                match std::str::from_utf8(&carry) {
                    Ok(s) => {
                        on_event_reader(PtyEvent::Data {
                            chunk: s.to_owned(),
                        });
                        carry.clear();
                    }
                    Err(e) => {
                        let valid_up_to = e.valid_up_to();
                        if valid_up_to > 0 {
                            // Safe by construction: bytes [..valid_up_to]
                            // are valid UTF-8.
                            let valid = std::str::from_utf8(&carry[..valid_up_to]).unwrap_or("");
                            on_event_reader(PtyEvent::Data {
                                chunk: valid.to_owned(),
                            });
                        }
                        if let Some(invalid_len) = e.error_len() {
                            // Definitely-malformed bytes — drop them
                            // (same as the old lossy path).
                            let drain_to = valid_up_to + invalid_len;
                            carry.drain(..drain_to);
                        } else {
                            // Trailing bytes are an incomplete sequence —
                            // wait for the next chunk.
                            carry.drain(..valid_up_to);
                        }
                    }
                }
            });
            tracing::info!(id = %coalescer_id, "pty coalescer exit");
        });

        // #171d: the writer thread owns `writer` from here on; `write`
        // only ever sends on this channel, never touches the PTY's I/O.
        let writer_tx = spawn_pty_writer(id.clone(), writer);

        let exit_id = id.clone();
        thread::spawn(move || {
            // Blocks on the OS process handle — returns the moment the
            // child dies, regardless of pipe state. This is the *only*
            // reliable way to detect a natural exit on Windows `ConPTY`.
            let code = child.wait().ok().map(|s| s.exit_code());
            tracing::info!(id = %exit_id, code = ?code, "pty exit");
            // #171: wait for the coalescer to signal it has flushed
            // every pending byte, bounded so a stream that never
            // naturally closes can't stall `Exit` forever. This
            // replaces the old unconditional 250 ms sleep — Chapter 7
            // phase 2's data-flush timeout, mirroring VS Code's
            // ShutdownConstants.DataFlushTimeout (see
            // microsoft/node-pty#72) — with the same 250 ms upper
            // bound, so this path is never *slower* than before, only
            // usually much faster: `Err(Disconnected)` means the
            // coalescer already drained and exited, so `Exit` fires
            // immediately instead of waiting out the rest of the bound.
            //
            // A real timeout still happens: on Windows `ConPTY` the
            // read pipe stays open until the master is dropped, so the
            // reader may not have seen EOF yet, and on Unix a
            // backgrounded grandchild can hold the PTY slave open
            // indefinitely. Past the bound this degrades to the old
            // best-effort behaviour and emits `Exit` regardless —
            // trailing output can still be lost, exactly as before.
            match done_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    tracing::warn!(
                        id = %exit_id,
                        "pty exit: coalescer still draining after 250ms, emitting Exit anyway"
                    );
                }
            }
            on_event_waiter(PtyEvent::Exit { code });
        });

        let pty = Pty {
            master: pair.master,
            writer_tx,
            killer,
            pid,
        };
        self.inner.lock().insert(id, pty);
        Ok(SpawnOutcome {
            injected,
            mouse_clicks_disabled: applied.mouse_clicks_disabled,
        })
    }

    /// Enqueue `data` for the PTY's dedicated writer thread (#171d).
    /// Only ever sends on the channel — never does I/O itself — so a
    /// child that has stopped reading stdin cannot block this call, nor
    /// the manager-wide lock it briefly holds.
    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), PtyError> {
        let map = self.inner.lock();
        let pty = map
            .get(id)
            .ok_or_else(|| PtyError(format!("pty_write: no pty with id {id}")))?;
        pty.writer_tx
            .send(data.to_vec())
            .map_err(|_| PtyError(format!("pty_write: writer closed for id {id}")))
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

    /// The PTY child's OS pid, if the PTY exists and the OS reported one.
    pub fn pid(&self, id: &str) -> Option<u32> {
        self.inner.lock().get(id).and_then(|p| p.pid)
    }

    /// Best-effort: if the child has already exited, this is a no-op.
    ///
    /// Removing the `Pty` drops `writer_tx`, which is the writer
    /// thread's own shutdown signal (#171d) — but this deliberately does
    /// not join it. A writer wedged on a `write_all` to a child that
    /// stopped reading stdin may never wake up; `kill` must return
    /// regardless.
    pub fn kill(&self, id: &str) {
        let mut map = self.inner.lock();
        if let Some(mut pty) = map.remove(id) {
            let _ = pty.killer.kill();
        }
    }
}

#[cfg(test)]
mod cwd_tests;

#[cfg(test)]
mod thread_tests;

#[cfg(all(test, unix))]
mod tests;

// Deliberately not inside the `#[cfg(all(test, unix))]` module above:
// #207 is a Windows bug, and a Windows bug guarded by Unix-only tests
// is how it shipped. Everything here runs on both.
/// The #213 spawn environment. Its own module because it must run on
/// Windows too — that is the daily-driver platform, and the variables
/// below are the only thing that gets a harness to the review at all.
#[cfg(test)]
mod agent_env_tests;

#[cfg(test)]
mod launch_tests;

#[cfg(test)]
mod login_env_tests;
