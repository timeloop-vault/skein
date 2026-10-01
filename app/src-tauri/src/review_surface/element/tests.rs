use super::super::dto::NewThread;
use super::super::write::add_thread_impl;
use super::*;

fn anchor() -> ElementAnchor {
    ElementAnchor {
        entry: "proto/index.html".into(),
        od_id: Some("hero".into()),
        selector: "#root > div:nth-of-type(1)".into(),
        tag: "button".into(),
        text: "First run".into(),
        attrs: BTreeMap::from([("class".to_owned(), "rgroup-h".to_owned())]),
        source: Some(ElementSource {
            file: "proto/shell.jsx".into(),
            line: 16,
            column: Some(9),
        }),
        rect: ElementRect {
            x: 41.0,
            y: 197.0,
            w: 235.0,
            h: 27.0,
        },
    }
}

fn check(mutate: impl FnOnce(&mut ElementAnchor)) -> Result<ElementAnchor, String> {
    let mut a = anchor();
    mutate(&mut a);
    validate_anchor(a, "proto/index.html")
}

#[test]
fn a_well_formed_anchor_round_trips() {
    assert_eq!(check(|_| {}).unwrap(), anchor());
}

#[test]
fn entry_must_be_a_safe_html_path_equal_to_the_threads_file() {
    for bad in [
        "",
        "/abs/index.html",
        "../index.html",
        "proto/../index.html",
        "proto\\index.html",
        "C:/index.html",
        "proto/index.html:stream",
        "proto//index.html",
        "proto/./index.html",
        "proto/index.txt",
        "proto/index.html\0",
    ] {
        assert!(check(|a| a.entry = bad.into()).is_err(), "{bad:?}");
    }
    assert!(check(|a| a.entry = "other.html".into()).is_err());
    let long = format!("{}.html", "a".repeat(500));
    assert!(
        validate_anchor(
            ElementAnchor {
                entry: long.clone(),
                ..anchor()
            },
            &long
        )
        .is_err()
    );
    assert!(
        validate_anchor(
            ElementAnchor {
                entry: "P/I.HTM".into(),
                ..anchor()
            },
            "P/I.HTM"
        )
        .is_ok()
    );
}

#[test]
fn selector_tag_and_id_are_capped() {
    assert!(check(|a| a.selector = String::new()).is_err());
    assert!(check(|a| a.selector = "a".repeat(1001)).is_err());
    assert!(check(|a| a.selector = "a".repeat(1000)).is_ok());
    assert!(check(|a| a.tag = String::new()).is_err());
    assert!(check(|a| a.tag = "Button".into()).is_err());
    assert!(check(|a| a.tag = "my_tag".into()).is_err());
    assert!(check(|a| a.tag = "x".repeat(65)).is_err());
    assert!(check(|a| a.tag = "my-tag2".into()).is_ok());
    assert!(check(|a| a.od_id = Some("x".repeat(201))).is_err());
    assert_eq!(
        check(|a| a.od_id = Some(String::new())).unwrap().od_id,
        None
    );
}

#[test]
fn text_is_clipped_on_a_char_boundary() {
    let got = check(|a| a.text = "å".repeat(800)).unwrap();
    assert_eq!(got.text.chars().count(), 500);
}

#[test]
fn attrs_are_counted_named_and_clipped() {
    let many: BTreeMap<String, String> =
        (0..17).map(|i| (format!("k{i}"), "v".to_owned())).collect();
    assert!(check(|a| a.attrs = many).is_err());
    let sixteen: BTreeMap<String, String> =
        (0..16).map(|i| (format!("k{i}"), "v".to_owned())).collect();
    assert!(check(|a| a.attrs = sixteen).is_ok());
    for bad in ["", "a b", "a=b", "é", &"k".repeat(65)] {
        let attrs = BTreeMap::from([(bad.to_owned(), "v".to_owned())]);
        assert!(check(|a| a.attrs = attrs).is_err(), "{bad:?}");
    }
    let attrs = BTreeMap::from([("data-x:y_z".to_owned(), "v".repeat(400))]);
    let got = check(|a| a.attrs = attrs).unwrap();
    assert_eq!(got.attrs["data-x:y_z"].len(), 300);
}

