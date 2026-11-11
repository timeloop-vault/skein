//! The Tauri `setup()` hook and everything it brings up, in the order it
//! must run. Order is behaviour here: logging first (so the rest can log),
//! the Windows toast registration right after it, then state, servers,
//! the window, and the macOS menu last.
//!
//! Construction order (#579, #582), after logging and the toast registration:
//! 1. `Paths` (app data + resource dir, `tauri_paths.rs`)
//! 2. spawn env, then the login-shell probe
//! 3. updater proxy init (reads `paths.data_dir`)
//! 4. `Daemon::build` over the Tauri side of the in-process transport:
//!    harness config, database, the managers, then the agent API (#213) and
//!    design preview (#433) listeners, each bound synchronously
//! 5. manage every piece of the daemon and spawn its two listener tasks
//!    (the db last)
//! 6. window, 7. open request, 8. menu, 9. perf
//!
//! Steps 2 and 4 are Tauri-free (`skein-daemon`'s `daemon.rs`); `embed`
//! only builds the Tauri client seams (#582) and hands the result to Tauri.
//!
//! Where things are:
//! - `logging` — the daily-rotating file log plus stderr, and `LogGuard`
//! - `embed` — builds the daemon and manages/spawns what it returns
//! - `window` — product name and window chrome
//! - `menu` — the macOS app menu and its `on_menu_event` handler

mod embed;
mod logging;
mod menu;
mod window;

pub(crate) use skein_daemon::tls::install_rustls_provider;

pub(crate) use menu::on_menu_event;
pub(crate) use window::{MAIN_LABEL, on_window_event, raise_main_window};

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

    let paths = embed::spawn_env(app)?;
    // #614: the updater plugin's client hook reads this global.
    crate::updater_proxy::init(&paths.data_dir);
    embed::daemon(app, &paths)?;

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
