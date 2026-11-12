//! Every refusal code, and that a refusal spawns nothing.

use super::*;

fn repo_with_commit(dir: &Path, message: &str) -> git2::Oid {
    std::fs::create_dir_all(dir).unwrap();
    let repo = git2::Repository::init(dir).unwrap();
    let sig = git2::Signature::now("Test", "test@example.invalid").unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[])
        .unwrap()
}

#[test]
fn each_refusal_has_its_code_and_spawns_nothing() {
    let fx = Fx::new();
    let sh = || Some(shell_cmd());

    fx.create(room(
        "ok",
        &fx.work,
        vec![harness("h-ok", "byoh", sh(), None)],
    ));
    fx.create(room(
        "files",
        &fx.work,
        vec![harness("h-files", "files", sh(), None)],
    ));
    fx.create(room(
        "nocmd",
        &fx.work,
        vec![harness("h-nocmd", "byoh", None, None)],
    ));
    fx.create(room(
        "emptycmd",
        &fx.work,
        vec![harness(
            "h-emptycmd",
            "byoh",
            Some(vec![String::new()]),
            None,
        )],
    ));
    fx.create(room(
        "gone",
        &fx.work.join("does-not-exist"),
        vec![harness("h-gone", "byoh", sh(), None)],
    ));
    fx.create(room(
        "noprogram",
        &fx.work,
        vec![harness(
            "h-noprogram",
            "byoh",
            Some(vec!["skein-no-such-program-731".to_owned()]),
            None,
        )],
    ));

    // The identity was stored from one repository; the room points at another.
    let recorded = repo_with_commit(&fx.work.join("repo-a"), "root of a");
    repo_with_commit(&fx.work.join("repo-b"), "root of b");
    let mut mismatch = room(
        "mismatch",
        &fx.work.join("repo-b"),
        vec![harness("h-mismatch", "byoh", sh(), None)],
    );
    mismatch.repo_identity = Some(skein_proto::rooms::RepoIdentity {
        root_commits: vec![recorded.to_string()],
        origin_url: None,
    });
    fx.create(mismatch);

    let mut archived = room(
        "archived",
        &fx.work,
        vec![harness("h-archived", "byoh", sh(), None)],
    );
    archived.archived = Some(1_700_000_000_000);
    fx.create(archived);

    let ask = |room_id: &str, harness_id: &str| {
        fx.runner()
            .ensure_running(
                &EnsureRequest {
                    room_id: room_id.to_owned(),
                    harness_id: harness_id.to_owned(),
                    rows: 24,
                    cols: 80,
                    env_profile: None,
                    restart: false,
                    expect_cmd: None,
                },
                None,
            )
            .err()
            .map(|e| e.code())
    };

    let table = [
        ("unknown_harness", "ok", "h-nope"),
        ("unknown_harness", "no-such-room", "h-ok"),
        ("unknown_harness", "files", "h-ok"),
        ("room_archived", "archived", "h-archived"),
        ("no_pty", "files", "h-files"),
        ("no_cmd", "nocmd", "h-nocmd"),
        ("no_cmd", "emptycmd", "h-emptycmd"),
        ("folder_missing", "gone", "h-gone"),
        ("repo_mismatch", "mismatch", "h-mismatch"),
        ("spawn_failed", "noprogram", "h-noprogram"),
    ];
    for (code, room_id, harness_id) in table {
        assert_eq!(
            ask(room_id, harness_id),
            Some(code),
            "{room_id}/{harness_id}"
        );
        assert!(
            fx.runner().pty_id(harness_id).is_none(),
            "{room_id}/{harness_id} must not leave an entry"
        );
    }

    // The control: the same fixture does spawn a sound record.
    assert_eq!(ask("ok", "h-ok"), None);
    assert!(fx.runner().pty_id("h-ok").is_some());
}

#[test]
fn a_stale_expected_cmd_is_refused_and_leaves_the_process_alone() {
    let fx = Fx::with_shell("byoh");
    let first = fx.ensure(false, None).unwrap();
    let id = first.spawn.id.clone();
    let pid = fx.daemon.pty.pid(&id).expect("pid");

    let ask = |expect: Option<Vec<String>>| {
        fx.runner().ensure_running(
            &EnsureRequest {
                room_id: "r1".to_owned(),
                harness_id: "h1".to_owned(),
                rows: 24,
                cols: 80,
                env_profile: None,
                restart: true,
                expect_cmd: expect,
            },
            None,
        )
    };
    let err = ask(Some(vec!["not-the-record".to_owned()]))
        .err()
        .expect("refused");
    assert_eq!(err.code(), "stale_record");
    assert!(fx.daemon.pty.is_alive(&id), "the process was stopped");
    assert!(pid_exists(pid));
    assert_eq!(fx.runner().pty_id("h1").as_deref(), Some(id.as_str()));

    // The matching argv passes the guard (and restarts, as asked).
    let ok = ask(Some(shell_cmd())).unwrap();
    assert!(ok.started);
}
