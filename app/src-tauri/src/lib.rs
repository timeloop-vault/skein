//! Skein — Tauri shell entrypoint.
//!
//! For now this is a thin shell: the entire prototype lives in the React
//! frontend. Tauri commands will land here as the BYOH/PTY layer grows.

/// Boots the Tauri runtime and blocks until the main window closes.
///
/// # Panics
///
/// Panics if the embedded Tauri context fails to build (missing config,
/// invalid capabilities, missing icon assets) — i.e. only on packaging
/// errors that would prevent the app from ever starting.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Smoke-test command — useful while wiring up the front/back bridge.
///
/// Tauri's invoke layer deserializes arguments by value, so `message`
/// must be owned. Allow `needless_pass_by_value` to keep the pedantic
/// lint level honest for the rest of the file.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn ping(message: String) -> String {
    format!("pong: {message}")
}
