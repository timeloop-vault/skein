use super::*;
use std::fs;
use tempfile::TempDir;

// ── path keys ─────────────────────────────────────────────────

/// An absolute-path prefix for this platform. `C:/…` is not absolute
/// on Unix, so a Windows-only literal would be joined under the
/// worktree and accepted there (#281).
const ABS: &str = if cfg!(windows) { "C:" } else { "" };

#[test]
fn relative_key_normalizes_separators_and_scopes_to_the_worktree() {
    let cwd = format!("{ABS}/git/skein-wt/room");
    let cwd = cwd.as_str();
    #[cfg(windows)]
    assert_eq!(
        relative_key(cwd, r"C:\git\skein-wt\room\src\a.rs").as_deref(),
        Some("src/a.rs")
    );
    assert_eq!(
        relative_key(cwd, &format!("{ABS}/git/skein-wt/room/src/a.rs")).as_deref(),
        Some("src/a.rs")
    );
    // Already relative — interpreted against the worktree.
    assert_eq!(relative_key(cwd, "src/a.rs").as_deref(), Some("src/a.rs"));
}

#[test]
fn relative_key_refuses_anything_outside_the_worktree() {
    let cwd = format!("{ABS}/git/skein-wt/room");
    let cwd = cwd.as_str();
    // A sibling directory that merely shares a prefix.
    assert_eq!(
        relative_key(cwd, &format!("{ABS}/git/skein-wt/room-other/a.rs")),
        None
    );
    // Somewhere else entirely — harnesses do edit global config.
    assert_eq!(
        relative_key(cwd, &format!("{ABS}/Users/x/.claude/settings.json")),
        None
    );
    // Traversal, however it is spelled.
    assert_eq!(relative_key(cwd, "../escape.rs"), None);
    assert_eq!(
        relative_key(cwd, &format!("{ABS}/git/skein-wt/room/../escape.rs")),
        None
    );
    // The worktree root itself is not a file.
    assert_eq!(relative_key(cwd, &format!("{ABS}/git/skein-wt/room")), None);
}

#[cfg(windows)]
#[test]
fn relative_key_is_case_insensitive_on_windows() {
    assert_eq!(
        relative_key("C:/Git/Room", "c:/git/room/src/a.rs").as_deref(),
        Some("src/a.rs")
    );
}

// ── payload parsing ───────────────────────────────────────────

