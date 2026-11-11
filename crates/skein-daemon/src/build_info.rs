//! The build version seam. The version comes from the app's `build.rs`
//! (`env!("SKEIN_VERSION")`), which cannot live here, so the app hands it
//! down once at startup and the daemon reads it back. The commit rides the
//! same way; the profile mapping is pure and lives here.

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// Record the build version. The first call wins; later calls are ignored.
pub fn set_version(v: &'static str) {
    let _ = VERSION.set(v);
}

/// The version set by [`set_version`], or `"0.0.0-unknown"` before then.
pub fn version() -> &'static str {
    VERSION.get().copied().unwrap_or("0.0.0-unknown")
}

static COMMIT: OnceLock<Option<&'static str>> = OnceLock::new();

/// Record the build commit (`None` when unknown). The first call wins.
pub fn set_commit(c: Option<&'static str>) {
    let _ = COMMIT.set(c);
}

/// The short commit set by [`set_commit`], when known.
pub fn commit() -> Option<&'static str> {
    COMMIT.get().copied().flatten()
}

/// Which build profile a bundle identifier belongs to.
pub fn profile_for_identifier(id: &str) -> &'static str {
    match id {
        "com.timeloop-vault.skein" => "release",
        "com.timeloop-vault.skein.local" => "local",
        "com.timeloop-vault.skein.dev" => "dev",
        _ => "unknown",
    }
}
