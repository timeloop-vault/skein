//! Native screenshot of a rectangle of a Tauri webview as a PNG (#552).
//!
//! The design pane's preview is a cross-origin iframe inside the main
//! webview, so script cannot read its pixels. The webview itself can:
//! `WebView2`'s `DevTools` `Page.captureScreenshot` with a `clip` (Windows)
//! and `WKWebView takeSnapshotWithConfiguration:` with a `rect` (macOS)
//! both render the rectangle even while the window is minimized or
//! covered. A CSS-hidden element yields a blank image; that is the
//! caller's concern.
//!
//! On other OSes there is a stub `capture` that reports an error, so the
//! app needs no cfg at the call site beyond the platform-specific
//! argument it passes.

mod geometry;
mod png;

pub use geometry::{Rect, Shot};

#[cfg(windows)]
mod cdp;
#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
pub use windows_impl::capture;

#[cfg(target_os = "macos")]
mod macos_impl;
#[cfg(target_os = "macos")]
pub use macos_impl::capture;

/// Unsupported platform: `done` gets an error right away.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn capture(
    _webview: *mut std::ffi::c_void,
    _rect: Rect,
    _scale: f64,
    done: impl FnOnce(Result<Shot, String>) + Send + 'static,
) {
    done(Err("screenshot is not supported on this platform".into()));
}
