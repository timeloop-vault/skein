//! Boot preparation: the pure core with injected probe, picker and running
//! check (this file), then the load wiring (`load.rs`).

mod load;
mod race;

use serde_json::json;
use skein_proto::rooms::{Harness, LaunchMode, Room, ShellClaim};

use super::{opencode_port_from_cmd, prepare_with, session_survives};

pub(super) fn harness(
    id: &str,
    kind: &str,
    sid: Option<&str>,
    cmd: &[&str],
    extra: &serde_json::Value,
) -> Harness {
    let mut v = json!({
        "id": id, "kind": kind, "name": id, "status": "idle", "model": "", "tokens": "0",
        "cmd": cmd,
    });
    if let Some(s) = sid {
        v["sessionId"] = json!(s);
    }
    if let Some(o) = extra.as_object() {
        for (k, val) in o {
            v[k] = val.clone();
        }
    }
    serde_json::from_value(v).unwrap()
}

pub(super) fn room(id: &str, archived: bool, harnesses: Vec<Harness>) -> Room {
    let mut v = json!({
        "id": id, "name": id, "task": "", "status": "idle", "badge": 0,
        "harnesses": [], "activeHarnessId": ""
    });
    if archived {
        v["archived"] = json!(1);
    }
    let mut r: Room = serde_json::from_value(v).unwrap();
    r.harnesses = harnesses;
    r
}

pub(super) fn find<'a>(rooms: &'a [Room], id: &str) -> &'a Harness {
    rooms
        .iter()
        .flat_map(|r| &r.harnesses)
        .find(|h| h.id == id)
        .unwrap()
}

pub(super) fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn as_json(h: &Harness) -> serde_json::Value {
    serde_json::to_value(h).unwrap()
}

/// The cmd an opencode harness holds after boot picked `port`.
pub(super) fn opencode_cmd(port: &str) -> Vec<String> {
    sv(&["opencode", "--port", port, "--hostname", "127.0.0.1"])
}

pub(super) fn sample() -> Vec<Room> {
    vec![
        room(
            "r1",
            false,
            vec![
                harness(
                    "c",
                    "claude",
                    Some("c-sid"),
                    &["claude", "--session-id", "c-sid"],
                    &json!({}),
                ),
                harness("o", "opencode", Some("o-sid"), &["opencode"], &json!({})),
                harness(
                    "h",
                    "hermes",
                    Some("h-sid"),
                    &["hermes", "--tui"],
                    &json!({}),
                ),
                harness("s", "shell", Some("keep"), &["bash"], &json!({})),
            ],
        ),
        room(
            "r2",
            true,
            vec![
                harness(
                    "c2",
                    "claude",
                    Some("gone"),
                    &["claude", "--session-id", "gone"],
                    &json!({}),
                ),
                harness(
                    "o2",
                    "opencode",
                    Some("o2-sid"),
                    &["opencode", "--port", "1111"],
                    &json!({}),
                ),
            ],
        ),
    ]
}

/// A harness in shell mode, with an optional #517 claim.
fn shell_harness(id: &str, kind: &str, sid: Option<&str>, claim: Option<&str>) -> Harness {
    let mut h = harness(id, kind, sid, &["pwsh.exe"], &json!({}));
    h.launch = Some(LaunchMode::Shell {
        argv: sv(&["pwsh.exe"]),
    });
    h.shell_claim = claim.map(|c| ShellClaim {
        session_id: c.to_owned(),
        port: Some(9),
    });
    h
}

fn prep(
    rooms: Vec<Room>,
    probe: impl Fn(&Harness) -> Result<bool, String>,
    port: u16,
) -> Vec<Room> {
    prepare_with(rooms, probe, || Ok(port), |_| false).unwrap()
}

#[test]
fn only_the_opencode_port_changes_and_nothing_is_dropped_or_minted() {
    let rooms = prep(sample(), |_| panic!("no claim, so no probe"), 4321);
    let before = sample();
    for id in ["c", "h", "s", "c2"] {
        assert_eq!(
            as_json(find(&rooms, id)),
            as_json(find(&before, id)),
            "{id}"
        );
    }
    let o = find(&rooms, "o");
    assert_eq!(o.session_id.as_deref(), Some("o-sid"));
    assert_eq!(o.cmd, Some(opencode_cmd("4321")));
    assert_eq!(opencode_port_from_cmd(o.cmd.as_ref().unwrap()), Some(4321));
    // A closed room's opencode gets no port: nothing will spawn it.
    assert_eq!(
        as_json(find(&rooms, "o2")),
        as_json(find(&before, "o2")),
        "archived"
    );
}

