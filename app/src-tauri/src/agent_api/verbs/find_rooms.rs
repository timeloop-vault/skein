//! `find_rooms_for_path` (#354): which rooms own, contain or sit under a path.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{VerbError, VerbResult, internal};
use crate::db::Database;
use crate::git::{IdentityCheck, check_identity};
use crate::room_paths::{normalize_path_for_match, path_match_kind};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FindRoomsForPathArgs {
    pub path: String,
}

// Independent wire flags a caller reads separately, not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct RoomPathMatch {
    pub room_id: String,
    pub name: String,
    pub cwd: String,
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub archived: bool,
    /// Always exactly `archived`'s value, spelled out as its own field
    /// because "is this folder free to touch" is the question a caller
    /// actually has — an open room is never safe, whatever else is true
    /// about it, and a caller told only `archived` would have to
    /// reconstruct that itself.
    pub safe_to_remove: bool,
    /// The room is retired (#417): archived history that open-from-outside
    /// never matches. Reported only so a sweep can see it; always
    /// `safe_to_remove`.
    pub retired: bool,
    /// The room recorded a repository identity (#418) and its folder now
    /// holds a different repository, or none. Informational only: it never
    /// changes `safe_to_remove`.
    pub repo_mismatch: bool,
    /// `"cwd"` for an exact match, `"inside_room"` when the queried path
    /// sits strictly under the room's cwd, `"contains_room"` when the
    /// room's cwd sits strictly under the queried path — never a bare
    /// bool, so a caller isn't left reconstructing which folder is
    /// inside which.
    #[serde(rename = "match")]
    pub match_kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FindRoomsForPathOut {
    pub rooms: Vec<RoomPathMatch>,
    /// Rooms Skein holds but could not parse into a `Room` — a
    /// `sessions` row that failed to deserialize, or one already parked
    /// in `sessions_quarantine` (#167) — and so could not be checked
    /// against `path` at all. Non-zero means `rooms` may be missing an
    /// entry: a caller must not read that absence as permission to
    /// remove a folder.
    pub unreadable_rooms: u32,
}

/// A repository group's key for comparison: the root canonicalized
/// while it still exists, then normalized. `repoRoot` gets stored in two
/// spellings — `repo_root_for_path` returns a plain checkout's path
/// exactly as it was handed over, but a linked worktree's main checkout
/// the way libgit2 resolved it, with every symlink followed (macOS's own
/// `/var` → `/private/var` is the everyday case) — so comparing the
/// strings splits one repository into two groups. A root that no longer
/// exists keeps its stored spelling: it can't be the same folder as one
/// that does.
pub(super) fn group_key_for_match(root: &str) -> String {
    let canonical = std::fs::canonicalize(root)
        .map_or_else(|_| root.to_owned(), |p| p.to_string_lossy().into_owned());
    normalize_path_for_match(&canonical)
}

/// Every room — open or archived — whose cwd equals, sits under, or
/// contains `path` (epic #266/#275, the director-room shape).
///
/// Deliberately **not** scoped to the caller's own room, unlike every
/// other verb in this file. The caller still has to authenticate the
/// same as always; only the *answer* crosses rooms. That is a
/// considered exception, not an oversight: the bearer token guards
/// against a leaked-log replay, not against the user's own other
/// agents, and a director room that spans several projects needs
/// exactly this question — "is anyone already working in this folder?"
/// — answered across the whole install, where no single room id could
/// scope it.
pub fn find_rooms_for_path(
    db: &Database,
    args: &FindRoomsForPathArgs,
) -> VerbResult<FindRoomsForPathOut> {
    let path = args.path.trim();
    if path.is_empty() {
        return Err(VerbError::Refused(
            "find_rooms_for_path needs a path".into(),
        ));
    }
    let query = normalize_path_for_match(path);
    let rooms = db.all_rooms().map_err(internal)?;
    let unreadable_rooms = db.unreadable_room_count().map_err(internal)?;

    let mut exact = Vec::new();
    let mut overlapping = Vec::new();
    for room in &rooms {
        let Some(cwd) = room.cwd.as_deref().filter(|c| !c.is_empty()) else {
            continue;
        };
        let Some(kind) = path_match_kind(&query, &normalize_path_for_match(cwd)) else {
            continue;
        };
        let retired = room.retired.is_some();
        let archived = room.archived.is_some();
        let entry = RoomPathMatch {
            room_id: room.id.clone(),
            name: room.name.clone(),
            cwd: cwd.to_owned(),
            repo_root: room.repo_root.clone(),
            branch: room.branch.clone(),
            archived,
            safe_to_remove: archived || retired,
            retired,
            repo_mismatch: check_identity(room.repo_identity.as_ref(), Path::new(cwd))
                == IdentityCheck::Mismatch,
            match_kind: kind.to_owned(),
        };
        if kind == "cwd" {
            exact.push(entry);
        } else {
            overlapping.push(entry);
        }
    }
    exact.extend(overlapping);
    Ok(FindRoomsForPathOut {
        rooms: exact,
        unreadable_rooms,
    })
}
