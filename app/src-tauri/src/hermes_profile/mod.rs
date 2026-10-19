//! Hermes profiles for Skein (#626/#627): the profile Skein owns, and
//! the opt-in edit that adds Skein's MCP entry to a user's own profile.
//!
//! Pure and Tauri-free: [`yaml`] edits text, this file does the safe
//! file I/O around it. Profile-name rules and home resolution live in
//! `skein_harness::hermes` (`valid_profile_name`, `profile_home`).

pub(crate) mod commands;
pub(crate) mod prepare;
pub(crate) mod yaml;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::spawn_settings::SpawnSettings;

/// The profile Skein creates and owns (`hermes -p skein`).
pub(crate) const SKEIN_PROFILE: &str = "skein";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnsureOutcome {
    AlreadyPresent,
    Added,
}

/// The Hermes root Skein's listing, consent edit and resume probe use:
/// the `HERMES_HOME` the user put in Settings -> Shell & environment
/// (the last non-empty one wins, as at spawn), else Skein's own process
/// environment. One resolver for all of them, so they agree with the
/// spawn on the common case.
///
/// Known limit: a `HERMES_HOME` set only in a per-harness env profile
/// (#615) is seen by the spawn but not here; the spawn logs a warning
/// when the two differ.
pub(crate) fn effective_root(settings: &SpawnSettings) -> Option<PathBuf> {
    settings
        .extra_env
        .iter()
        .rev()
        .find(|v| v.key.trim().eq_ignore_ascii_case("HERMES_HOME") && !v.value.trim().is_empty())
        .map(|v| PathBuf::from(v.value.trim()))
        .or_else(skein_harness::hermes::hermes_home)
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn read_existing(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Replace `path` with `new`, but only if it still holds `expected_old`
/// (`None`: it must not exist). Writes a sibling temp file and renames
/// it over the target, so Hermes never reads a half-written config.
pub(crate) fn write_config_atomic(
    path: &Path,
    expected_old: Option<&[u8]>,
    new: &[u8],
) -> Result<(), String> {
    let current = read_existing(path)?;
    if current.as_deref() != expected_old {
        return Err(format!(
            "{} changed while it was being edited; nothing was written",
            path.display()
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    let tmp = path.with_file_name(format!(
        ".{name}.skein-tmp-{}-{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(new)?;
        f.sync_all()?;
        drop(f);
        if current.is_some() {
            // Keep the file's mode; best effort.
            if let Ok(meta) = std::fs::metadata(path) {
                let _ = std::fs::set_permissions(&tmp, meta.permissions());
            }
        }
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

fn read_config_text(path: &Path) -> Result<(Vec<u8>, String), String> {
    let bytes = read_existing(path)?.ok_or_else(|| "profile config not found".to_string())?;
    let text = String::from_utf8(bytes.clone())
        .map_err(|_| format!("{} is not valid UTF-8; not editing it", path.display()))?;
    Ok((bytes, text))
}

/// Add Skein's MCP entry to the profile's `config.yaml`. A missing file
/// is an error: creating a profile is Hermes' job.
pub(crate) fn ensure_skein_entry(config_path: &Path) -> Result<EnsureOutcome, String> {
    let (bytes, text) = read_config_text(config_path)?;
    match yaml::insert_skein_entry(&text).map_err(|e| e.to_string())? {
        None => Ok(EnsureOutcome::AlreadyPresent),
        Some(new) => {
            write_config_atomic(config_path, Some(&bytes), new.as_bytes())?;
            Ok(EnsureOutcome::Added)
        }
    }
}

/// Remove the block Skein added. `Ok(false)`: there was none.
pub(crate) fn remove_skein_entry_at(config_path: &Path) -> Result<bool, String> {
    let (bytes, text) = read_config_text(config_path)?;
    match yaml::remove_skein_entry(&text) {
        None => Ok(false),
        Some(new) => {
            write_config_atomic(config_path, Some(&bytes), new.as_bytes())?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests;
