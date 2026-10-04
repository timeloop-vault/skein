//! Product name and window chrome.

use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};

/// Label of the main window.
pub(crate) const MAIN_LABEL: &str = "main";
/// Label of the Control Center pop-out (#493), created from the main
/// window's JS.
pub(crate) const CONTROL_CENTER_LABEL: &str = "control-center";

/// Resolve the product name from the merged tauri config —
/// base config gives "Skein"; the dev overlay
/// (tauri.dev.conf.json, issue #21) gives "Skein (dev)" so
/// the window/dock visibly reflect which build is running.
/// The fallback covers the (impossible-in-practice) case
/// where productName is missing from config entirely.
pub(super) fn product_name(app: &tauri::App) -> String {
    app.config()
        .product_name
        .clone()
        .unwrap_or_else(|| "Skein".to_owned())
}

/// Sync the window title to `product_name` so Windows/Linux
/// taskbar + alt-tab labels follow the dev/release split.
/// (macOS hides the title text via hiddenTitle, but does the
/// right thing in the dock/app-menu via `product_name`.) Also:
/// tauri.conf.json sets decorations: true so macOS draws its
/// standard traffic-light controls (titleBarStyle: Overlay
/// requires decorations to be true at window-creation time).
/// On Windows / Linux we still want the chrome-less custom
/// titlebar with our own min/max/close — strip the native
/// chrome here. macOS-only fields (titleBarStyle, hiddenTitle)
/// are quietly ignored on those platforms.
pub(super) fn configure(
    app: &mut tauri::App,
    product_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let window = app
        .get_webview_window("main")
        .ok_or("main window missing during setup")?;
    window.set_title(product_name)?;
    #[cfg(not(target_os = "macos"))]
    window.set_decorations(false)?;
    Ok(())
}

/// Bring the main window to the front, whether or not it was minimized
/// or hidden. Shared by open requests, OS toast activation and the
/// `window_raise_main` command (#493).
pub(crate) fn raise_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_LABEL) else {
        tracing::warn!("raise main window: main window missing");
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    // `set_focus` alone loses to the foreground lock when the input that
    // asked for the raise went elsewhere (a launch, a toast, the
    // pop-out). Same trick the toast activation uses (#155).
    #[cfg(windows)]
    match window.hwnd() {
        Ok(hwnd) => {
            skein_winnotify::bring_to_front(hwnd.0 as isize);
        }
        Err(e) => tracing::warn!(error = %e, "raise main window: hwnd lookup failed"),
    }
}

/// Whether destroying the window labelled `label` must take the
/// Control Center pop-out down with it: only the main window does.
fn destroys_pop_out(label: &str) -> bool {
    label == MAIN_LABEL
}

/// `Builder::on_window_event`: when `main` is destroyed (quit path),
/// destroy the pop-out too, or the process would linger with only it
/// open. Closing the pop-out alone never reaches `main`.
pub(crate) fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if matches!(event, WindowEvent::Destroyed)
        && destroys_pop_out(window.label())
        && let Some(pop_out) = window.app_handle().get_webview_window(CONTROL_CENTER_LABEL)
        && let Err(e) = pop_out.destroy()
    {
        tracing::warn!(error = %e, "control center: destroy with main failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_main_takes_the_pop_out_with_it() {
        assert!(destroys_pop_out("main"));
        assert!(!destroys_pop_out(CONTROL_CENTER_LABEL));
        assert!(!destroys_pop_out("other"));
    }
}