#[test]
fn patch_targets_reads_the_shapes_both_adapters_emit() {
    assert_eq!(
        patch_targets(r#"{"tool":"Edit","files":["/w/a.rs"]}"#),
        ["/w/a.rs"]
    );
    assert_eq!(
        patch_targets(r#"{"tool":"write","input":{"filePath":"/w/b.rs"}}"#),
        ["/w/b.rs"]
    );
    assert_eq!(
        patch_targets(r#"{"tool":"MultiEdit","input":{"file_path":"/w/c.rs"}}"#),
        ["/w/c.rs"]
    );
}

#[test]
fn patch_targets_ignores_errors_and_the_opencode_commit_snapshot() {
    // Errored patch — nothing landed on disk.
    assert!(patch_targets(r#"{"tool":"Edit","files":["/w/a.rs"],"is_error":true}"#).is_empty());
    // opencode's multi-file commit snapshot: no tool, so it must
    // not own a baseline (same exclusion as diff.ts's patchInfo).
    assert!(patch_targets(r#"{"files":["/w/a.rs","/w/b.rs"],"hash":"abc"}"#).is_empty());
    assert!(patch_targets(r#"{"tool":"Read","files":["/w/a.rs"]}"#).is_empty());
    assert!(patch_targets("not json").is_empty());
}

// ── fixtures ──────────────────────────────────────────────────

struct Room {
    _dir: TempDir,
    cwd: String,
    db: Database,
}

impl Room {
    /// A git room with `src/a.rs` committed.
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let repo = git2::Repository::init(dir.path()).unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.rs"), "one\ntwo\nthree\n").unwrap();
        commit(&repo, "init");
        let db = Database::open(&dir.path().join("skein.db")).unwrap();
        Self { _dir: dir, cwd, db }
    }

    /// A room that is not a git repo at all.
    fn bare() -> Self {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let db = Database::open(&dir.path().join("skein.db")).unwrap();
        Self { _dir: dir, cwd, db }
    }

    fn path(&self, rel: &str) -> PathBuf {
        abs_path(&self.cwd, rel)
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    /// What the adapters do: a live patch row naming `rel`.
    fn touched(&self, harness: &str, rel: &str) {
        let quoted = serde_json::to_string(&self.path(rel).to_string_lossy().to_string()).unwrap();
        let payload = format!("{{\"tool\":\"Edit\",\"files\":[{quoted}]}}");
        note_patch(&self.db, "r1", &self.cwd, harness, &payload);
    }

    fn pending(&self) -> Vec<PendingFileDto> {
        pending_impl(&self.db, "r1", &self.cwd).unwrap()
    }

    /// The #409 image mirror for `rel`, if any.
    fn image(&self, rel: &str) -> Option<Vec<u8>> {
        self.db.review_baseline_image("r1", rel).unwrap()
    }

    fn commit_all(&self, msg: &str) {
        let repo = git2::Repository::open(&self.cwd).unwrap();
        commit(&repo, msg);
    }

    fn numbered(&self, rel: &str, replacements: &[(usize, &str)]) -> String {
        let mut lines: Vec<String> = (1..=20).map(|i| format!("{i}\n")).collect();
        for (i, text) in replacements {
            lines[*i] = (*text).to_string();
        }
        let joined = lines.concat();
        self.write(rel, &joined);
        joined
    }

    /// What the watcher reports: absolute paths, the shape
    /// `event.path` comes in as.
    fn discover(&self, rels: &[&str]) -> usize {
        discover(
            &self.db,
            "r1",
            &self.cwd,
            rels.iter()
                .map(|r| self.path(r).to_string_lossy().to_string()),
        )
    }

    fn catch_up(&self) -> usize {
        catch_up(&self.db, "r1", &self.cwd)
    }
}

fn commit(repo: &git2::Repository, msg: &str) {
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
        .unwrap();
}

// ── capture ───────────────────────────────────────────────────

#[test]
fn first_touch_baselines_the_committed_content_not_what_is_on_disk() {
    let room = Room::new();
    // The agent edits, and only then does the patch row reach us.
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");

    let pending = room.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].path, "src/a.rs");
    assert_eq!(pending[0].change, "modified");
    assert_eq!((pending[0].additions, pending[0].deletions), (1, 1));
    assert_eq!(pending[0].harness_id, "h1");
}

#[test]
fn work_stays_pending_after_the_agent_commits_it() {
    // The property the whole issue turns on: the review unit is
    // committed work, so a commit must not clear the diff.
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");
    room.commit_all("agent commit");

    let pending = room.pending();
    assert_eq!(pending.len(), 1, "a commit must not clear the review");
    assert_eq!((pending[0].additions, pending[0].deletions), (1, 1));
}

#[test]
fn a_second_edit_still_diffs_against_the_original_baseline() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");
    room.write("src/a.rs", "one\nTWO\nTHREE\n");
    room.touched("h2", "src/a.rs");

    let pending = room.pending();
    assert_eq!((pending[0].additions, pending[0].deletions), (2, 2));
    assert_eq!(
        pending[0].harness_id, "h2",
        "the chip follows the latest write"
    );
}

#[test]
fn a_file_outside_the_worktree_is_not_baselined() {
    let room = Room::new();
    note_patch(
        &room.db,
        "r1",
        &room.cwd,
        "h1",
        r#"{"tool":"Edit","files":["/somewhere/else/global.json"]}"#,
    );
    assert!(room.pending().is_empty());
}

#[test]
fn a_new_file_in_a_non_git_room_reads_as_added() {
    let room = Room::bare();
    room.write("notes.md", "hello\n");
    room.touched("h1", "notes.md");

    let pending = room.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].change, "added");
    assert_eq!(pending[0].deletions, 0);
}

// ── accept ────────────────────────────────────────────────────

#[test]
fn accepting_a_whole_file_clears_it_and_leaves_the_worktree_untouched() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");
    let before = room.read("src/a.rs");
    let hash = room.pending()[0].content_hash.clone();

    assert_eq!(
        accept_impl(
            &room.db,
            "r1",
            &room.cwd,
            Some("src/a.rs"),
            &[],
            Some(&hash)
        )
        .unwrap(),
        1
    );
    assert!(room.pending().is_empty(), "accept must clear the diff");
    assert_eq!(room.read("src/a.rs"), before, "accept must not touch disk");
}

#[test]
fn accepting_one_hunk_leaves_the_other_pending() {
    let room = Room::new();
    room.numbered("src/wide.rs", &[]);
    room.commit_all("wide");
    room.touched("h1", "src/wide.rs");
    // Two changes, far enough apart to be separate hunks.
    room.numbered("src/wide.rs", &[(1, "TWO\n"), (17, "EIGHTEEN\n")]);

    let file = room.pending().into_iter().next().unwrap();
    assert_eq!(file.hunks.len(), 2);
    accept_impl(
        &room.db,
        "r1",
        &room.cwd,
        Some("src/wide.rs"),
        &file.hunks[..1],
        None,
    )
    .unwrap();

    let after = room.pending();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].hunks.len(), 1);
    assert!(
        after[0].hunks[0]
            .lines
            .iter()
            .any(|l| l.content == "EIGHTEEN")
    );
}

#[test]
fn accepting_a_deletion_leaves_the_baseline_missing_not_empty() {
    let room = Room::new();
    room.touched("h1", "src/a.rs");
    fs::remove_file(room.path("src/a.rs")).unwrap();
    assert_eq!(room.pending()[0].change, "deleted");

    let file = room.pending().into_iter().next().unwrap();
    accept_impl(
        &room.db,
        "r1",
        &room.cwd,
        Some("src/a.rs"),
        &file.hunks,
        None,
    )
    .unwrap();
    assert!(room.pending().is_empty());

    // Re-creating it must read as added, not as "unchanged".
    room.write("src/a.rs", "back\n");
    assert_eq!(room.pending()[0].change, "added");
}

#[test]
fn accept_all_clears_every_pending_file() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");
    room.write("src/b.rs", "new\n");
    room.touched("h2", "src/b.rs");
    assert_eq!(room.pending().len(), 2);

    assert_eq!(
        accept_impl(&room.db, "r1", &room.cwd, None, &[], None).unwrap(),
        2
    );
    assert!(room.pending().is_empty());
}

