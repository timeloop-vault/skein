use super::*;
use crate::time::parse_iso8601_ms;
use std::time::Instant;

// Verbatim from Hermes v0.21.7, minus its two trailing FOREIGN KEY
// clauses: they reference a `system_prompts` table the fixture does
// not carry, and the bundled SQLite then refuses every insert.
const SCHEMA: &str = "
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    created_source TEXT,
    user_id TEXT,
    session_key TEXT,
    chat_id TEXT,
    chat_type TEXT,
    thread_id TEXT,
    display_name TEXT,
    origin_json TEXT,
    expiry_finalized INTEGER DEFAULT 0,
    model TEXT,
    model_config TEXT,
    system_prompt TEXT,
    system_prompt_hash TEXT,
    parent_session_id TEXT,
    started_at REAL NOT NULL,
    ended_at REAL,
    end_reason TEXT,
    message_count INTEGER DEFAULT 0,
    tool_call_count INTEGER DEFAULT 0,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    cwd TEXT,
    git_branch TEXT,
    git_repo_root TEXT,
    git_metadata_generation INTEGER NOT NULL DEFAULT 0,
    billing_provider TEXT,
    billing_base_url TEXT,
    billing_mode TEXT,
    estimated_cost_usd REAL,
    actual_cost_usd REAL,
    cost_status TEXT,
    cost_source TEXT,
    pricing_version TEXT,
    title TEXT,
    title_source TEXT,
    last_activity_at REAL,
    last_activity_description TEXT,
    last_activity_provenance TEXT,
    api_call_count INTEGER DEFAULT 0,
    handoff_state TEXT,
    handoff_platform TEXT,
    handoff_error TEXT,
    compression_failure_cooldown_until REAL,
    compression_failure_error TEXT,
    compression_fallback_streak INTEGER NOT NULL DEFAULT 0,
    compression_ineffective_count INTEGER NOT NULL DEFAULT 0,
    compression_recovery_deadline REAL,
    compression_overload_streak INTEGER NOT NULL DEFAULT 0,
    profile_name TEXT,
    transport_profile TEXT,
    rewind_count INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,
    auto_archived INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0,
    last_read_at REAL,
    tool_names TEXT
);
";

struct Row<'a> {
    id: &'a str,
    source: &'a str,
    parent: Option<&'a str>,
    started_at: f64,
    cwd: &'a str,
    archived: i64,
}

fn row(id: &str, started_at: f64) -> Row<'_> {
    Row {
        id,
        source: "tui",
        parent: None,
        started_at,
        cwd: "C:\\git\\x\\y",
        archived: 0,
    }
}

fn make_db(dir: &Path, wal: bool, rows: &[Row]) -> PathBuf {
    let path = dir.join("state.db");
    let conn = Connection::open(&path).unwrap();
    if wal {
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
            .unwrap();
    }
    conn.execute_batch(SCHEMA).unwrap();
    for r in rows {
        conn.execute(
            "INSERT INTO sessions (id, source, parent_session_id, started_at, cwd, archived) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![r.id, r.source, r.parent, r.started_at, r.cwd, r.archived],
        )
        .unwrap();
    }
    path
}

fn ids(c: &[SessionCandidate]) -> Vec<&str> {
    c.iter().map(|c| c.id.as_str()).collect()
}

#[test]
fn finds_tui_sessions_by_cwd_source_time_archived_and_parent() {
    let dir = tempfile::tempdir().unwrap();
    let mut cli = row("20261013_100002_cccccc", 300.0);
    cli.source = "cli";
    let mut acp = row("20261013_100003_dddddd", 300.0);
    acp.source = "acp";
    let mut arch = row("20261013_100004_eeeeee", 300.0);
    arch.archived = 1;
    let mut child = row("20261013_100005_ffffff", 300.0);
    child.parent = Some("20261013_100000_aaaaaa");
    let mut other = row("20261013_100006_000000", 300.0);
    other.cwd = "C:\\git\\other";
    let rows = vec![
        row("20261013_100000_aaaaaa", 100.0),
        row("20261013_100001_bbbbbb", 200.0),
        cli,
        acp,
        arch,
        child,
        other,
    ];
    let db = make_db(dir.path(), true, &rows);

    let got = tui_sessions_in_cwd(&db, "C:\\git\\x\\y", 150.0).unwrap();
    assert_eq!(ids(&got), ["20261013_100001_bbbbbb"]);
    let all = tui_sessions_in_cwd(&db, "C:\\git\\x\\y", 0.0).unwrap();
    assert_eq!(
        ids(&all),
        ["20261013_100000_aaaaaa", "20261013_100001_bbbbbb"]
    );
    assert!((all[0].started_at - 100.0).abs() < f64::EPSILON);
}

