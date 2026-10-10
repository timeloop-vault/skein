use super::*;

#[test]
fn file_impl_maps_toolarge_and_binary_to_distinct_blocked_reasons() {
    // Pins the #171 slice (c) DTO mapping: a too-large file and a
    // real binary both come back from skein-git as `binary: true`
    // (`max_size` makes an oversized file look binary before any
    // content is read), and `file_impl` is the one place that tells
    // them apart for the pane — `blocked: Some("toolarge")` vs
    // `Some("binary")`. A small text file gets neither and keeps
    // its hunks.
    let (db, tmp) = tests_support::repo_with_commit();
    let cwd = tmp.path().to_str().unwrap();

    let big = "x".repeat(usize::try_from(skein_git::MAX_DIFF_FILE_BYTES).unwrap() + 1);
    std::fs::write(tmp.path().join("big.txt"), &big).unwrap();
    std::fs::write(tmp.path().join("bin.dat"), [0u8, 1, 2, 0, 3, 4]).unwrap();
    std::fs::write(tmp.path().join("small.txt"), "hello\n").unwrap();

    let big_detail = file_impl(&db, "r1", cwd, "big.txt", Scope::Branch, None).unwrap();
    assert_eq!(big_detail.blocked, Some("toolarge"));
    assert!(!big_detail.binary, "too-large is not the same as binary");
    assert_eq!(big_detail.hunks, Vec::new());

    let binary_detail = file_impl(&db, "r1", cwd, "bin.dat", Scope::Branch, None).unwrap();
    assert_eq!(binary_detail.blocked, Some("binary"));
    assert!(binary_detail.binary);
    assert_eq!(binary_detail.hunks, Vec::new());

    let small_detail = file_impl(&db, "r1", cwd, "small.txt", Scope::Branch, None).unwrap();
    assert_eq!(small_detail.blocked, None);
    assert!(!small_detail.binary);
    assert_ne!(small_detail.hunks, Vec::new());
}

#[test]
fn scope_files_snapshot_matches_file_impl_for_every_loaded_file() {
    // #171 slice (f): a `ScopeFiles` snapshot built over several
    // paths must answer each of them exactly as `file_impl`'s own
    // one-path call would — byte for byte, including placement and
    // the "not in the diff" branch — which is what lets a batch
    // caller reuse it instead of paying for one diff per file.
    let (db, tmp) = tests_support::repo_with_base_and_uncommitted_edits();
    let cwd = tmp.path().to_str().unwrap();
    db.set_review_base_ref("r1", "base", 1).unwrap();

    // A thread on the one file that has both a change and a comment
    // — exercises placement, not just the diff lookup.
    db.insert_review_thread(&ReviewThreadRow {
        id: "t1".into(),
        room_id: "r1".into(),
        scope: thread_scope::LINE.into(),
        file_path: Some("b.txt".into()),
        commit_sha: None,
        side: Some("new".into()),
        line_start: Some(1),
        line_end: Some(1),
        anchor_hash: None,
        anchor_lines: Some(serde_json::to_string(&["one"]).unwrap()),
        resolved_ms: None,
        created_ms: 1,
        updated_ms: 1,
    })
    .unwrap();

    // All three loaded — including c.txt, which is on disk
    // unchanged from the base and has no diff entry either way, so
    // it exercises the "not in the diff" branch while still being
    // a key this snapshot was actually asked to cover.
    let paths = vec!["a.txt".to_owned(), "b.txt".to_owned(), "c.txt".to_owned()];
    let snapshot = ScopeFiles::load(&db, "r1", cwd, Scope::Branch, None, &paths).unwrap();

    for path in ["a.txt", "b.txt", "c.txt"] {
        let want = file_impl(&db, "r1", cwd, path, Scope::Branch, None).unwrap();
        let got = snapshot.file(&db, path).unwrap();
        assert_eq!(
            serde_json::to_value(&want).unwrap(),
            serde_json::to_value(&got).unwrap(),
            "{path} diverged between file_impl and the ScopeFiles snapshot"
        );
    }
}

