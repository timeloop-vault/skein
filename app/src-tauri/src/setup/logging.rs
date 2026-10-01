//! Tracing setup: a daily-rotating file log plus stderr.

use tauri::Manager;
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Hold the non-blocking tracing-appender guard for the lifetime of
/// the app. Dropping it stops the background flush thread; pending
/// log lines from just-before-quit can be lost. Kept in Tauri state
/// so it lives until process exit. The guard's value is never read —
/// only its `Drop` matters.
#[allow(dead_code)]
struct LogGuard(tracing_appender::non_blocking::WorkerGuard);

pub(super) fn init(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Daily-rotating file log in the OS-conventional app log dir,
    // plus stderr (visible when launched from a terminal). The
    // bundled .app on macOS doesn't surface stderr anywhere
    // user-visible, so the file is what release-build debugging
    // actually relies on.
    //
    // macOS:   ~/Library/Logs/com.timeloop-vault.skein/skein.log.YYYY-MM-DD
    // Linux:   ~/.local/state/com.timeloop-vault.skein/logs/...
    // Windows: %LOCALAPPDATA%\com.timeloop-vault.skein\logs\...
    //
    // RUST_LOG env var overrides the default `info` level for
    // anyone debugging a particular subsystem (e.g.
    // `RUST_LOG=skein_app::pty=debug`).
    let log_dir = app.path().app_log_dir()?;
    std::fs::create_dir_all(&log_dir)?;
    let appender = tracing_appender::rolling::daily(&log_dir, "skein.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr.and(non_blocking))
        .init();
    app.manage(LogGuard(guard));
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        log_dir = %log_dir.display(),
        "Skein starting"
    );
    Ok(())
}
