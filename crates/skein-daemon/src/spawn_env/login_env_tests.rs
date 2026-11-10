use super::*;
use crate::spawn_env::{PATH_PROBE_END, PATH_PROBE_START, PROBE_SCRIPT};

fn wrap(payload: &[u8]) -> Vec<u8> {
    let mut v = ENV_PROBE_START.as_bytes().to_vec();
    v.extend_from_slice(payload);
    v.extend_from_slice(ENV_PROBE_END.as_bytes());
    v
}

fn kv(k: &str, v: &str) -> (String, String) {
    (k.to_owned(), v.to_owned())
}

#[test]
fn probe_script_uses_the_env_sentinels() {
    assert!(PROBE_SCRIPT.contains(ENV_PROBE_START));
    assert!(PROBE_SCRIPT.contains(ENV_PROBE_END));
    assert!(PROBE_SCRIPT.contains("env -0 || perl"));
    for forbidden in ["2>", "$(", "{ ", ">/"] {
        assert!(!PROBE_SCRIPT.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn parses_nul_separated_entries_sorted() {
    let out = extract_probe_env(&wrap(b"B=2\0A=1\0")).unwrap();
    assert_eq!(out, vec![kv("A", "1"), kv("B", "2")]);
}

#[test]
fn value_with_newline_and_equals_and_empty_value() {
    let out = extract_probe_env(&wrap(b"A=l1\nl2\0B=x=y=z\0C=\0")).unwrap();
    assert_eq!(out, vec![kv("A", "l1\nl2"), kv("B", "x=y=z"), kv("C", "")]);
}

#[test]
fn garbage_around_payload_is_ignored() {
    let mut raw = b"banner\0junk\n".to_vec();
    raw.extend(wrap(b"A=1\0"));
    raw.extend_from_slice(b"\0trap garbage\0X=9");
    assert_eq!(extract_probe_env(&raw).unwrap(), vec![kv("A", "1")]);
}

#[test]
fn last_end_sentinel_wins() {
    let mut raw = ENV_PROBE_START.as_bytes().to_vec();
    raw.extend_from_slice(b"A=1\0");
    raw.extend_from_slice(ENV_PROBE_END.as_bytes());
    raw.extend_from_slice(b"B=2\0");
    raw.extend_from_slice(ENV_PROBE_END.as_bytes());
    let out = extract_probe_env(&raw).unwrap();
    // The payload extends to the final sentinel, so the first END text
    // is part of a (junk) segment and the later entry is still seen.
    assert!(out.contains(&kv("A", "1")));
    assert!(out.iter().any(|(k, v)| k.ends_with('B') && v == "2"));
}

#[test]
fn missing_sentinels_or_empty_payload_is_none() {
    let mut no_end = ENV_PROBE_START.as_bytes().to_vec();
    no_end.extend_from_slice(b"A=1\0");
    assert_eq!(extract_probe_env(&no_end), None);
    assert_eq!(extract_probe_env(b"A=1\0"), None);
    assert_eq!(extract_probe_env(&wrap(b"")), None);
    assert_eq!(extract_probe_env(&wrap(b"\0\0")), None);
    assert_eq!(extract_probe_env(&wrap(b"nokey\0=v\0")), None);
}

#[test]
fn non_utf8_and_no_equals_entries_are_skipped() {
    let out = extract_probe_env(&wrap(b"A=1\0B=\xff\xfe\0noequals\0=empty\0C=3\0")).unwrap();
    assert_eq!(out, vec![kv("A", "1"), kv("C", "3")]);
}

#[test]
fn duplicate_keys_last_wins() {
    let out = extract_probe_env(&wrap(b"A=1\0A=2\0")).unwrap();
    assert_eq!(out, vec![kv("A", "2")]);
}

#[test]
fn path_section_before_env_does_not_confuse_it() {
    let mut raw = format!("{PATH_PROBE_START}/usr/bin:/bin{PATH_PROBE_END}").into_bytes();
    raw.extend(wrap(b"PATH=/usr/bin:/bin\0A=1\0"));
    assert_eq!(
        extract_probe_env(&raw).unwrap(),
        vec![kv("A", "1"), kv("PATH", "/usr/bin:/bin")]
    );
}

#[test]
fn noise_filter_drops_each_noise_key_and_keeps_others() {
    for k in [
        "SHLVL",
        "PWD",
        "OLDPWD",
        "_",
        "SKEIN_PROBE",
        "DISABLE_AUTO_UPDATE",
        "PATH",
    ] {
        assert!(is_login_env_noise(k), "{k}");
    }
    for k in ["HTTP_PROXY", "TERM", "SHELL", "pwd", "Path", "HOME"] {
        assert!(!is_login_env_noise(k), "{k}");
    }
}

#[test]
fn overlay_drops_only_noise_and_keeps_order() {
    let cap = vec![
        kv("Z", "1"),
        kv("PWD", "/x"),
        kv("HTTP_PROXY", "http://p"),
        kv("PATH", "/bin"),
        kv("A", "2"),
    ];
    assert_eq!(
        login_env_overlay(&cap),
        vec![kv("Z", "1"), kv("HTTP_PROXY", "http://p"), kv("A", "2")]
    );
}

#[test]
fn no_proxy_both_unset() {
    assert_eq!(merge_no_proxy(None, None), "127.0.0.1,localhost,::1");
}

#[test]
fn no_proxy_upper_only_comes_first() {
    assert_eq!(
        merge_no_proxy(Some("corp.example,.internal"), None),
        "corp.example,.internal,127.0.0.1,localhost,::1"
    );
}

#[test]
fn no_proxy_lower_with_localhost_is_not_duplicated() {
    assert_eq!(
        merge_no_proxy(None, Some("localhost,a.b")),
        "localhost,a.b,127.0.0.1,::1"
    );
}

#[test]
fn no_proxy_union_dedupes_overlap() {
    assert_eq!(
        merge_no_proxy(Some("a.com,b.com"), Some("b.com,c.com")),
        "a.com,b.com,c.com,127.0.0.1,localhost,::1"
    );
}

#[test]
fn no_proxy_trims_and_drops_empty_entries() {
    assert_eq!(
        merge_no_proxy(Some(" a.com , ,,b.com "), Some("")),
        "a.com,b.com,127.0.0.1,localhost,::1"
    );
}

#[test]
fn no_proxy_dedupes_case_insensitively_keeping_first() {
    assert_eq!(
        merge_no_proxy(Some("LOCALHOST,Corp.Example"), Some("corp.example")),
        "LOCALHOST,Corp.Example,127.0.0.1,::1"
    );
}