#[test]
fn scope_files_snapshot_refuses_a_key_it_was_never_asked_to_load() {
    // The D6 footgun: a file excluded from `paths` must not answer
    // "no diff" — that is indistinguishable from a file that
    // genuinely has none. b.txt has a real, non-empty diff here;
    // excluding it from the snapshot must make `.file` refuse it
    // outright rather than silently reporting it as unchanged.
    let (db, tmp) = tests_support::repo_with_base_and_uncommitted_edits();
    let cwd = tmp.path().to_str().unwrap();
    db.set_review_base_ref("r1", "base", 1).unwrap();

    let paths = vec!["a.txt".to_owned()];
    let snapshot = ScopeFiles::load(&db, "r1", cwd, Scope::Branch, None, &paths).unwrap();

    assert!(
        snapshot.file(&db, "b.txt").is_err(),
        "a key outside the requested set must be refused, not answered"
    );

    let real = file_impl(&db, "r1", cwd, "b.txt", Scope::Branch, None).unwrap();
    assert!(
        !real.hunks.is_empty(),
        "b.txt does have a real diff — file_impl still sees it"
    );
}

#[test]
fn an_unfiltered_snapshot_serves_any_changed_file_like_file_impl() {
    // An empty `paths` means "the whole scope's diff", so every
    // changed file is fair game with no exclusion at all.
    let (db, tmp) = tests_support::repo_with_base_and_uncommitted_edits();
    let cwd = tmp.path().to_str().unwrap();
    db.set_review_base_ref("r1", "base", 1).unwrap();

    let snapshot = ScopeFiles::load(&db, "r1", cwd, Scope::Branch, None, &[]).unwrap();

    for path in ["a.txt", "b.txt", "c.txt"] {
        let want = file_impl(&db, "r1", cwd, path, Scope::Branch, None).unwrap();
        let got = snapshot.file(&db, path).unwrap();
        assert_eq!(
            serde_json::to_value(&want).unwrap(),
            serde_json::to_value(&got).unwrap(),
            "{path} diverged between file_impl and the unfiltered snapshot"
        );
    }
}

#[test]
fn scope_with_files_builds_summary_and_snapshot_from_one_diff() {
    // #287: `get_diff` used to diff once for the file list and again for
    // the hunks. The pair must agree: every listed file is answerable by
    // the snapshot, and its hunks add up to the counts the summary gives.
    let (db, tmp) = tests_support::repo_with_base_and_uncommitted_edits();
    let cwd = tmp.path().to_str().unwrap();
    db.set_review_base_ref("r1", "base", 1).unwrap();

    let (summary, snapshot) = scope_with_files(&db, "r1", cwd, Scope::Branch, None, &[]).unwrap();
    let alone = scope_impl(&db, "r1", cwd, Scope::Branch, None).unwrap();
    assert_eq!(
        serde_json::to_value(&summary).unwrap(),
        serde_json::to_value(&alone).unwrap(),
        "the shared-diff summary must equal the stand-alone one"
    );

    let listed: Vec<&str> = summary.files.iter().map(|f| f.path.as_str()).collect();
    assert!(listed.contains(&"a.txt") && listed.contains(&"b.txt"));
    assert!(!listed.contains(&"c.txt"), "c.txt is unchanged");
    for f in &summary.files {
        let detail = snapshot.file(&db, &f.path).unwrap();
        let (mut add, mut del) = (0, 0);
        for h in &detail.hunks {
            for l in &h.lines {
                match l.kind {
                    skein_review::LineKind::Add => add += 1,
                    skein_review::LineKind::Delete => del += 1,
                    skein_review::LineKind::Context => {}
                }
            }
        }
        assert_eq!((add, del), (f.additions, f.deletions), "{}", f.path);
    }

    // A path-restricted pair lists only that file, and the snapshot
    // answers it.
    let only = vec!["b.txt".to_owned()];
    let (summary, snapshot) = scope_with_files(&db, "r1", cwd, Scope::Branch, None, &only).unwrap();
    assert_eq!(summary.files.len(), 1);
    assert_eq!(summary.files[0].path, "b.txt");
    assert_ne!(snapshot.file(&db, "b.txt").unwrap().hunks, Vec::new());
}

fn thread_row(id: &str, scope: &str, path: &str, resolved: Option<i64>) -> ReviewThreadRow {
    ReviewThreadRow {
        id: id.into(),
        room_id: "r1".into(),
        scope: scope.into(),
        file_path: Some(path.into()),
        commit_sha: None,
        side: None,
        line_start: None,
        line_end: None,
        anchor_hash: None,
        anchor_lines: None,
        resolved_ms: resolved,
        created_ms: 1,
        updated_ms: 1,
    }
}

