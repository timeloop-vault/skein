//! Lifted from `app/src-tauri/src/pty.rs` for the pure-Rust spike.
//! Trimmed for the spike: no serde, no tracing, no login-shell PATH
//! probing. The two-thread pattern (reader + waiter) is preserved
//! verbatim — load-bearing on Windows `ConPTY` where the read pipe
//! stays open after the child exits.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

#[derive(Debug, Clone)]
pub enum PtyEvent {
    Data {
        chunk: String,
    },
    #[allow(dead_code)] // surfaced to the UI in a later iteration
    Exit {
        code: Option<u32>,
    },
}

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

struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    /// Held only so dropping `Pty` reliably terminates the child;
    /// the explicit `PtyManager::kill` path uses a separate handle.
    #[allow(dead_code)]
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
                        let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                        on_event_reader(PtyEvent::Data { chunk });
                    }
                }
            }
        });

        thread::spawn(move || {
            let code = child.wait().ok().map(|s| s.exit_code());
            thread::sleep(Duration::from_millis(250));
            on_event_waiter(PtyEvent::Exit { code });
        });

        let pty = Pty {
            master: pair.master,
            writer,
            killer,
        };
        self.inner
            .lock()
            .expect("pty manager mutex poisoned")
            .insert(id, pty);
        Ok(())
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), PtyError> {
        let mut map = self.inner.lock().expect("pty manager mutex poisoned");
        let pty = map
            .get_mut(id)
            .ok_or_else(|| PtyError(format!("pty_write: no pty with id {id}")))?;
        pty.writer.write_all(data).map_err(PtyError::from_err)?;
        pty.writer.flush().map_err(PtyError::from_err)?;
        Ok(())
    }

    pub fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), PtyError> {
        let map = self.inner.lock().expect("pty manager mutex poisoned");
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

    #[allow(dead_code)] // exposed for future session/harness teardown
    pub fn kill(&self, id: &str) {
        let mut map = self.inner.lock().expect("pty manager mutex poisoned");
        if let Some(mut pty) = map.remove(id) {
            let _ = pty.killer.kill();
        }
    }
}