#[test]
fn cwd_matching_variants() {
    assert!(cwd_matches("C:\\git\\x\\y", "c:/git/x/y/", true));
    assert!(cwd_matches("C:\\git\\x\\y\\", "C:\\GIT\\X\\Y", true));
    assert!(!cwd_matches("C:\\git\\x\\y", "C:\\git\\x\\z", true));
    assert!(cwd_matches("/home/a/b/", "/home/a/b", false));
    assert!(!cwd_matches("/home/a/B", "/home/a/b", false));
    assert!(!cwd_matches("/home/a/b", "\\home\\a\\b", false));
}

#[test]
fn session_exists_ignores_archived() {
    let dir = tempfile::tempdir().unwrap();
    let mut arch = row("20261013_100001_bbbbbb", 2.0);
    arch.archived = 1;
    let db = make_db(
        dir.path(),
        true,
        &[row("20261013_100000_aaaaaa", 1.0), arch],
    );
    assert!(session_exists(&db, "20261013_100000_aaaaaa").unwrap());
    assert!(!session_exists(&db, "20261013_100001_bbbbbb").unwrap());
    assert!(!session_exists(&db, "nope").unwrap());
}

#[test]
fn missing_db_is_empty_and_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let db = state_db_path(dir.path());
    assert!(!session_exists(&db, "x").unwrap());
    assert_eq!(tui_sessions_in_cwd(&db, "/a", 0.0).unwrap().len(), 0);
    assert!(!db.exists());
}

#[test]
fn read_only_open_works_on_closed_wal_db_and_leaves_it_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), true, &[row("20261013_100000_aaaaaa", 1.0)]);
    let before = std::fs::read(&db).unwrap();
    assert!(session_exists(&db, "20261013_100000_aaaaaa").unwrap());
    assert_eq!(std::fs::read(&db).unwrap(), before);
}

#[test]
fn reads_while_a_wal_writer_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), true, &[row("20261013_100000_aaaaaa", 1.0)]);
    let writer = Connection::open(&db).unwrap();
    writer
        .execute("UPDATE sessions SET message_count = 3", [])
        .unwrap();
    assert!(session_exists(&db, "20261013_100000_aaaaaa").unwrap());
    drop(writer);
}

#[test]
fn locked_db_errors_within_the_busy_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), false, &[row("20261013_100000_aaaaaa", 1.0)]);
    let writer = Connection::open(&db).unwrap();
    writer.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let t = Instant::now();
    assert!(session_exists(&db, "20261013_100000_aaaaaa").is_err());
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(tui_sessions_in_cwd(&db, "/a", 0.0).is_err());
    writer.execute_batch("ROLLBACK").unwrap();
    assert!(session_exists(&db, "20261013_100000_aaaaaa").unwrap());
}

#[test]
fn corrupt_db_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = state_db_path(dir.path());
    std::fs::write(&db, vec![0x41u8; 4096]).unwrap();
    assert!(session_exists(&db, "x").is_err());
}

#[test]
fn launch_time_respects_zone_offsets() {
    let id = "20261013_183141_ab12cd";
    let utc = parse_iso8601_ms("2026-10-13T16:31:41Z").unwrap();
    assert_eq!(launch_epoch_ms(id, 120), Some(utc));
    assert_eq!(launch_epoch_ms(id, 0), Some(utc + 2 * 3_600_000));
    assert_eq!(launch_epoch_ms(id, -300), Some(utc + 7 * 3_600_000));
    // DST: the same wall clock under +60 (winter) vs +120 (summer).
    let summer = launch_epoch_ms(id, 120).unwrap();
    let winter = launch_epoch_ms(id, 60).unwrap();
    assert_eq!(winter - summer, 3_600_000);
}