#[test]
fn an_unchanged_entry_with_an_open_element_thread_is_listed_in_every_scope() {
    // #434: sign-off counts the thread, so the reviewer must be able
    // to find it even though nothing in the entry changed.
    let (db, tmp) = tests_support::repo_with_base_and_uncommitted_edits();
    let cwd = tmp.path().to_str().unwrap();
    db.set_review_base_ref("r1", "base", 1).unwrap();
    let anchor = |id: &str, path: &str| crate::db::ReviewElementAnchorRow {
        thread_id: id.into(),
        room_id: "r1".into(),
        file_path: path.into(),
        anchor_json: "{}".into(),
        last_seen_json: None,
        updated_ms: 1,
    };
    // c.txt is unchanged from base: open element thread. a.txt has a
    // diff already: must not be listed twice.
    db.insert_review_element_thread(
        &thread_row("e1", "element", "c.txt", None),
        &anchor("e1", "c.txt"),
    )
    .unwrap();
    db.insert_review_element_thread(
        &thread_row("e2", "element", "a.txt", None),
        &anchor("e2", "a.txt"),
    )
    .unwrap();
    // A file-scope thread on an unchanged file is deliberately not listed.
    db.insert_review_thread(&thread_row("f1", thread_scope::FILE, "d.txt", None))
        .unwrap();

    for scope in [Scope::Branch, Scope::Pending] {
        let dto = scope_impl(&db, "r1", cwd, scope, None).unwrap();
        let c: Vec<_> = dto.files.iter().filter(|f| f.path == "c.txt").collect();
        assert_eq!(c.len(), 1, "{scope:?}");
        assert_eq!(c[0].change, "unchanged");
        assert_eq!((c[0].additions, c[0].deletions, c[0].binary), (0, 0, false));
        assert_eq!((c[0].thread_count, c[0].unresolved_count), (1, 1));
        assert_eq!(dto.files.iter().filter(|f| f.path == "a.txt").count(), 1);
        assert!(dto.files.iter().all(|f| f.path != "d.txt"));
    }

    // Once resolved it is no longer worth a row.
    db.set_review_thread_resolved("e1", Some(2), 2).unwrap();
    let dto = scope_impl(&db, "r1", cwd, Scope::Branch, None).unwrap();
    assert!(dto.files.iter().all(|f| f.path != "c.txt"));
}

/// Fixtures. Kept beside the tests rather than in a shared helper —
/// see `signoff.rs`'s copy of the same note.
mod tests_support {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use crate::db::Database;

    /// A repository with one commit, and a database beside it.
    ///
    /// **git2, never a spawned `git`.** The pre-commit hook runs
    /// these tests with `GIT_DIR` exported, and a spawned git
    /// inherits it — see `signoff.rs`'s `tests_support` for the
    /// full story of why that is dangerous.
    pub fn repo_with_commit() -> (Database, TempDir) {
        let tmp = TempDir::new().unwrap();
        git2::Repository::init(tmp.path()).unwrap();
        commit(tmp.path(), &[("a.txt", "one\n")], "init");
        let db = Database::open(&tmp.path().join("skein.db")).unwrap();
        (db, tmp)
    }

    /// A repository with a `base` branch holding three files, and
    /// uncommitted edits to two of them — the third stays byte for
    /// byte what `base` committed, so it has no diff at all.
    pub fn repo_with_base_and_uncommitted_edits() -> (Database, TempDir) {
        let tmp = TempDir::new().unwrap();
        commit(
            tmp.path(),
            &[("a.txt", "one\n"), ("b.txt", "one\n"), ("c.txt", "same\n")],
            "base",
        );
        let repo = git2::Repository::open(tmp.path()).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("base", &head, false).unwrap();

        fs::write(tmp.path().join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(tmp.path().join("b.txt"), "one\nchanged\n").unwrap();

        let db = Database::open(&tmp.path().join("skein.db")).unwrap();
        (db, tmp)
    }

    fn commit(cwd: &Path, files: &[(&str, &str)], msg: &str) {
        for (name, body) in files {
            fs::write(cwd.join(name), body).unwrap();
        }
        let repo = git2::Repository::open(cwd)
            .or_else(|_| git2::Repository::init(cwd))
            .unwrap();
        let mut index = repo.index().unwrap();
        for (name, _) in files {
            index.add_path(Path::new(name)).unwrap();
        }
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
}
