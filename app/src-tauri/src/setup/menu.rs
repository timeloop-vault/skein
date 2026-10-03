//! The macOS app menu and the handler for its custom items.

/// The macOS app menu (built in `build`) drives this. Phase 4
/// wires a frontend listener for `skein://open-settings` to open
/// the settings modal; for now the event is fire-and-forget.
// The signature is fixed by `Builder::on_menu_event`.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn on_menu_event(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    if event.id() == "preferences" {
        use tauri::Emitter;
        let _ = app.emit("skein://open-settings", ());
    }
    if event.id() == "quit" {
        use tauri::Emitter;
        let _ = app.emit("skein://quit-requested", ());
    }
}

/// macOS expects an app menu — without one ⌘Q doesn't work,
/// there's no Edit menu for cut/copy/paste/select-all in
/// text fields, and the app feels web-shimmed. Tauri's
/// predefined items wrap `AppKit`'s standard responder-chain
/// selectors, so they target the focused element (xterm
/// selection, modal text input, etc.) without per-surface
/// wiring.
///
/// "Preferences…" is custom — it carries id "preferences"
/// and the `on_menu_event` handler above emits a tauri event
/// that phase 4's settings modal will listen for.
#[cfg(target_os = "macos")]
pub(super) fn build(
    app: &mut tauri::App,
    product_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let about = AboutMetadataBuilder::new()
        .name(Some(product_name.to_owned()))
        .version(Some(crate::build_info::VERSION))
        .build();

    let preferences = MenuItemBuilder::new("Preferences…")
        .id("preferences")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;

    // #185: NOT the predefined quit item — that fires
    // NSApp terminate: directly, bypassing the webview's
    // close-requested hook and the unsaved-editor-buffer
    // prompt. This routes Cmd+Q through the frontend,
    // which confirms and then destroys the window.
    let quit = MenuItemBuilder::new(format!("Quit {product_name}"))
        .id("quit")
        .accelerator("CmdOrCtrl+Q")
        .build(app)?;

    let app_menu = SubmenuBuilder::new(app, product_name)
        .about(Some(about))
        .separator()
        .item(&preferences)
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(&quit)
        .build()?;

    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let menu = MenuBuilder::new(app)
        .items(&[&app_menu, &edit_menu])
        .build()?;
    app.set_menu(menu)?;
    Ok(())
}

/// No app menu off macOS.
#[cfg(not(target_os = "macos"))]
#[allow(clippy::unnecessary_wraps)]
pub(super) fn build(
    _app: &mut tauri::App,
    _product_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
