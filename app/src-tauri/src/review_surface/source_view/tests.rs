//! Mirrors against a real (git2-built) repository. Never a spawned
//! `git`: see `query.rs`'s `tests_support` for why.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::super::element::{ElementAnchor, ElementRect, ElementSource};
use super::super::query::{file_impl, scope_impl};
use super::super::{Scope, thread_scope};
use crate::db::{Database, ReviewElementAnchorRow, ReviewThreadRow};

const ENTRY: &str = "proto/index.html";
const JSX: &str = "proto/shell.jsx";
const JSX_BODY: &str =
    "import x;\n\nconst a = 1;\nreturn <Button>First run</Button>;\nexport {};\n";
const LINE: &str = "return <Button>First run</Button>;";

struct Fx {
    db: Database,
    tmp: TempDir,
}

fn commit(cwd: &Path, files: &[(&str, &str)]) {
    let repo = git2::Repository::init(cwd).unwrap();
    let mut index = repo.index().unwrap();
    for (name, body) in files {
        fs::create_dir_all(cwd.join(name).parent().unwrap()).unwrap();
        fs::write(cwd.join(name), body).unwrap();
        index.add_path(Path::new(name)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
        .unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("base", &head, false).unwrap();
}

impl Fx {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        commit(
            tmp.path(),
            &[(ENTRY, "<p>x</p>\n"), (JSX, JSX_BODY), ("other.txt", "o\n")],
        );
        let db = Database::open(&tmp.path().join("skein.db")).unwrap();
        db.set_review_base_ref("r1", "base", 1).unwrap();
        Self { db, tmp }
    }

    fn cwd(&self) -> &str {
        self.tmp.path().to_str().unwrap()
    }

    fn write(&self, path: &str, body: &str) {
        fs::write(self.tmp.path().join(path), body).unwrap();
    }

    /// An element thread on `ENTRY`; `source` is (file, line, lineText).
    fn element(&self, id: &str, source: Option<(&str, u32, Option<&str>)>, resolved: bool) {
        let anchor = ElementAnchor {
            entry: ENTRY.into(),
            od_id: None,
            selector: "#a".into(),
            tag: "button".into(),
            text: "First run".into(),
            attrs: BTreeMap::new(),
            source: source.map(|(file, line, text)| ElementSource {
                file: file.into(),
                line,
                column: None,
                line_text: text.map(str::to_owned),
            }),
            rect: ElementRect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        };
        let thread = thread_row(id, thread_scope::ELEMENT, ENTRY, resolved);
        let row = ReviewElementAnchorRow {
            thread_id: id.into(),
            room_id: "r1".into(),
            file_path: ENTRY.into(),
            anchor_json: serde_json::to_string(&anchor).unwrap(),
            last_seen_json: None,
            updated_ms: 1,
        };
        self.db
            .insert_review_element_thread_proposal(
                &thread,
                &row,
                Some(r#"{"changes":[{"kind":"offset","dx":1,"dy":1}]}"#),
            )
            .unwrap();
    }

    fn jsx_threads(&self, scope: Scope) -> Vec<super::super::dto::ThreadDto> {
        file_impl(&self.db, "r1", self.cwd(), JSX, scope, None)
            .unwrap()
            .threads
    }
}

fn thread_row(id: &str, scope: &str, path: &str, resolved: bool) -> ReviewThreadRow {
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
        resolved_ms: resolved.then_some(5),
        created_ms: 1,
        updated_ms: 1,
    }
}

#[test]
fn a_mirror_is_placed_at_its_source_line_when_the_text_matches() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 4, Some(LINE))), false);
    let ts = f.jsx_threads(Scope::Branch);
    assert_eq!(ts.len(), 1);
    let t = &ts[0];
    assert!(t.via_source);
    assert!(t.proposal.is_some());
    assert_eq!(
        (t.scope.as_str(), t.file_path.as_deref()),
        ("element", Some(ENTRY))
    );
    assert_eq!((t.line_start, t.line_end), (Some(4), Some(4)));
    assert_eq!(
        (t.placement, t.outdated, t.confidence),
        ("unmoved", false, None)
    );
    assert_eq!(t.anchor_lines, [LINE]);
    assert!(t.element.is_some());
}

