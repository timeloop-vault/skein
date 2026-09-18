//! Parsing the toast `launch=`/`Arguments` string this crate writes
//! into every toast (#155): `skein-notify:<id>`. Shared by the
//! in-process banner-click handler (`toast::show_toast`) and the COM
//! activator (`activator::start_activator`), so both agree on one
//! format.

/// Extract the notification id from a toast activation string.
/// `None` for anything that isn't exactly `skein-notify:<u32>` —
/// garbage, empty, or an id that overflows `u32` — so a caller can
/// drop a stray or legacy activation instead of guessing at a
/// default target.
pub fn parse_launch(args: &str) -> Option<u32> {
    args.strip_prefix("skein-notify:")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_launch;

    #[test]
    fn parses_a_well_formed_launch_string() {
        assert_eq!(parse_launch("skein-notify:42"), Some(42));
    }

    #[test]
    fn empty_string_is_none() {
        assert_eq!(parse_launch(""), None);
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(parse_launch("not-a-launch-string"), None);
        assert_eq!(parse_launch("skein-notify:"), None);
        assert_eq!(parse_launch("skein-notify:abc"), None);
        assert_eq!(parse_launch("Skein-Notify:42"), None);
    }

    #[test]
    fn overflowing_id_is_none() {
        assert_eq!(parse_launch("skein-notify:99999999999999999999"), None);
    }

    #[test]
    fn negative_id_is_none() {
        assert_eq!(parse_launch("skein-notify:-1"), None);
    }

    #[test]
    fn max_u32_parses() {
        assert_eq!(parse_launch("skein-notify:4294967295"), Some(u32::MAX));
    }
}
