//! Which room a folder belongs to, by path — the one matcher shared by
//! the agent API's `find_rooms_for_path` (#354) and opening a folder
//! from outside Skein (`open_request`, epic #255). Two answers to the
//! same question must not be allowed to drift, so neither caller keeps
//! a copy.
//!
//! These compare *strings*. Callers that can touch the filesystem
//! canonicalize first (`open_request` does); the matcher itself stays
//! pure so it can be table-tested without a disk.

/// Strip a Windows verbatim (`\\?\`) prefix, mirroring dunce's rule: only a
/// disk or UNC verbatim prefix is stripped. `\\?\C:\x` (or bare `\\?\C:`)
/// becomes `C:\x`, `\\?\UNC\server\share\x` becomes `\\server\share\x`,
/// **anything else is returned unchanged** — most importantly a volume-GUID
/// path like `\\?\Volume{guid}\x`, which `dunce::canonicalize` (unlike
/// `std::fs::canonicalize`) leaves verbatim because it has no non-verbatim
/// spelling at all; stripping the prefix there would turn an absolute path
/// into a relative-looking one. `dunce::canonicalize` avoids the prefix for
/// an ordinary path, but still emits it for a UNC path, one over 260
/// characters, or one hitting a reserved name — so a room whose folder
/// trips any of those reads back verbatim from disk (#393). This is a
/// deliberate addition beyond the frontend's `normalizePath` mirror: the
/// frontend never sees a verbatim path at all, since only Rust-side
/// canonicalization produces one. The `UNC` marker is matched
/// case-insensitively; Windows itself does not care about its case.
pub(crate) fn strip_verbatim(path: &str) -> std::borrow::Cow<'_, str> {
    let Some(rest) = path.strip_prefix(r"\\?\") else {
        return std::borrow::Cow::Borrowed(path);
    };
    if let Some(share) = rest
        .get(..3)
        .filter(|p| p.eq_ignore_ascii_case("UNC"))
        .and(rest.get(3..))
        .and_then(|after_unc| after_unc.strip_prefix('\\'))
    {
        return std::borrow::Cow::Owned(format!(r"\\{share}"));
    }
    let is_disk_prefix = {
        let mut chars = rest.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
            && matches!(chars.next(), Some(':'))
            && matches!(chars.next(), None | Some('\\'))
    };
    if is_disk_prefix {
        return std::borrow::Cow::Borrowed(rest);
    }
    std::borrow::Cow::Borrowed(path)
}

/// Normalize a filesystem path for comparison, mirroring the frontend's
/// `normalizePath` (`app/src/roomGroups.ts`) exactly: backslashes
/// become forward slashes, one trailing slash is stripped (never down
/// to an empty string), and the result is lowercased. Kept in
/// lock-step with that function rather than shared with it — this side
/// reads sqlite JSON, that side reads `Room[]`, and there is no module
/// to share across the FFI boundary. [`strip_verbatim`] runs first, so a
/// Windows verbatim path and its plain spelling compare equal (#393).
pub(crate) fn normalize_path_for_match(path: &str) -> String {
    let mut s = strip_verbatim(path).replace('\\', "/");
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
    use super::{normalize_path_for_match as norm, path_match_kind, strip_verbatim};

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

    #[test]
    fn normalize_strips_a_windows_verbatim_prefix() {
        assert_eq!(norm(r"\\?\UNC\Server\Share\Repo"), "//server/share/repo");
        assert_eq!(norm(r"\\?\C:\Long\Repo\"), "c:/long/repo");
        // A verbatim spelling and its plain equivalent must compare equal.
        let verbatim = norm(r"\\?\C:\Long\Repo");
        let plain = norm(r"C:\Long\Repo");
        assert_eq!(verbatim, plain);
        assert_eq!(path_match_kind(&verbatim, &plain), Some("cwd"));
        // A volume-GUID verbatim path has no non-verbatim spelling — it must
        // keep its prefix rather than becoming a relative-looking path (#393).
        assert_eq!(
            strip_verbatim(r"\\?\Volume{0000-1111}\Repo"),
            r"\\?\Volume{0000-1111}\Repo"
        );
        assert_eq!(strip_verbatim(r"\\?\C:"), "C:");
    }
}
