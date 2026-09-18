//! Parsing the toast `launch=`/`Arguments` string this crate writes
//! into every toast (#155, #294): `skein-notify:<room_id>:<harness_id>`.
//! Shared by the in-process banner-click handler
//! (`toast::show_toast`) and the COM activator
//! (`activator::start_activator`), so both agree on one format.
//!
//! Ids are the harness/room uuid-simple hex Skein already uses
//! elsewhere, but nothing here assumes that shape beyond "no `:`, no
//! whitespace, no XML-hostile characters" — the string round-trips
//! through a toast's `launch` XML attribute, so it has to survive
//! that trip even though the `XmlDocument` DOM API (see `toast.rs`)
//! already escapes attribute values for us.

/// The room + harness a toast activation should jump to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchTarget {
    pub room_id: String,
    pub harness_id: String,
}

impl LaunchTarget {
    /// Build a target, rejecting either id if it's empty or would not
    /// round-trip through [`parse_launch`] (a `:`, whitespace, or an
    /// XML-hostile character).
    #[must_use]
    pub fn new(room_id: impl Into<String>, harness_id: impl Into<String>) -> Option<Self> {
        let room_id = room_id.into();
        let harness_id = harness_id.into();
        if is_valid_id(&room_id) && is_valid_id(&harness_id) {
            Some(Self {
                room_id,
                harness_id,
            })
        } else {
            None
        }
    }

    /// Encode as the `launch=`/`Arguments` string [`parse_launch`]
    /// decodes back into this same target.
    #[must_use]
    pub fn encode(&self) -> String {
        format!("skein-notify:{}:{}", self.room_id, self.harness_id)
    }
}

/// An id is safe to embed in a `:`-delimited launch string and in a
/// toast's XML `launch` attribute if it's non-empty and free of `:`
/// (the delimiter), whitespace, and the handful of characters that are
/// meaningful in XML markup.
fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| !c.is_whitespace() && !matches!(c, ':' | '<' | '>' | '&' | '\'' | '"'))
}

/// Extract the launch target from a toast activation string. `None`
/// for anything that isn't exactly `skein-notify:<room_id>:<harness_id>`
/// with both ids valid per [`LaunchTarget::new`] — garbage, empty
/// parts, a missing prefix, extra `:` separators, or (deliberately)
/// the pre-#294 numeric `skein-notify:<id>` format, so a stale toast
/// left over from before this upgrade just raises the window with no
/// target rather than guessing one.
#[must_use]
pub fn parse_launch(args: &str) -> Option<LaunchTarget> {
    let rest = args.strip_prefix("skein-notify:")?;
    let mut parts = rest.split(':');
    let room_id = parts.next()?;
    let harness_id = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    LaunchTarget::new(room_id, harness_id)
}

#[cfg(test)]
mod tests {
    use super::{LaunchTarget, parse_launch};

    #[test]
    fn round_trips_a_well_formed_target() {
        let target = LaunchTarget::new("room1", "harness2").unwrap();
        assert_eq!(target.encode(), "skein-notify:room1:harness2");
        assert_eq!(parse_launch(&target.encode()), Some(target));
    }

    #[test]
    fn old_numeric_format_is_none() {
        assert_eq!(parse_launch("skein-notify:42"), None);
    }

    #[test]
    fn empty_string_is_none() {
        assert_eq!(parse_launch(""), None);
    }

    #[test]
    fn missing_prefix_is_none() {
        assert_eq!(parse_launch("not-a-launch-string"), None);
        assert_eq!(parse_launch("Skein-Notify:room:harness"), None);
    }

    #[test]
    fn empty_parts_are_none() {
        assert_eq!(parse_launch("skein-notify::"), None);
        assert_eq!(parse_launch("skein-notify:room:"), None);
        assert_eq!(parse_launch("skein-notify::harness"), None);
    }

    #[test]
    fn extra_colon_is_none() {
        assert_eq!(parse_launch("skein-notify:room:harness:extra"), None);
    }

    #[test]
    fn garbage_ids_are_none() {
        assert_eq!(parse_launch("skein-notify:"), None);
        assert_eq!(parse_launch("skein-notify:room id:harness"), None);
        assert_eq!(parse_launch("skein-notify:room<:harness"), None);
        assert_eq!(parse_launch("skein-notify:room&:harness"), None);
    }

    #[test]
    fn new_rejects_invalid_ids() {
        assert_eq!(LaunchTarget::new("", "harness"), None);
        assert_eq!(LaunchTarget::new("room", ""), None);
        assert_eq!(LaunchTarget::new("ro:om", "harness"), None);
        assert_eq!(LaunchTarget::new("room", "harness with space"), None);
    }
}
