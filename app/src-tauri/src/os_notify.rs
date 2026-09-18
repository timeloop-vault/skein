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
//! launch with Skein closed — brings the window forward and lets
//! `App.tsx` recover the click's target the same way regardless of
//! which path fired (#118, #294).
//!
//! `init()` (called once from `setup()`), `os_notify_show` and
//! `os_notify_take_pending` exist on every OS so the app compiles and
//! the `generate_handler!` registry stays uniform; all three are
//! stubs everywhere but Windows.
//!
//! #294: a cold `-Embedding` launch's COM activation and a warm click
//! both race the frontend's own boot/listener setup, so the target
//! can't just ride the click event's payload — it has to land
//! somewhere durable first. Every activation (banner click, warm COM
//! `Activate`, or cold-start COM `Activate`) writes the decoded
//! [`skein_winnotify::LaunchTarget`] into a latest-wins pending slot,
//! then emits [`OS_NOTIFICATION_CLICKED_EVENT`] as a bare poke with no
//! payload; [`os_notify_take_pending`] is how the frontend actually
//! reads (and clears) it, whenever it's ready to.

use tauri::AppHandle;

/// Emitted when the user activates (clicks) a toast shown via
/// [`os_notify_show`], or a COM activation reaches a running Skein
/// with a parseable target. Carries no payload — it's only a poke;
/// the target itself is read via [`os_notify_take_pending`], because
/// a cold-start activation can win the race against the frontend's
/// own listener setup. Windows-only: every emitter is, and an
/// ungated const is dead code (`-D warnings`) elsewhere.
#[cfg(windows)]
pub const OS_NOTIFICATION_CLICKED_EVENT: &str = "skein://os-notification-clicked";

/// The room + harness a pending activation should jump to, as handed
/// to the frontend by [`os_notify_take_pending`]. Not `cfg`-gated:
/// both the Windows and stub command implementations return
/// `Option<PendingActivation>`, so the type has to exist everywhere
/// the `generate_handler!` registry does.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingActivation {
    room_id: String,
    harness_id: String,
}

#[cfg(windows)]
impl From<skein_winnotify::LaunchTarget> for PendingActivation {
    fn from(target: skein_winnotify::LaunchTarget) -> Self {
        Self {
            room_id: target.room_id,
            harness_id: target.harness_id,
        }
    }
}

/// Latest-wins slot for the target of the most recent toast
/// activation that hasn't yet been claimed by
/// [`os_notify_take_pending`]. A `Mutex` rather than per-window Tauri
/// state because it's written from the COM activator's own dedicated
/// thread (see `skein_winnotify::start_activator`), which never sees
/// the `AppHandle`'s managed state setup.
#[cfg(windows)]
static PENDING_ACTIVATION: std::sync::Mutex<Option<skein_winnotify::LaunchTarget>> =
    std::sync::Mutex::new(None);

/// Record `target` as the pending activation, overwriting whatever
/// was there before (latest wins). Recovers from a poisoned mutex
/// rather than propagating the panic — a lock held during an
/// unrelated panic elsewhere must not cost every later toast click.
#[cfg(windows)]
fn set_pending(target: skein_winnotify::LaunchTarget) {
    match PENDING_ACTIVATION.lock() {
        Ok(mut guard) => *guard = Some(target),
        Err(poisoned) => *poisoned.into_inner() = Some(target),
    }
}

/// Take (clearing) the pending activation, if any. Same poisoned-lock
/// recovery as [`set_pending`].
#[cfg(windows)]
fn take_pending() -> Option<skein_winnotify::LaunchTarget> {
    match PENDING_ACTIVATION.lock() {
        Ok(mut guard) => guard.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    }
}

/// Record `target` as the pending activation and poke the frontend.
#[cfg(windows)]
fn set_pending_and_notify(app: &AppHandle, target: skein_winnotify::LaunchTarget) {
    set_pending(target);
    emit_click(app);
}

