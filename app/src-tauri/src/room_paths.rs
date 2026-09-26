//! Which room a folder belongs to, by path — the one matcher shared by
//! the agent API's `find_rooms_for_path` (#354) and opening a folder
//! from outside Skein (`open_request`, epic #255). Two answers to the
//! same question must not be allowed to drift, so neither caller keeps
//! a copy.
//!
//! These compare *strings*. Callers that can touch the filesystem
//! canonicalize first (`open_request` does); the matcher itself stays
//! pure so it can be table-tested without a disk.

/// Normalize a filesystem path for comparison, mirroring the frontend's
/// `normalizePath` (`app/src/roomGroups.ts`) exactly: backslashes
/// become forward slashes, one trailing slash is stripped (never down
/// to an empty string), and the result is lowercased. Kept in
/// lock-step with that function rather than shared with it — this side
/// reads sqlite JSON, that side reads `Room[]`, and there is no module
/// to share across the FFI boundary.
pub(crate) fn normalize_path_for_match(path: &str) -> String {
    let mut s = path.replace('\\', "/");
    if s.chars().count() > 1 && s.ends_with('/') {
        s.pop();
    }
    s.to_lowercase()
}

/// Whether normalized `a` sits under normalized `b` as a strict
/// descendant, on path-segment boundaries — `/a/foo` must never match
/// under `/a/foobar`. `b` already ending in `/` (a normalized root)
/// is not given a second one.
fn is_strictly_under(a: &str, b: &str) -> bool {
    let prefix = if b.ends_with('/') {
        b.to_owned()
    } else {
        format!("{b}/")
    };
    a.starts_with(&prefix)
}

/// `"cwd"` for an exact match, `"inside_room"` when the query sits
/// strictly under the room's cwd (the query is somewhere inside the
/// room), `"contains_room"` when the room's cwd sits strictly under the
/// query (the query is a parent folder of the room), `None` otherwise.
pub(crate) fn path_match_kind(query_norm: &str, cwd_norm: &str) -> Option<&'static str> {
    if query_norm == cwd_norm {
        Some("cwd")
    } else if is_strictly_under(query_norm, cwd_norm) {
        Some("inside_room")
    } else if is_strictly_under(cwd_norm, query_norm) {
        Some("contains_room")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_path_for_match as norm, path_match_kind};

    #[test]
    fn normalize_unifies_separators_case_and_one_trailing_slash() {
        assert_eq!(norm(r"C:\Users\Me\Repo\"), "c:/users/me/repo");
        assert_eq!(norm("/Users/me/Repo/"), "/users/me/repo");
        // A bare root keeps its only character.
        assert_eq!(norm("/"), "/");
    }

    #[test]
    fn match_kind_respects_segment_boundaries() {
        let cases = [
            ("/a/foo", "/a/foo", Some("cwd")),
            ("/a/foo/src", "/a/foo", Some("inside_room")),
            ("/a", "/a/foo", Some("contains_room")),
            // The whole point of the boundary check.
            ("/a/foobar", "/a/foo", None),
            ("/a/foo", "/a/foobar", None),
            // A root cwd contains everything below it without a `//`.
            ("/a", "/", Some("inside_room")),
        ];
        for (query, cwd, want) in cases {
            assert_eq!(path_match_kind(query, cwd), want, "{query} vs {cwd}");
        }
    }
}
