//! `get_design_screenshot` and `show_design_pane` (#552): see what the
//! design pane's preview shows, and make it visible when it is not.
//!
//! The preview is a cross-origin iframe, so only the webview itself can
//! read its pixels. The frontend knows where the preview sits and whether
//! it is on screen; Rust owns the native capture (`skein-webshot`).
//!
//! | kind | args | answer |
//! |---|---|---|
//! | `design.capture_target` | `{harnessId}` | `{visible, rect?, devicePixelRatio, entry?, reason?}` |
//! | `design.show_pane` | `{harnessId}` | `{shown, switchedRoom, revealed, reason?}` |
//!
//! `rect` is in CSS px of the main webview; `reason` is one of
//! `other_room`, `hidden_tab`, `not_laid_out`, `no_pane`. Capture never
//! changes the UI: a pane that is not visible is refused with
//! `not_visible:` and the caller decides whether to `show_design_pane`.
//!
//! Error codes: `not_visible:` (refused), `too_large:` (refused),
//! `capture_failed:` (unavailable).

use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use skein_webshot::{Rect, Shot};
use tauri::Manager as _;

use super::design::{ask, caller_room, guard_write, pick_harness, with_harness_id};
use super::{DesignTargetArgs, VerbError, VerbResult};
use crate::agent_api::auth::Caller;
use crate::agent_api::state::AgentApiState;

const CAPTURE_TARGET_TIMEOUT: Duration = Duration::from_secs(5);
const SHOW_PANE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the native capture may take before the call gives up.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest image edge when the caller names none; roughly what vision
/// models downscale to anyway.
pub const DEFAULT_MAX_EDGE: u64 = 1568;
const MIN_MAX_EDGE: u64 = 256;
const MAX_MAX_EDGE: u64 = 2576;
/// A PNG larger than this is refused rather than sent.
pub const MAX_PNG_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenshotArgs {
    #[serde(default)]
    pub harness: Option<String>,
    /// Longest edge of the image in px; clamped to 256–2576.
    #[serde(default)]
    pub max_edge: Option<u64>,
}

/// Device pixels per CSS pixel to render at: the screen's own ratio, but
/// never so high that the longest edge exceeds `max_edge`.
pub fn capture_scale(
    device_pixel_ratio: f64,
    width: f64,
    height: f64,
    max_edge: Option<u64>,
) -> f64 {
    // Clamped to a few thousand, so the narrowing is lossless.
    let edge = f64::from(
        u32::try_from(
            max_edge
                .unwrap_or(DEFAULT_MAX_EDGE)
                .clamp(MIN_MAX_EDGE, MAX_MAX_EDGE),
        )
        .unwrap_or(2576),
    );
    let dpr = if device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0 {
        device_pixel_ratio
    } else {
        1.0
    };
    let longest = width.max(height);
    if longest > 0.0 {
        dpr.min(edge / longest)
    } else {
        dpr
    }
}

fn not_visible(reason: &str) -> VerbError {
    VerbError::Refused(format!(
        "not_visible: the design pane is not on screen ({reason}); call \
         show_design_pane first, then retry"
    ))
}

fn capture_failed(msg: &str) -> VerbError {
    VerbError::Unavailable(format!("capture_failed: {msg}"))
}

/// What the frontend says about where the preview is.
#[derive(Debug, PartialEq)]
pub struct Target {
    pub rect: Rect,
    pub device_pixel_ratio: f64,
    pub entry: Option<String>,
}