#[test]
fn malformed_ids_have_no_launch_time() {
    for bad in [
        "",
        "20261013_183141",
        "20261013_183141_ab12c",
        "20261013_183141_ab12cde",
        "20261013-183141_ab12cd",
        "20261013_183141-ab12cd",
        "2026101a_183141_ab12cd",
        "20261313_183141_ab12cd",
        "20261013_253141_ab12cd",
        "20261013_186141_ab12cd",
        "20261013_183141_zzzzzz",
        "20261000_183141_ab12cd",
    ] {
        assert_eq!(launch_epoch_ms(bad, 0), None, "{bad}");
    }
}

fn cand(id: &str) -> SessionCandidate {
    SessionCandidate {
        id: id.into(),
        started_at: 0.0,
    }
}

fn pend(id: &str, iso: &str) -> PendingHarness {
    PendingHarness {
        harness_id: id.into(),
        spawned_at_ms: parse_iso8601_ms(iso).unwrap(),
    }
}

#[test]
fn two_harnesses_in_one_worktree_do_not_steal_when_b_prompts_first() {
    // A spawned 16:31:30Z, B 10 s later. B prompted first, so B's row
    // has the earlier started_at and comes first in the candidate list.
    let a_id = "20261013_183132_aaaaaa";
    let b_id = "20261013_183142_bbbbbb";
    let mut cands = vec![
        SessionCandidate {
            id: b_id.into(),
            started_at: 10.0,
        },
        SessionCandidate {
            id: a_id.into(),
            started_at: 20.0,
        },
    ];
    let pending = [
        pend("A", "2026-10-13T16:31:30Z"),
        pend("B", "2026-10-13T16:31:40Z"),
    ];
    let got = assign_sessions(&cands, &pending, &HashSet::new(), 120);
    assert_eq!(got.get("A").map(String::as_str), Some(a_id));
    assert_eq!(got.get("B").map(String::as_str), Some(b_id));

    // Only B's row exists yet: B gets it, A stays unassigned.
    cands.remove(1);
    let got = assign_sessions(&cands, &pending, &HashSet::new(), 120);
    assert_eq!(got.len(), 1);
    assert_eq!(got.get("B").map(String::as_str), Some(b_id));
}

#[test]
fn claimed_sessions_are_never_reassigned() {
    let id = "20261013_183132_aaaaaa";
    let claimed = HashSet::from([id.to_string()]);
    let got = assign_sessions(
        &[cand(id)],
        &[pend("A", "2026-10-13T16:31:30Z")],
        &claimed,
        120,
    );
    assert_eq!(got.len(), 0);
}

#[test]
fn a_harness_takes_only_its_earliest_session() {
    let got = assign_sessions(
        &[
            cand("20261013_183232_bbbbbb"),
            cand("20261013_183132_aaaaaa"),
        ],
        &[pend("A", "2026-10-13T16:31:30Z")],
        &HashSet::new(),
        120,
    );
    assert_eq!(
        got.get("A").map(String::as_str),
        Some("20261013_183132_aaaaaa")
    );
    assert_eq!(got.len(), 1);
}

#[test]
fn clock_skew_is_tolerated_up_to_five_seconds() {
    let c = [cand("20261013_183127_aaaaaa")]; // launch 16:31:27Z
    let near = [pend("A", "2026-10-13T16:31:30Z")]; // 3 s after the stamp
    let far = [pend("A", "2026-10-13T16:31:37Z")]; // 10 s after
    let got = assign_sessions(&c, &near, &HashSet::new(), 120);
    assert_eq!(
        got.get("A").map(String::as_str),
        Some("20261013_183127_aaaaaa")
    );
    assert_eq!(assign_sessions(&c, &far, &HashSet::new(), 120).len(), 0);
}

