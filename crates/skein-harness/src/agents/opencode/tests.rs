use std::ffi::OsStr;
use std::fs;
use std::time::Duration;

use super::disk::{definitions, read_config};
use super::v1::{parse_list, selectable};
use super::v2::{parse_debug_agents, run_args, selectable_v2};
use super::{Mode, list_v2};

// `opencode agent list` interleaves each name with an indented
// permission blob; the indentation is what separates them.
const OPENCODE_LIST: &str = "build (primary)\n  [\n  {\n    \"permission\": \"*\"\n  }\n  ]\ncompaction (primary)\n  [ ]\nexplore (subagent)\n  [ ]\ngeneral (subagent)\nplan (primary)\nsummary (primary)\ntitle (primary)\n";

// Captured from opencode v2.2.1, permissions trimmed.
const DEBUG_AGENTS: &str = include_str!("fixtures/debug-agents.json");

#[test]
fn parses_the_real_opencode_list() {
    let pairs = parse_list(OPENCODE_LIST);
    assert_eq!(
        pairs,
        vec![
            ("build".to_owned(), Mode::Primary),
            ("compaction".to_owned(), Mode::Primary),
            ("explore".to_owned(), Mode::Subagent),
            ("general".to_owned(), Mode::Subagent),
            ("plan".to_owned(), Mode::Primary),
            ("summary".to_owned(), Mode::Primary),
            ("title".to_owned(), Mode::Primary),
        ]
    );
}

#[test]
fn offers_only_what_opencode_would_accept() {
    // Subagents are refused by `--agent`; the three internal
    // primaries are machinery. What is left is what a user starts a
    // harness as.
    let names = selectable(parse_list(OPENCODE_LIST));
    assert_eq!(names, vec!["build", "plan"]);
}

#[test]
fn an_unknown_mode_is_skipped_not_guessed() {
    let pairs = parse_list("weird (someday)\nbuild (primary)\n");
    assert_eq!(pairs, vec![("build".to_owned(), Mode::Primary)]);
}

#[test]
fn reads_agents_declared_in_opencode_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("opencode.json");
    let json = serde_json::json!({
        "agent": {
            "reviewer": {
                "description": "reviews",
                "model": "m",
                "tools": { "read": true, "write": false }
            }
        }
    });
    fs::write(&cfg, json.to_string()).expect("write");
    let defs = read_config(&cfg);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "reviewer");
    assert_eq!(defs[0].description.as_deref(), Some("reviews"));
    // opencode states tools as a bool map; the allowlist is the
    // enabled keys, so a disabled one must not count as allowed.
    assert_eq!(defs[0].tools, Some(vec!["read".to_owned()]));
}

#[test]
fn the_agents_key_is_read_and_wins_over_agent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("opencode.json");
    let json = serde_json::json!({
        "agent": { "old": { "description": "v1" }, "both": { "description": "from agent" } },
        "agents": { "new": { "description": "v2" }, "both": { "description": "from agents" } }
    });
    fs::write(&cfg, json.to_string()).expect("write");
    let defs = read_config(&cfg);
    let desc = |n: &str| {
        defs.iter()
            .find(|d| d.name == n)
            .and_then(|d| d.description.clone())
    };
    assert_eq!(desc("old").as_deref(), Some("v1"));
    assert_eq!(desc("new").as_deref(), Some("v2"));
    assert_eq!(desc("both").as_deref(), Some("from agents"));
    assert_eq!(defs.len(), 3);
}

#[test]
fn the_plural_agents_dir_is_picked_up_and_wins() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let global = home.path().join(".config").join("opencode");
    let project = cwd.path().join(".opencode");
    for (dir, name, desc) in [
        (global.join("agent"), "g1", "global singular"),
        (global.join("agents"), "g2", "global plural"),
        (project.join("agent"), "p1", "project singular"),
        (project.join("agents"), "p2", "project plural"),
        (project.join("agent"), "clash", "singular"),
        (project.join("agents"), "clash", "plural"),
    ] {
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            dir.join(format!("{name}.md")),
            format!("---\ndescription: {desc}\n---\nbody\n"),
        )
        .expect("write");
    }
    let defs = definitions(home.path(), cwd.path());
    for (name, desc) in [
        ("g1", "global singular"),
        ("g2", "global plural"),
        ("p1", "project singular"),
        ("p2", "project plural"),
        ("clash", "plural"),
    ] {
        assert_eq!(
            defs.get(name).and_then(|d| d.description.as_deref()),
            Some(desc),
            "{name}"
        );
    }
}

#[test]
fn the_v2_fixture_yields_exactly_the_selectable_ids() {
    let all = parse_debug_agents(DEBUG_AGENTS).expect("parse");
    assert_eq!(all.len(), 13);
    let ids: Vec<_> = selectable_v2(all).into_iter().map(|a| a.id).collect();
    assert_eq!(
        ids,
        [
            "build",
            "cfgagent",
            "plan",
            "pluralfoo",
            "projfoo",
            "singularbar"
        ]
    );
}

#[test]
fn v2_uses_id_never_name_and_tolerates_a_missing_description() {
    let json = r#"[
        {"id":"build","name":"Build","mode":"primary","hidden":false},
        {"id":"Mixed_Case","name":"Mixed_Case","mode":"all","description":"d"}
    ]"#;
    let got = selectable_v2(parse_debug_agents(json).expect("parse"));
    assert_eq!(got[0].id, "build");
    assert_eq!(got[0].description, None);
    assert_eq!(got[1].id, "Mixed_Case");
    assert_eq!(got[1].description.as_deref(), Some("d"));
}

#[test]
fn v2_drops_hidden_subagents_internals_and_unknown_modes() {
    let json = r#"[
        {"id":"a","mode":"primary","hidden":true},
        {"id":"b","mode":"subagent","hidden":false},
        {"id":"c","mode":"someday","hidden":false},
        {"id":"title","mode":"primary","hidden":false},
        {"id":"d","mode":"primary","hidden":false}
    ]"#;
    let ids: Vec<_> = selectable_v2(parse_debug_agents(json).expect("parse"))
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(ids, ["d"]);
}

#[test]
fn v2_garbage_and_non_arrays_are_errors() {
    for bad in ["", "not json", "{\"id\":\"build\"}", "\"x\"", "null"] {
        assert!(parse_debug_agents(bad).is_err(), "{bad:?}");
    }
    assert_eq!(parse_debug_agents("[]"), Ok(Vec::new()));
}

#[test]
fn v2_degrades_to_disk_definitions_when_the_cli_will_not_run() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let dir = cwd.path().join(".opencode").join("agents");
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(dir.join("foo.md"), "---\ndescription: d\n---\nbody\n").expect("write");
    let list = list_v2(
        OsStr::new("definitely-not-a-real-program-680"),
        None,
        home.path(),
        cwd.path(),
    );
    assert!(list.degraded.is_some());
    assert!(list.agents.iter().any(|a| a.name == "foo"));
}

#[test]
fn v2_run_kills_a_process_that_outlives_the_timeout() {
    let (prog, args): (&str, &[&str]) = if cfg!(windows) {
        ("ping", &["-n", "30", "127.0.0.1"])
    } else {
        ("sleep", &["30"])
    };
    let started = std::time::Instant::now();
    let err = run_args(
        OsStr::new(prog),
        None,
        &std::env::temp_dir(),
        args,
        Duration::from_millis(300),
    )
    .expect_err("must time out");
    assert!(err.contains("timed out"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
}
