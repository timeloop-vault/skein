//! The Tauri `setup()` hook and everything it brings up, in the order it
//! must run. Order is behaviour here: logging first (so the rest can log),
//! the Windows toast registration right after it, then state, servers,
//! the window, and the macOS menu last.
//!
//! Where things are:
//! - `logging` — the daily-rotating file log plus stderr, and `LogGuard`
//! - `state` — app data dir, spawn env, harness config, the database and
//!   the managers that hang off it
//! - `servers` — the agent API (#213) and design preview (#433) listeners
//! - `window` — product name and window chrome
//! - `menu` — the macOS app menu and its `on_menu_event` handler

mod logging;
mod menu;
mod servers;
mod state;
mod window;

/// Install rustls's default crypto provider so reqwest doesn't panic
/// with "No provider set" on the first `Client::builder().build()`.
/// Reqwest 0.13 + rustls 0.23 require an explicit `install_default`
/// call before any TLS context is constructed — and reqwest constructs
/// one eagerly even when we only ever use plain HTTP (the L2c-2
/// opencode adapter talks to 127.0.0.1). Idempotent: `install_default`
/// returns Err on the second call, which we ignore.
pub(crate) fn install_rustls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub(crate) use menu::on_menu_event;

use tauri::Manager;

/// The body of `.setup(...)`.
pub(crate) fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    logging::init(app)?;

    // #155: registers Skein's unpackaged-build AUMID and COM
    // toast activator on Windows so a notification click
    // reaches it from Action Center or a cold `-Embedding`
    // launch, not only while the banner itself is on screen.
    // A no-op on every other OS. Needs only the app handle
    // (config, its own app-data dir for the icon) — placed as
    // early in `setup()` as that allows, right after logging
    // comes up, because a cold `-Embedding` launch blocks on
    // this registration with a timeout rather than waiting
    // behind the DB open and agent-API bind below.
    // Best-effort — see `os_notify::init` — never aborts
    // startup.
    crate::os_notify::init(app.handle());

    let data_dir = state::spawn_env(app)?;
    state::harness_config(app);
    let db = state::open_database(app, &data_dir)?;
    servers::agent_api(app, &db);
    servers::design_preview(app, &db);

    app.manage(db);

    let product_name = window::product_name(app);
    window::configure(app, &product_name)?;

    // Epic #255: a folder this process was launched with, and
    // `<scheme>://open` links from now on. After the window
    // exists, since a link raises it.
    crate::open_request::init(app);

    menu::build(app, &product_name)?;

    Ok(())
}
