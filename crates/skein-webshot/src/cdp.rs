//! Pure parts of the `WebView2` path: request JSON and response parse.

use base64::Engine as _;

use crate::geometry::Rect;

/// Parameters for `Page.captureScreenshot`; `clip_scale` is already in
/// CDP terms (see `geometry::clip_scale`).
pub(crate) fn capture_params(rect: Rect, clip_scale: f64) -> String {
    serde_json::json!({
        "format": "png",
        "clip": {
            "x": rect.x,
            "y": rect.y,
            "width": rect.width,
            "height": rect.height,
            "scale": clip_scale,
        },
        "captureBeyondViewport": false,
    })
    .to_string()
}

/// PNG bytes out of `{"data": "<base64>"}`.
pub(crate) fn parse_result(json: &str) -> Result<Vec<u8>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("bad DevTools reply: {e}"))?;
    let data = v
        .get("data")
        .and_then(serde_json::Value::as_str)
        .ok_or("DevTools reply has no image data")?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("bad image data: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_shape() {
        let r = Rect {
            x: 1.0,
            y: 2.0,
            width: 300.0,
            height: 200.0,
        };
        let v: serde_json::Value = serde_json::from_str(&capture_params(r, 0.5)).unwrap();
        assert_eq!(v["format"], "png");
        assert_eq!(v["captureBeyondViewport"], false);
        assert_eq!(v["clip"]["x"], 1.0);
        assert_eq!(v["clip"]["width"], 300.0);
        assert_eq!(v["clip"]["scale"], 0.5);
    }

    #[test]
    fn parses_result() {
        assert_eq!(parse_result(r#"{"data":"aGk="}"#).unwrap(), b"hi");
        assert!(parse_result(r#"{"x":1}"#).is_err());
        assert!(parse_result("{").is_err());
        assert!(parse_result(r#"{"data":"!!"}"#).is_err());
    }
}
