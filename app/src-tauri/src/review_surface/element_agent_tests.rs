//! The agent API reads an element thread's state through the same rule
//! the pane does. This test touches `agent_api`, which stays in the app,
//! so it moved here from `skein_daemon::review_surface::element::tests`
//! (#581) with a minimal copy of that module's fixture.

use std::collections::{BTreeMap, HashMap};

use skein_daemon::review_surface::dto::NewThread;
use skein_daemon::review_surface::element::{
    ElementAnchor, ElementRect, ElementSource, SeenInput, seen_impl,
};
use skein_daemon::review_surface::write::add_thread_impl;

use crate::agent_api::auth::Caller;
use crate::agent_api::verbs::{GetCommentArgs, ListArgs, get_comment, list_comments};
use crate::db::Database;

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
            line_text: None,
        }),
        frame: None,
        rect: ElementRect {
            x: 41.0,
            y: 197.0,
            w: 235.0,
            h: 27.0,
        },
    }
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
fn the_agent_is_told_the_state_and_never_unmoved() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Database::open(&tmp.path().join("skein.db")).unwrap();
    std::fs::create_dir_all(tmp.path().join("proto")).unwrap();
    std::fs::write(tmp.path().join("proto/index.html"), "<p>one</p>").unwrap();
    std::fs::write(tmp.path().join("proto/shell.jsx"), "v1").unwrap();
    let cwd = tmp.path().to_str().unwrap().to_owned();

    let id = add_thread_impl(
        &db,
        "r1",
        &cwd,
        &NewThread {
            scope: "element".into(),
            file_path: Some("proto/index.html".into()),
            commit_sha: None,
            side: None,
            line_start: None,
            line_end: None,
            anchor_lines: Vec::new(),
            element: Some(anchor()),
            proposal: None,
            body: "make it bigger".into(),
        },
        &HashMap::new(),
    )
    .unwrap()
    .id;
    let caller = Caller {
        room_id: "r1".into(),
        cwd: Some(cwd),
        harness_id: None,
        harness_label: None,
    };
    let list = || {
        list_comments(&db, &caller, &ListArgs::default())
            .unwrap()
            .threads
            .remove(0)
    };

    let t = list();
    assert_eq!(t.placement, Some("element"));
    assert!(t.outdated, "never seen, so unknown, so a guess");
    assert_eq!((t.line_start, t.line_end), (None, None));
    assert_eq!(t.element.as_ref().unwrap()["state"], "unknown");

    seen_impl(
        &db,
        "r1",
        tmp.path(),
        &id,
        &seen("anchored", &[]),
        &HashMap::new(),
    )
    .unwrap();
    let t = list();
    assert!(!t.outdated);
    assert_eq!(t.element.as_ref().unwrap()["state"], "anchored");
    assert_eq!(t.element.as_ref().unwrap()["anchor"]["tag"], "button");

    let detail = get_comment(&db, &caller, &GetCommentArgs { thread_id: id }).unwrap();
    assert_eq!(detail.thread.placement, Some("element"));
    assert!(detail.diff_context.is_none() && detail.current_context.is_none());

    std::fs::write(tmp.path().join("proto/index.html"), "<p>two</p>").unwrap();
    assert!(list().outdated);
}