/// The notify icon `skein-winnotify::register_app` points the AUMID's
/// `IconUri` at. Embedded rather than referenced by path so it
/// survives being written into `app_data_dir()` regardless of where
/// Skein itself is installed.
#[cfg(windows)]
const ICON_BYTES: &[u8] = include_bytes!("../icons/128x128.png");

/// Bring the main window to the front. Best-effort: a missing window
/// or a failed `hwnd()` lookup is logged and otherwise ignored — the
/// click still isn't lost if the target below can still be recorded.
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

/// Poke the frontend that a pending activation is waiting in
/// [`PENDING_ACTIVATION`]. No payload — see [`OS_NOTIFICATION_CLICKED_EVENT`].
#[cfg(windows)]
fn emit_click(app: &AppHandle) {
    use tauri::Emitter;

    if let Err(e) = app.emit(OS_NOTIFICATION_CLICKED_EVENT, ()) {
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
        if let Some(target) = skein_winnotify::parse_launch(&invoked_args) {
            set_pending_and_notify(&activation_app, target);
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
/// case) to [`OS_NOTIFICATION_CLICKED_EVENT`] via the pending slot.
///
/// Async: `show_toast` is blocking (WinRT/COM calls), so it runs on a
/// dedicated blocking thread rather than the async runtime Tauri
/// commands otherwise share.
///
/// # Errors
/// Returns an error if `room_id`/`harness_id` don't round-trip
/// through the toast's `launch` string (see
/// [`skein_winnotify::LaunchTarget::new`]) or if showing the toast
/// itself fails.
#[cfg(windows)]
#[tauri::command]
pub async fn os_notify_show(
    app: AppHandle,
    room_id: String,
    harness_id: String,
    title: String,
    body: String,
) -> Result<(), String> {
    let identifier = app.config().identifier.clone();
    let target = skein_winnotify::LaunchTarget::new(room_id, harness_id)
        .ok_or_else(|| "invalid room/harness id for a notification target".to_owned())?;

    tauri::async_runtime::spawn_blocking(move || {
        let click_app = app;
        skein_winnotify::show_toast(&identifier, &target, &title, &body, move |clicked_target| {
            raise_main_window(&click_app);
            set_pending_and_notify(&click_app, clicked_target);
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
    _room_id: String,
    _harness_id: String,
    _title: String,
    _body: String,
) -> Result<(), String> {
    Err("os_notify_show is Windows-only".into())
}

/// Take (clearing) the pending activation target, if any — the
/// frontend's side of the pending slot described at the top of this
/// module. Returns `None` on every OS but Windows, and once the slot
/// has already been claimed.
#[cfg(windows)]
#[tauri::command]
pub fn os_notify_take_pending() -> Option<PendingActivation> {
    take_pending().map(PendingActivation::from)
}

/// Stub for every OS but Windows, so the `generate_handler!` registry
/// stays uniform.
#[cfg(not(windows))]
#[tauri::command]
pub fn os_notify_take_pending() -> Option<PendingActivation> {
    None
}

// `PENDING_ACTIVATION` is a single process-wide static, so these run
// serialized under one mutex guard rather than each grabbing the real
// slot and racing every other `#[test]` in this file.
#[cfg(all(test, windows))]
mod pending_tests {
    use std::sync::Mutex;

    use skein_winnotify::LaunchTarget;

    use super::{set_pending, take_pending};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn target(room: &str, harness: &str) -> LaunchTarget {
        LaunchTarget::new(room, harness).expect("valid test ids")
    }

    #[test]
    fn take_after_set_returns_it_once_then_none() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = take_pending(); // drain anything left by another test

        set_pending(target("room1", "harness1"));
        assert_eq!(take_pending(), Some(target("room1", "harness1")));
        assert_eq!(take_pending(), None);
    }

    #[test]
    fn latest_set_wins_over_an_earlier_unread_one() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = take_pending();

        set_pending(target("room1", "harness1"));
        set_pending(target("room2", "harness2"));
        assert_eq!(take_pending(), Some(target("room2", "harness2")));
        assert_eq!(take_pending(), None);
    }

    #[test]
    fn take_on_an_empty_slot_is_none() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = take_pending();

        assert_eq!(take_pending(), None);
    }
}
