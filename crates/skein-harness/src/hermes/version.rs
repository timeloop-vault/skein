//! Parsing `hermes --version` (issue #625).
//!
//! First line, current builds:
//! `Hermes Agent v0.21.7+184.g79fe847 (2026.9.24) · stable · local 79fe8472856f`;
//! older builds omit the `+build` suffix; older still were `CalVer` tags
//! (`v2026.9.24`), which parse but are never supported.

/// The numeric part of a Hermes version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HermesVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// `(major, minor)` pairs whose hook payloads were verified. Verified on
/// v0.21.7; an unknown major/minor degrades to screen-read (#625).
pub const TELEMETRY_SUPPORTED: &[(u32, u32)] = &[(0, 21)];

/// Parse the first `Hermes Agent v…` line of `hermes --version` output.
/// Tolerates a BOM, leading whitespace and CRLF; ignores the `+build`
/// suffix and anything after the version token.
#[must_use]
pub fn parse_version(stdout: &str) -> Option<HermesVersion> {
    let rest = stdout.lines().find_map(|line| {
        line.trim_start_matches(['\u{feff}', ' ', '\t'])
            .strip_prefix("Hermes Agent v")
    })?;
    let token = rest.split(char::is_whitespace).next()?;
    let core = token.split('+').next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some(HermesVersion {
        major,
        minor,
        patch,
    })
}

/// Whether hook telemetry was verified against this version's line.
#[must_use]
pub fn telemetry_supported(v: &HermesVersion) -> bool {
    TELEMETRY_SUPPORTED.contains(&(v.major, v.minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unnecessary_wraps)]
    fn v(major: u32, minor: u32, patch: u32) -> Option<HermesVersion> {
        Some(HermesVersion {
            major,
            minor,
            patch,
        })
    }

    #[test]
    fn parses_known_shapes() {
        let cases: &[(&str, Option<HermesVersion>, bool)] = &[
            (
                "Hermes Agent v0.21.7+184.g79fe847 (2026.9.24) · stable · local 79fe8472856f\n",
                v(0, 21, 7),
                true,
            ),
            ("Hermes Agent v0.21.7 (2026.9.24)", v(0, 21, 7), true),
            ("Hermes Agent v0.22.0 (2026.10.1)\n", v(0, 22, 0), false),
            ("Hermes Agent v2026.9.24", v(2026, 9, 24), false),
            (
                "\u{feff}  Hermes Agent v0.21.1\r\nProject: x\r\n",
                v(0, 21, 1),
                true,
            ),
            (
                "Update available\nHermes Agent v0.21.7 (x)",
                v(0, 21, 7),
                true,
            ),
            ("hermes: command not found", None, false),
            ("Hermes Agent v0.21", None, false),
            ("Hermes Agent vX.Y.Z", None, false),
            ("", None, false),
        ];
        for (input, want, supported) in cases {
            let got = parse_version(input);
            assert_eq!(&got, want, "{input:?}");
            assert_eq!(got.as_ref().is_some_and(telemetry_supported), *supported);
        }
    }
}
