/// A rectangle in CSS pixels of the webview's viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A captured PNG and its size in output pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// Finite and non-empty, with a positive scale.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub(crate) fn validate(&self, scale: f64) -> Result<(), String> {
        let all = [self.x, self.y, self.width, self.height, scale];
        if all.iter().any(|v| !v.is_finite()) {
            return Err("rect and scale must be finite".into());
        }
        if self.width <= 0.0 || self.height <= 0.0 || scale <= 0.0 {
            return Err("rect and scale must be positive".into());
        }
        Ok(())
    }
}

/// CDP `clip.scale` multiplies CSS px into *device* px, so to get
/// `scale` output pixels per CSS pixel on a display whose rasterization
/// scale (device pixel ratio) is `dpr`, the clip scale is `scale / dpr`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn clip_scale(scale: f64, dpr: f64) -> f64 {
    let dpr = if dpr.is_finite() && dpr > 0.0 {
        dpr
    } else {
        1.0
    };
    scale / dpr
}

/// `WKSnapshotConfiguration.snapshotWidth` is in points and the image is
/// rendered at the window's backing scale, so for an output of
/// `rect_width * scale` pixels the width in points is that over `backing`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn snapshot_width_points(rect_width: f64, scale: f64, backing: f64) -> f64 {
    let backing = if backing.is_finite() && backing > 0.0 {
        backing
    } else {
        1.0
    };
    rect_width * scale / backing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_scale_divides_by_dpr() {
        assert!((clip_scale(1.0, 2.0) - 0.5).abs() < 1e-12);
        assert!((clip_scale(1.5, 1.5) - 1.0).abs() < 1e-12);
        assert!((clip_scale(1.0, 0.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn snapshot_width_divides_by_backing() {
        assert!((snapshot_width_points(800.0, 1.0, 2.0) - 400.0).abs() < 1e-12);
        assert!((snapshot_width_points(800.0, 2.0, 2.0) - 800.0).abs() < 1e-12);
        assert!((snapshot_width_points(800.0, 1.0, f64::NAN) - 800.0).abs() < 1e-12);
    }

    #[test]
    fn validate_rejects_bad_input() {
        let ok = Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        assert!(ok.validate(1.0).is_ok());
        assert!(ok.validate(0.0).is_err());
        assert!(ok.validate(f64::NAN).is_err());
        assert!(Rect { width: 0.0, ..ok }.validate(1.0).is_err());
        assert!(
            Rect {
                x: f64::INFINITY,
                ..ok
            }
            .validate(1.0)
            .is_err()
        );
    }
}
