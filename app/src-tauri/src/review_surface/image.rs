//! One side of one file's raw bytes, for the review pane's image
//! preview (#409).
//!
//! [`image_bytes_impl`] is the read path behind the `review_image_bytes`
//! command: given a scope, resolve which git blob or disk file is the
//! "old" or "new" side of an image, and hand back its bytes (or a
//! reason there are none). It reuses exactly the resolution
//! `query::file_impl` already relies on for the same three scopes — the
//! merge-base for `branch`, the first parent for `commit`, the #211
//! baseline for `pending` — rather than growing a second notion of what
//! a scope covers.
//!
//! Every error is a plain string the frontend switches on: `"missing"`
//! (that side doesn't exist — an added or deleted file), `"notimage"`,
//! `"toolarge:<len>"` (over the cap — [`skein_review::MAX_IMAGE_BYTES`]
//! at the command boundary; threaded through as `max_bytes` so tests can
//! exercise the cap without an 16 MiB fixture), or `"unavailable"` (the
//! pending old side, when Skein genuinely never kept the bytes).
//! Anything else is a free-form failure (no repo, no `commit_sha`, an
//! unrecognised `side`).
//!
//! # The pending old side
//!
//! A `review_baselines` row's own `content` column only ever holds text
//! (SVG included — it is UTF-8). A `Binary`/`TooLarge` baseline keeps
//! nothing but a digest or a length: enough to detect a change, not
//! enough to render one. #409 adds `review_baseline_images` as a
//! sibling table for exactly that gap — [`crate::review`]'s baseline
//! writers mirror the raw bytes there whenever a baseline lands on an
//! image path and fits under [`skein_review::MAX_IMAGE_BYTES`]. The
//! lookup here tries that mirror first, then falls back to HEAD's own
//! blob when its classification still matches what the baseline
//! recorded exactly — the case of a baseline captured before #409
//! shipped, or one whose bytes were over the cap at capture time but
//! whose HEAD content now fits. Anything else reads as `"unavailable"`
//! rather than guessing.

use std::path::Path;

use skein_git::{CappedBlob, Repo};
use skein_review::FileState;

use super::Scope;
use super::git::resolve_range;
use crate::db::Database;
use crate::review::{abs_path, relative_key};

fn open_repo(cwd: &str) -> Result<Repo, String> {
    Repo::open(Path::new(cwd)).map_err(|e| e.to_string())
}

/// A disk file's bytes, capped the same way `fs::read_image_bytes` caps
/// the Files editor's reads. `"missing"` covers both "no such file" and
/// any other stat failure — the caller already knows `key` is a known
/// image extension, so the only honest reasons left are "not there" and
/// "too big".
fn read_disk_image(cwd: &str, key: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let abs = abs_path(cwd, key);
    let meta = std::fs::metadata(&abs).map_err(|_| "missing".to_string())?;
    if meta.len() > max_bytes {
        return Err(format!("toolarge:{}", meta.len()));
    }
    std::fs::read(&abs).map_err(|e| format!("read: {e}"))
}

fn blob_image(repo: &Repo, rev: &str, key: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    match repo
        .blob_at_capped(rev, key, max_bytes)
        .map_err(|e| e.to_string())?
    {
        CappedBlob::Missing => Err("missing".to_string()),
        CappedBlob::TooLarge(len) => Err(format!("toolarge:{len}")),
        CappedBlob::Content(bytes) => Ok(bytes),
    }
}

fn new_side(
    cwd: &str,
    key: &str,
    scope: Scope,
    commit_sha: Option<&str>,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    match scope {
        // branch's new side is the working tree (the scope counts
        // committed and uncommitted work together); pending's new side
        // is the working tree by definition.
        Scope::Branch | Scope::Pending => read_disk_image(cwd, key, max_bytes),
        Scope::Commit => {
            let sha = commit_sha
                .ok_or_else(|| "commit_sha is required for the commit scope".to_string())?;
            let repo = open_repo(cwd)?;
            blob_image(&repo, sha, key, max_bytes)
        }
    }
}

