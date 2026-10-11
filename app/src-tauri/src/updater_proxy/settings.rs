//! `updater.json`: the persisted updater proxy, beside `skein.db`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const SETTINGS_FILE: &str = "updater.json";

/// Field policy as for the other settings files: the container is
/// `#[serde(default)]`, so a stale or hand-edited file still parses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct UpdaterSettings {
    /// The normalised proxy URL, `None` = not configured.
    pub(crate) proxy: Option<String>,
}

pub(crate) fn file_path(dir: &Path) -> PathBuf {
    dir.join(SETTINGS_FILE)
}

/// Never fails: a missing file means no setting, and an unreadable or
/// unparseable one is logged and ignored rather than stopping boot. The
/// second value is that problem (no file contents), for the UI.
pub(crate) fn load(dir: &Path) -> (UpdaterSettings, Option<String>) {
    let path = file_path(dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (UpdaterSettings::default(), None);
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "updater settings: read failed");
            return (
                UpdaterSettings::default(),
                Some(format!("could not read {SETTINGS_FILE}: {e}")),
            );
        }
    };
    match serde_json::from_str(&raw) {
        Ok(parsed) => (parsed, None),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "updater settings: parse failed");
            (
                UpdaterSettings::default(),
                Some(format!("{SETTINGS_FILE} is not valid ({e}); ignoring it")),
            )
        }
    }
}

/// Temp file + rename, so an interrupted save can't leave half a file.
pub(crate) fn save(dir: &Path, settings: &UpdaterSettings) -> Result<(), String> {
    let path = file_path(dir);
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("replacing {}: {e}", path.display()))?;
    Ok(())
}
