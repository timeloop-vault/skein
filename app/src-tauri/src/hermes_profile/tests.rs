use super::yaml::{
    BEGIN, entry_preview, insert_skein_entry, remove_skein_entry, skein_entry_present,
};
use super::*;

fn ins(t: &str) -> String {
    insert_skein_entry(t).expect("accepted").expect("inserted")
}

fn refused(t: &str) -> bool {
    insert_skein_entry(t).is_err()
}

/// Insert then remove must give back the input byte for byte.
fn round_trip(t: &str) -> String {
    let out = ins(t);
    assert!(skein_entry_present(&out), "present after insert: {out:?}");
    assert_eq!(remove_skein_entry(&out).as_deref(), Some(t), "{out:?}");
    out
}

#[test]
fn empty_file_is_just_the_block() {
    let out = round_trip("");
    assert!(out.starts_with(BEGIN));
    assert!(out.contains("\nmcp_servers:\n  skein:\n    url: ${SKEIN_REVIEW_URL}\n"));
    assert!(out.contains("      Authorization: Bearer ${SKEIN_REVIEW_TOKEN}\n"));
    assert!(out.contains("      X-Skein-Harness: ${SKEIN_HARNESS_ID}\n"));
    assert!(out.ends_with("# skein:end\n"));
}

#[test]
fn appends_when_there_is_no_mcp_servers() {
    let t = "model: x\n# note\nagent:\n  a: 1\n";
    let out = round_trip(t);
    assert!(out.starts_with(t));
    // Case (a) removal takes the key it created too.
    assert!(!remove_skein_entry(&out).unwrap().contains("mcp_servers"));
}

#[test]
fn no_trailing_newline_round_trips() {
    for t in ["model: x", "mcp_servers:", "mcp_servers:\n  a:\n    url: u"] {
        let out = round_trip(t);
        assert!(!out.ends_with('\n'), "{out:?}");
    }
}

#[test]
fn crlf_and_bom_are_preserved() {
    let t = "\u{feff}model: x\r\nmcp_servers:\r\n  a:\r\n    url: u\r\nother: 1\r\n";
    let out = round_trip(t);
    assert!(out.starts_with('\u{feff}'));
    assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    let out = round_trip("\u{feff}model: x\r\n");
    assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    round_trip("\u{feff}");
}

#[test]
fn inserts_after_last_child_before_next_key() {
    let t = "a: 1\nmcp_servers:\n  one:\n    url: u\n    headers:\n      k: v\n\n# about b\nb: 2\n";
    let out = ins(t);
    let expected_head =
        "a: 1\nmcp_servers:\n  one:\n    url: u\n    headers:\n      k: v\n  # skein:begin";
    assert!(out.starts_with(expected_head), "{out}");
    assert!(out.ends_with("  # skein:end\n\n# about b\nb: 2\n"), "{out}");
    round_trip(t);
}

#[test]
fn keeps_four_space_unit_and_trailing_comments() {
    let t = "mcp_servers:\n    one:\n        url: u\n    # old = 1\nz: 1\n";
    let out = ins(t);
    assert!(out.contains("\n    # skein:begin"), "{out}");
    assert!(
        out.contains("\n    skein:\n        url: ${SKEIN_REVIEW_URL}\n"),
        "{out}"
    );
    assert!(
        out.ends_with("    # skein:end\n    # old = 1\nz: 1\n"),
        "{out}"
    );
    round_trip(t);
}

#[test]
fn comments_between_children_and_column0_comments() {
    let t = "mcp_servers:\n  # first\n  one:\n    url: u\n# col0 comment\n  two:\n    url: v\n";
    let out = ins(t);
    assert!(out.contains("    url: v\n  # skein:begin"), "{out}");
    round_trip(t);
}

#[test]
fn empty_value_gets_children_directly_under() {
    for t in [
        "mcp_servers:\nz: 1\n",
        "mcp_servers: # none yet\nz: 1\n",
        "mcp_servers:\n",
        "mcp_servers:\n\n# c\n",
    ] {
        let out = ins(t);
        assert!(
            out.contains("mcp_servers:") && out.contains("\n  # skein:begin"),
            "{out}"
        );
        let after = remove_skein_entry(&out).unwrap();
        assert_eq!(after, t);
        // Case (c): the user's own key survives removal.
        assert!(after.contains("mcp_servers:"));
    }
}

#[test]
fn explicit_null_is_refused() {
    assert!(refused("mcp_servers: null\n"));
    assert!(refused("mcp_servers: ~\n"));
}