pub fn parse_target(answer: &Value) -> VerbResult<Target> {
    if answer.get("visible").and_then(Value::as_bool) != Some(true) {
        let reason = answer
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("hidden");
        return Err(not_visible(reason));
    }
    let num = |o: &Value, k: &str| o.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
    let rect = answer.get("rect").and_then(|r| {
        Some(Rect {
            x: num(r, "x")?,
            y: num(r, "y")?,
            width: num(r, "width")?,
            height: num(r, "height")?,
        })
    });
    let Some(rect) = rect.filter(|r| r.width >= 1.0 && r.height >= 1.0) else {
        return Err(not_visible("not_laid_out"));
    };
    Ok(Target {
        rect,
        device_pixel_ratio: answer
            .get("devicePixelRatio")
            .and_then(Value::as_f64)
            .unwrap_or(1.0),
        entry: answer
            .get("entry")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// The verb's answer. `png` is base64; the MCP layer lifts it into an
/// image block, the HTTP mirror returns it as is.
pub fn build_output(
    harness_id: &str,
    target: &Target,
    scale: f64,
    shot: &Shot,
) -> VerbResult<Value> {
    if shot.png.len() > MAX_PNG_BYTES {
        return Err(VerbError::Refused(format!(
            "too_large: the screenshot is {} bytes, over the {MAX_PNG_BYTES}-byte cap; \
             pass a smaller maxEdge",
            shot.png.len()
        )));
    }
    Ok(json!({
        "harnessId": harness_id,
        "entry": target.entry,
        "width": shot.width,
        "height": shot.height,
        "scale": scale,
        "cssRect": {
            "x": target.rect.x,
            "y": target.rect.y,
            "width": target.rect.width,
            "height": target.rect.height,
        },
        "png": base64::engine::general_purpose::STANDARD.encode(&shot.png),
    }))
}

/// Start the platform capture. Must run on the main thread (inside
/// `with_webview`).
#[allow(clippy::needless_pass_by_value)] // consumed on macOS and Windows only
fn start_capture(
    pw: tauri::webview::PlatformWebview,
    rect: Rect,
    scale: f64,
    done: impl FnOnce(Result<Shot, String>) + Send + 'static,
) {
    #[cfg(windows)]
    skein_webshot::capture(&pw.controller(), rect, scale, done);
    #[cfg(target_os = "macos")]
    skein_webshot::capture(pw.inner(), rect, scale, done);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = &pw;
        skein_webshot::capture(std::ptr::null_mut(), rect, scale, done);
    }
}

/// Capture `rect` of the main webview: hop to the main thread, wait for
/// the result on a oneshot, give up after [`CAPTURE_TIMEOUT`].
async fn grab(state: &AgentApiState, rect: Rect, scale: f64) -> Result<Shot, String> {
    let app = state
        .app
        .clone()
        .ok_or_else(|| "no window is available".to_owned())?;
    let window = app
        .get_webview_window(crate::setup::MAIN_LABEL)
        .ok_or_else(|| "the main window is gone".to_owned())?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |pw| {
            start_capture(pw, rect, scale, move |result| {
                let _ = tx.send(result);
            });
        })
        .map_err(|e| format!("could not reach the webview: {e}"))?;
    match tokio::time::timeout(CAPTURE_TIMEOUT, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("the capture was dropped".into()),
        Err(_) => Err(format!("timed out after {}s", CAPTURE_TIMEOUT.as_secs())),
    }
}

/// A PNG of what the design pane's preview shows right now. Read-only
/// towards the UI: it never shows the pane, it refuses when it is hidden.
pub async fn get_design_screenshot(
    state: &AgentApiState,
    caller: &Caller,
    args: &ScreenshotArgs,
    harness_control_enabled: bool,
) -> VerbResult<Value> {
    guard_write(
        state,
        caller,
        "get_design_screenshot",
        harness_control_enabled,
    )?;
    let room = caller_room(state, caller)?;
    let harness = pick_harness(&room, args.harness.as_deref())?;
    let answer = ask(
        state,
        "get_design_screenshot",
        caller,
        &harness.id,
        "design.capture_target",
        json!({ "roomId": room.id, "harnessId": harness.id }),
        CAPTURE_TARGET_TIMEOUT,
    )
    .await?;
    let target = parse_target(&answer)?;
    let scale = capture_scale(
        target.device_pixel_ratio,
        target.rect.width,
        target.rect.height,
        args.max_edge,
    );
    let shot = grab(state, target.rect, scale)
        .await
        .map_err(|e| capture_failed(&e))?;
    build_output(&harness.id, &target, scale, &shot)
}

/// Make the design pane visible, switching the active room and tab when
/// needed. The one verb allowed to move the user's view.
pub async fn show_design_pane(
    state: &AgentApiState,
    caller: &Caller,
    args: &DesignTargetArgs,
    harness_control_enabled: bool,
) -> VerbResult<Value> {
    guard_write(state, caller, "show_design_pane", harness_control_enabled)?;
    let room = caller_room(state, caller)?;
    let harness = pick_harness(&room, args.harness.as_deref())?;
    let answer = ask(
        state,
        "show_design_pane",
        caller,
        &harness.id,
        "design.show_pane",
        json!({ "roomId": room.id, "harnessId": harness.id }),
        SHOW_PANE_TIMEOUT,
    )
    .await?;
    with_harness_id(answer, &harness.id)
}
