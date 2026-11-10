//! The Tauri [`Paths`] source: reads the directories off a `tauri::App`.
//! The only place server-side state asks Tauri for them. (#579)

use tauri::Manager;

use crate::paths::Paths;

/// Persist Skein state under the OS-conventional app data dir
/// (e.g. `%APPDATA%/com.timeloop-vault.skein` on Windows).
///
/// # Errors
/// The app data dir cannot be resolved. A missing resource dir is not an
/// error here; it is carried inside the returned `Paths`.
pub fn from_app(app: &tauri::App) -> Result<Paths, Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    let resource_dir = app.path().resource_dir().map_err(|e| e.to_string());
    Ok(Paths {
        data_dir,
        resource_dir,
    })
}
