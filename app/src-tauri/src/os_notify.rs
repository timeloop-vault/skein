//! Windows-only OS toast notifications with working click activation
//! (#155), on top of `skein-winnotify`.
//!
//! `tauri-plugin-notifications`' desktop (notify-rust) backend is
//! fire-and-forget on Windows: it never wires up a click handler at
//! all (macOS's native Swift bridge is unaffected and stays on the
//! plugin path — see `lib.rs::run`). Skein registers its own
//! `AppUserModelId` + COM `CustomActivator` and shows toasts through
//! `skein-winnotify` instead, purely so a click — whether on the live
//! banner or later from Action Center, or even a cold `-Embedding`
//! launch with Skein closed — brings the window forward and forwards
//! the same `{ id }` shape the plugin's own click event carries
//! (#118), letting `App.tsx` share one handler for both.
//!
//! `init()` (called once from `setup()`) and the `os_notify_show`
//! command exist on every OS so the app compiles and the
//! `generate_handler!` registry stays uniform; both are stubs
//! everywhere but Windows.

use tauri::AppHandle;

/// Emitted when the user activates (clicks) a toast shown via
/// [`os_notify_show`], or a COM activation reaches a running Skein
/// with a parseable id. Frontend contract — do not rename `id`
/// without checking `App.tsx`'s listener.
pub const OS_NOTIFICATION_CLICKED_EVENT: &str = "skein://os-notification-clicked";

/// Payload of [`OS_NOTIFICATION_CLICKED_EVENT`].
#[cfg(windows)]
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OsNotificationClicked {
    id: u32,
}

/// The notify icon `skein-winnotify::register_app` points the AUMID's
/// `IconUri` at. Embedded rather than referenced by path so it
/// survives being written into `app_data_dir()` regardless of where
/// Skein itself is installed.
#[cfg(windows)]
const ICON_BYTES: &[u8] = include_bytes!("../icons/128x128.png");

/// Bring the main window to the front. Best-effort: a missing window
/// or a failed `hwnd()` lookup is logged and otherwise ignored — the
/// click still isn't lost if the id below can still be emitted.
#[cfg(windows)]
fn raise_main_window(app: &AppHandle) {
    use tauri::Manager;

    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("os notify: main window missing during activation");
        return;
    };
    match window.hwnd() {
        Ok(hwnd) => {
            skein_winnotify::bring_to_front(hwnd.0 as isize);
        }
        Err(e) => tracing::warn!(error = %e, "os notify: hwnd lookup failed"),
    }
}

/// Forward a notification id to the frontend.
#[cfg(windows)]
fn emit_click(app: &AppHandle, id: u32) {
    use tauri::Emitter;

    if let Err(e) = app.emit(OS_NOTIFICATION_CLICKED_EVENT, OsNotificationClicked { id }) {
        tracing::warn!(error = %e, "os notify: click emit failed");
    }
}

/// Write [`ICON_BYTES`] to `<app_data_dir>/notify-icon.png` if it's
/// missing or stale (e.g. Skein was upgraded to a build with a new
/// icon) and return the path. Never fatal on its own — a failure here
/// just means `register_app` gets no icon path at all.
#[cfg(windows)]
fn ensure_notify_icon(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;

    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("notify-icon.png");
    let needs_write = match std::fs::read(&path) {
        Ok(existing) => existing != ICON_BYTES,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&path, ICON_BYTES).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

/// Registers Skein's unpackaged-build `AppUserModelId` and COM toast
/// activator (#155), so a click can reach Skein even from Action
/// Center or a cold `-Embedding` launch — not only while the toast
/// banner from [`os_notify_show`] is still on screen. Called once
/// from `setup()`.
///
/// Best-effort throughout: any failure is logged and swallowed, never
/// fatal — Skein is fully usable without OS notification clicks
/// working, exactly as it was before #155.
#[cfg(windows)]
pub(crate) fn init(app: &AppHandle) {
    use tauri::Manager;

    let identifier = app.config().identifier.clone();
    let display_name = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| "Skein".to_owned());
    let clsid = skein_winnotify::clsid_for(&identifier);

    let exe_path = match tauri::utils::platform::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "os notify: current_exe failed; registration skipped");
            return;
        }
    };

    let icon_path = match ensure_notify_icon(app) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "os notify: writing notify icon failed; continuing without one");
            std::path::PathBuf::new()
        }
    };

    if let Err(e) =
        skein_winnotify::register_app(&identifier, &display_name, &icon_path, clsid, &exe_path)
    {
        tracing::warn!(error = %e, "os notify: registering AUMID/activator failed");
        return;
    }

    let activation_app = app.clone();
    match skein_winnotify::start_activator(clsid, move |invoked_args| {
        raise_main_window(&activation_app);
        if let Some(id) = skein_winnotify::parse_launch(&invoked_args) {
            emit_click(&activation_app, id);
        }
    }) {
        Ok(handle) => {
            app.manage(handle);
            tracing::info!(
                aumid = %identifier,
                clsid = %skein_winnotify::clsid_braced(clsid),
                "os notify: AUMID + COM activator registered"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "os notify: starting COM activator failed");
        }
    }
}

/// Stub for every OS but Windows.
#[cfg(not(windows))]
pub(crate) fn init(_app: &AppHandle) {}

/// Show a Windows toast and wire its click (while the banner is still
/// on screen — see `init()` above for the Action Center / cold-start
/// case) to [`OS_NOTIFICATION_CLICKED_EVENT`].
///
/// Async: `show_toast` is blocking (WinRT/COM calls), so it runs on a
/// dedicated blocking thread rather than the async runtime Tauri
/// commands otherwise share.
#[cfg(windows)]
#[tauri::command]
pub async fn os_notify_show(
    app: AppHandle,
    id: u32,
    title: String,
    body: String,
) -> Result<(), String> {
    let identifier = app.config().identifier.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let click_app = app;
        skein_winnotify::show_toast(&identifier, id, &title, &body, move |clicked_id| {
            raise_main_window(&click_app);
            emit_click(&click_app, clicked_id);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Stub for every OS but Windows, so the `generate_handler!` registry
/// stays uniform. macOS keeps the plugin's native Swift bridge; Linux
/// keeps the plugin's notify-rust path — neither needs this command.
#[cfg(not(windows))]
#[tauri::command]
#[allow(clippy::unused_async)]
pub async fn os_notify_show(
    _app: AppHandle,
    _id: u32,
    _title: String,
    _body: String,
) -> Result<(), String> {
    Err("os_notify_show is Windows-only".into())
}
