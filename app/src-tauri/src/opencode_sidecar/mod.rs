//! Process manager for a per-harness, Skein-owned `opencode serve` sidecar
//! (#676). A non-PTY child: this module covers spawn, kill, the PID record
//! and the boot sweep. HTTP readiness lives in `http`; the commands and the
//! PTY wiring sit above it.

pub(crate) mod http;
mod process;
mod record;
mod sweep;
mod tree;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

#[allow(unused_imports)] // the report type is part of the API
pub use sweep::{SweepReport, sweep_orphans};

/// How often the exit watcher polls `try_wait`.
const EXIT_POLL: Duration = Duration::from_millis(500);

/// A sidecar that exited on its own (never sent after `stop`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitNotice {
    pub harness_id: String,
    pub generation: u64,
    pub code: Option<i32>,
}

/// Whether a known sidecar is still alive. `Exited(-1)`: no exit code
/// (killed by a signal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarState {
    Running,
    Exited(i32),
}

pub type OnExit = Arc<dyn Fn(ExitNotice) + Send + Sync>;

pub struct StartRequest {
    pub harness_id: String,
    pub cwd: PathBuf,
    /// The complete child environment, resolved by the caller. The
    /// password is added here, not by the caller.
    pub env: Vec<(OsString, OsString)>,
    pub exe: PathBuf,
    pub port: u16,
    /// Lower the child's CPU priority (`SpawnSettings.lower_harness_priority`).
    pub lower_priority: bool,
    pub on_exit: OnExit,
    /// Replaces `serve --port <port> --hostname 127.0.0.1`; for tests.
    pub args_override: Option<Vec<OsString>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarInfo {
    pub pid: u32,
    pub port: u16,
    pub generation: u64,
}

struct Sidecar {
    child: Child,
    pid: u32,
    port: u16,
    generation: u64,
    password: String,
    exe: PathBuf,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// Set by the watcher when it saw a natural exit.
    exit: Option<i32>,
}

impl std::fmt::Debug for Sidecar {
    // Manual: the password must never reach a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sidecar")
            .field("pid", &self.pid)
            .field("port", &self.port)
            .field("generation", &self.generation)
            .field("exe", &self.exe)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Inner {
    sidecars: Mutex<HashMap<String, Sidecar>>,
    next_generation: AtomicU64,
    /// Directory of the PID record; `None` means no persistence.
    record_dir: Option<PathBuf>,
    /// Serialises the record file's read-modify-write.
    record_lock: Mutex<()>,
    /// Held for the whole of `start`, so a replace (stop, spawn, insert) is
    /// atomic: two overlapping starts for one harness cannot leak a child.
    start_lock: Mutex<()>,
}

impl Inner {
    fn record_add(&self, rec: record::PidRecord) {
        if let Some(dir) = &self.record_dir {
            let _g = self.record_lock.lock();
            record::add(dir, rec);
        }
    }

    fn record_remove(&self, harness_id: &str, pid: u32) {
        if let Some(dir) = &self.record_dir {
            let _g = self.record_lock.lock();
            record::remove(dir, harness_id, pid);
        }
    }