#[test]
fn unparseable_candidates_are_dropped() {
    let got = assign_sessions(
        &[cand("garbage")],
        &[pend("A", "2026-10-13T16:31:30Z")],
        &HashSet::new(),
        0,
    );
    assert_eq!(got.len(), 0);
}

#[test]
fn hermes_home_resolution() {
    let home = Path::new("/home/u");
    let e = OsStr::new("/custom");
    let l = OsStr::new("C:\\Users\\u\\AppData\\Local");
    assert_eq!(
        hermes_home_from(Some(e), Some(l), home, true),
        PathBuf::from("/custom")
    );
    assert_eq!(
        hermes_home_from(Some(e), None, home, false),
        PathBuf::from("/custom")
    );
    assert_eq!(
        hermes_home_from(Some(OsStr::new("")), Some(l), home, true),
        PathBuf::from(l).join("hermes")
    );
    assert_eq!(
        hermes_home_from(None, Some(l), home, true),
        PathBuf::from(l).join("hermes")
    );
    assert_eq!(
        hermes_home_from(None, Some(l), home, false),
        home.join(".hermes")
    );
    assert_eq!(
        hermes_home_from(None, None, home, true),
        home.join(".hermes")
    );
    assert_eq!(
        hermes_home_from(Some(OsStr::new("~/.hermes")), None, home, false),
        home.join(".hermes")
    );
    assert_eq!(
        hermes_home_from(Some(OsStr::new("~")), None, home, false),
        home.to_path_buf()
    );
    // No known home: stays relative, as before.
    assert_eq!(
        hermes_home_from(Some(OsStr::new("~/.hermes")), None, Path::new(""), false),
        PathBuf::from("~/.hermes")
    );
    assert_eq!(state_db_path(home), home.join("state.db"));
}

#[test]
fn a_session_launched_elsewhere_in_the_cwd_is_claimed_by_the_harness() {
    // Known limit: a Hermes started from a plain terminal after the
    // harness spawned looks identical in the row, so the harness takes it.
    let terminal = "20261013_183135_cccccc";
    let got = assign_sessions(
        &[cand(terminal)],
        &[pend("A", "2026-10-13T16:31:30Z")],
        &HashSet::new(),
        120,
    );
    assert_eq!(got.get("A").map(String::as_str), Some(terminal));
}

#[test]
fn profile_home_selects_root_or_profiles_dir() {
    let root = Path::new("/h");
    assert_eq!(profile_home(root, None), root);
    assert_eq!(profile_home(root, Some("default")), root);
    assert_eq!(profile_home(root, Some("  DeFault ")), root);
    assert_eq!(
        profile_home(root, Some(" Skein ")),
        root.join("profiles").join("skein")
    );
}

#[test]
fn profile_name_validation_mirrors_hermes() {
    for ok in [
        "skein",
        "a",
        "0x",
        "my-work_2",
        " Skein ",
        "default",
        "DEFAULT",
    ] {
        assert!(valid_profile_name(ok), "{ok:?} should be valid");
    }
    for bad in [
        "", "  ", "-a", "_a", "a b", "a/b", "..", "a.b", "é", "hermes", "Test", "tmp", "root",
        "sudo",
    ] {
        assert!(!valid_profile_name(bad), "{bad:?} should be invalid");
    }
    assert!(valid_profile_name(&"a".repeat(64)));
    assert!(!valid_profile_name(&"a".repeat(65)));
}

#[test]
fn expand_tilde_from_handles_each_shape() {
    let home = Path::new("/home/u");
    let x = |v| expand_tilde_from(v, Some(home));
    assert_eq!(x("~"), PathBuf::from("/home/u"));
    assert_eq!(x("~/"), PathBuf::from("/home/u"));
    assert_eq!(x("~/h/p"), home.join("h/p"));
    assert_eq!(x(r"~\h"), home.join("h"));
    assert_eq!(x("~other/h"), PathBuf::from("~other/h"));
    assert_eq!(x("/abs/~/x"), PathBuf::from("/abs/~/x"));
    assert_eq!(x(""), PathBuf::new());
    assert_eq!(expand_tilde_from("~/h", None), PathBuf::from("~/h"));
}