#[test]
fn source_and_rect_are_range_checked() {
    let src = |file: &str, line, column| {
        Some(ElementSource {
            file: file.into(),
            line,
            column,
        })
    };
    assert!(check(|a| a.source = src("../x.jsx", 1, None)).is_err());
    assert!(check(|a| a.source = src("/x.jsx", 1, None)).is_err());
    assert!(check(|a| a.source = src("x.jsx", 0, None)).is_err());
    assert!(check(|a| a.source = src("x.jsx", 10_000_001, None)).is_err());
    assert!(check(|a| a.source = src("x.jsx", 1, Some(0))).is_err());
    assert!(check(|a| a.source = src("x.jsx", 10_000_000, Some(10_000_000))).is_ok());
    assert!(check(|a| a.source = None).is_ok());
    assert!(check(|a| a.rect.w = f64::NAN).is_err());
    assert!(check(|a| a.rect.x = f64::INFINITY).is_err());
    assert!(check(|a| a.rect.y = 1_000_001.0).is_err());
    assert!(check(|a| a.rect.y = -1_000_000.0).is_ok());
}

fn seen(state: &str, files: &[&str]) -> SeenInput {
    SeenInput {
        state: state.into(),
        selector: Some("#a".into()),
        rect: Some(anchor().rect),
        files: files.iter().map(|f| (*f).to_owned()).collect(),
    }
}

#[test]
fn a_seen_report_is_validated() {
    assert_eq!(
        validate_seen(&seen("anchored", &["proto/a.jsx"])).unwrap(),
        ["proto/a.jsx"]
    );
    assert!(validate_seen(&seen("guessing", &[])).is_err());
    // A bad or surplus file is skipped, never a reason to refuse.
    assert_eq!(
        validate_seen(&seen("lost", &["../x", "ok.js", "a\\b", "http://x/y"])).unwrap(),
        ["ok.js"]
    );
    let many: Vec<String> = (0..201).map(|i| format!("f{i}.js")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(validate_seen(&seen("lost", &refs)).unwrap().len(), 200);
    let mut s = seen("lost", &[]);
    s.selector = Some(String::new());
    assert!(validate_seen(&s).is_err());
    let mut s = seen("lost", &[]);
    s.rect = Some(ElementRect {
        x: 1e7,
        ..anchor().rect
    });
    assert!(validate_seen(&s).is_err());
}

// ── against a database and a folder ──

/// `seen_impl` with nothing served: every file stamps from disk.
fn seen_impl(
    db: &Database,
    room_id: &str,
    root: &Path,
    thread_id: &str,
    seen: &SeenInput,
) -> Result<(), String> {
    super::seen_impl(db, room_id, root, thread_id, seen, &HashMap::new())
}

struct Fixture {
    db: Database,
    tmp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let db = Database::open(&tmp.path().join("skein.db")).unwrap();
        std::fs::create_dir_all(tmp.path().join("proto")).unwrap();
        std::fs::write(tmp.path().join("proto/index.html"), "<p>one</p>").unwrap();
        std::fs::write(tmp.path().join("proto/shell.jsx"), "v1").unwrap();
        Self { db, tmp }
    }

    fn cwd(&self) -> &str {
        self.tmp.path().to_str().unwrap()
    }

    fn new_thread(scope: &str, element: Option<ElementAnchor>) -> NewThread {
        NewThread {
            scope: scope.into(),
            file_path: Some("proto/index.html".into()),
            commit_sha: None,
            side: None,
            line_start: None,
            line_end: None,
            anchor_lines: Vec::new(),
            element,
            body: "make it bigger".into(),
        }
    }

    fn add_element(&self) -> ThreadDto {
        add_thread_impl(
            &self.db,
            "r1",
            self.cwd(),
            &Self::new_thread("element", Some(anchor())),
        )
        .unwrap()
    }
}