    /// Kill and reap an already-removed sidecar.
    fn kill(&self, harness_id: &str, mut sc: Sidecar) {
        // Only a child known to be still running has descendants worth
        // collecting: once it has exited its pid may belong to someone else.
        let running = sc.exit.is_none() && matches!(sc.child.try_wait(), Ok(None));
        // Collected before the parent dies, while the table still links them.
        let descendants = if running {
            tree::collect(sc.pid)
        } else {
            Vec::new()
        };
        if running && let Err(e) = sc.child.kill() {
            tracing::debug!(harness = %harness_id, pid = sc.pid, error = %e, "sidecar kill");
        }
        tree::kill_all(&descendants);
        self.record_remove(harness_id, sc.pid);
        let mut child = sc.child;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[derive(Default)]
pub struct OpencodeSidecarManager {
    inner: Arc<Inner>,
}

impl OpencodeSidecarManager {
    /// With a directory: sweeps orphans of a previous run first (finishing
    /// before any record of ours is written), then persists PIDs there.
    pub fn new(record_dir: Option<PathBuf>) -> Self {
        if let Some(dir) = &record_dir {
            let report = sweep_orphans(dir);
            if !(report.killed.is_empty() && report.mismatched.is_empty()) {
                tracing::info!(?report, "opencode sidecar boot sweep");
            }
        }
        Self {
            inner: Arc::new(Inner {
                record_dir,
                ..Inner::default()
            }),
        }
    }

    /// Start (or restart) the sidecar for `req.harness_id`.
    pub fn start(&self, req: StartRequest) -> Result<SidecarInfo, String> {
        let _start = self.inner.start_lock.lock();
        self.stop(&req.harness_id);
        let password = process::mint_password();
        let args = req.args_override.clone().unwrap_or_else(|| {
            [
                "serve",
                "--port",
                &req.port.to_string(),
                "--hostname",
                "127.0.0.1",
            ]
            .map(OsString::from)
            .to_vec()
        });
        let mut child = process::spawn(&req.exe, &args, &req.cwd, &req.env, &password)
            .map_err(|e| format!("starting {}: {e}", req.exe.display()))?;
        let pid = child.id();
        if req.lower_priority
            && let Err(e) = skein_procprio::lower_priority(pid)
        {
            tracing::warn!(harness = %req.harness_id, pid, error = %e, "sidecar: could not lower priority");
        }

        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(out) = child.stdout.take() {
            process::drain(
                out,
                req.harness_id.clone(),
                "stdout",
                password.clone(),
                None,
            );
        }
        if let Some(err) = child.stderr.take() {
            process::drain(
                err,
                req.harness_id.clone(),
                "stderr",
                password.clone(),
                Some(Arc::clone(&stderr)),
            );
        }

        match record::describe(&req.harness_id, pid) {
            Some(rec) => self.inner.record_add(rec),
            None => {
                tracing::warn!(harness = %req.harness_id, pid, "sidecar: pid not observable, not recorded");
            }
        }

        let generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let sc = Sidecar {
            child,
            pid,
            port: req.port,
            generation,
            password,
            exe: req.exe,
            stderr,
            exit: None,
        };
        let replaced = self
            .inner
            .sidecars
            .lock()
            .insert(req.harness_id.clone(), sc);
        if let Some(old) = replaced {
            // Unreachable while `start_lock` is held; never drop a live child.
            self.inner.kill(&req.harness_id, old);
        }
        spawn_watcher(
            Arc::clone(&self.inner),
            req.harness_id,
            generation,
            req.on_exit,
        );
        Ok(SidecarInfo {
            pid,
            port: req.port,
            generation,
        })
    }

    pub fn stop(&self, harness_id: &str) {
        let removed = self.inner.sidecars.lock().remove(harness_id);
        if let Some(sc) = removed {
            self.inner.kill(harness_id, sc);
        }
    }

    /// Stop only if the running sidecar is still `generation`.
    pub fn stop_if_generation(&self, harness_id: &str, generation: u64) -> bool {
        let removed = {
            let mut map = self.inner.sidecars.lock();
            if map
                .get(harness_id)
                .is_some_and(|s| s.generation == generation)
            {
                map.remove(harness_id)
            } else {
                None
            }
        };
        removed.map(|sc| self.inner.kill(harness_id, sc)).is_some()
    }

    pub fn stop_all(&self) {
        let all: Vec<_> = self.inner.sidecars.lock().drain().collect();
        for (id, sc) in all {
            self.inner.kill(&id, sc);
        }
    }

    /// `None`: no such sidecar (or another generation).
    pub fn exited(&self, harness_id: &str, generation: u64) -> Option<SidecarState> {
        let mut map = self.inner.sidecars.lock();
        let sc = map
            .get_mut(harness_id)
            .filter(|s| s.generation == generation)?;
        if let Some(code) = sc.exit {
            return Some(SidecarState::Exited(code));
        }
        match sc.child.try_wait() {
            Ok(Some(st)) => Some(SidecarState::Exited(st.code().unwrap_or(-1))),
            Ok(None) | Err(_) => Some(SidecarState::Running),
        }
    }

    /// The last ~20 stderr lines, already redacted.
    pub fn stderr_tail(&self, harness_id: &str) -> Vec<String> {
        self.inner
            .sidecars
            .lock()
            .get(harness_id)
            .map(|s| s.stderr.lock().iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The generation's password, for the PTY step only. `None` also for a
    /// sidecar that has already exited: a TUI attached to it would
    /// reconnect forever.
    pub(crate) fn password_for(&self, harness_id: &str, generation: u64) -> Option<String> {
        let mut map = self.inner.sidecars.lock();
        let sc = map
            .get_mut(harness_id)
            .filter(|s| s.generation == generation)?;
        if sc.exit.is_some() || !matches!(sc.child.try_wait(), Ok(None)) {
            return None;
        }
        Some(sc.password.clone())
    }
}

/// Polls `try_wait` under the lock; stops when the entry is gone or
/// replaced (so `stop` never produces a notice).
fn spawn_watcher(inner: Arc<Inner>, harness_id: String, generation: u64, on_exit: OnExit) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(EXIT_POLL);
            let (pid, code) = {
                let mut map = inner.sidecars.lock();
                let Some(sc) = map
                    .get_mut(&harness_id)
                    .filter(|s| s.generation == generation)
                else {
                    return;
                };
                match sc.child.try_wait() {
                    Ok(None) => continue,
                    Ok(Some(st)) => {
                        sc.exit = Some(st.code().unwrap_or(-1));
                        (sc.pid, st.code())
                    }
                    Err(e) => {
                        tracing::warn!(harness = %harness_id, error = %e, "sidecar try_wait failed");
                        return;
                    }
                }
            };
            inner.record_remove(&harness_id, pid);
            on_exit(ExitNotice {
                harness_id,
                generation,
                code,
            });
            return;
        }
    });
}
