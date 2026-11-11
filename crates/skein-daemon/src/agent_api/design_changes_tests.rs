//! `show_changes` (#547): the pure changed-lines rule, the caps, the
//! argument checks, the guards, and the request the frontend receives.

use serde_json::{Value, json};
use skein_review::{Hunk, HunkLine, LineKind};

use super::design_tests::{design_room, folder, is_refused, recording};
use super::review_tests::{commit_file, git_repo_with_commit};
use super::tests::{caller_for, fixture, save};
use super::verbs::{self, ShowChangesArgs, ShowElementArgs, VerbError};
use crate::review_surface::changes::{
    ChangedFile, MAX_CHANGED_FILES, MAX_CHANGED_LINES, cap, changed_lines,
};

// ── the pure rule ───────────────────────────────────────────────────────

/// `+n` is an added line numbered n, `-` a deleted one, `n` context.
fn hunk(spec: &[&str]) -> Hunk {
    let lines = spec
        .iter()
        .map(|s| {
            let (kind, new_lineno) = if let Some(n) = s.strip_prefix('+') {
                (LineKind::Add, Some(n.parse().unwrap()))
            } else if *s == "-" {
                (LineKind::Delete, None)
            } else {
                (LineKind::Context, Some(s.parse().unwrap()))
            };
            HunkLine {
                kind,
                content: String::new(),
                old_lineno: None,
                new_lineno,
            }
        })
        .collect();
    Hunk {
        header: String::new(),
        old_start: 0,
        old_lines: 0,
        new_start: 0,
        new_lines: 0,
        lines,
    }
}

#[test]
fn added_lines_are_the_changed_lines() {
    assert_eq!(changed_lines(&[hunk(&["3", "+4", "+5", "6"])]), [4, 5]);
}

#[test]
fn a_deleted_run_anchors_to_the_next_line() {
    // Lines 4 and 5 were removed; what is now line 4 follows them.
    assert_eq!(changed_lines(&[hunk(&["3", "-", "-", "4", "5"])]), [4]);
}

#[test]
fn a_deletion_replaced_by_an_add_is_just_the_add() {
    assert_eq!(changed_lines(&[hunk(&["3", "-", "+4", "5"])]), [4]);
}

#[test]
fn a_deletion_at_the_end_of_a_hunk_anchors_to_the_previous_line() {
    assert_eq!(changed_lines(&[hunk(&["8", "9", "-", "-"])]), [9]);
}

#[test]
fn a_deletion_at_the_start_of_a_file_hunk_with_no_neighbour_adds_nothing() {
    assert_eq!(changed_lines(&[hunk(&["-", "-"])]), Vec::<usize>::new());
}

#[test]
fn several_hunks_are_merged_sorted_and_deduplicated() {
    let hunks = [
        hunk(&["20", "+21", "22"]),
        hunk(&["1", "-", "2", "+3"]),
        hunk(&["20", "-", "21"]),
    ];
    assert_eq!(changed_lines(&hunks), [2, 3, 21]);
}

// ── the caps ────────────────────────────────────────────────────────────

fn file(path: &str, n: usize) -> ChangedFile {
    ChangedFile {
        path: path.to_owned(),
        lines: (1..=n).collect(),
        deleted: false,
    }
}

#[test]
fn nothing_is_cut_under_the_caps() {
    let (kept, truncated) = cap(vec![file("a", 5), file("b", 5)], 300, 20_000);
    assert_eq!(kept.len(), 2);
    assert!(!truncated);
}

#[test]
fn more_than_the_file_cap_is_truncated() {
    let files: Vec<_> = (0..=MAX_CHANGED_FILES)
        .map(|i| file(&format!("f{i}"), 1))
        .collect();
    let (kept, truncated) = cap(files, MAX_CHANGED_FILES, MAX_CHANGED_LINES);
    assert_eq!(kept.len(), MAX_CHANGED_FILES);
    assert!(truncated);
}

#[test]
fn more_than_the_line_cap_is_truncated_mid_file() {
    let (kept, truncated) = cap(
        vec![
            file("a", MAX_CHANGED_LINES - 2),
            file("b", 10),
            file("c", 1),
        ],
        MAX_CHANGED_FILES,
        MAX_CHANGED_LINES,
    );
    assert!(truncated);
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[1].lines, [1, 2]);
}

#[test]
fn a_deleted_file_has_no_lines_to_cut() {
    let gone = ChangedFile {
        path: "gone".into(),
        lines: vec![],
        deleted: true,
    };
    let (kept, truncated) = cap(vec![file("a", 3), gone], 300, 3);
    assert_eq!(kept.len(), 2);
    assert!(!truncated);
}

// ── the verb ────────────────────────────────────────────────────────────

fn args(v: Value) -> ShowChangesArgs {
    serde_json::from_value(v).unwrap()
}

