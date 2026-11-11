use std::fmt::Write;

use super::yaml_hooks::{
    HOOK_EVENTS, HookSpec, auto_accept_value, hooks_present, hooks_preview, plan_hooks, quote_arg,
    remove_hooks,
};

const WIN: &str = r"C:\Users\Some One\AppData\skein hook.cmd";

fn spec() -> HookSpec {
    HookSpec {
        prefix: quote_arg(WIN).unwrap(),
        timeout_secs: 10,
    }
}

fn plan(text: &str, auto: bool) -> String {
    plan_hooks(text, &spec(), auto).unwrap().expect("a change")
}

fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

#[test]
fn quote_arg_keeps_windows_path_with_spaces_intact() {
    assert_eq!(quote_arg(WIN).unwrap(), format!("\"{WIN}\""));
    assert_eq!(quote_arg("plain-name").unwrap(), "plain-name");
    assert_eq!(quote_arg("a\"b"), None);
    assert_eq!(quote_arg(r"C:\dir\"), None);
    assert_eq!(quote_arg(""), None);
}

#[test]
fn case_a_appends_a_top_level_block_exactly() {
    let out = plan("model: x\n", true);
    let mut want = String::from("model: x\n");
    want.push_str(
        "# skein-hooks:begin \u{2014} added by Skein for harness status; inert outside Skein\n",
    );
    want.push_str("hooks_auto_accept: true\nhooks:\n");
    for ev in HOOK_EVENTS {
        write!(
            want,
            "  {ev}:\n    - command: '\"{WIN}\" {ev}'\n      timeout: 10\n"
        )
        .unwrap();
    }
    want.push_str("# skein-hooks:end\n");
    assert_eq!(out, want);
    assert_eq!(
        hooks_preview(&spec(), true).join("\n") + "\n",
        &want["model: x\n".len()..]
    );
}

#[test]
fn case_a_without_auto_accept_or_with_existing_key() {
    let out = plan("model: x\n", false);
    assert!(!out.contains("hooks_auto_accept"));
    let out = plan("hooks_auto_accept: false\nmodel: x\n", true);
    assert_eq!(count(&out, "hooks_auto_accept"), 1);
    assert_eq!(auto_accept_value(&out), Some(false));
}

#[test]
fn round_trip_restores_the_original_bytes() {
    for orig in [
        "model: x\n",
        "model: x",
        "",
        "model: x\r\nfoo: 1\r\n",
        "\u{feff}model: x\n",
        "# only a comment\n",
    ] {
        for auto in [false, true] {
            let out = plan(orig, auto);
            assert!(hooks_present(&out, &spec()), "{orig:?}");
            assert_eq!(remove_hooks(&out), orig, "{orig:?} auto={auto}");
        }
    }
}

#[test]
fn second_plan_is_none() {
    for orig in [
        "model: x\n",
        "model: x",
        "hooks:\n  on_session_start:\n    - command: ./user.sh\n      timeout: 5\n",
        "hooks:\n",
        "hooks:\n  # nothing yet\nmodel: y\n",
    ] {
        for auto in [false, true] {
            let out = plan(orig, auto);
            assert_eq!(plan_hooks(&out, &spec(), auto).unwrap(), None, "{orig:?}");
        }
    }
}

#[test]
fn crlf_is_preserved() {
    let out = plan("model: x\r\n", true);
    assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    assert!(out.starts_with("model: x\r\n# skein-hooks:begin"));
    assert_eq!(remove_hooks(&out), "model: x\r\n");
    let out = plan("hooks:\r\n  pre_llm_call:\r\n    - command: a\r\n", false);
    assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    assert!(out.contains("    - command: a\r\n    # skein-hooks:begin"));
}

#[test]
fn case_b_appends_to_an_existing_sequence_and_adds_missing_events() {
    let orig = "hooks:\n  pre_tool_call:\n    - command: ./mine.sh\n      timeout: 5\n  other: 1\nmodel: y\n";
    let out = plan(orig, false);
    // our item sits right after the user's item, inside the same list
    assert!(out.contains(
        "      timeout: 5\n    # skein-hooks:begin \u{2014} added by Skein for harness status; inert outside Skein\n    - command: '\"C:\\Users\\Some One\\AppData\\skein hook.cmd\" pre_tool_call'\n      timeout: 10\n    # skein-hooks:end\n  other: 1\n"
    ));
    // the seven other events are one marked group after the last child
    assert!(out.contains("  other: 1\n  # skein-hooks:begin"));
    assert_eq!(count(&out, "- command: '\"C:"), 8);
    assert!(out.ends_with("  # skein-hooks:end\nmodel: y\n"));
    assert!(hooks_present(&out, &spec()));
    assert_eq!(remove_hooks(&out), orig);
}

#[test]
fn case_b_compact_sequence_indent_and_four_space_unit() {
    let orig = "hooks:\n    pre_llm_call:\n    - command: a\n";
    let out = plan(orig, false);
    assert!(out.contains("    - command: a\n    # skein-hooks:begin"));
    assert_eq!(remove_hooks(&out), orig);
    assert!(out.contains("    on_session_start:\n        - command:"));
}

#[test]
fn case_b_event_key_without_items_gets_our_item_below_it() {
    let orig = "hooks:\n  subagent_stop:\nmodel: y\n";
    let out = plan(orig, false);
    assert!(out.starts_with("hooks:\n  subagent_stop:\n    # skein-hooks:begin"));
    assert_eq!(remove_hooks(&out), orig);
}

#[test]
fn case_c_empty_hooks_value_is_filled_in() {
    let orig = "hooks:   # none yet\nmodel: y\n";
    let out = plan(orig, true);
    assert!(out.starts_with("hooks:   # none yet\n  # skein-hooks:begin"));
    assert!(out.contains("hooks_auto_accept: true"));
    assert_eq!(remove_hooks(&out), orig);
}

#[test]
fn case_c_refusals() {
    for orig in [
        "hooks: null\n",
        "hooks: ~\n",
        "hooks: {}\n",
        "hooks: []\n",
        "hooks:\n  - a\n",
        "hooks: &h\n  a: 1\n",
        "\"hooks\": 1\n",
        "hooks:\n\ton_session_start:\n",
        "hooks:\n  pre_llm_call: []\n",
        "hooks:\n  pre_llm_call: ~\n",
        "hooks:\n  pre_llm_call:\n    a: 1\n",
        "hooks:\n  x: |\n    text\n",
        "---\na: 1\n---\nb: 2\n",
        "- a\n- b\n",
        "[1]\n",
    ] {
        assert!(plan_hooks(orig, &spec(), false).is_err(), "{orig:?}");
    }
}

#[test]
fn auto_accept_is_never_overwritten() {
    let orig = "hooks_auto_accept: false\nhooks:\n";
    let out = plan(orig, true);
    assert_eq!(count(&out, "hooks_auto_accept"), 1);
    assert_eq!(auto_accept_value(&out), Some(false));
    let out = plan("hooks_auto_accept: true # yes\n", false);
    assert_eq!(auto_accept_value(&out), Some(true));
    assert_eq!(auto_accept_value("model: x\n"), None);
    assert_eq!(auto_accept_value("hooks_auto_accept: maybe\n"), None);
    assert_eq!(
        auto_accept_value("hooks:\n  hooks_auto_accept: true\n"),
        None
    );
}

#[test]
fn auto_accept_region_is_separate_when_hooks_exists() {
    let orig = "hooks:\n  pre_tool_call:\n    - command: a\n";
    let out = plan(orig, true);
    assert!(out.ends_with("# skein-hooks:begin \u{2014} added by Skein for harness status; inert outside Skein\nhooks_auto_accept: true\n# skein-hooks:end\n"));
    assert_eq!(remove_hooks(&out), orig);
    let unterminated = plan("hooks:\n  pre_tool_call:\n    - command: a", true);
    assert_eq!(
        remove_hooks(&unterminated),
        "hooks:\n  pre_tool_call:\n    - command: a"
    );
}

#[test]
fn changed_spec_rewrites_only_our_regions() {
    let orig = "hooks:\n  pre_tool_call:\n    - command: ./mine.sh\n      timeout: 5\n";
    let out = plan(orig, false);
    let new = HookSpec {
        prefix: "other --flag".into(),
        timeout_secs: 30,
    };
    assert!(!hooks_present(&out, &new));
    let out2 = plan_hooks(&out, &new, false).unwrap().unwrap();
    assert!(hooks_present(&out2, &new));
    assert!(!out2.contains("skein hook.cmd"));
    assert_eq!(count(&out2, "- command: 'other --flag "), 8);
    assert!(out2.contains("- command: ./mine.sh\n      timeout: 5\n"));
    assert_eq!(remove_hooks(&out2), orig);
    assert_eq!(plan_hooks(&out2, &new, false).unwrap(), None);
}

#[test]
fn unmarked_user_entries_are_never_touched() {
    let orig = format!(
        "hooks:\n  pre_tool_call:\n    - command: '{}'\n      timeout: 10\n",
        spec().command_for("pre_tool_call")
    );
    let out = plan(&orig, false);
    assert!(out.starts_with(&orig));
    assert!(!hooks_present(&orig, &spec()));
    assert!(hooks_present(&out, &spec()));
    assert_eq!(remove_hooks(&out), orig);
}

#[test]
fn removal_keeps_hooks_key_when_user_added_children_below_our_block() {
    let out = plan("model: x\n", false);
    let edited = format!("{out}  my_event:\n    - command: z\n");
    let removed = remove_hooks(&edited);
    assert_eq!(removed, "model: x\nhooks:\n  my_event:\n    - command: z\n");
    // and planning on it converges after one rewrite
    let again = plan_hooks(&edited, &spec(), false).unwrap().unwrap();
    assert_eq!(plan_hooks(&again, &spec(), false).unwrap(), None);
}

#[test]
fn mcp_markers_are_independent() {
    let mcp = "# skein:begin \u{2014} added by Skein for its review tools; inert outside Skein\nmcp_servers:\n  skein:\n    url: x\n# skein:end\n";
    let out = plan(mcp, false);
    assert!(out.starts_with(mcp));
    assert_eq!(remove_hooks(&out), mcp);
    assert_eq!(remove_hooks(mcp), mcp);
    assert!(!hooks_present(mcp, &spec()));
}

#[test]
fn invalid_specs_are_refused() {
    let bad = |command: &str, t: u32| HookSpec {
        prefix: command.into(),
        timeout_secs: t,
    };
    assert!(plan_hooks("a: 1\n", &bad(" ", 10), false).is_err());
    assert!(plan_hooks("a: 1\n", &bad("x", 0), false).is_err());
    assert!(plan_hooks("a: 1\n", &bad("x", 301), false).is_err());
    assert!(plan_hooks("a: 1\n", &bad("x\ny", 10), false).is_err());
    assert_eq!(hooks_preview(&bad("x", 0), false).len(), 0);
    let q = plan_hooks("a: 1\n", &bad("it's", 10), false)
        .unwrap()
        .unwrap();
    assert!(q.contains("command: 'it''s "));
}

#[test]
fn a_begin_without_its_end_never_pairs_with_a_later_region() {
    let mut good = plan("model: x\n", true);
    let begin = good.lines().nth(1).unwrap().to_string();
    let lone = format!("{begin}\nuser: keep-me\nmore: user-text\n");
    let text = format!("{lone}{good}");
    assert_eq!(remove_hooks(&text), format!("{lone}model: x\n"));
    assert!(plan_hooks(&text, &spec(), false).is_err());
    // An END with no BEGIN is refused too.
    good.push_str("# skein-hooks:end\n");
    assert!(plan_hooks(&good, &spec(), false).is_err());
}

#[test]
fn auto_accept_blocks_only_for_a_value_other_than_true() {
    use super::yaml_hooks::auto_accept_blocks;
    assert!(!auto_accept_blocks("model: x\n"));
    assert!(!auto_accept_blocks("hooks_auto_accept: true\n"));
    assert!(auto_accept_blocks("hooks_auto_accept: false\n"));
    assert!(auto_accept_blocks("hooks_auto_accept: maybe\n"));
}
