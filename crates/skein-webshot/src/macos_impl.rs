use std::cell::RefCell;
use std::ffi::c_void;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

use crate::geometry::{Rect, Shot, snapshot_width_points};
use crate::png;

/// Capture `rect` (CSS px = points of the view) of a `WKWebView` as a
/// PNG with about `scale` output pixels per CSS pixel.
///
/// `snapshotWidth` is in points and the image is rendered at the
/// window's `backingScaleFactor`, so it is set to
/// `rect.width * scale / backingScaleFactor`; the height follows the
/// rect's aspect ratio. The real size is read back from the PNG.
///
/// `wk_webview` is the `WKWebView` pointer from Tauri's
/// `PlatformWebview::inner()`. Must be called on the main thread;
/// `done` runs exactly once, later on the main thread, or at once when
/// the call cannot start.
pub fn capture(
    wk_webview: *mut c_void,
    rect: Rect,
    scale: f64,
    done: impl FnOnce(Result<Shot, String>) + Send + 'static,
) {
    if let Err(e) = rect.validate(scale) {
        done(Err(e));
        return;
    }
    if wk_webview.is_null() {
        done(Err("no webview".into()));
        return;
    }
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        done(Err("screenshot must run on the main thread".into()));
        return;
    };
    // SAFETY: the caller passes the live WKWebView that Tauri's
    // `PlatformWebview::inner()` returns, and we just checked we are on
    // the main thread, so the pointer is valid for the duration of this call.
    let webview: &WKWebView = unsafe { &*wk_webview.cast::<WKWebView>() };
    let backing = webview.window().map_or(1.0, |w| w.backingScaleFactor());

    // SAFETY: plain property setters on a configuration object we own.
    let config = unsafe {
        let config = WKSnapshotConfiguration::new(mtm);
        config.setRect(CGRect::new(
            CGPoint::new(rect.x, rect.y),
            CGSize::new(rect.width, rect.height),
        ));
        let width = snapshot_width_points(rect.width, scale, backing);
        config.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width)));
        config.setAfterScreenUpdates(true);
        config
    };

    // The block is `Fn`, so `done` lives in a take-once cell.
    let slot = RefCell::new(Some(done));
    let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let Some(done) = slot.borrow_mut().take() else {
            return;
        };
        // SAFETY: WebKit passes either null or a live object for each
        // argument, valid for the duration of this callback.
        let (image, error) = unsafe { (image.as_ref(), error.as_ref()) };
        done(match image {
            Some(image) => to_shot(image),
            None => Err(error.map_or_else(
                || "snapshot failed".to_string(),
                |e| format!("snapshot failed: {}", e.localizedDescription()),
            )),
        });
    });
    // SAFETY: called on the main thread with a valid configuration; the
    // block is copied by WebKit, so it outlives this scope.
    unsafe { webview.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block) };
}

fn to_shot(image: &NSImage) -> Result<Shot, String> {
    let tiff = image.TIFFRepresentation().ok_or("snapshot has no pixels")?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("snapshot is not a bitmap")?;
    let props: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
    // SAFETY: `props` is an empty dictionary, which is valid for every
    // storage type; the generic key/value types match the declaration.
    let data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
            .ok_or("PNG encoding failed")?;
    let png = data.to_vec();
    let (width, height) = png::dimensions(&png)?;
    Ok(Shot { png, width, height })
}
