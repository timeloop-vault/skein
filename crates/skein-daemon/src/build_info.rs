//! The build version seam. The version comes from the app's `build.rs`
//! (`env!("SKEIN_VERSION")`), which cannot live here, so the app hands it
//! down once at startup and the daemon reads it back.

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