#[test]
fn an_element_thread_stores_null_lines_and_an_anchor_row() {
    let f = Fixture::new();
    let dto = f.add_element();
    assert_eq!(dto.scope, "element");
    assert_eq!(dto.file_path.as_deref(), Some("proto/index.html"));
    assert_eq!((dto.line_start, dto.line_end), (None, None));

    let row = f.db.review_thread(&dto.id).unwrap().unwrap();
    assert_eq!(row.file_path.as_deref(), Some("proto/index.html"));
    assert_eq!((row.side, row.line_start, row.line_end), (None, None, None));
    assert_eq!((row.anchor_hash, row.anchor_lines), (None, None));

    let a = f.db.review_element_anchor(&dto.id).unwrap().unwrap();
    assert_eq!(a.file_path, "proto/index.html");
    assert_eq!(a.room_id, "r1");
    assert_eq!(
        serde_json::from_str::<ElementAnchor>(&a.anchor_json).unwrap(),
        anchor()
    );
    assert_eq!(dto.element.unwrap().anchor, anchor());
}

#[test]
fn the_stored_anchor_is_the_validated_struct_not_the_raw_input() {
    let f = Fixture::new();
    let mut a = anchor();
    a.text = "x".repeat(900);
    let dto = add_thread_impl(
        &f.db,
        "r1",
        f.cwd(),
        &Fixture::new_thread("element", Some(a)),
    )
    .unwrap();
    let stored = f.db.review_element_anchor(&dto.id).unwrap().unwrap();
    let back: ElementAnchor = serde_json::from_str(&stored.anchor_json).unwrap();
    assert_eq!(back.text.chars().count(), 500);
}

#[test]
fn scope_element_needs_an_element_and_nothing_else_may_carry_one() {
    let f = Fixture::new();
    let err = add_thread_impl(&f.db, "r1", f.cwd(), &Fixture::new_thread("element", None));
    assert!(err.is_err());
    let err = add_thread_impl(
        &f.db,
        "r1",
        f.cwd(),
        &Fixture::new_thread("file", Some(anchor())),
    );
    assert!(err.is_err());
    let mut bad = anchor();
    bad.selector = String::new();
    let err = add_thread_impl(
        &f.db,
        "r1",
        f.cwd(),
        &Fixture::new_thread("element", Some(bad)),
    );
    assert!(err.is_err());
    assert_eq!(f.db.review_threads_for_room("r1").unwrap(), Vec::new());
    let mut other = anchor();
    other.entry = "proto/other.html".into();
    let err = add_thread_impl(
        &f.db,
        "r1",
        f.cwd(),
        &Fixture::new_thread("element", Some(other)),
    );
    assert!(err.is_err(), "the entry must equal the thread's file");
}

#[test]
fn a_new_element_thread_is_never_unmoved_and_starts_unknown() {
    let f = Fixture::new();
    let dto = f.add_element();
    assert_eq!(dto.placement, "element");
    assert!(dto.outdated);
    let e = dto.element.unwrap();
    assert_eq!(e.state, "unknown");
    assert!(e.last_seen.is_none());
}

#[test]
fn set_last_seen_never_changes_the_anchor() {
    let f = Fixture::new();
    let id = f.add_element().id;
    let before = f.db.review_element_anchor(&id).unwrap().unwrap();
    let root = f.tmp.path();
    seen_impl(&f.db, "r1", root, &id, &seen("lost", &["proto/shell.jsx"])).unwrap();
    let after = f.db.review_element_anchor(&id).unwrap().unwrap();
    assert_eq!(after.anchor_json, before.anchor_json);
    assert_ne!(after.last_seen_json, before.last_seen_json);
}

