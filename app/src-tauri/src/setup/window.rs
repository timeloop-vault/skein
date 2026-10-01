//! Product name and window chrome.

use tauri::Manager;

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