#[test]
fn a_never_prompted_claude_keeps_its_id_and_cmd_across_boots() {
    let seed = vec![room(
        "r",
        false,
        vec![harness(
            "c",
            "claude",
            Some("fresh"),
            &["claude", "--session-id", "fresh"],
            &json!({}),
        )],
    )];
    let first = prep(seed.clone(), |_| panic!("not probed"), 1);
    let second = prep(first.clone(), |_| panic!("not probed"), 2);
    for rooms in [&first, &second] {
        assert_eq!(as_json(find(rooms, "c")), as_json(find(&seed, "c")));
    }
}

#[test]
fn opencode_gets_a_bare_port_cmd_and_never_continue() {
    let seed = vec![room(
        "r",
        false,
        vec![
            harness("none", "opencode", None, &["opencode"], &json!({})),
            harness(
                "cont",
                "opencode",
                None,
                &["opencode", "--continue", "--port", "7"],
                &json!({}),
            ),
            harness(
                "sess",
                "opencode",
                Some("s1"),
                &[
                    "opencode",
                    "--port",
                    "7",
                    "--hostname",
                    "127.0.0.1",
                    "--session",
                    "s1",
                ],
                &json!({}),
            ),
        ],
    )];
    let rooms = prep(seed, |_| panic!("not probed"), 4000);
    for id in ["none", "cont", "sess"] {
        assert_eq!(find(&rooms, id).cmd, Some(opencode_cmd("4000")), "{id}");
    }
    assert_eq!(find(&rooms, "sess").session_id.as_deref(), Some("s1"));
    assert_eq!(find(&rooms, "none").session_id, None);
}

#[test]
fn a_live_claim_puts_the_harness_back_on_its_tool() {
    let seed = vec![room(
        "r",
        false,
        vec![
            shell_harness("o", "opencode", Some("sid"), Some("sid")),
            shell_harness("c", "claude", Some("sid"), Some("sid")),
        ],
    )];
    let rooms = prep(seed, |_| Ok(true), 4100);
    let o = find(&rooms, "o");
    assert_eq!((o.launch.clone(), o.shell_claim.clone()), (None, None));
    assert_eq!(o.cmd, Some(opencode_cmd("4100")));
    assert_eq!(o.session_id.as_deref(), Some("sid"));
    let c = find(&rooms, "c");
    assert_eq!((c.launch.clone(), c.shell_claim.clone()), (None, None));
    assert_eq!(c.cmd, Some(sv(&["claude"])));
    assert_eq!(c.session_id.as_deref(), Some("sid"));
}

#[test]
fn a_stale_claim_is_cleared_and_the_shell_is_kept() {
    let seed = vec![room(
        "r",
        false,
        vec![
            // Session no longer exists.
            shell_harness("gone", "opencode", Some("sid"), Some("sid")),
            // The claim names another session than the record holds.
            shell_harness("other", "claude", Some("sid"), Some("different")),
            shell_harness("blank", "claude", Some(" "), Some(" ")),
        ],
    )];
    let rooms = prepare_with(
        seed,
        |h| {
            assert_ne!(h.id, "other", "a mismatched claim is not probed");
            assert_ne!(h.id, "blank", "a blank claim is not probed");
            Ok(false)
        },
        || panic!("a shell-mode opencode gets no port"),
        |_| false,
    )
    .unwrap();
    for (id, sid) in [("gone", "sid"), ("other", "sid"), ("blank", " ")] {
        let h = find(&rooms, id);
        assert_eq!(h.shell_claim, None, "{id}");
        assert_eq!(h.cmd, Some(sv(&["pwsh.exe"])), "{id}");
        assert!(h.launch.is_some(), "{id}");
        assert_eq!(h.session_id.as_deref(), Some(sid), "{id}");
    }
}

#[test]
fn a_tool_mode_claim_is_dropped_and_other_kinds_keep_theirs() {
    let mut tool = harness("c", "claude", Some("s"), &["claude"], &json!({}));
    tool.shell_claim = Some(ShellClaim {
        session_id: "s".into(),
        port: None,
    });
    let mut hermes = shell_harness("h", "hermes", Some("s"), Some("s"));
    hermes.shell_claim = Some(ShellClaim {
        session_id: "s".into(),
        port: None,
    });
    let rooms = prep(
        vec![room("r", false, vec![tool, hermes.clone()])],
        |_| panic!("not probed"),
        1,
    );
    assert_eq!(find(&rooms, "c").shell_claim, None);
    assert_eq!(as_json(find(&rooms, "h")), as_json(&hermes));
}

