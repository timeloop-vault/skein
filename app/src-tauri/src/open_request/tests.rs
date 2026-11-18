use super::*;

fn room(id: &str, cwd: &str, archived: Option<i64>) -> Room {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": id,
        "task": "",
        "status": "idle",
        "badge": 0,
        "harnesses": [],
        "activeHarnessId": "",
        "cwd": cwd,
        "archived": archived,
    }))
    .unwrap()
}

fn ids(rooms: &[Room], query: &str) -> Vec<String> {
    let candidates: Vec<Candidate> = rooms
        .iter()
        .map(|r| Candidate {
            room: r,
            cwd_norm: normalize_path_for_match(r.cwd.as_deref().unwrap_or_default()),
        })
        .collect();
    best_rooms(&normalize_path_for_match(query), &candidates)
        .into_iter()
        .map(|r| r.id.clone())
        .collect()
}

#[test]
fn args_skip_flags_and_links_and_resolve_against_the_callers_cwd() {
    let cwd = Path::new("/home/me/code");
    assert_eq!(
        path_from_args(&["."], cwd),
        Some(PathBuf::from("/home/me/code/."))
    );
    assert_eq!(
        path_from_args(&["-psn_0_123", "/abs/repo"], cwd),
        Some(PathBuf::from("/abs/repo"))
    );
    assert_eq!(path_from_args(&["-Embedding"], cwd), None);
    assert_eq!(path_from_args(&["skein://open?path=/x"], cwd), None);
    assert_eq!(path_from_args::<&str>(&[], cwd), None);
    // The last path wins.
    assert_eq!(
        path_from_args(&["a", "b"], cwd),
        Some(PathBuf::from("/home/me/code/b"))
    );
}