#[test]
fn existing_skein_server_means_no_edit() {
    let t = "mcp_servers:\n  other:\n    url: u\n  skein:\n    url: mine\n";
    assert!(skein_entry_present(t));
    assert_eq!(insert_skein_entry(t), Ok(None));
    assert_eq!(entry_preview(t).as_deref(), Ok(""));
    // Unmarked: never removed.
    assert_eq!(remove_skein_entry(t), None);
    assert!(skein_entry_present(
        "mcp_servers:\n  \"skein\":\n    url: u\n"
    ));
}

#[test]
fn only_a_top_level_mcp_servers_counts() {
    let t = "outer:\n  mcp_servers:\n    skein:\n      url: u\n";
    assert!(!skein_entry_present(t));
    let out = ins(t);
    assert!(out.starts_with(t));
    assert!(out.contains("\nmcp_servers:\n"));
    // And a skein key nested deeper is not a direct child.
    let t = "mcp_servers:\n  other:\n    skein: 1\n";
    assert!(!skein_entry_present(t));
    ins(t);
}

#[test]
fn other_servers_are_untouched_and_remove_finds_nothing_without_markers() {
    let t = "mcp_servers:\n  a:\n    url: u\n";
    assert_eq!(remove_skein_entry(t), None);
    assert_eq!(remove_skein_entry(""), None);
}

#[test]
fn removal_keeps_children_added_after_a_created_block() {
    let out = ins("model: x\n");
    let edited = format!("{out}  mine:\n    url: u\n");
    let removed = remove_skein_entry(&edited).unwrap();
    assert_eq!(removed, "model: x\nmcp_servers:\n  mine:\n    url: u\n");
}

#[test]
fn refusals() {
    for (name, t) in [
        ("flow map", "mcp_servers: {a: {url: u}}\n"),
        ("flow seq", "mcp_servers: []\n"),
        ("tabs", "mcp_servers:\n\ta:\n\t\turl: u\n"),
        ("two docs", "a: 1\n---\nb: 2\n"),
        ("two docs, leading", "---\na: 1\n---\nb: 2\n"),
        ("doc end", "a: 1\n...\n"),
        ("anchor", "mcp_servers: &m\n  a:\n    url: u\n"),
        ("alias", "mcp_servers: *m\n"),
        ("merge", "mcp_servers:\n  <<: *base\n"),
        ("list", "mcp_servers:\n  - a\n"),
        ("col0 list", "mcp_servers:\n- a\n"),
        ("scalar", "mcp_servers: nope\n"),
        ("quoted key", "\"mcp_servers\":\n  a:\n    url: u\n"),
    ] {
        assert!(refused(t), "{name} should be refused");
        assert!(entry_preview(t).is_err(), "{name} preview");
    }
    // A single leading document marker is fine.
    ins("---\na: 1\n");
}

#[test]
fn preview_is_exactly_the_inserted_lines() {
    let t = "mcp_servers:\r\n  a:\r\n    url: u\r\n";
    let p = entry_preview(t).unwrap();
    let out = ins(t);
    assert!(out.contains(&format!("{p}\r\n")), "{out}");
    assert!(p.starts_with("  # skein:begin") && p.ends_with("# skein:end"));
}

#[test]
fn atomic_write_checks_and_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.yaml");
    write_config_atomic(&p, None, b"a: 1\n").unwrap();
    assert!(write_config_atomic(&p, None, b"x").is_err());
    assert!(write_config_atomic(&p, Some(b"stale"), b"x").is_err());
    write_config_atomic(&p, Some(b"a: 1\n"), b"b: 2\n").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"b: 2\n");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temp file left behind");
    let missing = dir.path().join("none.yaml");
    assert!(write_config_atomic(&missing, Some(b"x"), b"y").is_err());
}

#[test]
fn ensure_and_remove_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.yaml");
    assert_eq!(
        ensure_skein_entry(&p).unwrap_err(),
        "profile config not found"
    );
    assert!(!p.exists(), "ensure never creates the file");

    let original = "model: x\nmcp_servers:\n  a:\n    url: u\n";
    std::fs::write(&p, original).unwrap();
    assert_eq!(ensure_skein_entry(&p).unwrap(), EnsureOutcome::Added);
    assert!(skein_entry_present(&std::fs::read_to_string(&p).unwrap()));
    assert_eq!(
        ensure_skein_entry(&p).unwrap(),
        EnsureOutcome::AlreadyPresent
    );
    assert!(remove_skein_entry_at(&p).unwrap());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), original);
    assert!(!remove_skein_entry_at(&p).unwrap());
}

#[test]
fn creating_surfaces_a_real_write_error() {
    let dir = tempfile::tempdir().unwrap();
    // The parent is a regular file, so the file is absent and can't be made.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let err = ensure_skein_entry_creating(&blocker.join("config.yaml")).unwrap_err();
    // Windows reports the failed write; Unix fails the first read (ENOTDIR).
    // Either way a real error names the path, never "profile config not found".
    assert!(
        err.contains("cannot write") || err.contains("cannot read"),
        "{err}"
    );
    assert!(err.contains("config.yaml"), "{err}");
}

