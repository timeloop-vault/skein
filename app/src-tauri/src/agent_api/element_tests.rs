//! Element threads through the agent verbs (#435): what an agent sees of
//! a comment made on the design pane, and that its replies round-trip to
//! both of the reviewer's read paths.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::auth::{self, Caller};
use super::verbs::{self, AddressedArgs, GetCommentArgs, ListArgs, ReplyArgs};
use crate::db::{
    Database, Harness, ReviewCommentRow, ReviewElementAnchorRow, ReviewThreadRow, Room,
};
use crate::review_surface::Scope;
use crate::review_surface::element::{SeenInput, element_threads_impl, seen_impl};
use crate::review_surface::query::file_impl;

const ENTRY: &str = "design/index.html";
const APP: &str = "design/App.jsx";
const APP_BODY: &str = "export function App() {\n  return (\n    <div data-od-id=\"hero\">\n      <h1>Welcome aboard</h1>\n    </div>\n  );\n}\n";

struct Fx {
    _dir: TempDir,
    db: Database,
    root: std::path::PathBuf,
}

fn commit_all(dir: &Path, files: &[&str]) {
    let repo = git2::Repository::init(dir).unwrap();
    let mut index = repo.index().unwrap();
    for f in files {
        index.add_path(Path::new(f)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
}

fn fixture() -> Fx {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("wt");
    std::fs::create_dir_all(root.join("design")).unwrap();
    std::fs::write(
        root.join(ENTRY),
        "<html><body><div id=\"root\"></div></body></html>\n",
    )
    .unwrap();
    std::fs::write(root.join(APP), APP_BODY).unwrap();
    commit_all(&root, &[ENTRY, APP]);

    let db = Database::open(&dir.path().join("skein.db")).unwrap();
    let harness = Harness {
        id: "h1".into(),
        kind: "claude".into(),
        name: "main".into(),
        status: "running".into(),
        model: String::new(),
        tokens: "0".into(),
        live: None,
        cmd: None,
        cwd: None,
        session_id: None,
        agent: None,
        design_entry: None,
        design_device: None,
        pending_notifications: None,
        created_by: None,
        shell_claim: None,
    };
    let r1 = Room {
        id: "r1".into(),
        name: "room r1".into(),
        task: String::new(),
        status: "running".into(),
        badge: 0,
        active_harness_id: "h1".into(),
        harnesses: vec![harness],
        cwd: Some(root.to_str().unwrap().to_owned()),
        branch: None,
        repo: None,
        archived: None,
        repo_root: None,
        attention: None,
        created_by: None,
        closed_by: None,
        todos: None,
        retired: None,
        repo_identity: None,
    };
    db.load_all().unwrap();
    db.save_all(&[r1]).unwrap();
    Fx {
        _dir: dir,
        db,
        root,
    }
}

fn caller(f: &Fx) -> Caller {
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    auth::authenticate(&f.db, Some(&token), Some("h1")).unwrap()
}

fn anchor(source: Option<Value>, od_id: Option<&str>, text: &str) -> Value {
    let mut a = json!({
        "entry": ENTRY, "selector": "#root > div", "tag": "div", "text": text,
        "attrs": {}, "rect": {"x": 1, "y": 2, "w": 30, "h": 40}
    });
    if let Some(s) = source {
        a["source"] = s;
    }
    if let Some(id) = od_id {
        a["odId"] = json!(id);
    }
    a
}

/// What `add_thread_impl` stores for an element comment, minus the
/// validation (the anchors here are already well-formed).
fn seed_element(f: &Fx, id: &str, anchor: &Value) {
    seed_element_with(f, id, anchor, None);
}

fn seed_element_with(f: &Fx, id: &str, anchor: &Value, proposal: Option<&str>) {
    let row = ReviewThreadRow {
        id: id.into(),
        room_id: "r1".into(),
        scope: "element".into(),
        file_path: Some(ENTRY.into()),
        commit_sha: None,
        side: None,
        line_start: None,
        line_end: None,
        anchor_hash: None,
        anchor_lines: None,
        resolved_ms: None,
        created_ms: 1,
        updated_ms: 1,
    };
    f.db.insert_review_element_thread_proposal(
        &row,
        &ReviewElementAnchorRow {
            thread_id: id.into(),
            room_id: "r1".into(),
            file_path: ENTRY.into(),
            anchor_json: anchor.to_string(),
            last_seen_json: None,
            updated_ms: 1,
        },
        proposal,
    )
    .unwrap();
    f.db.insert_review_comment(&ReviewCommentRow {
        id: format!("{id}-c1"),
        thread_id: id.into(),
        room_id: "r1".into(),
        author_kind: "user".into(),
        author_id: None,
        body: "make this bigger".into(),
        created_ms: 1,
        updated_ms: 1,
    })
    .unwrap();
}

fn hero(f: &Fx) {
    seed_element(f, "e1", &anchor(None, Some("hero"), "Welcome aboard"));
}

fn report(f: &Fx, state: &str) {
    let seen = SeenInput {
        state: state.into(),
        selector: Some("#root > div:nth-of-type(1)".into()),
        rect: serde_json::from_value(json!({"x": 5, "y": 6, "w": 70, "h": 80})).unwrap(),
        files: vec![APP.into()],
    };
    seen_impl(&f.db, "r1", &f.root, "e1", &seen, &HashMap::new()).unwrap();
}

fn listed(f: &Fx) -> Value {
    let out = verbs::list_comments(&f.db, &caller(f), &ListArgs::default()).unwrap();
    assert_eq!(out.threads.len(), 1);
    serde_json::to_value(&out.threads[0]).unwrap()
}

fn detail(f: &Fx) -> Value {
    let args = GetCommentArgs {
        thread_id: "e1".into(),
    };
    serde_json::to_value(verbs::get_comment(&f.db, &caller(f), &args).unwrap()).unwrap()
}

#[test]
fn an_agent_sees_an_element_thread_with_its_source_lines() {
    let f = fixture();
    hero(&f);

    let t = listed(&f);
    assert_eq!(t["scope"], "element");
    assert_eq!(t["placement"], "source_guess");
    assert_eq!(t["source"]["file"], APP);
    assert_eq!(t["source"]["line_start"], 3);
    assert_eq!(t["source"]["line_end"], 3);
    assert_eq!(t["source"]["via"], "od_id");
    assert_eq!(t["element"]["anchor"]["odId"], "hero");
    assert_eq!(
        t["element"]["state"], "unknown",
        "the pane has not reported"
    );
    assert!(t.get("line_start").is_none(), "an element has no line");

    let d = detail(&f);
    let ctx = d["current_context"].as_str().unwrap();
    assert!(ctx.contains("data-od-id=\"hero\""), "{ctx}");
    assert!(d.get("diff_context").is_none());
    assert_eq!(d["anchor_lines"], json!([]));
}

#[test]
fn the_anchors_own_source_beats_a_search_and_nothing_found_is_no_guess() {
    let f = fixture();
    // The anchor names line 5 even though od_id would find line 3.
    seed_element(
        &f,
        "e1",
        &anchor(
            Some(json!({"file": APP, "line": 5})),
            Some("hero"),
            "Welcome aboard",
        ),
    );
    let t = listed(&f);
    assert_eq!(t["source"]["via"], "anchor_source");
    assert_eq!(t["source"]["line_start"], 5);
    assert_eq!(t["placement"], "source_guess");

    let g = fixture();
    seed_element(
        &g,
        "e1",
        &anchor(None, Some("nowhere"), "zzz qqq unmatched"),
    );
    let t = listed(&g);
    assert_eq!(t["placement"], "element");
    assert!(t.get("source").is_none());
    let d = detail(&g);
    assert!(d.get("current_context").is_none());
}

fn ids_for(f: &Fx, file: &str) -> Vec<String> {
    let args = ListArgs {
        file: Some(file.into()),
        ..ListArgs::default()
    };
    verbs::list_comments(&f.db, &caller(f), &args)
        .unwrap()
        .threads
        .into_iter()
        .map(|t| t.thread_id)
        .collect()
}

#[test]
fn a_file_filter_finds_element_threads_by_their_source_file() {
    let f = fixture();
    seed_element(
        &f,
        "e1",
        &anchor(Some(json!({"file": APP, "line": 3})), None, "Welcome"),
    );
    seed_element(&f, "e2", &anchor(None, None, "no hint"));

    assert_eq!(ids_for(&f, APP), vec!["e1"]);
    assert_eq!(ids_for(&f, "design\\App.jsx"), vec!["e1"]);
    assert_eq!(ids_for(&f, "design/Other.jsx"), Vec::<String>::new());
    let mut by_entry = ids_for(&f, ENTRY);
    by_entry.sort();
    assert_eq!(by_entry, vec!["e1", "e2"]);
}

#[test]
fn replies_and_addressing_reach_both_of_the_reviewers_read_paths() {
    let f = fixture();
    hero(&f);
    let c = caller(&f);
    verbs::reply(
        &f.db,
        &c,
        &ReplyArgs {
            thread_id: "e1".into(),
            body: "done, enlarged the heading".into(),
        },
    )
    .unwrap();
    verbs::mark_addressed(
        &f.db,
        &c,
        &AddressedArgs {
            thread_id: "e1".into(),
            commit_sha: Some("abc123".into()),
            note: Some("css".into()),
        },
    )
    .unwrap();

    let t = listed(&f);
    assert_eq!(t["comments"].as_array().unwrap().len(), 2);
    assert_eq!(t["comments"][1]["author"], "claude · main");
    assert_eq!(t["addressed"]["commit_sha"], "abc123");
    assert_eq!(t["resolved"], false, "addressing is a claim, not a closure");

    let cwd = f.root.to_str().unwrap();
    let design = element_threads_impl(&f.db, "r1", &f.root, ENTRY).unwrap();
    let review = file_impl(&f.db, "r1", cwd, ENTRY, Scope::Branch, None)
        .unwrap()
        .threads;
    for threads in [design, review] {
        assert_eq!(threads.len(), 1);
        let th = &threads[0];
        assert_eq!(th.scope, "element");
        assert_eq!(th.comments.len(), 2);
        assert_eq!(th.comments[1].author_kind, "agent");
        assert_eq!(th.comments[1].body, "done, enlarged the heading");
        let a = th.addressed.as_ref().expect("addressed block");
        assert_eq!(a.commit_sha.as_deref(), Some("abc123"));
        assert_eq!(a.note.as_deref(), Some("css"));
        assert!(th.element.is_some());
    }
}

#[test]
fn review_status_counts_an_element_thread_and_addressing_adjusts_it() {
    let f = fixture();
    hero(&f);
    let s = verbs::review_status(&f.db, &caller(&f)).unwrap();
    assert_eq!((s.unresolved_count, s.unaddressed_count), (1, 1));

    verbs::mark_addressed(
        &f.db,
        &caller(&f),
        &AddressedArgs {
            thread_id: "e1".into(),
            commit_sha: None,
            note: None,
        },
    )
    .unwrap();
    let s = verbs::review_status(&f.db, &caller(&f)).unwrap();
    assert_eq!((s.unresolved_count, s.unaddressed_count), (1, 0));

    f.db.set_review_thread_resolved("e1", Some(2), 2).unwrap();
    let s = verbs::review_status(&f.db, &caller(&f)).unwrap();
    assert_eq!((s.unresolved_count, s.unaddressed_count), (0, 0));
}

#[test]
fn an_anchored_report_goes_stale_when_a_stamped_file_changes() {
    let f = fixture();
    hero(&f);
    report(&f, "anchored");

    let t = listed(&f);
    assert_eq!(t["outdated"], false);
    assert_eq!(t["element"]["state"], "anchored");
    let seen = &t["element"]["lastSeen"];
    assert_eq!(seen["selector"], "#root > div:nth-of-type(1)");
    assert_eq!(seen["rect"]["w"], 70.0);
    let stamp = seen["stamp"].as_str().unwrap().to_owned();
    assert_ne!(stamp, "");

    // The agent edits a stamped file; the pane has not re-rendered.
    std::fs::write(f.root.join(APP), format!("{APP_BODY}// edited\n")).unwrap();

    let t = listed(&f);
    assert_eq!(t["outdated"], true);
    assert_eq!(t["element"]["state"], "unknown");
    let seen = &t["element"]["lastSeen"];
    assert!(seen.get("selector").is_none());
    assert!(seen.get("rect").is_none());
    assert_eq!(seen["state"], "anchored", "the report itself is kept");
    assert_eq!(seen["stamp"], stamp);
    assert!(seen["seenMs"].as_i64().unwrap() > 0);
}

#[test]
fn a_reanchored_report_is_still_a_guess_to_the_agent() {
    let f = fixture();
    hero(&f);
    report(&f, "reanchored");
    let t = listed(&f);
    assert_eq!(t["outdated"], true);
    assert_eq!(t["element"]["state"], "reanchored");
    assert_eq!(
        t["element"]["lastSeen"]["selector"],
        "#root > div:nth-of-type(1)"
    );
}

#[test]
fn an_agent_gets_the_proposal_next_to_the_element() {
    let f = fixture();
    hero(&f);
    let proposal = json!({"changes": [
        {"kind": "style", "property": "padding-left", "from": "12px", "to": "16px", "token": "--space-4"},
        {"kind": "offset", "dx": 4.0, "dy": 0.0},
    ]});
    seed_element_with(
        &f,
        "e2",
        &anchor(None, Some("hero"), "Welcome aboard"),
        Some(&proposal.to_string()),
    );

    let args = GetCommentArgs {
        thread_id: "e2".into(),
    };
    let got = serde_json::to_value(verbs::get_comment(&f.db, &caller(&f), &args).unwrap()).unwrap();
    assert_eq!(got["proposal"], proposal);
    assert!(got["element"].is_object());

    let out = verbs::list_comments(&f.db, &caller(&f), &ListArgs::default()).unwrap();
    let by_id = |id: &str| {
        serde_json::to_value(out.threads.iter().find(|t| t.thread_id == id).unwrap()).unwrap()
    };
    assert_eq!(by_id("e2")["proposal"], proposal);
    assert!(by_id("e1").get("proposal").is_none());
}