#[test]
fn a_stale_hash_refuses_the_accept() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");
    let hash = room.pending()[0].content_hash.clone();
    // The agent writes again between render and click.
    room.write("src/a.rs", "one\nTWO AGAIN\nthree\n");

    let err = accept_impl(
        &room.db,
        "r1",
        &room.cwd,
        Some("src/a.rs"),
        &[],
        Some(&hash),
    )
    .unwrap_err();
    assert!(err.contains("refresh"), "unexpected error: {err}");
    assert_eq!(room.pending().len(), 1, "nothing may have been absorbed");
}

// ── reject ────────────────────────────────────────────────────

#[test]
fn rejecting_a_whole_file_restores_the_baseline_on_disk() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");
    let hash = room.pending()[0].content_hash.clone();

    reject_impl(&room.db, "r1", &room.cwd, "src/a.rs", &[], Some(&hash)).unwrap();
    assert_eq!(room.read("src/a.rs"), "one\ntwo\nthree\n");
    assert!(room.pending().is_empty());
}

#[test]
fn rejecting_a_file_the_agent_created_deletes_it() {
    let room = Room::new();
    room.write("src/new.rs", "unwanted\n");
    room.touched("h1", "src/new.rs");
    assert_eq!(room.pending()[0].change, "added");

    reject_impl(&room.db, "r1", &room.cwd, "src/new.rs", &[], None).unwrap();
    assert!(!room.path("src/new.rs").exists());
    assert!(room.pending().is_empty());
}

