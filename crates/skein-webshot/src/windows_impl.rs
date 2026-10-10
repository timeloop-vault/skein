use std::sync::{Arc, Mutex};

use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, ICoreWebView2Controller3,
};
use windows_core::{HSTRING, Interface};

use crate::geometry::{Rect, Shot, clip_scale};
use crate::{cdp, png};

type Done = Box<dyn FnOnce(Result<Shot, String>) + Send>;

/// Capture `rect` (CSS px of the viewport) of the webview as a PNG with
/// about `scale` output pixels per CSS pixel.
///
/// Sends `Page.captureScreenshot` with a `clip` to the top-level page.
/// Chromium multiplies `clip.scale` by the display's device pixel ratio,
/// so the clip scale sent is `scale / RasterizationScale`.
///
/// Must be called on the webview's UI thread. `done` is called exactly
/// once, later on that thread, or at once when the call cannot start.
pub fn capture(
    controller: &ICoreWebView2Controller,
    rect: Rect,
    scale: f64,
    done: impl FnOnce(Result<Shot, String>) + Send + 'static,
) {
    if let Err(e) = rect.validate(scale) {
        done(Err(e));
        return;
    }
    // The handler owns `done`; if starting fails, take it back so it
    // still runs exactly once.
    let slot: Arc<Mutex<Option<Done>>> = Arc::new(Mutex::new(Some(Box::new(done))));
    if let Err(e) = start(controller, rect, scale, &slot) {
        let taken = slot.lock().ok().and_then(|mut s| s.take());
        if let Some(done) = taken {
            done(Err(e));
        }
    }
}

fn start(
    controller: &ICoreWebView2Controller,
    rect: Rect,
    scale: f64,
    slot: &Arc<Mutex<Option<Done>>>,
) -> Result<(), String> {
    // SAFETY: `controller` is a live COM interface and this runs on its
    // UI thread, the only requirement of the getter.
    let webview = unsafe { controller.CoreWebView2() }.map_err(|e| format!("no webview: {e}"))?;
    let dpr = controller
        .cast::<ICoreWebView2Controller3>()
        .ok()
        .and_then(|c3| {
            let mut s = 0.0f64;
            // SAFETY: `c3` is live and `s` is a valid out pointer for one f64.
            unsafe { c3.RasterizationScale(&raw mut s) }
                .ok()
                .map(|()| s)
        })
        .unwrap_or(1.0);
    let params = HSTRING::from(cdp::capture_params(rect, clip_scale(scale, dpr)));
    let slot = Arc::clone(slot);
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, json: String| {
            let taken = slot.lock().ok().and_then(|mut s| s.take());
            if let Some(done) = taken {
                done(
                    hr.map_err(|e| format!("capture failed: {e}"))
                        .and_then(|()| {
                            let png = cdp::parse_result(&json)?;
                            let (width, height) = png::dimensions(&png)?;
                            Ok(Shot { png, width, height })
                        }),
                );
            }
            Ok(())
        }));
    // SAFETY: the strings and the handler outlive the call; WebView2
    // takes its own references to what it keeps.
    unsafe {
        webview.CallDevToolsProtocolMethod(
            &HSTRING::from("Page.captureScreenshot"),
            &params,
            &handler,
        )
    }
    .map_err(|e| format!("DevTools call failed: {e}"))
}
