//! Per-request device shims (#528): what the preview's query string asks
//! the served page to pretend about its device.

/// Lowest and highest `dpr` accepted; anything else is ignored.
const DPR_RANGE: (f64, f64) = (0.5, 4.0);

/// The value of `key` in a raw query string, undecoded. `dpr` is read without
/// percent-decoding and first-wins, by design: only a parsed f64 is ever used.
fn param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find_map(|(k, v)| (k == key).then_some(v))
}

/// A valid `dpr` from the query: a finite number in [0.5, 4.0].
fn parse_dpr(query: &str) -> Option<f64> {
    let n: f64 = param(query, "dpr")?.parse().ok()?;
    (n.is_finite() && (DPR_RANGE.0..=DPR_RANGE.1).contains(&n)).then_some(n)
}

/// The `(script attribute, source)` pairs the query asks for, in the order
/// they are injected. This is the one place the query maps to shims:
/// #529 and #530 add theirs here. JS only, so a shim does not reach
/// `(resolution)` media queries or `image-set`. Only parsed numbers are
/// ever interpolated, never raw query text.
pub fn device_shims(query: &str) -> Vec<(&'static str, String)> {
    let mut shims = Vec::new();
    if let Some(d) = parse_dpr(query) {
        shims.push((
            "data-skein-device",
            format!(
                "(function(){{var d={d};try{{Object.defineProperty(window,\"devicePixelRatio\",{{get:function(){{return d}},configurable:true}});}}catch(e){{}}}})();"
            ),
        ));
    }
    shims
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpr_is_parsed_only_when_valid() {
        assert_eq!(parse_dpr(""), None);
        assert_eq!(parse_dpr("v=1"), None);
        assert_eq!(parse_dpr("v=1&dpr=2"), Some(2.0));
        assert_eq!(parse_dpr("dpr=1.5"), Some(1.5));
        assert_eq!(parse_dpr("dpr=0.5"), Some(0.5));
        assert_eq!(parse_dpr("dpr=4"), Some(4.0));
        for bad in [
            "0.1",
            "9",
            "NaN",
            "inf",
            "-inf",
            "abc",
            "",
            "2;alert(1)",
            "2%3B1",
        ] {
            assert_eq!(parse_dpr(&format!("dpr={bad}")), None, "{bad}");
        }
    }

    #[test]
    fn a_shim_carries_only_the_parsed_number() {
        assert_eq!(device_shims("v=1").len(), 0);
        assert_eq!(device_shims("dpr=2;alert(1)").len(), 0);
        let s = device_shims("v=3&dpr=1.5");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, "data-skein-device");
        assert!(s[0].1.contains("var d=1.5;"));
        assert!(s[0].1.contains("devicePixelRatio"));
    }
}