#[test]
fn rejecting_one_hunk_leaves_the_other_on_disk() {
    let room = Room::new();
    room.numbered("src/wide.rs", &[]);
    room.commit_all("wide");
    room.touched("h1", "src/wide.rs");
    room.numbered("src/wide.rs", &[(1, "TWO\n"), (17, "EIGHTEEN\n")]);

    let file = room.pending().into_iter().next().unwrap();
    reject_impl(
        &room.db,
        "r1",
        &room.cwd,
        "src/wide.rs",
        &file.hunks[1..],
        None,
    )
    .unwrap();

    let disk = room.read("src/wide.rs");
    assert!(disk.contains("TWO\n"), "the unrejected hunk must survive");
    assert!(!disk.contains("EIGHTEEN"));
}

#[test]
fn rejecting_a_deletion_puts_the_file_back() {
    let room = Room::new();
    room.touched("h1", "src/a.rs");
    fs::remove_file(room.path("src/a.rs")).unwrap();

    reject_impl(&room.db, "r1", &room.cwd, "src/a.rs", &[], None).unwrap();
    assert_eq!(room.read("src/a.rs"), "one\ntwo\nthree\n");
    assert!(room.pending().is_empty());
}

#[test]
fn reject_never_moves_the_baseline() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");
    reject_impl(&room.db, "r1", &room.cwd, "src/a.rs", &[], None).unwrap();

    let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
    assert_eq!(row.content.as_deref(), Some("one\ntwo\nthree\n"));
    // So the next edit is pending again, against the same baseline.
    room.write("src/a.rs", "one\nTHREE\nthree\n");
    assert_eq!(room.pending().len(), 1);
}

// ── non-text ──────────────────────────────────────────────────

#[test]
fn a_binary_file_is_reported_as_blocked_and_still_clears_on_accept() {
    let room = Room::new();
    fs::write(room.path("logo.png"), [0x89, 0x50, 0x00, 0x01]).unwrap();
    room.touched("h1", "logo.png");

    let pending = room.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].blocked, Some("binary"));
    assert!(pending[0].hunks.is_empty());

    accept_impl(&room.db, "r1", &room.cwd, Some("logo.png"), &[], None).unwrap();
    assert!(room.pending().is_empty(), "an accepted binary must clear");

    // A different image of the same length is pending again.
    fs::write(room.path("logo.png"), [0x89, 0x50, 0x00, 0x02]).unwrap();
    assert_eq!(room.pending().len(), 1);
}

// ── #409 image mirror ────────────────────────────────────────

#[test]
fn a_binary_image_baseline_mirrors_its_head_bytes() {
    let room = Room::new();
    let original = [0x89, b'P', b'N', b'G', 1, 2, 3, 4];
    fs::write(room.path("logo.png"), original).unwrap();
    room.commit_all("add logo");
    fs::write(room.path("logo.png"), [0x89, b'P', b'N', b'G', 9, 9, 9, 9]).unwrap();
    room.touched("h1", "logo.png");

    assert_eq!(room.image("logo.png").as_deref(), Some(&original[..]));
}

#[test]
fn a_non_image_binary_baseline_mirrors_nothing() {
    let room = Room::new();
    fs::write(room.path("data.bin"), [0x00, 0x01, 0x02]).unwrap();
    room.commit_all("add data");
    fs::write(room.path("data.bin"), [0x00, 0x01, 0x03]).unwrap();
    room.touched("h1", "data.bin");
    assert!(room.image("data.bin").is_none());
}

#[test]
fn accepting_a_binary_image_mirrors_the_accepted_disk_bytes() {
    let room = Room::new();
    fs::write(room.path("logo.png"), [0x89, b'P', b'N', b'G', 1]).unwrap();
    room.touched("h1", "logo.png"); // never committed — baseline is Missing
    let accepted = [0x89, b'P', b'N', b'G', 2];
    fs::write(room.path("logo.png"), accepted).unwrap();

    accept_impl(&room.db, "r1", &room.cwd, Some("logo.png"), &[], None).unwrap();
    assert_eq!(room.image("logo.png").as_deref(), Some(&accepted[..]));
}

