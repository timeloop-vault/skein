//! Where Skein keeps its state and finds its shipped files. (#579)
//!
//! A daemon has no `tauri::App`, so the server-side setup code must not ask
//! one where the app data or resource directory is. It reads a [`Paths`]
//! instead: `tauri_paths.rs` builds one from the Tauri path API, and a
//! daemon builds its own later (#580). This module deliberately imports
//! nothing from `tauri`.
//!
//! Not covered, on purpose: the log directory. Logging
//! (`setup/logging.rs`) is a client concern and keeps its own
//! `app.path().app_log_dir()`.

use std::path::PathBuf;

/// The directories server-side state lives in.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Beside which `skein.db`, `settings.json`, the probe spool files and
    /// the opencode sidecar records live. Fatal when it cannot be resolved,
    /// so a `Paths` always has one.
    pub data_dir: PathBuf,
    /// Where the shipped harness-config bundle lives. Failing to resolve it
    /// is never fatal (a bundle that did not package only costs the agent
    /// its review tools), so the reason is carried as a string for
    /// `HarnessConfig::unavailable` instead of aborting the boot.
    pub resource_dir: Result<PathBuf, String>,
}

impl Paths {
    /// The `SQLite` database, `<data_dir>/skein.db`.
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("skein.db")
    }
}