#[test]
fn a_shell_mode_opencode_without_a_claim_is_untouched_and_picks_no_port() {
    let seed = vec![room(
        "r",
        false,
        vec![shell_harness("o", "opencode", Some("s"), None)],
    )];
    let rooms = prepare_with(
        seed.clone(),
        |_| panic!("not probed"),
        || panic!("no port for a shell"),
        |_| false,
    )
    .unwrap();
    assert_eq!(as_json(find(&rooms, "o")), as_json(find(&seed, "o")));
}

#[test]
fn an_archived_room_is_migrated_but_picks_no_port() {
    let legacy = harness("o", "opencode", Some("s"), &["pwsh.exe"], &json!({}));
    let rooms = prepare_with(
        vec![room("r", true, vec![legacy])],
        |_| panic!("not probed"),
        || panic!("no port for an archived room"),
        |_| false,
    )
    .unwrap();
    assert_eq!(
        find(&rooms, "o").launch,
        Some(LaunchMode::Shell {
            argv: sv(&["pwsh.exe"])
        })
    );
}

#[test]
fn a_running_harness_is_left_alone() {
    let seed = vec![room(
        "r",
        false,
        vec![
            shell_harness("up", "opencode", Some("sid"), Some("sid")),
            harness(
                "up2",
                "opencode",
                None,
                &["opencode", "--port", "1"],
                &json!({}),
            ),
            harness("down", "opencode", None, &["opencode"], &json!({})),
        ],
    )];
    let rooms = prepare_with(
        seed.clone(),
        |_| panic!("a running harness is not probed"),
        || Ok(4200),
        |id| id.starts_with("up"),
    )
    .unwrap();
    for id in ["up", "up2"] {
        assert_eq!(as_json(find(&rooms, id)), as_json(find(&seed, id)), "{id}");
    }
    assert_eq!(find(&rooms, "down").cmd, Some(opencode_cmd("4200")));
}

#[test]
fn a_failing_port_picker_fails_the_whole_preparation() {
    let out = prepare_with(sample(), |_| Ok(true), || Err("no ports".into()), |_| false);
    assert_eq!(out.unwrap_err(), "no ports");
}

#[test]
fn opencode_v2_skips_the_probe() {
    let h = harness(
        "o",
        "opencode",
        Some("v2-sid"),
        &["opencode"],
        &json!({"opencodeVersion": {"major": 2, "version": "2.0.1"}}),
    );
    assert!(session_survives(&h, &|_| panic!("v2 must not be probed")));
    let blank = Harness {
        session_id: Some("  ".into()),
        ..h
    };
    assert!(!session_survives(&blank, &|_| panic!(
        "v2 must not be probed"
    )));
}

#[test]
fn a_probe_error_keeps_the_claimed_session() {
    let seed = vec![room(
        "r",
        false,
        vec![shell_harness("c", "claude", Some("sid"), Some("sid"))],
    )];
    let rooms = prep(seed, |_| Err("boom".into()), 1);
    assert_eq!(find(&rooms, "c").cmd, Some(sv(&["claude"])));
}

#[test]
fn port_is_recovered_from_an_opencode_cmd() {
    assert_eq!(
        opencode_port_from_cmd(&sv(&["opencode", "--port", "99"])),
        Some(99)
    );
    assert_eq!(
        opencode_port_from_cmd(&sv(&["opencode", "--continue"])),
        None
    );
    assert_eq!(opencode_port_from_cmd(&sv(&["opencode", "--port"])), None);
}

#[test]
fn prepare_migrates_a_legacy_shell_record_once() {
    let legacy = harness("c", "claude", Some("s"), &["pwsh.exe"], &json!({}));
    let tool = harness(
        "t",
        "claude",
        Some("s2"),
        &["claude", "--resume", "s2"],
        &json!({}),
    );
    let out = prep(vec![room("r", false, vec![legacy, tool])], |_| Ok(true), 1);
    assert_eq!(
        find(&out, "c").launch,
        Some(LaunchMode::Shell {
            argv: sv(&["pwsh.exe"])
        })
    );
    assert_eq!(find(&out, "c").cmd, Some(sv(&["pwsh.exe"])));
    assert_eq!(find(&out, "t").launch, None);
    let again = prep(out.clone(), |_| Ok(true), 1);
    assert_eq!(find(&again, "c").launch, find(&out, "c").launch);
    assert_eq!(find(&again, "t").launch, None);
}