#[test]
fn accept_all_mirrors_every_pending_image() {
    let room = Room::new();
    let bytes = [0x89, b'P', b'N', b'G', 7];
    fs::write(room.path("logo.png"), bytes).unwrap();
    room.touched("h1", "logo.png");

    accept_impl(&room.db, "r1", &room.cwd, None, &[], None).unwrap();
    assert_eq!(room.image("logo.png").as_deref(), Some(&bytes[..]));
}

#[test]
fn a_failed_reject_of_a_binary_image_leaves_its_mirror_untouched() {
    // Reject never moves the baseline (and, on a binary baseline, it
    // cannot even restore the file — there is no text to write
    // back), so the mirror must be exactly as untouched as the row
    // it belongs to.
    let room = Room::new();
    let original = [0x89, b'P', b'N', b'G', 1];
    fs::write(room.path("logo.png"), original).unwrap();
    room.commit_all("add logo");
    fs::write(room.path("logo.png"), [0x89, b'P', b'N', b'G', 2]).unwrap();
    room.touched("h1", "logo.png");
    assert_eq!(room.image("logo.png").as_deref(), Some(&original[..]));

    assert!(reject_impl(&room.db, "r1", &room.cwd, "logo.png", &[], None).is_err());
    assert_eq!(room.image("logo.png").as_deref(), Some(&original[..]));
}

#[test]
fn discovery_mirrors_a_shell_written_binary_image() {
    let room = Room::new();
    let bytes = [0x89, b'P', b'N', b'G', 3];
    fs::write(room.path("logo.png"), bytes).unwrap();
    room.commit_all("add logo");
    fs::write(room.path("logo.png"), [0x89, b'P', b'N', b'G', 4]).unwrap();

    room.discover(&["logo.png"]);
    assert_eq!(room.image("logo.png").as_deref(), Some(&bytes[..]));
}

#[test]
fn a_text_svg_baseline_mirrors_nothing_it_already_carries_its_own_bytes() {
    let room = Room::new();
    fs::write(room.path("icon.svg"), "<svg>one</svg>").unwrap();
    room.commit_all("add icon");
    fs::write(room.path("icon.svg"), "<svg>two</svg>").unwrap();
    room.touched("h1", "icon.svg");
    assert!(room.image("icon.svg").is_none());
}

#[test]
fn hunk_operations_are_refused_on_a_file_with_no_line_diff() {
    let room = Room::new();
    fs::write(room.path("logo.png"), [0x00, 0x01]).unwrap();
    room.touched("h1", "logo.png");
    let fake = skein_review::diff_lines("a\n", "b\n");

    assert!(
        accept_impl(&room.db, "r1", &room.cwd, Some("logo.png"), &fake, None)
            .unwrap_err()
            .contains("no line diff")
    );
    assert!(
        reject_impl(&room.db, "r1", &room.cwd, "logo.png", &fake, None)
            .unwrap_err()
            .contains("no line diff")
    );
}

// ── persistence ───────────────────────────────────────────────

#[test]
fn the_review_survives_a_restart() {
    let dir = TempDir::new().unwrap();
    let cwd = dir.path().to_string_lossy().to_string();
    let db_path = dir.path().join("skein.db");
    let repo = git2::Repository::init(dir.path()).unwrap();
    fs::write(dir.path().join("a.rs"), "one\n").unwrap();
    commit(&repo, "init");

    let quoted =
        serde_json::to_string(&dir.path().join("a.rs").to_string_lossy().to_string()).unwrap();
    let payload = format!("{{\"tool\":\"Edit\",\"files\":[{quoted}]}}");

    {
        let db = Database::open(&db_path).unwrap();
        fs::write(dir.path().join("a.rs"), "ONE\n").unwrap();
        note_patch(&db, "r1", &cwd, "h1", &payload);
        assert_eq!(pending_impl(&db, "r1", &cwd).unwrap().len(), 1);
    }

    // Skein restarts. An empty tracker here would make the pending
    // change disappear — the user would see it as auto-applied.
    let db = Database::open(&db_path).unwrap();
    let pending = pending_impl(&db, "r1", &cwd).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].path, "a.rs");
    assert_eq!((pending[0].additions, pending[0].deletions), (1, 1));
}