#[test]
fn a_reported_state_holds_until_a_served_file_changes() {
    let f = Fixture::new();
    let id = f.add_element().id;
    let root = f.tmp.path();
    seen_impl(
        &f.db,
        "r1",
        root,
        &id,
        &seen("anchored", &["proto/shell.jsx"]),
    )
    .unwrap();

    let state = |f: &Fixture| {
        element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html")
            .unwrap()
            .remove(0)
    };
    let t = state(&f);
    assert_eq!(t.element.as_ref().unwrap().state, "anchored");
    assert!(!t.outdated);
    assert_eq!(t.placement, "element");

    // A file the pane listed changes while the pane is closed.
    std::fs::write(root.join("proto/shell.jsx"), "v2").unwrap();
    let t = state(&f);
    let e = t.element.unwrap();
    assert_eq!(e.state, "unknown");
    assert!(t.outdated);
    assert_eq!(
        e.last_seen.unwrap().state,
        "anchored",
        "the report is kept, not believed"
    );

    // So does the entry, even though the pane never listed it.
    seen_impl(&f.db, "r1", root, &id, &seen("reanchored", &[])).unwrap();
    assert_eq!(state(&f).element.unwrap().state, "reanchored");
    std::fs::write(root.join("proto/index.html"), "<p>two</p>").unwrap();
    assert_eq!(state(&f).element.unwrap().state, "unknown");

    // A listed file that disappears is a change too.
    seen_impl(
        &f.db,
        "r1",
        root,
        &id,
        &seen("anchored", &["proto/shell.jsx"]),
    )
    .unwrap();
    assert_eq!(state(&f).element.unwrap().state, "anchored");
    std::fs::remove_file(root.join("proto/shell.jsx")).unwrap();
    assert_eq!(state(&f).element.unwrap().state, "unknown");
}

#[test]
fn a_file_changed_after_it_was_served_reads_unknown() {
    let f = Fixture::new();
    let id = f.add_element().id;
    let root = f.tmp.path();
    let read = |f: &Fixture| {
        element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html")
            .unwrap()
            .remove(0)
            .element
            .unwrap()
            .state
    };
    let served = |bytes: &[u8]| {
        HashMap::from([
            ("proto/shell.jsx".to_owned(), digest_bytes(bytes)),
            ("proto/index.html".to_owned(), digest_bytes(b"<p>one</p>")),
        ])
    };

    // Served v1, then the file became v2 before the report landed:
    // the stamp is v1's, so the freshness check disagrees.
    std::fs::write(root.join("proto/shell.jsx"), "v2").unwrap();
    super::seen_impl(
        &f.db,
        "r1",
        root,
        &id,
        &seen("anchored", &["proto/shell.jsx"]),
        &served(b"v1"),
    )
    .unwrap();
    assert_eq!(read(&f), "unknown");

    // Served what is still on disk: the state is kept.
    super::seen_impl(
        &f.db,
        "r1",
        root,
        &id,
        &seen("anchored", &["proto/shell.jsx"]),
        &served(b"v2"),
    )
    .unwrap();
    assert_eq!(read(&f), "anchored");
}

#[test]
fn a_file_past_the_serve_cap_is_not_read() {
    let f = Fixture::new();
    let big = f.tmp.path().join("proto/big.bin");
    let file = std::fs::File::create(&big).unwrap();
    file.set_len(MAX_SERVED + 1).unwrap();
    let d = file_digest(f.tmp.path(), "proto/big.bin");
    assert!(
        d.starts_with(&format!("toolarge:{}:", MAX_SERVED + 1)),
        "{d}"
    );
}

#[test]
fn stale_and_lost_render_as_outdated() {
    let f = Fixture::new();
    let id = f.add_element().id;
    for (st, outdated) in [("stale", true), ("lost", true), ("anchored", false)] {
        seen_impl(&f.db, "r1", f.tmp.path(), &id, &seen(st, &[])).unwrap();
        let t = element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html")
            .unwrap()
            .remove(0);
        assert_eq!(t.outdated, outdated, "{st}");
        assert_eq!(t.element.unwrap().state, st);
    }
}

