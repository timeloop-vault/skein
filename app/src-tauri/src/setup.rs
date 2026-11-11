//! The Tauri `setup()` hook and everything it brings up, in the order it
//! must run. Order is behaviour here: logging first (so the rest can log),
//! the Windows toast registration right after it, then state, servers,
//! the window, and the macOS menu last.
//!
//! Construction order (#579), after logging and the toast registration:
//! 1. `Paths` (app data + resource dir, `tauri_paths.rs`)
//! 2. spawn env, then the login-shell probe
//! 3. updater proxy init (reads `paths.data_dir`)
//! 4. harness config (from `paths.resource_dir`)
//! 5. database, then the managers on it and the event sink
//! 6. agent API server (`AgentApiState` needs the db; manages its endpoint)
//! 7. design preview (`PreviewState` needs the db; manages its endpoint)
//! 8. manage the db
//! 9. window, 10. open request, 11. menu, 12. perf
//!
//! Steps 2, 4 and 5 are Tauri-free (`shared_state.rs`). `servers` still
//! takes `app`, because `AgentApiState` holds an `AppHandle` (#580, #582).
//!
//! Where things are:
//! - `logging` — the daily-rotating file log plus stderr, and `LogGuard`
//! - `state` — feeds `Paths` and the event sink to `shared_state` and
//!   manages the result
//! - `servers` — the agent API (#213) and design preview (#433) listeners
//! - `window` — product name and window chrome
//! - `menu` — the macOS app menu and its `on_menu_event` handler

mod logging;
mod menu;
mod servers;
mod state;
mod window;

pub(crate) use skein_daemon::tls::install_rustls_provider;

pub(crate) use menu::on_menu_event;
pub(crate) use window::{MAIN_LABEL, on_window_event, raise_main_window};

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

    let paths = state::spawn_env(app)?;
    // #614: the updater plugin's client hook reads this global.
    crate::updater_proxy::init(&paths.data_dir);
    let db = state::open_database(app, &paths)?;
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

    // #639: host main-thread lag, shown in the perf summary line.
    crate::perf_load::start();
    crate::perf_main_lag::start(app.handle().clone());

    Ok(())
}