// ── discovery (#221) ─────────────────────────────────────────

#[test]
fn discover_baselines_a_shell_write_in_a_non_git_room() {
    let room = Room::bare();
    room.write("notes.md", "hello\n");
    assert_eq!(room.discover(&["notes.md"]), 1);

    let pending = room.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].change, "added");
    assert_eq!(
        pending[0].harness_id, "",
        "no chip — discovery has no attribution"
    );
}

#[test]
fn discover_baselines_a_modified_tracked_file_from_head() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    assert_eq!(room.discover(&["src/a.rs"]), 1);

    let pending = room.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].change, "modified");
    assert_eq!(pending[0].harness_id, "");
    let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
    assert_eq!(
        row.content.as_deref(),
        Some("one\ntwo\nthree\n"),
        "baseline is HEAD content, not what's on disk"
    );
}

#[test]
fn discover_never_touches_an_existing_baseline() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.touched("h1", "src/a.rs");
    room.write("src/a.rs", "one\nTWO\nTHREE\n");

    assert_eq!(room.discover(&["src/a.rs"]), 0);

    let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
    assert_eq!(row.content.as_deref(), Some("one\ntwo\nthree\n"));
    assert_eq!(row.harness_id, "h1", "discovery must not steal the chip");
}

#[test]
fn discover_skips_git_internals_ignored_directory_and_unchanged_paths() {
    let room = Room::new();
    room.write(".gitignore", "ignored.log\n");
    room.commit_all("add gitignore");
    room.write("ignored.log", "noise\n");

    // `.git/`, a gitignored file, and a directory (src already holds
    // a.rs from Room::new).
    assert_eq!(room.discover(&[".git/HEAD", "ignored.log", "src"]), 0);
    assert!(
        room.db
            .review_baseline("r1", "ignored.log")
            .unwrap()
            .is_none()
    );
    assert!(room.db.review_baseline("r1", "src").unwrap().is_none());

    // Outside the worktree entirely.
    assert_eq!(
        discover(
            &room.db,
            "r1",
            &room.cwd,
            vec![format!("{ABS}/elsewhere/x.rs")]
        ),
        0
    );

    // An unchanged tracked file — nothing pending yet.
    assert_eq!(room.discover(&["src/a.rs"]), 0);
    assert!(room.db.review_baseline("r1", "src/a.rs").unwrap().is_none());
}

#[test]
fn discover_baselines_a_deleted_tracked_file() {
    let room = Room::new();
    fs::remove_file(room.path("src/a.rs")).unwrap();
    assert_eq!(room.discover(&["src/a.rs"]), 1);

    let pending = room.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].change, "deleted");
}

#[test]
fn discover_falls_back_to_catch_up_for_a_deleted_tracked_directory() {
    let room = Room::new();
    room.write("src/inner/b.rs", "b\n");
    room.write("src/inner/c.rs", "c\n");
    room.commit_all("add inner dir");
    fs::remove_dir_all(room.path("src/inner")).unwrap();

    // The watcher only ever reports the directory itself vanishing —
    // one OS event, not one per file inside it.
    let inserted = room.discover(&["src/inner"]);
    assert_eq!(
        inserted, 2,
        "both files must surface via the catch-up fallback"
    );

    let mut paths: Vec<String> = room.pending().into_iter().map(|f| f.path).collect();
    paths.sort();
    assert_eq!(paths, ["src/inner/b.rs", "src/inner/c.rs"]);
    assert!(
        room.db
            .review_baseline("r1", "src/inner")
            .unwrap()
            .is_none(),
        "the directory path itself is never baselined"
    );
}