fn old_side(
    db: &Database,
    room_id: &str,
    cwd: &str,
    key: &str,
    scope: Scope,
    commit_sha: Option<&str>,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    match scope {
        Scope::Branch => {
            let repo = open_repo(cwd)?;
            let range = resolve_range(db, room_id, &repo)?;
            let Some(base_sha) = range.base_sha else {
                // No resolvable base — `scope_diffs` treats the branch
                // as wholly added against the empty tree, so there is
                // no old side either.
                return Err("missing".to_string());
            };
            blob_image(&repo, &base_sha, key, max_bytes)
        }
        Scope::Commit => {
            let sha = commit_sha
                .ok_or_else(|| "commit_sha is required for the commit scope".to_string())?;
            let repo = open_repo(cwd)?;
            match repo.first_parent(sha).map_err(|e| e.to_string())? {
                Some(parent) => blob_image(&repo, &parent, key, max_bytes),
                // A root commit, or a sha that doesn't resolve at all —
                // either way there is nothing before it.
                None => Err("missing".to_string()),
            }
        }
        Scope::Pending => pending_old(db, room_id, cwd, key, max_bytes),
    }
}

fn pending_old(
    db: &Database,
    room_id: &str,
    cwd: &str,
    key: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let Some(row) = db.review_baseline(room_id, key)? else {
        return Err("missing".to_string());
    };
    let state = skein_review::decode(&row.kind, row.content.as_deref());
    match &state {
        FileState::Missing => return Err("missing".to_string()),
        // A Text baseline (SVG included) already carries its bytes.
        FileState::Text(text) => return Ok(text.clone().into_bytes()),
        _ => {}
    }

    if let Some(bytes) = db.review_baseline_image(room_id, key)? {
        return if u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= max_bytes {
            Ok(bytes)
        } else {
            Err(format!("toolarge:{}", bytes.len()))
        };
    }

    // No mirror kept (captured before #409, or over the cap at capture
    // time) — HEAD's own blob still works if it classifies to exactly
    // the state the baseline recorded.
    if let Ok(repo) = open_repo(cwd) {
        if let Ok(Some(head_bytes)) = repo.head_blob(key) {
            if skein_review::classify_bytes(&head_bytes) == state {
                return if u64::try_from(head_bytes.len()).unwrap_or(u64::MAX) <= max_bytes {
                    Ok(head_bytes)
                } else {
                    Err(format!("toolarge:{}", head_bytes.len()))
                };
            }
        }
    }
    Err("unavailable".to_string())
}