// `skein "C:\dir\"` arrives on argv with the closing quote glued onto
// the path, since a trailing backslash escapes it rather than ending
// the argument (#393).
#[cfg(windows)]
#[test]
fn args_trim_one_trailing_quote_from_an_escaped_backslash() {
    let cwd = Path::new(r"C:\home\me\code");
    assert_eq!(
        path_from_args(&[r#"C:\dir""#], cwd),
        Some(PathBuf::from(r"C:\dir"))
    );
    // Only one quote is trimmed, never two.
    assert_eq!(
        path_from_args(&[r#"C:\dir"""#], cwd),
        Some(PathBuf::from(r#"C:\dir""#))
    );
    // Trimming down to nothing is no path at all.
    assert_eq!(path_from_args(&[r#"""#], cwd), None);
}

#[test]
fn links_open_absolute_paths_and_refuse_everything_else() {
    let schemes = vec!["skein-dev".to_owned()];
    let parse = |s: &str| link_action(&Url::parse(s).unwrap(), &schemes);

    #[cfg(unix)]
    assert_eq!(
        parse("skein-dev://open?path=%2Fhome%2Fme%2Fmy%20repo"),
        Some(LinkAction::Open(PathBuf::from("/home/me/my repo")))
    );
    #[cfg(windows)]
    assert_eq!(
        parse("skein-dev://open?path=C%3A%5Cme%5Cmy%20repo"),
        Some(LinkAction::Open(PathBuf::from(r"C:\me\my repo")))
    );
    assert_eq!(parse("skein-dev://open"), Some(LinkAction::Focus));
    assert_eq!(parse("skein-dev://open?path="), Some(LinkAction::Focus));
    // Another profile's scheme is not ours.
    assert_eq!(parse("skein://open?path=%2Fx"), None);
    assert_eq!(parse("skein-dev://close?path=%2Fx"), None);
    // No working directory to resolve a relative path against.
    assert_eq!(parse("skein-dev://open?path=relative%2Fdir"), None);
}

#[test]
fn room_links_carry_a_room_and_optionally_a_harness() {
    let schemes = vec!["skein-dev".to_owned()];
    let parse = |s: &str| link_action(&Url::parse(s).unwrap(), &schemes);
    let room = |r: &str, h: Option<&str>| {
        Some(LinkAction::Room {
            room_id: r.to_owned(),
            harness_id: h.map(str::to_owned),
        })
    };
    let long_ok = "a".repeat(64);

    for (link, want) in [
        ("skein-dev://room/abc-123_X", room("abc-123_X", None)),
        ("skein-dev://room/abc/", room("abc", None)),
        ("skein-dev://room/abc?harness=h-1", room("abc", Some("h-1"))),
        ("skein-dev://room/abc?harness=", room("abc", None)),
        (
            "skein-dev://room/abc?other=1&harness=h",
            room("abc", Some("h")),
        ),
        ("skein-dev://room/abc?x=%20", room("abc", None)),
        // Host-less spellings, as for `open`.
        ("skein-dev:room/abc", room("abc", None)),
        ("skein-dev:///room/abc?harness=h", room("abc", Some("h"))),
        (&format!("skein-dev://room/{long_ok}"), room(&long_ok, None)),
        // Bad room ids.
        ("skein-dev://room", None),
        ("skein-dev://room/", None),
        ("skein-dev://room/a/b", None),
        ("skein-dev://room/a%20b", None),
        ("skein-dev://room/a b", None),
        ("skein-dev://room/..", None),
        ("skein-dev:///room/", None),
        ("skein-dev:room/a/b", None),
        (&format!("skein-dev://room/{}", "a".repeat(65)), None),
        // A bad harness id spoils the whole link.
        ("skein-dev://room/abc?harness=a%20b", None),
        ("skein-dev://room/abc?harness=a%2Fb", None),
        (
            &format!("skein-dev://room/abc?harness={}", "h".repeat(65)),
            None,
        ),
        // Unknown hosts and other schemes.
        ("skein-dev://rooms/abc", None),
        ("skein-dev://harness/abc", None),
        ("skein://room/abc", None),
        ("https://room/abc", None),
    ] {
        assert_eq!(parse(link), want, "{link}");
    }
    // `open` is unchanged, including its host-less forms.
    assert_eq!(parse("skein-dev:open"), Some(LinkAction::Focus));
    assert_eq!(parse("skein-dev:///open"), Some(LinkAction::Focus));
    assert_eq!(parse("skein-dev:///open/extra"), None);
}

#[test]
fn jump_serializes_with_camel_case_ids() {
    let with = OpenTarget::Jump {
        room_id: "r".to_owned(),
        harness_id: Some("h".to_owned()),
    };
    assert_eq!(
        serde_json::to_value(&with).unwrap(),
        serde_json::json!({"kind": "jump", "roomId": "r", "harnessId": "h"})
    );
    let without = OpenTarget::Jump {
        room_id: "r".to_owned(),
        harness_id: None,
    };
    assert_eq!(
        serde_json::to_value(&without).unwrap(),
        serde_json::json!({"kind": "jump", "roomId": "r", "harnessId": null})
    );
}

#[test]
fn the_deepest_owning_room_wins_and_a_contained_room_never_does() {
    let rooms = [
        room("outer", "/code/mono", None),
        room("inner", "/code/mono/packages/app", None),
        room("below", "/code/mono/packages/app/deep", None),
    ];
    assert_eq!(ids(&rooms, "/code/mono/packages/app/src"), ["inner"]);
    assert_eq!(ids(&rooms, "/code/mono"), ["outer"]);
    // A parent folder of every room owns none of them.
    assert_eq!(ids(&rooms, "/code"), Vec::<String>::new());
}

#[test]
fn open_rooms_beat_archived_ones_at_the_same_folder() {
    let rooms = [
        room("old", "/code/repo", Some(100)),
        room("live", "/code/repo", None),
    ];
    assert_eq!(ids(&rooms, "/code/repo"), ["live"]);
}

#[test]
fn archived_rooms_rank_most_recently_archived_first() {
    let rooms = [
        room("older", "/code/repo", Some(100)),
        room("newer", "/code/repo", Some(300)),
        room("middle", "/code/repo", Some(200)),
    ];
    assert_eq!(ids(&rooms, "/code/repo"), ["newer", "middle", "older"]);
}

#[test]
fn every_open_room_at_the_folder_is_returned_for_the_frontend_to_pick() {
    let rooms = [room("a", "/code/repo", None), room("b", "/code/repo", None)];
    assert_eq!(ids(&rooms, "/code/repo"), ["a", "b"]);
}

#[test]
fn resolve_reports_a_missing_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let gone = tmp.path().join("nope");
    assert_eq!(
        resolve(&[], &gone),
        OpenTarget::Missing {
            path: gone.to_string_lossy().into_owned()
        }
    );
}

fn retired(mut r: Room, at: i64) -> Room {
    r.retired = Some(at);
    r
}

#[test]
fn a_retired_room_owns_nothing_so_the_folder_resolves_to_new_room() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = canonical_string(tmp.path()).unwrap();
    let rooms = [retired(room("gone", &cwd, Some(100)), 200)];
    assert_eq!(
        resolve(&rooms, tmp.path()),
        OpenTarget::NewRoom { folder: cwd }
    );
}

#[test]
fn a_retired_deeper_owner_yields_to_a_plain_archived_shallower_one() {
    let rooms = [
        room("shallow", "/code/mono", Some(100)),
        retired(room("deep", "/code/mono/app", Some(300)), 400),
    ];
    assert_eq!(ids(&rooms, "/code/mono/app/src"), ["shallow"]);
}

#[test]
fn an_open_room_is_unaffected_by_retired_neighbours() {
    let rooms = [
        retired(room("gone", "/code/repo", Some(100)), 200),
        room("live", "/code/repo", None),
    ];
    assert_eq!(ids(&rooms, "/code/repo"), ["live"]);
}

/// A repo at `dir` with one root commit whose message is `msg`
/// (distinct messages give distinct root ids).
fn init_repo_at(dir: &Path, msg: &str) {
    let repo = git2::Repository::init(dir).unwrap();
    let sig = git2::Signature::now("t", "t@example.com").unwrap();
    let tree = repo
        .find_tree(repo.index().unwrap().write_tree().unwrap())
        .unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[])
        .unwrap();
}

fn with_identity(mut r: Room, dir: &Path) -> Room {
    let dto = crate::git::repo_identity_impl(&dir.to_string_lossy())
        .unwrap()
        .unwrap();
    r.repo_identity = Some(crate::db::RepoIdentity {
        root_commits: dto.root_commits,
        origin_url: dto.origin_url,
    });
    r
}

#[test]
fn a_folder_now_holding_an_unrelated_repo_is_not_the_rooms() {
    let tmp = tempfile::TempDir::new().unwrap();
    init_repo_at(tmp.path(), "original");
    let cwd = canonical_string(tmp.path()).unwrap();
    let open = with_identity(room("open", &cwd, None), tmp.path());
    let old = with_identity(room("old", &cwd, Some(100)), tmp.path());
    // Same repo: still owned.
    assert_eq!(
        resolve(&[open.clone(), old.clone()], tmp.path()),
        OpenTarget::Room {
            room_ids: vec!["open".into()],
            archived: false,
            folder: cwd.clone()
        }
    );
    // The clone at that path is replaced by a different repo.
    let other = tempfile::TempDir::new().unwrap();
    init_repo_at(other.path(), "something else entirely");
    let foreign = with_identity(room("x", &cwd, None), other.path());
    let foreign_old = with_identity(room("y", &cwd, Some(100)), other.path());
    assert_eq!(
        resolve(&[foreign, foreign_old], tmp.path()),
        OpenTarget::NewRoom {
            folder: cwd.clone()
        }
    );
    // No recorded identity: unknown, so still matched.
    assert_eq!(
        resolve(&[room("bare", &cwd, None)], tmp.path()),
        OpenTarget::Room {
            room_ids: vec!["bare".into()],
            archived: false,
            folder: cwd
        }
    );
}

#[test]
fn a_folder_now_holding_an_empty_repo_is_not_the_rooms() {
    let orig = tempfile::TempDir::new().unwrap();
    init_repo_at(orig.path(), "original");
    let fresh = tempfile::TempDir::new().unwrap();
    git2::Repository::init(fresh.path()).unwrap();
    let cwd = canonical_string(fresh.path()).unwrap();
    let r = with_identity(room("r", &cwd, None), orig.path());
    assert_eq!(
        resolve(&[r], fresh.path()),
        OpenTarget::NewRoom { folder: cwd }
    );
}

#[test]
fn resolve_opens_a_files_folder_and_seeds_new_room_with_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let file = tmp.path().join("notes.txt");
    std::fs::write(&file, "x").unwrap();
    let folder = canonical_string(tmp.path()).unwrap();
    assert_eq!(resolve(&[], &file), OpenTarget::NewRoom { folder });
}

#[test]
fn resolve_seeds_new_room_with_the_enclosing_checkout() {
    let tmp = tempfile::TempDir::new().unwrap();
    git2::Repository::init(tmp.path()).unwrap();
    let nested = tmp.path().join("src");
    std::fs::create_dir(&nested).unwrap();
    let root = canonical_string(tmp.path()).unwrap();
    assert_eq!(resolve(&[], &nested), OpenTarget::NewRoom { folder: root });
}

// The shell's spelling and the room's spelling of one folder can
// differ by a symlink — macOS's own `/tmp` → `/private/tmp` is the
// everyday case. Both sides are canonicalized before matching.
#[cfg(unix)]
#[test]
fn resolve_matches_through_a_symlink_on_either_side() {
    let tmp = tempfile::TempDir::new().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let via_real = [room("r", &real.to_string_lossy(), None)];
    let via_link = [room("r", &link.to_string_lossy(), None)];
    let want = OpenTarget::Room {
        room_ids: vec!["r".to_owned()],
        archived: false,
        folder: canonical_string(&real).unwrap(),
    };
    assert_eq!(resolve(&via_real, &link), want);
    assert_eq!(resolve(&via_link, &real), want);
}

/// A path long enough that `dunce::canonicalize` falls back to a
/// Windows verbatim (`\\?\`) spelling (#393): neither `resolve`'s
/// `OpenTarget::NewRoom.folder` nor a room match through it may leak
/// that prefix to the frontend.
#[cfg(windows)]
#[test]
fn resolve_strips_a_verbatim_prefix_from_a_long_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut long = tmp.path().to_path_buf();
    while long.to_string_lossy().len() < 260 {
        long = long.join("a".repeat(50));
    }
    std::fs::create_dir_all(&long).unwrap();

    let target = resolve(&[], &long);
    let OpenTarget::NewRoom { folder } = &target else {
        panic!("expected NewRoom, got {target:?}");
    };
    assert!(
        !folder.starts_with(r"\\?\"),
        "folder leaked a verbatim prefix: {folder}"
    );

    // A room stored with the plain (non-prefixed) long spelling still
    // resolves as its owner.
    let plain_cwd = long.to_string_lossy().into_owned();
    let rooms = [room("r", &plain_cwd, None)];
    assert_eq!(
        resolve(&rooms, &long),
        OpenTarget::Room {
            room_ids: vec!["r".to_owned()],
            archived: false,
            folder: folder.clone(),
        }
    );
}

// `PENDING` is a process-global static; every assertion touching it
// lives in this one test so parallel tests can't race on it (#393).
#[test]
fn take_and_resolve_survives_a_failed_load() {
    let tmp = tempfile::TempDir::new().unwrap();
    let folder = canonical_string(tmp.path()).unwrap();

    // A failing loader leaves the path pending rather than losing it.
    *PENDING.lock() = Some(Pending::Path(tmp.path().to_path_buf()));
    let err = take_and_resolve(|| Err("db locked".to_owned()));
    assert_eq!(err, Err("db locked".to_owned()));
    assert_eq!(
        *PENDING.lock(),
        Some(Pending::Path(tmp.path().to_path_buf()))
    );

    // A subsequent successful call takes the restored path.
    let ok = take_and_resolve(|| Ok(Vec::new()));
    assert_eq!(ok, Ok(Some(OpenTarget::NewRoom { folder })));
    assert_eq!(*PENDING.lock(), None);

    // `restore_pending` never overwrites a newer pending path — a
    // second request may have arrived while the failed load ran.
    *PENDING.lock() = Some(Pending::Path(PathBuf::from("/newer")));
    restore_pending(Pending::Path(PathBuf::from("/older")));
    assert_eq!(
        *PENDING.lock(),
        Some(Pending::Path(PathBuf::from("/newer")))
    );

    // A room request is taken without loading rooms, and latest wins
    // across both kinds.
    set_pending(PathBuf::from("/a/path"));
    set_pending_room("r1".to_owned(), Some("h1".to_owned()));
    let jump = take_and_resolve(|| Err("must not load".to_owned()));
    assert_eq!(
        jump,
        Ok(Some(OpenTarget::Jump {
            room_id: "r1".to_owned(),
            harness_id: Some("h1".to_owned()),
        }))
    );
    set_pending_room("r2".to_owned(), None);
    set_pending(PathBuf::from("/later"));
    assert_eq!(
        *PENDING.lock(),
        Some(Pending::Path(PathBuf::from("/later")))
    );
    // A room request is restored after a failure too, but never over a newer one.
    *PENDING.lock() = None;
    restore_pending(Pending::Room {
        room_id: "r3".to_owned(),
        harness_id: None,
    });
    assert!(matches!(*PENDING.lock(), Some(Pending::Room { .. })));

    // Do not leak this static's state into any other test.
    *PENDING.lock() = None;
}