#[allow(clippy::unnecessary_wraps)] // the recording frontend's answer shape
fn answer(_: &str, _: &Value) -> Result<Value, String> {
    Ok(json!({ "highlighted": 2, "mapped": [], "unmapped": [], "limits": "x" }))
}

#[tokio::test]
async fn bad_arguments_are_refused_before_any_guard_or_frontend_call() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let cases = [
        json!({ "scope": "everything" }),
        json!({ "scope": "commit" }),
        json!({ "commit_sha": "abc123" }),
        json!({ "scope": "branch", "commit_sha": "abc123" }),
        json!({ "scope": "pending", "commit_sha": "abc123" }),
        json!({ "reveal": "yes" }),
        json!({ "reveal": 1 }),
        json!({ "reveal": null }),
    ];
    for _ in 0..2 {
        for c in &cases {
            let e = verbs::show_changes(&state, &caller, &args(c.clone()), false)
                .await
                .unwrap_err();
            assert!(is_refused(&e, "bad_arguments:"), "{c} -> {e:?}");
        }
    }
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn kill_switch_rate_cap_and_harness_scoping_apply() {
    let f = fixture();
    let dir = folder();
    save(
        &f.db,
        &[
            design_room("A", &dir, &["d1"]),
            design_room("B", &dir, &["dB"]),
        ],
    );
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    let a = args(json!({}));
    let e = verbs::show_changes(&state, &caller, &a, false)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "disabled:"), "{e:?}");

    // A foreign harness and a non-design harness are both not found.
    for id in ["dB", "A-claude", "nope"] {
        let e = verbs::show_changes(&state, &caller, &args(json!({ "harness": id })), true)
            .await
            .unwrap_err();
        assert!(matches!(e, VerbError::NotFound(_)), "{id}: {e:?}");
    }
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_cap_is_shared_with_the_other_design_write_verbs() {
    let f = fixture();
    let dir = folder();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, _) = recording(&f, answer);
    for _ in 0..10 {
        let a = ShowElementArgs {
            harness: None,
            selector: Some("p".into()),
            anchor: None,
            reveal: None,
        };
        verbs::show_element(&state, &caller, &a, true)
            .await
            .unwrap();
    }
    let e = verbs::show_changes(&state, &caller, &args(json!({})), true)
        .await
        .unwrap_err();
    assert!(is_refused(&e, "rate_limited:"), "{e:?}");
}

#[tokio::test]
async fn no_changes_answers_without_asking_the_frontend() {
    let f = fixture();
    let dir = folder();
    git_repo_with_commit(dir.path());
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);
    // Everything in the folder is untracked-or-committed; the pending
    // scope has no baselines, so nothing changed there.
    let out = verbs::show_changes(&state, &caller, &args(json!({ "scope": "pending" })), true)
        .await
        .unwrap();
    assert_eq!(
        out,
        json!({
            "scope": "pending", "harnessId": "d1", "files": 0, "highlighted": 0,
            "mapped": [], "unmapped": [], "note": "no changes in this scope",
        })
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_branch_scope_sends_changed_lines_and_the_answer_comes_back_tagged() {
    let f = fixture();
    let dir = folder();
    git_repo_with_commit(dir.path());
    std::fs::create_dir(dir.path().join("src")).unwrap();
    commit_file(
        dir.path(),
        "src/App.jsx",
        "line1\nline2\nline3\nline4\n",
        "add app",
    );
    // Uncommitted: line 2 rewritten, a line added at the end.
    std::fs::write(
        dir.path().join("src/App.jsx"),
        "line1\nCHANGED\nline3\nline4\nline5\n",
    )
    .unwrap();
    save(&f.db, &[design_room("A", &dir, &["d1"])]);
    let caller = caller_for(&f.db, "A", Some("A-claude"));
    let (state, calls) = recording(&f, answer);

    let out = verbs::show_changes(&state, &caller, &args(json!({ "reveal": true })), true)
        .await
        .unwrap();
    assert_eq!(out["highlighted"], json!(2));
    assert_eq!(out["harnessId"], json!("d1"));
    assert_eq!(out["scope"], json!("branch"));

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "design.show_changes");
    let req = &calls[0].1;
    assert_eq!(req["roomId"], json!("A"));
    assert_eq!(req["harnessId"], json!("d1"));
    assert_eq!(req["scope"], json!("branch"));
    assert_eq!(req["reveal"], json!(true));
    assert!(req.get("truncated").is_none());
    let files = req["files"].as_array().unwrap();
    let app = files
        .iter()
        .find(|f| f["path"] == json!("src/App.jsx"))
        .unwrap_or_else(|| panic!("{files:?}"));
    assert_eq!(app["deleted"], json!(false));
    let lines = app["lines"].as_array().unwrap();
    assert!(
        lines.contains(&json!(2)) && lines.contains(&json!(5)),
        "{app}"
    );
}