#[test]
fn discover_ignores_untracked_temp_file_churn_and_does_not_sweep() {
    // A never-tracked path that is gone by the time discovery runs
    // (an editor swap file, a lockfile, any create-then-delete temp
    // file) must not latch the catch-up fallback — only a
    // directory that existed at HEAD may. Observed through the
    // sweep's own effect: a dirty tracked file that was never
    // reported in the batch would only surface if a full
    // `git status` sweep ran.
    let room = Room::new();
    room.write("src/other.rs", "hello\n");
    room.commit_all("add other");
    room.write("src/other.rs", "HELLO\n"); // dirty, but not reported below

    room.write("tmp.x", "noise");
    fs::remove_file(room.path("tmp.x")).unwrap();

    assert_eq!(
        room.discover(&["tmp.x"]),
        0,
        "temp churn must not trigger a sweep"
    );
    assert!(
        room.db
            .review_baseline("r1", "src/other.rs")
            .unwrap()
            .is_none(),
        "a full sweep would have picked up the unreported dirty file"
    );
}

#[test]
fn catch_up_discovers_dirty_and_untracked_files() {
    let room = Room::new();
    // The fixture's own sqlite file lives inside this same worktree
    // (real rooms never do — the db is in APP_DATA), so keep it out
    // of `git status` or it would show up as untracked noise.
    room.write(".gitignore", "skein.db*\n");
    room.commit_all("ignore fixture db");

    room.write("src/a.rs", "one\nTWO\nthree\n"); // dirty
    room.write("new.rs", "brand new\n"); // untracked

    assert_eq!(room.catch_up(), 2);
    let mut paths: Vec<String> = room.pending().into_iter().map(|f| f.path).collect();
    paths.sort();
    assert_eq!(paths, ["new.rs", "src/a.rs"]);
}

#[test]
fn catch_up_in_a_non_git_room_does_nothing() {
    let room = Room::bare();
    room.write("notes.md", "hello\n");
    assert_eq!(room.catch_up(), 0);
    assert!(room.pending().is_empty(), "non-git rooms cannot catch up");
}

#[test]
fn accepting_a_discovered_file_clears_it() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.discover(&["src/a.rs"]);
    let hash = room.pending()[0].content_hash.clone();

    accept_impl(
        &room.db,
        "r1",
        &room.cwd,
        Some("src/a.rs"),
        &[],
        Some(&hash),
    )
    .unwrap();
    assert!(room.pending().is_empty());
}

#[test]
fn rejecting_a_discovered_created_file_deletes_it_in_a_non_git_room() {
    let room = Room::bare();
    room.write("notes.md", "unwanted\n");
    room.discover(&["notes.md"]);
    assert_eq!(room.pending()[0].change, "added");

    reject_impl(&room.db, "r1", &room.cwd, "notes.md", &[], None).unwrap();
    assert!(!room.path("notes.md").exists());
    assert!(room.pending().is_empty());
}

#[test]
fn note_patch_after_discovery_attributes_the_row() {
    let room = Room::new();
    room.write("src/a.rs", "one\nTWO\nthree\n");
    room.discover(&["src/a.rs"]);
    assert_eq!(room.pending()[0].harness_id, "");

    room.touched("h1", "src/a.rs");
    assert_eq!(room.pending()[0].harness_id, "h1");

    let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
    assert_eq!(
        row.content.as_deref(),
        Some("one\ntwo\nthree\n"),
        "note_patch must attribute, not recapture"
    );
}

// ── insert_or_attribute (lost-insert race, #221) ────────────────

#[test]
fn insert_or_attribute_claims_a_row_a_concurrent_capture_already_won() {
    // Simulates the race the module docs describe: watcher
    // discovery's own insert lands first, with no attribution,
    // in the window between `note_patch`'s baseline read and its
    // own insert.
    let room = Room::new();
    assert!(
        room.db
            .insert_review_baseline_if_absent(
                "r1",
                "src/a.rs",
                "text",
                Some("one\ntwo\nthree\n"),
                "",
                100,
            )
            .unwrap()
    );

    insert_or_attribute(
        &room.db,
        "r1",
        "src/a.rs",
        "text",
        Some("a different snapshot"),
        "h1",
        200,
        None,
    )
    .unwrap();

    let row = room.db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
    assert_eq!(row.harness_id, "h1", "the chip must not be lost");
    assert_eq!(
        row.content.as_deref(),
        Some("one\ntwo\nthree\n"),
        "content must never be re-captured, only attribution moves"
    );
}