#[test]
fn seen_refuses_other_rooms_other_scopes_and_unknown_threads() {
    let f = Fixture::new();
    let id = f.add_element().id;
    let root = f.tmp.path();
    assert!(seen_impl(&f.db, "r2", root, &id, &seen("lost", &[])).is_err());
    assert!(seen_impl(&f.db, "r1", root, "nope", &seen("lost", &[])).is_err());
    assert!(seen_impl(&f.db, "r1", root, &id, &seen("bogus", &[])).is_err());
    let file = add_thread_impl(&f.db, "r1", f.cwd(), &Fixture::new_thread("file", None)).unwrap();
    assert!(seen_impl(&f.db, "r1", root, &file.id, &seen("lost", &[])).is_err());
}

#[test]
fn element_threads_are_scoped_to_the_entry_and_include_resolved() {
    let f = Fixture::new();
    let a = f.add_element().id;
    let mut other = anchor();
    other.entry = "proto/two.html".into();
    let mut nt = Fixture::new_thread("element", Some(other));
    nt.file_path = Some("proto/two.html".into());
    add_thread_impl(&f.db, "r1", f.cwd(), &nt).unwrap();
    f.db.set_review_thread_resolved(&a, Some(5), 5).unwrap();
    let got = element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, a);
    assert_eq!(got[0].resolved_ms, Some(5));
    assert_eq!(got[0].comments.len(), 1);
    assert!(
        element_threads_impl(&f.db, "r2", f.tmp.path(), "proto/index.html")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_agent_is_told_the_state_and_never_unmoved() {
    use crate::agent_api::auth::Caller;
    use crate::agent_api::verbs::{GetCommentArgs, ListArgs, get_comment, list_comments};

    let f = Fixture::new();
    let id = f.add_element().id;
    let caller = Caller {
        room_id: "r1".into(),
        cwd: Some(f.cwd().to_owned()),
        harness_id: None,
        harness_label: None,
    };
    let list = |f: &Fixture| {
        list_comments(&f.db, &caller, &ListArgs::default())
            .unwrap()
            .threads
            .remove(0)
    };

    let t = list(&f);
    assert_eq!(t.placement, Some("element"));
    assert!(t.outdated, "never seen, so unknown, so a guess");
    assert_eq!((t.line_start, t.line_end), (None, None));
    assert_eq!(t.element.as_ref().unwrap()["state"], "unknown");

    seen_impl(&f.db, "r1", f.tmp.path(), &id, &seen("anchored", &[])).unwrap();
    let t = list(&f);
    assert!(!t.outdated);
    assert_eq!(t.element.as_ref().unwrap()["state"], "anchored");
    assert_eq!(t.element.as_ref().unwrap()["anchor"]["tag"], "button");

    let detail = get_comment(&f.db, &caller, &GetCommentArgs { thread_id: id }).unwrap();
    assert_eq!(detail.thread.placement, Some("element"));
    assert!(detail.diff_context.is_none() && detail.current_context.is_none());

    std::fs::write(f.tmp.path().join("proto/index.html"), "<p>two</p>").unwrap();
    assert!(list(&f).outdated);
}

#[test]
fn deleting_a_thread_deletes_its_anchor() {
    let f = Fixture::new();
    let id = f.add_element().id;
    assert!(f.db.delete_review_thread(&id).unwrap());
    assert!(f.db.review_element_anchor(&id).unwrap().is_none());

    let id = f.add_element().id;
    let comment = f.db.review_comments_for_room("r1").unwrap().remove(0);
    assert_eq!(
        f.db.delete_review_comment(&comment.id).unwrap(),
        Some(id.clone())
    );
    assert!(f.db.review_element_anchor(&id).unwrap().is_none());
}

#[test]
fn a_corrupt_anchor_row_degrades_instead_of_failing() {
    let f = Fixture::new();
    let id = f.add_element().id;
    rusqlite::Connection::open(f.tmp.path().join("skein.db"))
        .unwrap()
        .execute(
            "UPDATE review_element_anchors SET anchor_json = 'junk' WHERE thread_id = ?1",
            [&id],
        )
        .unwrap();
    let got = element_threads_impl(&f.db, "r1", f.tmp.path(), "proto/index.html").unwrap();
    assert!(got[0].element.is_none());
    assert_eq!(got[0].placement, "element");
    assert!(got[0].outdated);
}