#[test]
fn an_insert_above_moves_the_mirror_with_the_line() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 4, Some(LINE))), false);
    f.write(JSX, &format!("// new\n{JSX_BODY}"));
    let ts = f.jsx_threads(Scope::Branch);
    assert_eq!(
        (ts[0].line_start, ts[0].placement, ts[0].outdated),
        (Some(5), "moved", false)
    );
}

#[test]
fn an_edited_line_is_outdated_with_no_line_but_keeps_its_text() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 4, Some(LINE))), false);
    f.write(
        JSX,
        &JSX_BODY.replace("First run", "Something else entirely"),
    );
    let ts = f.jsx_threads(Scope::Branch);
    let t = &ts[0];
    assert_eq!((t.line_start, t.line_end), (None, None));
    assert_eq!((t.placement, t.outdated), ("outdated", true));
    assert_eq!(t.anchor_lines, [LINE]);
}

#[test]
fn a_thread_without_captured_text_is_outdated_with_no_line() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 4, None)), false);
    let t = &f.jsx_threads(Scope::Branch)[0];
    assert!(t.via_source);
    assert_eq!(
        (t.line_start, t.placement, t.outdated),
        (None, "outdated", true)
    );
    assert_eq!(t.anchor_lines, Vec::<String>::new());
}

#[test]
fn a_source_line_past_the_end_is_placed_when_the_text_is_still_there() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 99, Some(LINE))), false);
    let t = &f.jsx_threads(Scope::Branch)[0];
    assert_eq!(
        (t.line_start, t.placement, t.outdated),
        (Some(4), "moved", false)
    );
}

#[test]
fn a_source_line_past_the_end_is_outdated_when_the_text_is_gone() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 99, Some(LINE))), false);
    f.write(JSX, "import x;\n");
    let t = &f.jsx_threads(Scope::Branch)[0];
    assert_eq!((t.line_start, t.outdated), (None, true));
}

#[test]
fn the_commit_scope_places_a_mirror_against_the_commit_blob() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 4, Some(LINE))), false);
    let repo = git2::Repository::open(f.tmp.path()).unwrap();
    let sha = {
        fs::write(f.tmp.path().join("other.txt"), "p\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("other.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.com").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "second", &tree, &[&parent])
            .unwrap()
            .to_string()
    };
    // The line is in the committed file but gone from the worktree.
    f.write(JSX, "import x;\n");
    let on_disk = file_impl(&f.db, "r1", f.cwd(), JSX, Scope::Branch, None).unwrap();
    assert!(on_disk.threads[0].outdated);
    let in_commit = file_impl(&f.db, "r1", f.cwd(), JSX, Scope::Commit, Some(&sha)).unwrap();
    let t = &in_commit.threads[0];
    assert_eq!(
        (t.line_start, t.placement, t.outdated),
        (Some(4), "unmoved", false)
    );
}

#[test]
fn a_mirror_writes_nothing_back() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 4, Some(LINE))), false);
    f.write(JSX, &format!("// new\n{JSX_BODY}"));
    for scope in [Scope::Branch, Scope::Pending] {
        f.jsx_threads(scope);
    }
    let row = f.db.review_thread("e1").unwrap().unwrap();
    assert_eq!((row.line_start, row.line_end), (None, None));
    assert_eq!((row.anchor_hash, row.anchor_lines), (None, None));
}