/// One side of one image file, for `scope` — see the module docs.
/// `max_bytes` is [`skein_review::MAX_IMAGE_BYTES`] at the command
/// boundary; a parameter here so tests can exercise the cap cheaply.
#[allow(clippy::too_many_arguments)]
pub(super) fn image_bytes_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    path: &str,
    scope: Scope,
    commit_sha: Option<&str>,
    side: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let key =
        relative_key(cwd, path).ok_or_else(|| "path is outside the room's worktree".to_string())?;
    if !skein_review::is_image_path(&key) {
        return Err("notimage".to_string());
    }
    match side {
        "new" => new_side(cwd, &key, scope, commit_sha, max_bytes),
        "old" => old_side(db, room_id, cwd, &key, scope, commit_sha, max_bytes),
        other => Err(format!("unknown side: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::db::Database;

    const PNG_A: [u8; 8] = [0x89, b'P', b'N', b'G', 1, 2, 3, 4];
    const PNG_B: [u8; 8] = [0x89, b'P', b'N', b'G', 5, 6, 7, 8];
    const CAP: u64 = skein_review::MAX_IMAGE_BYTES;

    struct Room {
        _dir: TempDir,
        cwd: String,
        db: Database,
    }

    impl Room {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let cwd = dir.path().to_string_lossy().to_string();
            git2::Repository::init(dir.path()).unwrap();
            let db = Database::open(&dir.path().join("skein.db")).unwrap();
            Self { _dir: dir, cwd, db }
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let p = Path::new(&self.cwd).join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, bytes).unwrap();
        }

        /// Branch off HEAD and check it out — what the branch-scope
        /// tests need so `default_base_branch` has something to resolve
        /// to other than the room's own current branch (which it never
        /// returns — reviewing a branch against itself is empty).
        fn checkout_new_branch(&self, name: &str) {
            let repo = git2::Repository::open(&self.cwd).unwrap();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.branch(name, &head, false).unwrap();
            repo.set_head(&format!("refs/heads/{name}")).unwrap();
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
                .unwrap();
        }

        fn commit(&self, msg: &str) -> String {
            let repo = git2::Repository::open(&self.cwd).unwrap();
            let mut index = repo.index().unwrap();
            index
                .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("t", "t@example.com").unwrap();
            let parents = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_commit().ok())
                .map(|c| vec![c])
                .unwrap_or_default();
            let refs: Vec<&git2::Commit> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs)
                .unwrap()
                .to_string()
        }

        fn side(
            &self,
            path: &str,
            scope: Scope,
            commit_sha: Option<&str>,
            side: &str,
        ) -> Result<Vec<u8>, String> {
            self.side_capped(path, scope, commit_sha, side, CAP)
        }

        fn side_capped(
            &self,
            path: &str,
            scope: Scope,
            commit_sha: Option<&str>,
            side: &str,
            max_bytes: u64,
        ) -> Result<Vec<u8>, String> {
            image_bytes_impl(
                &self.db, "r1", &self.cwd, path, scope, commit_sha, side, max_bytes,
            )
        }
    }

    #[test]
    fn refuses_a_non_image_path() {
        let room = Room::new();
        room.write("notes.txt", b"hi");
        assert_eq!(
            room.side("notes.txt", Scope::Branch, None, "new")
                .unwrap_err(),
            "notimage"
        );
    }

    #[test]
    fn refuses_traversal_and_paths_outside_the_worktree() {
        let room = Room::new();
        let err = room
            .side("../escape.png", Scope::Branch, None, "new")
            .unwrap_err();
        assert!(!err.is_empty());
        assert_ne!(err, "notimage");
    }

    #[test]
    fn refuses_an_unknown_side() {
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        assert_eq!(
            room.side("shot.png", Scope::Branch, None, "sideways")
                .unwrap_err(),
            "unknown side: sideways"
        );
    }

    // ── branch ────────────────────────────────────────────────────

    #[test]
    fn branch_old_is_the_merge_base_blob_and_new_is_disk() {
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        room.commit("base");
        room.checkout_new_branch("feature");
        room.write("shot.png", &PNG_B);
        room.commit("committed on feature"); // branch counts committed work too
        room.write("shot.png", &PNG_B); // and uncommitted — write it again for clarity

        assert_eq!(
            room.side("shot.png", Scope::Branch, None, "old").unwrap(),
            PNG_A
        );
        assert_eq!(
            room.side("shot.png", Scope::Branch, None, "new").unwrap(),
            PNG_B
        );
    }

    #[test]
    fn branch_old_is_missing_for_a_file_added_since_the_base() {
        let room = Room::new();
        room.write("keep.txt", b"anchor");
        room.commit("base");
        room.checkout_new_branch("feature");
        room.write("shot.png", &PNG_A);

        assert_eq!(
            room.side("shot.png", Scope::Branch, None, "old")
                .unwrap_err(),
            "missing"
        );
    }

    #[test]
    fn branch_new_over_the_cap_reports_toolarge_with_the_length() {
        let room = Room::new();
        room.write("keep.txt", b"anchor");
        room.commit("base");
        room.write("shot.png", &PNG_A); // 8 bytes
        assert_eq!(
            room.side_capped("shot.png", Scope::Branch, None, "new", 4)
                .unwrap_err(),
            "toolarge:8"
        );
    }

    // ── commit ────────────────────────────────────────────────────

    #[test]
    fn commit_scope_reads_the_commit_and_its_first_parent() {
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        room.commit("first");
        room.write("shot.png", &PNG_B);
        let second = room.commit("second");

        assert_eq!(
            room.side("shot.png", Scope::Commit, Some(&second), "old")
                .unwrap(),
            PNG_A
        );
        assert_eq!(
            room.side("shot.png", Scope::Commit, Some(&second), "new")
                .unwrap(),
            PNG_B
        );
    }

    #[test]
    fn commit_scope_root_commit_has_no_old_side() {
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        let sha = room.commit("root");

        assert_eq!(
            room.side("shot.png", Scope::Commit, Some(&sha), "old")
                .unwrap_err(),
            "missing"
        );
        assert_eq!(
            room.side("shot.png", Scope::Commit, Some(&sha), "new")
                .unwrap(),
            PNG_A
        );
    }

    #[test]
    fn commit_scope_requires_a_commit_sha() {
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        room.commit("only");
        let err = room
            .side("shot.png", Scope::Commit, None, "new")
            .unwrap_err();
        assert!(err.contains("commit_sha"));
    }

    #[test]
    fn commit_scope_deleted_file_has_no_new_side() {
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        room.commit("added");
        fs::remove_file(Path::new(&room.cwd).join("shot.png")).unwrap();
        let deleted = room.commit("removed");

        assert_eq!(
            room.side("shot.png", Scope::Commit, Some(&deleted), "new")
                .unwrap_err(),
            "missing"
        );
        assert_eq!(
            room.side("shot.png", Scope::Commit, Some(&deleted), "old")
                .unwrap(),
            PNG_A
        );
    }

    #[test]
    fn commit_scope_over_the_cap_reports_toolarge_without_the_full_blob() {
        let room = Room::new();
        room.write("shot.png", &PNG_A); // 8 bytes
        let only = room.commit("only");
        assert_eq!(
            room.side_capped("shot.png", Scope::Commit, Some(&only), "new", 4)
                .unwrap_err(),
            "toolarge:8"
        );
    }

    // ── pending ───────────────────────────────────────────────────

    #[test]
    fn pending_old_reads_the_mirrored_baseline_bytes() {
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        room.commit("base");
        room.db
            .insert_review_baseline_if_absent("r1", "shot.png", "binary", Some("deadbeef"), "h1", 1)
            .unwrap();
        room.db
            .set_review_baseline_image("r1", "shot.png", Some(&PNG_A))
            .unwrap();
        room.write("shot.png", &PNG_B);

        assert_eq!(
            room.side("shot.png", Scope::Pending, None, "old").unwrap(),
            PNG_A
        );
        assert_eq!(
            room.side("shot.png", Scope::Pending, None, "new").unwrap(),
            PNG_B
        );
    }

    #[test]
    fn pending_old_falls_back_to_a_matching_head_blob() {
        // A baseline row with no mirror (captured before #409, say) —
        // HEAD's own blob still classifies to the same digest, so it
        // stands in.
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        room.commit("base");
        let digest = match skein_review::classify_bytes(&PNG_A) {
            FileState::Binary { digest } => format!("{digest:x}"),
            _ => unreachable!("PNG bytes classify as binary"),
        };
        room.db
            .insert_review_baseline_if_absent("r1", "shot.png", "binary", Some(&digest), "h1", 1)
            .unwrap();
        room.write("shot.png", &PNG_B);

        assert_eq!(
            room.side("shot.png", Scope::Pending, None, "old").unwrap(),
            PNG_A
        );
    }

    #[test]
    fn pending_old_is_unavailable_with_no_mirror_and_no_matching_head() {
        let room = Room::new();
        room.write("shot.png", &PNG_B); // HEAD never had this content
        room.commit("unrelated head");
        room.db
            .insert_review_baseline_if_absent(
                "r1",
                "shot.png",
                "binary",
                Some("0000000000000000"),
                "h1",
                1,
            )
            .unwrap();

        assert_eq!(
            room.side("shot.png", Scope::Pending, None, "old")
                .unwrap_err(),
            "unavailable"
        );
    }

    #[test]
    fn pending_old_is_missing_with_no_baseline_at_all() {
        let room = Room::new();
        room.write("shot.png", &PNG_A);
        assert_eq!(
            room.side("shot.png", Scope::Pending, None, "old")
                .unwrap_err(),
            "missing"
        );
    }

    #[test]
    fn pending_old_svg_reads_straight_from_the_text_baseline() {
        let room = Room::new();
        let svg = "<svg>old</svg>";
        room.db
            .insert_review_baseline_if_absent("r1", "icon.svg", "text", Some(svg), "h1", 1)
            .unwrap();
        room.write("icon.svg", b"<svg>new</svg>");

        assert_eq!(
            room.side("icon.svg", Scope::Pending, None, "old").unwrap(),
            svg.as_bytes()
        );
    }

    #[test]
    fn pending_old_mirror_over_the_cap_reports_toolarge() {
        let room = Room::new();
        room.db
            .insert_review_baseline_if_absent("r1", "shot.png", "binary", Some("deadbeef"), "h1", 1)
            .unwrap();
        room.db
            .set_review_baseline_image("r1", "shot.png", Some(&PNG_A)) // 8 bytes
            .unwrap();

        assert_eq!(
            room.side_capped("shot.png", Scope::Pending, None, "old", 4)
                .unwrap_err(),
            "toolarge:8"
        );
    }
}