#[test]
fn owned_off_then_on_deletes_then_recreates_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.yaml");
    assert_eq!(
        ensure_skein_entry_creating(&p).unwrap(),
        EnsureOutcome::Added
    );
    assert!(remove_skein_entry_owned(&p).unwrap());
    assert!(!p.exists(), "an emptied config is deleted");
    assert_eq!(
        ensure_skein_entry_creating(&p).unwrap(),
        EnsureOutcome::Added
    );
    assert!(skein_entry_present(&std::fs::read_to_string(&p).unwrap()));

    // A config with other content is kept.
    std::fs::write(&p, "model: x\n").unwrap();
    ensure_skein_entry(&p).unwrap();
    assert!(remove_skein_entry_owned(&p).unwrap());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "model: x\n");
}

#[test]
fn non_utf8_and_refused_files_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.yaml");
    std::fs::write(&p, [0xff, 0xfe, b'a']).unwrap();
    assert!(ensure_skein_entry(&p).unwrap_err().contains("UTF-8"));
    assert!(remove_skein_entry_at(&p).is_err());
    assert_eq!(std::fs::read(&p).unwrap(), [0xff, 0xfe, b'a']);

    std::fs::write(&p, "mcp_servers: {}\n").unwrap();
    assert!(ensure_skein_entry(&p).is_err());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "mcp_servers: {}\n");
}

#[test]
fn profile_constant() {
    assert_eq!(SKEIN_PROFILE, "skein");
    assert!(skein_harness::hermes::valid_profile_name(SKEIN_PROFILE));
}

#[test]
fn a_flow_or_sequence_root_is_refused_when_appending() {
    for t in [
        "{a: 1}\n",
        "[1, 2]\n",
        "# c\n\n- a\n- b\n",
        "---\n- a\n",
        "\u{feff}{a: 1}\n",
        "-\n",
    ] {
        assert!(refused(t), "{t:?}");
    }
    assert!(!refused("# c\n---\na: 1\n"));
    assert!(!refused("# only a comment\n"));
}

#[test]
fn a_block_scalar_in_mcp_servers_refuses_the_insert() {
    for ind in ["|", ">", "|-", "|+", ">-", ">+", "|2", ">2-", "| # note"] {
        let t =
            format!("mcp_servers:\n  other:\n    note: {ind}\n      # not a comment\n      text\n");
        assert!(refused(&t), "{ind}");
    }
    // A block scalar elsewhere in the file is fine.
    assert!(!refused(
        "x: |\n  text\nmcp_servers:\n  other:\n    url: u\n"
    ));
    // A value that merely contains the characters is fine.
    assert!(!refused("mcp_servers:\n  other:\n    cmd: a | b\n"));
}

#[test]
fn removal_refuses_an_edited_marker_or_a_block_scalar_around_it() {
    let out = ins("mcp_servers:\n  other:\n    url: u\n");
    assert!(remove_skein_entry(&out).is_some());
    let edited = out.replace(BEGIN, "# skein:begin (edited)");
    assert_eq!(remove_skein_entry(&edited), None);
    let scalar = out.replace("    url: u\n", "    note: |\n      text\n");
    assert_eq!(remove_skein_entry(&scalar), None);
}

#[test]
fn effective_root_prefers_the_settings_hermes_home() {
    use crate::spawn_settings::EnvVar;
    let var = |k: &str, v: &str| EnvVar {
        key: k.to_string(),
        value: v.to_string(),
    };
    let mut s = SpawnSettings {
        extra_env: vec![
            var("HERMES_HOME", "/first"),
            var(" hermes_home ", " /second "),
        ],
        ..SpawnSettings::default()
    };
    assert_eq!(effective_root(&s), Some(PathBuf::from("/second")));
    // An empty value is ignored: fall back to the earlier one.
    s.extra_env.push(var("HERMES_HOME", "  "));
    assert_eq!(effective_root(&s), Some(PathBuf::from("/second")));
    // No setting: the process-environment answer, whatever it is.
    s.extra_env = vec![var("OTHER", "/x")];
    assert_eq!(effective_root(&s), skein_harness::hermes::hermes_home());
}

#[test]
fn atomic_write_temp_names_are_unique() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("config.yaml");
    write_config_atomic(&p, None, b"a").expect("first");
    write_config_atomic(&p, Some(b"a"), b"b").expect("second");
    assert_eq!(std::fs::read(&p).expect("read"), b"b");
    assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 1);
}