#[test]
fn a_mirror_is_counted_once_and_beside_the_threads_not_in_them() {
    let f = Fx::new();
    f.write(JSX, &format!("// new\n{JSX_BODY}"));
    f.element("e1", Some((JSX, 4, Some(LINE))), false);
    f.element("e2", Some((JSX, 4, Some(LINE))), true);
    let dto = scope_impl(&f.db, "r1", f.cwd(), Scope::Branch, None).unwrap();
    let entry = dto.files.iter().find(|x| x.path == ENTRY).unwrap();
    assert_eq!((entry.thread_count, entry.unresolved_count), (2, 1));
    assert_eq!(
        (entry.source_thread_count, entry.source_unresolved_count),
        (0, 0)
    );
    let jsx = dto.files.iter().find(|x| x.path == JSX).unwrap();
    assert_eq!((jsx.thread_count, jsx.unresolved_count), (0, 0));
    assert_eq!(
        (jsx.source_thread_count, jsx.source_unresolved_count),
        (2, 1)
    );
    assert_eq!(dto.unresolved_count, 1);
}

#[test]
fn an_unchanged_source_file_with_an_open_mirror_is_listed_in_every_scope() {
    let f = Fx::new();
    f.element("e1", Some((JSX, 4, Some(LINE))), false);
    for scope in [Scope::Branch, Scope::Pending] {
        let dto = scope_impl(&f.db, "r1", f.cwd(), scope, None).unwrap();
        let rows: Vec<_> = dto.files.iter().filter(|x| x.path == JSX).collect();
        assert_eq!(rows.len(), 1, "{scope:?}");
        assert_eq!(rows[0].change, "unchanged");
        assert_eq!((rows[0].additions, rows[0].deletions), (0, 0));
        assert_eq!((rows[0].thread_count, rows[0].unresolved_count), (0, 0));
        assert_eq!(
            (rows[0].source_thread_count, rows[0].source_unresolved_count),
            (1, 1)
        );
    }
    // Resolved-only is not worth a row.
    f.db.set_review_thread_resolved("e1", Some(2), 2).unwrap();
    let dto = scope_impl(&f.db, "r1", f.cwd(), Scope::Branch, None).unwrap();
    assert!(dto.files.iter().all(|x| x.path != JSX));
    // But the file's own view still shows the resolved mirror.
    assert_eq!(f.jsx_threads(Scope::Branch).len(), 1);
}

#[test]
fn an_element_thread_without_a_source_appears_only_under_its_entry() {
    let f = Fx::new();
    f.element("e1", None, false);
    // A source that is the entry itself is not a mirror either.
    f.element("e2", Some((ENTRY, 1, Some("<p>x</p>"))), false);
    let dto = scope_impl(&f.db, "r1", f.cwd(), Scope::Branch, None).unwrap();
    assert!(dto.files.iter().all(|x| x.path != JSX));
    assert!(dto.files.iter().all(|x| x.source_thread_count == 0));
    assert!(f.jsx_threads(Scope::Branch).is_empty());
    let entry = file_impl(&f.db, "r1", f.cwd(), ENTRY, Scope::Branch, None).unwrap();
    assert_eq!(entry.threads.len(), 2);
    assert!(entry.threads.iter().all(|t| !t.via_source));
}

#[test]
fn a_normal_thread_and_a_mirror_coexist_without_duplicate_ids() {
    let f = Fx::new();
    f.write(JSX, &format!("// new\n{JSX_BODY}"));
    f.element("e1", Some((JSX, 4, Some(LINE))), false);
    let mut line = thread_row("l1", thread_scope::LINE, JSX, false);
    line.side = Some("new".into());
    line.line_start = Some(1);
    line.line_end = Some(1);
    line.anchor_lines = Some(r#"["// new"]"#.into());
    f.db.insert_review_thread(&line).unwrap();

    let ts = f.jsx_threads(Scope::Branch);
    let mut ids: Vec<_> = ts.iter().map(|t| t.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["e1", "l1"]);
    assert!(!ts.iter().find(|t| t.id == "l1").unwrap().via_source);
    assert!(ts.iter().find(|t| t.id == "e1").unwrap().via_source);
}
