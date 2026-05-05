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
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
#[cfg(not(target_os = "windows"))]
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;

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

        // Forward the user's environment so spawned CLIs find their
        // auth tokens, PATH entries, locale, etc. We override TERM and
        // COLORTERM unconditionally — those are about how we render,
        // not about what the user has configured.
        for (k, v) in std::env::vars() {
            builder.env(k, v);
        }
        // GUI-launched .app bundles on macOS / Linux inherit a stripped
        // PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) — not the user's shell
        // PATH. So `claude`, `opencode`, Homebrew binaries, etc. aren't
        // found. Read PATH from a login + interactive shell once and
        // use it for every spawn. No-op on Windows (different launch
        // model, no equivalent issue).
        if let Some(path) = login_shell_path() {
            builder.env("PATH", path);
        }
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

        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // UTF-8 lossy is good enough here. Most TUI
                        // traffic is valid UTF-8; the few invalid
                        // sequences (e.g. mid-frame splits) become
                        // replacement chars in the renderer.
                        let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                        on_event_reader(PtyEvent::Data { chunk });
                    }
                }
            }
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

/// PATH from the user's login + interactive shell. Cached on first call.
///
/// `Some` on macOS / Linux when the shell prints something usable.
/// Always `None` on Windows (different launch model — Explorer-launched
/// apps already inherit the user's PATH from the registry, no fix
/// needed).
///
/// This is the "Finder/Dock launches my .app with PATH=`/usr/bin:/bin`"
/// fix. The user's `.zshrc` / `.bashrc` typically prepends Homebrew,
/// nvm, pyenv, `~/.local/bin`, etc. — none of which a Finder launch
/// inherits. We invoke the shell as `-il` (interactive + login) so
/// every rc / profile gets sourced, then read `$PATH`. Does cost a
/// shell startup once per Skein run (~50 ms on macOS), only on the
/// first PTY spawn.
#[cfg(not(target_os = "windows"))]
fn login_shell_path() -> Option<&'static str> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
            let output = std::process::Command::new(&shell)
                .args(["-ilc", "echo $PATH"])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if path.is_empty() { None } else { Some(path) }
        })
        .as_deref()
}

#[cfg(target_os = "windows")]
fn login_shell_path() -> Option<&'static str> {
    None
}
