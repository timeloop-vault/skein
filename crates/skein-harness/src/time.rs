//! Timestamp parsing for the harness stores. Claude writes ISO 8601
//! UTC strings (`2026-05-15T21:16:22.572Z`); opencode writes epoch
//! milliseconds directly. The surface is narrow and stable, so we
//! parse it by hand rather than pull in a date crate.

/// ISO 8601 UTC → epoch milliseconds. Accepts `YYYY-MM-DDTHH:MM:SSZ`
/// with an optional fractional-seconds part (any number of digits;
/// only the first three count). `None` for anything else.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
pub fn parse_iso8601_ms(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 20 || bytes[10] != b'T' || *bytes.last().unwrap_or(&b'_') != b'Z' {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    let second: i64 = s.get(17..19)?.parse().ok()?;
    let millis: i64 = if bytes.get(19) == Some(&b'.') {
        // Up to 3 fractional digits, zero-padded on the right.
        let frac_end = s.len() - 1; // strip trailing Z
        let frac = s.get(20..frac_end)?;
        let mut padded = String::with_capacity(3);
        padded.push_str(frac);
        while padded.len() < 3 {
            padded.push('0');
        }
        padded.get(..3)?.parse().ok()?
    } else {
        0
    };
    Some(
        days_from_civil(year, month, day) * 86_400_000
            + hour * 3_600_000
            + minute * 60_000
            + second * 1_000
            + millis,
    )
}

/// Howard Hinnant's date algorithm — days since 1970-01-01. Exact
/// for the proleptic Gregorian calendar over the whole i64 range.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-05-15T21:16:22.572Z: 20588 days since the epoch
    /// (`1_778_803_200_000` ms) + 21h16m22.572s (`76_582_572` ms).
    const TS_572: i64 = 1_778_879_782_572;

    #[test]
    fn parses_iso8601_with_millis() {
        assert_eq!(parse_iso8601_ms("2026-05-15T21:16:22.572Z"), Some(TS_572));
    }

    #[test]
    fn parses_iso8601_without_millis() {
        let with = parse_iso8601_ms("2026-05-15T21:16:22.000Z").unwrap();
        let without = parse_iso8601_ms("2026-05-15T21:16:22Z").unwrap();
        assert_eq!(with, without);
    }

    #[test]
    fn pads_short_fractions_and_truncates_long_ones() {
        assert_eq!(
            parse_iso8601_ms("2026-05-15T21:16:22.5Z"),
            Some(TS_572 - 72)
        );
        assert_eq!(
            parse_iso8601_ms("2026-05-15T21:16:22.572999Z"),
            Some(TS_572)
        );
    }

    #[test]
    fn rejects_invalid_timestamp_returns_none() {
        assert_eq!(parse_iso8601_ms("not a timestamp"), None);
        assert_eq!(parse_iso8601_ms("2026-05-15"), None);
        assert_eq!(parse_iso8601_ms(""), None);
    }

    #[test]
    fn epoch_day_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }
}
