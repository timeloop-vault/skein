//! Clickable links to a room or a harness (#812), built with the running
//! build's deep-link scheme (`skein`, `skein-dev`, `skein-local`).
//!
//! Shapes: `<scheme>://room/<roomId>` and
//! `<scheme>://room/<roomId>?harness=<harnessId>`. Ids are minted in
//! `[A-Za-z0-9_-]`; anything else is percent-encoded and the app's parser
//! will refuse it, which is the safe direction.

use std::fmt::Write as _;

use super::state::AgentApiState;

/// Everything outside `[A-Za-z0-9_-]` becomes `%XX`.
fn encode(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The link to a room.
pub fn room_url(scheme: &str, room_id: &str) -> String {
    format!("{scheme}://room/{}", encode(room_id))
}

/// The link to one harness of a room.
pub fn harness_url(scheme: &str, room_id: &str, harness_id: &str) -> String {
    format!(
        "{}?harness={}",
        room_url(scheme, room_id),
        encode(harness_id)
    )
}

impl AgentApiState {
    /// The room's link, or `None` when no scheme is configured.
    pub fn room_link(&self, room_id: &str) -> Option<String> {
        self.link_scheme.as_deref().map(|s| room_url(s, room_id))
    }

    /// The harness's link, or `None` when no scheme is configured.
    pub fn harness_link(&self, room_id: &str, harness_id: &str) -> Option<String> {
        self.link_scheme
            .as_deref()
            .map(|s| harness_url(s, room_id, harness_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_link_has_the_scheme_and_id() {
        assert_eq!(room_url("skein-dev", "r-1_a"), "skein-dev://room/r-1_a");
    }

    #[test]
    fn harness_link_names_the_harness_in_the_query() {
        assert_eq!(
            harness_url("skein", "r1", "h-2"),
            "skein://room/r1?harness=h-2"
        );
    }

    #[test]
    fn unsafe_characters_are_percent_encoded() {
        assert_eq!(
            harness_url("skein", "a b/c", "d&e=f"),
            "skein://room/a%20b%2Fc?harness=d%26e%3Df"
        );
    }

    #[test]
    fn dot_and_tilde_are_encoded() {
        assert_eq!(room_url("skein", "a.b~c"), "skein://room/a%2Eb%7Ec");
    }
}
