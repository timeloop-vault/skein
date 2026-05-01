//! PTY layer — wraps `portable-pty` so the rest of the app sees a
//! string-in / string-out handle keyed by an opaque id.
//!
//! Phase 1 keeps this deliberately small: spawn, write, resize, kill.
//! No reconnects, no scrollback persistence, no env discovery. The
//! reader runs on its own OS thread and pushes UTF-8-lossy chunks to
//! a callback the caller wires into a `tauri::ipc::Channel`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::thread;

use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

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
    /// dimensions. `on_output` is called from a dedicated reader thread
    /// every time we get a chunk — the caller is expected to forward
    /// it to the frontend.
    ///
    /// Returns the id you should pass to `write` / `resize` / `kill`.
    pub fn spawn<F>(
        &self,
        id: String,
        cmd: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        on_output: F,
    ) -> Result<(), PtyError>
    where
        F: Fn(String) + Send + 'static,
    {
        let Some((program, args)) = cmd.split_first() else {
            return Err(PtyError("pty_spawn: empty cmd".into()));
        };

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
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");

        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(PtyError::from_err)?;
        let killer = child.clone_killer();

        // Drop the slave handle so EOF works correctly when the child
        // exits. We don't need it again — only the master matters.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(PtyError::from_err)?;
        let writer = pair.master.take_writer().map_err(PtyError::from_err)?;

        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // UTF-8 lossy is good enough for Phase 1. Most
                        // TUI traffic is valid UTF-8; the few invalid
                        // sequences (e.g. mid-frame splits) become
                        // replacement chars in the renderer.
                        let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                        on_output(chunk);
                    }
                }
            }
            // Reap to keep the OS happy — if the caller already killed
            // it, this is a no-op.
            let _ = child.wait();
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
