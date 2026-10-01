use super::*;

/// Split a joined `PATH` back into comparable pieces, so assertions
/// don't hardcode `:` vs `;`.
fn parts(joined: &OsStr) -> Vec<String> {
    std::env::split_paths(joined)
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// The resolved PATH of a merge, as comparable pieces.
fn merged_parts(m: &MergedPath) -> Vec<String> {
    parts(&m.path)
}

fn reasons(m: &MergedPath) -> Vec<(String, DropReason)> {
    m.dropped.clone()
}

fn join(entries: &[&str]) -> OsString {
    std::env::join_paths(entries.iter().map(Path::new)).expect("test paths are joinable")
}

fn no_vars(_: &str) -> Option<String> {
    None
}

fn home() -> PathBuf {
    PathBuf::from("/home/tester")
}

// ── probe_args ────────────────────────────────────────────────

#[test]
fn probe_args_posix_shells_use_separate_flags() {
    // Pinned as three entries on purpose: the bundled "-ilc" form is
    // what broke csh/tcsh, and a "tidy-up" back to one string would
    // silently reintroduce it.
    for shell in [
        "/bin/sh",
        "/bin/bash",
        "/bin/zsh",
        "/bin/ksh",
        "/bin/dash",
        "/opt/homebrew/bin/fish",
    ] {
        assert_eq!(
            probe_args(shell, CaptureMode::LoginInteractive),
            Some(&["-l", "-i", "-c"][..]),
            "{shell}"
        );
        assert_eq!(
            probe_args(shell, CaptureMode::Login),
            Some(&["-l", "-c"][..]),
            "{shell}"
        );
    }
}

#[test]
fn probe_args_csh_family_drops_the_login_flag() {
    for shell in ["/bin/csh", "/bin/tcsh"] {
        for mode in [CaptureMode::LoginInteractive, CaptureMode::Login] {
            assert_eq!(probe_args(shell, mode), Some(&["-i", "-c"][..]), "{shell}");
        }
    }
}

#[test]
fn probe_args_unknown_shells_are_skipped_not_guessed() {
    for shell in [
        "/usr/bin/nu",
        "/usr/bin/xonsh",
        "/usr/bin/elvish",
        "/usr/local/bin/pwsh",
        "powershell.exe",
        "",
    ] {
        assert_eq!(
            probe_args(shell, CaptureMode::LoginInteractive),
            None,
            "{shell}"
        );
    }
}

#[test]
fn probe_args_tolerates_versioned_names() {
    let m = CaptureMode::LoginInteractive;
    assert_eq!(
        probe_args("/usr/bin/bash-5.2", m),
        Some(&["-l", "-i", "-c"][..])
    );
    assert_eq!(
        probe_args("/usr/bin/zsh-5.9", m),
        Some(&["-l", "-i", "-c"][..])
    );
}

#[test]
fn capture_mode_none_skips_every_shell() {
    // The escape hatch: "don't ask a shell at all". It has to hold
    // even for shells we know how to drive.
    for shell in ["/bin/zsh", "/bin/bash", "/bin/tcsh"] {
        assert_eq!(probe_args(shell, CaptureMode::None), None, "{shell}");
    }
}

// ── extract_probe_path ────────────────────────────────────────

#[test]
fn probe_script_uses_the_declared_sentinels() {
    assert!(PROBE_SCRIPT.contains(PATH_PROBE_START));
    assert!(PROBE_SCRIPT.contains(PATH_PROBE_END));
}

#[test]
fn extract_probe_path_ignores_surrounding_shell_noise() {
    let out = format!("Now using node v22\n{PATH_PROBE_START}/usr/bin:/bin{PATH_PROBE_END}\nbye\n");
    assert_eq!(extract_probe_path(&out).as_deref(), Some("/usr/bin:/bin"));
}

#[test]
fn extract_probe_path_rejects_a_truncated_payload() {
    // The shape a shell killed on the deadline leaves behind. A
    // best-effort prefix here would silently drop the tail of the
    // user's PATH, which is worse than having no probe result.
    let out = format!("{PATH_PROBE_START}/usr/bin:/opt/homebrew/b");
    assert_eq!(extract_probe_path(&out), None);
}

#[test]
fn extract_probe_path_rejects_missing_or_empty_payloads() {
    assert_eq!(extract_probe_path(""), None);
    assert_eq!(extract_probe_path("no sentinels at all"), None);
    assert_eq!(
        extract_probe_path(&format!("{PATH_PROBE_END}/usr/bin")),
        None
    );
    assert_eq!(
        extract_probe_path(&format!("{PATH_PROBE_START}{PATH_PROBE_END}")),
        None
    );
}

#[test]
fn extract_probe_path_takes_the_first_complete_payload() {
    let out = format!(
        "{PATH_PROBE_START}/first{PATH_PROBE_END}{PATH_PROBE_START}/second{PATH_PROBE_END}"
    );
    assert_eq!(extract_probe_path(&out).as_deref(), Some("/first"));
}

// ── expand_entry ──────────────────────────────────────────────

#[test]
fn expand_entry_expands_leading_tilde() {
    assert_eq!(
        expand_entry("~/.local/bin", Some(&home()), &no_vars).as_deref(),
        Some("/home/tester/.local/bin")
    );
    assert_eq!(
        expand_entry("~", Some(&home()), &no_vars).as_deref(),
        Some("/home/tester")
    );
}

#[test]
fn expand_entry_refuses_tilde_user() {
    // Needs a passwd lookup we don't do; emitting it literally would
    // put a directory that cannot exist into PATH.
    assert_eq!(expand_entry("~root/bin", Some(&home()), &no_vars), None);
}

#[test]
fn expand_entry_expands_variable_forms() {
    assert_eq!(
        expand_entry("$HOME/bin", Some(&home()), &no_vars).as_deref(),
        Some("/home/tester/bin")
    );
    assert_eq!(
        expand_entry("${HOME}/bin", Some(&home()), &no_vars).as_deref(),
        Some("/home/tester/bin")
    );
    assert_eq!(
        expand_entry("%USERPROFILE%/bin", Some(&home()), &no_vars).as_deref(),
        Some("/home/tester/bin")
    );
}

#[test]
fn expand_entry_uses_the_injected_lookup() {
    let lookup = |k: &str| (k == "SDK").then(|| "/opt/sdk".to_owned());
    assert_eq!(
        expand_entry("$SDK/bin", None, &lookup).as_deref(),
        Some("/opt/sdk/bin")
    );
    assert_eq!(
        expand_entry("%SDK%/bin", None, &lookup).as_deref(),
        Some("/opt/sdk/bin")
    );
}

#[test]
fn expand_entry_drops_entries_referencing_unset_variables() {
    // Better a missing entry than a literal "%LOCALAPPDATA%\bin" in
    // PATH, which silently matches nothing forever.
    assert_eq!(expand_entry("$NOPE/bin", Some(&home()), &no_vars), None);
    assert_eq!(expand_entry("%NOPE%/bin", Some(&home()), &no_vars), None);
    assert_eq!(expand_entry("~/x", None, &no_vars), None);
}

#[test]
fn expand_entry_leaves_stray_sigils_literal() {
    assert_eq!(
        expand_entry("/opt/100%/bin", Some(&home()), &no_vars).as_deref(),
        Some("/opt/100%/bin")
    );
    assert_eq!(
        expand_entry("/opt/a$/bin", Some(&home()), &no_vars).as_deref(),
        Some("/opt/a$/bin")
    );
}

#[test]
fn expand_entry_ignores_blank_input() {
    assert_eq!(expand_entry("", Some(&home()), &no_vars), None);
    assert_eq!(expand_entry("   ", Some(&home()), &no_vars), None);
}

#[test]
fn expand_entry_handles_multibyte_content() {
    assert_eq!(
        expand_entry("~/kläder/bin", Some(&home()), &no_vars).as_deref(),
        Some("/home/tester/kläder/bin")
    );
}

// ── merge_path ────────────────────────────────────────────────
//
// Split by platform (#202). `merge_path` is platform-generic —
// `split_paths`/`join_paths` for the separator, a cfg'd
// `PATH_ENTRY_FORBIDDEN` — but its *fixtures* cannot be: `/usr/bin`
// is not absolute on Windows (an absolute path there needs a prefix,
// not just a root), so a Unix-shaped base is entirely discarded and
// the assertions would be wrong to pass. The cases below are the
// Unix half; `windows_merge_path` mirrors each one, and anything
// genuinely platform-neutral (`merge_path_is_idempotent`,
// `concat_paths_joins_with_the_platform_separator`) stays ungated.

#[test]
#[cfg(unix)]
fn merge_path_prepends_existing_directories_in_order() {
    let base = join(&["/usr/bin", "/bin"]);
    let exists = |p: &Path| p.starts_with("/home/tester");
    let out = merge_path(
        &base,
        &["~/.local/bin".into(), "~/bin".into()],
        Some(&home()),
        &no_vars,
        &exists,
    );
    assert_eq!(
        merged_parts(&out),
        vec![
            "/home/tester/.local/bin",
            "/home/tester/bin",
            "/usr/bin",
            "/bin"
        ]
    );
}

#[test]
#[cfg(unix)]
fn merge_path_skips_directories_that_do_not_exist() {
    let base = join(&["/usr/bin"]);
    let exists = |p: &Path| p != Path::new("/home/tester/bin");
    let out = merge_path(
        &base,
        &["~/.local/bin".into(), "~/bin".into()],
        Some(&home()),
        &no_vars,
        &exists,
    );
    assert_eq!(
        merged_parts(&out),
        vec!["/home/tester/.local/bin", "/usr/bin"]
    );
}

#[test]
fn merge_path_is_idempotent() {
    let base = join(&["/usr/bin", "/bin"]);
    let prepends = vec!["~/.local/bin".to_owned(), "~/bin".to_owned()];
    let exists = |_: &Path| true;
    let once = merge_path(&base, &prepends, Some(&home()), &no_vars, &exists);
    let twice = merge_path(&once.path, &prepends, Some(&home()), &no_vars, &exists);
    assert_eq!(merged_parts(&once), merged_parts(&twice));
}

#[test]
#[cfg(unix)]
fn merge_path_collapses_duplicates_including_trailing_separators() {
    let base = join(&["/usr/bin", "/home/tester/bin/"]);
    let out = merge_path(
        &base,
        &["~/bin".into(), "~/bin".into()],
        Some(&home()),
        &no_vars,
        &|_| true,
    );
    assert_eq!(merged_parts(&out), vec!["/usr/bin", "/home/tester/bin/"]);
}

#[test]
#[cfg(unix)]
fn merge_path_never_emits_an_empty_entry() {
    // An empty PATH element means *the current directory* on Unix.
    // Inherited into an agent harness that runs what it finds, that
    // is a real hazard — and it is exactly what splitting an empty
    // PATH produces.
    let out = merge_path(
        OsStr::new(""),
        &["~/bin".into()],
        Some(&home()),
        &no_vars,
        &|_| true,
    );
    assert_eq!(merged_parts(&out), vec!["/home/tester/bin"]);
    assert!(!merged_parts(&out).iter().any(String::is_empty));

    let with_hole = join(&["/usr/bin", "", "/bin"]);
    let out = merge_path(&with_hole, &[], Some(&home()), &no_vars, &|_| true);
    assert!(!merged_parts(&out).iter().any(String::is_empty));
}

#[test]
#[cfg(unix)]
fn merge_path_rejects_an_entry_containing_the_separator() {
    // Must be dropped whole, never split into two entries — and it
    // must not fail the merge for everything else.
    let sneaky = "/a:/b".to_owned();
    let base = join(&["/usr/bin"]);
    let out = merge_path(&base, &[sneaky], Some(&home()), &no_vars, &|_| true);
    assert_eq!(merged_parts(&out), vec!["/usr/bin"]);
}

#[test]
fn concat_paths_joins_with_the_platform_separator() {
    // Compiled and exercised on every platform even though only the
    // Windows branch calls it — a cfg'd union would be verifiable
    // only on the machine that can't easily run these tests.
    let a = join(&["/one", "/two"]);
    let b = join(&["/three"]);
    assert_eq!(
        parts(&concat_paths(Some(a.clone()), Some(b.clone()))),
        vec!["/one", "/two", "/three"]
    );
    assert_eq!(
        parts(&concat_paths(Some(a.clone()), None)),
        vec!["/one", "/two"]
    );
    assert_eq!(parts(&concat_paths(None, Some(b))), vec!["/three"]);
    assert_eq!(concat_paths(None, None), OsString::new());
    // Overlap is left to merge_path's dedupe, not silently dropped
    // here, so the two responsibilities stay separable.
    assert_eq!(
        parts(&concat_paths(Some(a.clone()), Some(a))),
        vec!["/one", "/two", "/one", "/two"]
    );
}

#[test]
#[cfg(unix)]
fn merge_path_refuses_relative_additions() {
    // Same hazard as an empty entry, and this is the side the user
    // controls *and* the side that goes first: a `.` here makes
    // every agent command prefer whatever is in the worktree.
    let base = join(&["/usr/bin"]);
    let out = merge_path(
        &base,
        &[".".into(), "..".into(), "bin".into()],
        Some(&home()),
        &no_vars,
        &|_| true,
    );
    assert_eq!(merged_parts(&out), vec!["/usr/bin"]);
    assert_eq!(
        reasons(&out),
        vec![
            (".".to_owned(), DropReason::NotAbsolute),
            ("..".to_owned(), DropReason::NotAbsolute),
            ("bin".to_owned(), DropReason::NotAbsolute),
        ]
    );
}

#[test]
#[cfg(unix)]
fn merge_path_reports_why_each_addition_was_dropped() {
    // "I added a directory and nothing happened" is the confusion
    // the whole settings panel exists to end, so every skip has to
    // be attributable to a reason the UI can show.
    let base = join(&["/usr/bin", "/home/tester/bin"]);
    let sep = "/a:/b";
    let out = merge_path(
        &base,
        &[
            "$NOPE/bin".into(),
            sep.to_owned(),
            "/does/not/exist".into(),
            "~/bin".into(),
        ],
        Some(&home()),
        &no_vars,
        &|p| p != Path::new("/does/not/exist"),
    );
    assert_eq!(
        reasons(&out),
        vec![
            ("$NOPE/bin".to_owned(), DropReason::Unresolved),
            (sep.to_owned(), DropReason::Separator),
            ("/does/not/exist".to_owned(), DropReason::Missing),
            ("~/bin".to_owned(), DropReason::Duplicate),
        ]
    );
    assert_eq!(out.added, Vec::<String>::new());
}

#[test]
#[cfg(unix)]
fn merge_path_dedupes_the_base_too() {
    // Real shells hand back PATHs with repeated entries — this
    // machine's own does — and each duplicate is a wasted stat on
    // every command lookup for the life of the harness.
    let base = join(&["/usr/bin", "/bin", "/usr/bin", "/bin/"]);
    let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
    assert_eq!(merged_parts(&out), vec!["/usr/bin", "/bin"]);
}

#[test]
#[cfg(unix)]
fn merge_path_drops_relative_entries_from_the_base() {
    let base = join(&["/usr/bin", ".", "relative/dir"]);
    let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
    assert_eq!(merged_parts(&out), vec!["/usr/bin"]);
}

#[test]
#[cfg(unix)]
fn merge_path_reports_what_it_accepted() {
    let base = join(&["/usr/bin"]);
    let out = merge_path(
        &base,
        &["~/.local/bin".into(), "/does/not/exist".into()],
        Some(&home()),
        &no_vars,
        &|p| p != Path::new("/does/not/exist"),
    );
    // `added` must reflect the merge's own filters, not a
    // re-expansion of the raw list — the preview labels PATH rows
    // from it, and crediting Skein for an entry it dropped would
    // make the panel disagree with reality.
    assert_eq!(out.added, vec!["/home/tester/.local/bin".to_owned()]);
}

#[test]
#[cfg(unix)]
fn merge_path_with_no_additions_returns_the_base_unchanged() {
    let base = join(&["/usr/bin", "/bin"]);
    let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
    assert_eq!(merged_parts(&out), parts(&base));
}

/// The Windows half of the `merge_path` cases above (#202).
///
/// Same behaviours, Windows-shaped fixtures: `C:\…` bases, `;` as
/// the forbidden in-entry separator, `\` as the trailing separator
/// that must collapse. Two cases have no Unix counterpart because
/// the behaviour only exists here — case-insensitive dedupe and
/// `%VAR%` expansion — and one Unix case (the empty PATH element
/// meaning "current directory") is asserted for the same reason it
/// is there: an inherited empty entry is a real hazard for a harness
/// that runs what it finds.
#[cfg(windows)]
mod windows_merge_path {
    use super::{DropReason, Path, PathBuf, join, merge_path, merged_parts, no_vars, reasons};
    use std::ffi::OsStr;

    /// A Windows `HOME` for `~` expansion. `merge_path` only ever
    /// reads it as a prefix, so it needs to exist as a path shape,
    /// not on disk.
    fn home() -> PathBuf {
        PathBuf::from(r"C:\Users\tester")
    }

    #[test]
    fn prepends_existing_directories_in_order() {
        let base = join(&[r"C:\Windows\System32", r"C:\Windows"]);
        let exists = |p: &Path| p.starts_with(r"C:\Users\tester");
        let out = merge_path(
            &base,
            &[r"~\.local\bin".into(), r"~\bin".into()],
            Some(&home()),
            &no_vars,
            &exists,
        );
        assert_eq!(
            merged_parts(&out),
            vec![
                r"C:\Users\tester\.local\bin",
                r"C:\Users\tester\bin",
                r"C:\Windows\System32",
                r"C:\Windows",
            ]
        );
    }

    #[test]
    fn skips_directories_that_do_not_exist() {
        let base = join(&[r"C:\Windows\System32"]);
        let exists = |p: &Path| p != Path::new(r"C:\Users\tester\bin");
        let out = merge_path(
            &base,
            &[r"~\.local\bin".into(), r"~\bin".into()],
            Some(&home()),
            &no_vars,
            &exists,
        );
        assert_eq!(
            merged_parts(&out),
            vec![r"C:\Users\tester\.local\bin", r"C:\Windows\System32"]
        );
    }

    #[test]
    fn collapses_duplicates_including_trailing_separators() {
        let base = join(&[r"C:\Windows\System32", r"C:\Users\tester\bin\"]);
        let out = merge_path(
            &base,
            &[r"~\bin".into(), r"~\bin".into()],
            Some(&home()),
            &no_vars,
            &|_| true,
        );
        assert_eq!(
            merged_parts(&out),
            vec![r"C:\Windows\System32", r"C:\Users\tester\bin\"]
        );
    }

    /// Windows-only: `dedupe_key` lowercases, so entries differing
    /// only in case are the same directory and must collapse.
    /// Getting this wrong costs a redundant stat on every command
    /// lookup for the life of the harness.
    #[test]
    fn dedupes_case_insensitively() {
        let base = join(&[
            r"C:\Windows\System32",
            r"c:\windows\system32\",
            r"C:\Windows",
        ]);
        let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
        assert_eq!(
            merged_parts(&out),
            vec![r"C:\Windows\System32", r"C:\Windows"]
        );
    }

    #[test]
    fn never_emits_an_empty_entry() {
        let out = merge_path(
            OsStr::new(""),
            &[r"~\bin".into()],
            Some(&home()),
            &no_vars,
            &|_| true,
        );
        assert_eq!(merged_parts(&out), vec![r"C:\Users\tester\bin"]);
        assert!(!merged_parts(&out).iter().any(String::is_empty));

        let with_hole = join(&[r"C:\Windows\System32", "", r"C:\Windows"]);
        let out = merge_path(&with_hole, &[], Some(&home()), &no_vars, &|_| true);
        assert!(!merged_parts(&out).iter().any(String::is_empty));
    }

    /// `;` is the Windows separator, so an entry containing one must
    /// be dropped whole rather than split into two — and must not
    /// take the rest of the merge down with it.
    #[test]
    fn rejects_an_entry_containing_the_separator() {
        let base = join(&[r"C:\Windows\System32"]);
        let out = merge_path(
            &base,
            &[r"C:\a;C:\b".to_owned()],
            Some(&home()),
            &no_vars,
            &|_| true,
        );
        assert_eq!(merged_parts(&out), vec![r"C:\Windows\System32"]);
    }

    #[test]
    fn refuses_relative_additions() {
        let base = join(&[r"C:\Windows\System32"]);
        let out = merge_path(
            &base,
            &[
                ".".into(),
                "..".into(),
                "bin".into(),
                r"\rooted-no-prefix".into(),
            ],
            Some(&home()),
            &no_vars,
            &|_| true,
        );
        assert_eq!(merged_parts(&out), vec![r"C:\Windows\System32"]);
        assert_eq!(
            reasons(&out),
            vec![
                (".".to_owned(), DropReason::NotAbsolute),
                ("..".to_owned(), DropReason::NotAbsolute),
                ("bin".to_owned(), DropReason::NotAbsolute),
                // Root without a drive prefix is *not* absolute on
                // Windows — the case that makes the Unix fixtures
                // collapse to nothing here.
                (r"\rooted-no-prefix".to_owned(), DropReason::NotAbsolute),
            ]
        );
    }

    #[test]
    fn reports_why_each_addition_was_dropped() {
        let base = join(&[r"C:\Windows\System32", r"C:\Users\tester\bin"]);
        let out = merge_path(
            &base,
            &[
                r"%NOPE%\bin".into(),
                r"C:\a;C:\b".into(),
                r"C:\does\not\exist".into(),
                r"~\bin".into(),
            ],
            Some(&home()),
            &no_vars,
            &|p| p != Path::new(r"C:\does\not\exist"),
        );
        assert_eq!(
            reasons(&out),
            vec![
                (r"%NOPE%\bin".to_owned(), DropReason::Unresolved),
                (r"C:\a;C:\b".to_owned(), DropReason::Separator),
                (r"C:\does\not\exist".to_owned(), DropReason::Missing),
                (r"~\bin".to_owned(), DropReason::Duplicate),
            ]
        );
        assert_eq!(out.added, Vec::<String>::new());
    }

    /// Windows-only: `%VAR%` is the native form, and an addition
    /// that resolves through it must be credited like any other.
    #[test]
    fn expands_percent_vars_in_additions() {
        let base = join(&[r"C:\Windows\System32"]);
        let lookup = |name: &str| {
            (name == "LOCALAPPDATA").then(|| r"C:\Users\tester\AppData\Local".to_owned())
        };
        let out = merge_path(
            &base,
            &[r"%LOCALAPPDATA%\Programs\bin".into()],
            Some(&home()),
            &lookup,
            &|_| true,
        );
        assert_eq!(
            out.added,
            vec![r"C:\Users\tester\AppData\Local\Programs\bin".to_owned()]
        );
        assert_eq!(
            merged_parts(&out),
            vec![
                r"C:\Users\tester\AppData\Local\Programs\bin",
                r"C:\Windows\System32",
            ]
        );
    }

    #[test]
    fn dedupes_the_base_too() {
        let base = join(&[
            r"C:\Windows\System32",
            r"C:\Windows",
            r"C:\Windows\System32",
            r"C:\Windows\",
        ]);
        let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
        assert_eq!(
            merged_parts(&out),
            vec![r"C:\Windows\System32", r"C:\Windows"]
        );
    }

    #[test]
    fn drops_relative_entries_from_the_base() {
        let base = join(&[r"C:\Windows\System32", ".", r"relative\dir"]);
        let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
        assert_eq!(merged_parts(&out), vec![r"C:\Windows\System32"]);
    }

    #[test]
    fn reports_what_it_accepted() {
        let base = join(&[r"C:\Windows\System32"]);
        let out = merge_path(
            &base,
            &[r"~\.local\bin".into(), r"C:\does\not\exist".into()],
            Some(&home()),
            &no_vars,
            &|p| p != Path::new(r"C:\does\not\exist"),
        );
        assert_eq!(out.added, vec![r"C:\Users\tester\.local\bin".to_owned()]);
    }

    #[test]
    fn with_no_additions_returns_the_base_unchanged() {
        let base = join(&[r"C:\Windows\System32", r"C:\Windows"]);
        let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
        assert_eq!(merged_parts(&out), super::parts(&base));
    }
}

// ── host-terminal identity ────────────────────────────────────

#[test]
fn every_listed_host_terminal_var_is_stripped() {
    for key in HOST_TERMINAL_ENV_VARS {
        assert!(is_host_terminal_var(key), "{key} should be stripped");
    }
}

#[test]
fn the_keep_list_always_survives() {
    // The single most important assertion in this module. A careless
    // `SSH_*` prefix rule would eat the agent's ssh-agent socket and
    // break `git push` in a way that looks like a git bug.
    for key in HOST_TERMINAL_ENV_KEEP {
        assert!(!is_host_terminal_var(key), "{key} must be kept");
    }
}

#[test]
fn variables_the_harness_depends_on_are_untouched() {
    for key in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "SHELL",
        "USER",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
        "ANTHROPIC_API_KEY",
        "GITHUB_TOKEN",
        "NVM_DIR",
        "JAVA_HOME",
        "SSH_AUTH_SOCK",
        "CLAUDE_CODE_GIT_BASH_PATH",
    ] {
        assert!(!is_host_terminal_var(key), "{key} must be kept");
    }
}

#[test]
fn vendor_prefixes_match_their_families() {
    for key in [
        "KITTY_WINDOW_ID",
        "WEZTERM_PANE",
        "WEZTERM_UNIX_SOCKET",
        "ALACRITTY_SOCKET",
        "GHOSTTY_RESOURCES_DIR",
        "VSCODE_INJECTION",
        "VSCODE_GIT_IPC_HANDLE",
        "WT_SESSION",
        "ZELLIJ_SESSION_NAME",
        "ITERM_PROFILE",
        "LC_TERMINAL_VERSION",
        "VTE_VERSION",
        "KONSOLE_VERSION",
        "GNOME_TERMINAL_SCREEN",
        "MSYSTEM",
        "ANSICON_DEF",
    ] {
        assert!(is_host_terminal_var(key), "{key} should be stripped");
    }
}

#[test]
fn matching_is_case_insensitive() {
    // Windows environment keys are case-insensitive, and portable-pty
    // lowercases them in its own map.
    assert!(is_host_terminal_var("tmux"));
    assert!(is_host_terminal_var("Term_Program"));
    assert!(is_host_terminal_var("wt_session"));
    assert!(!is_host_terminal_var("ssh_auth_sock"));
}

#[test]
fn the_lists_contain_no_duplicates() {
    for list in [
        HOST_TERMINAL_ENV_VARS,
        HOST_TERMINAL_ENV_PREFIXES,
        HOST_TERMINAL_ENV_KEEP,
    ] {
        let mut seen: Vec<String> = list.iter().map(|k| k.to_ascii_uppercase()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate entry in {list:?}");
    }
}
