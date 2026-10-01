use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use portable_pty::CommandBuilder;

use super::env::{RESERVED_ENV_KEYS, apply_env};
use super::preview::{describe_probe, env_preview, resolve_program};
use super::probe::{ProbeFailure, ProbeOutcome, probe_snapshot, run_probe};
use super::{PtyEvent, PtyManager, SpawnRequest};
use crate::harness_config::Injection;
use crate::harness_kind::HarnessKind;
use crate::spawn_env;
use crate::spawn_settings::{CaptureMode, SpawnSettings};
use std::os::unix::fs::PermissionsExt as _;

/// Write an executable stand-in shell. `probe_args` dispatches on the
/// file name, so naming the script `bash` makes `run_probe` drive it
/// exactly as it would drive a real one — which lets us pin the
/// behaviour that matters without depending on the developer's own
/// rc files.
fn fake_shell(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).expect("create fake shell");
    write!(f, "#!/bin/sh\n{body}\n").expect("write fake shell");
    drop(f);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake shell");
    path
}

const PAYLOAD: &str =
    "printf '%s%s%s' '___SKEIN_PATH_BEGIN___' '/probe/bin:/usr/bin' '___SKEIN_PATH_END___'";

#[test]
fn probe_survives_an_rc_file_that_backgrounds_a_daemon() {
    // The bug this whole rewrite exists for. `Command::output()`
    // drains stdout to EOF before reaping, so a grandchild holding
    // the write end blocks the read forever even though the shell
    // itself exited immediately. Measured: the pipe form hangs past
    // 8 s here, the spooled-file form returns in ~0.02 s.
    //
    // `sleep 45` outlives the 5 s deadline by design — if this test
    // ever starts taking 5 s, the file redirection has regressed
    // into a pipe.
    let dir = tempfile::tempdir().expect("tempdir");
    let shell = fake_shell(dir.path(), "bash", &format!("( sleep 45 ) &\n{PAYLOAD}"));

    let started = Instant::now();
    let outcome = run_probe(
        shell.to_str().expect("utf-8 path"),
        CaptureMode::LoginInteractive,
        Duration::from_secs(5),
        dir.path(),
    );

    assert_eq!(outcome.path(), Some("/probe/bin:/usr/bin"));
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "probe waited on the backgrounded grandchild ({:?})",
        started.elapsed()
    );
}

#[test]
fn probe_kills_a_shell_that_overruns_the_deadline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shell = fake_shell(dir.path(), "zsh", "sleep 30");

    let started = Instant::now();
    let outcome = run_probe(
        shell.to_str().expect("utf-8 path"),
        CaptureMode::LoginInteractive,
        Duration::from_millis(300),
        dir.path(),
    );

    assert!(
        matches!(
            outcome,
            ProbeOutcome::Failed {
                reason: ProbeFailure::Timeout,
                ..
            }
        ),
        "expected a timeout, got {outcome:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn probe_accepts_a_good_payload_from_a_shell_that_exits_non_zero() {
    // `bash -l -i -c` prints "no job control in this shell" and can
    // exit non-zero while having produced a perfectly usable PATH.
    // The old code threw those away.
    let dir = tempfile::tempdir().expect("tempdir");
    let shell = fake_shell(dir.path(), "bash", &format!("{PAYLOAD}\nexit 3"));

    let outcome = run_probe(
        shell.to_str().expect("utf-8 path"),
        CaptureMode::LoginInteractive,
        Duration::from_secs(5),
        dir.path(),
    );
    assert_eq!(outcome.path(), Some("/probe/bin:/usr/bin"));
}

#[test]
fn probe_reports_no_payload_rather_than_inventing_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shell = fake_shell(dir.path(), "bash", "echo 'welcome to your shell'");

    let outcome = run_probe(
        shell.to_str().expect("utf-8 path"),
        CaptureMode::LoginInteractive,
        Duration::from_secs(5),
        dir.path(),
    );
    assert!(matches!(
        outcome,
        ProbeOutcome::Failed {
            reason: ProbeFailure::NoPayload,
            ..
        }
    ));
    assert_eq!(outcome.path(), None);
}

#[test]
fn probe_skips_shells_it_cannot_drive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shell = fake_shell(dir.path(), "nu", PAYLOAD);

    let outcome = run_probe(
        shell.to_str().expect("utf-8 path"),
        CaptureMode::LoginInteractive,
        Duration::from_secs(5),
        dir.path(),
    );
    assert!(matches!(
        outcome,
        ProbeOutcome::Failed {
            reason: ProbeFailure::UnsupportedShell,
            ..
        }
    ));
}

#[test]
fn probe_leaves_no_spool_files_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shell = fake_shell(dir.path(), "bash", PAYLOAD);
    run_probe(
        shell.to_str().expect("utf-8 path"),
        CaptureMode::LoginInteractive,
        Duration::from_secs(5),
        dir.path(),
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read tempdir")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("probe-"))
        .collect();
    assert!(leftovers.is_empty(), "spool files left: {leftovers:?}");
}

/// Run a real PTY to completion and return everything it wrote.
fn capture_pty(cmd: &[String], settings: &SpawnSettings) -> String {
    use std::sync::mpsc;

    let manager = PtyManager::new();
    let out = Arc::new(StdMutex::new(String::new()));
    let (done_tx, done_rx) = mpsc::channel();
    let sink = Arc::clone(&out);
    manager
        .spawn(
            SpawnRequest {
                id: "test".to_owned(),
                cmd,
                cwd: Path::new("/"),
                rows: 24,
                cols: 80,
                settings,
                // Neutral values: these tests exercise the
                // environment/PTY plumbing, not #213/#215's agent
                // identity or config injection. `Byoh` (bring your
                // own harness — the plain-shell kind, see
                // `HarnessKind::program`) has no injection mechanism
                // of its own, so `None` config is honest rather than
                // a stand-in.
                agent: None,
                kind: HarnessKind::Byoh,
                harness_config: None,
            },
            move |event| match event {
                PtyEvent::Data { chunk } => {
                    sink.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push_str(&chunk);
                }
                PtyEvent::Exit { .. } => {
                    let _ = done_tx.send(());
                }
            },
        )
        .expect("spawn");
    done_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("child exited");
    let text = out.lock().unwrap_or_else(PoisonError::into_inner);
    text.clone()
}

#[test]
fn the_preview_matches_what_a_real_child_receives() {
    // The guarantee the Settings panel rests on. Both paths go
    // through `apply_env`, and this pins that they stay that way —
    // a preview that drifts from reality is worse than no preview,
    // because it turns a debuggable problem into a lie.
    let settings = SpawnSettings::default();
    let preview = env_preview(&settings);

    let cmd = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "printf '<%s>' \"$PATH\"".to_owned(),
    ];
    let raw = capture_pty(&cmd, &settings);
    let actual = raw
        .split_once('<')
        .and_then(|(_, r)| r.rsplit_once('>'))
        .map(|(v, _)| v.replace(['\r', '\n'], ""))
        .expect("child printed a delimited PATH");

    let previewed: Vec<String> = preview.path.iter().map(|e| e.entry.clone()).collect();
    let received: Vec<String> = actual.split(':').map(ToOwned::to_owned).collect();
    assert_eq!(previewed, received);
}

#[test]
fn host_terminal_vars_do_not_reach_the_child_but_the_keep_list_does() {
    // Both halves in one assertion on purpose: over-stripping is the
    // dangerous direction, and a test that only checked the removal
    // would happily pass while eating the agent's ssh-agent socket.
    let settings = SpawnSettings::default();
    let cmd = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "printf '<%s|%s|%s>' \"${TMUX:-none}\" \"${TERM:-none}\" \"${PATH:+set}\"".to_owned(),
    ];
    let raw = capture_pty(&cmd, &settings);
    let body = raw
        .split_once('<')
        .and_then(|(_, r)| r.rsplit_once('>'))
        .map(|(v, _)| v.replace(['\r', '\n'], ""))
        .expect("child printed the delimited probe");
    let fields: Vec<&str> = body.split('|').collect();
    assert_eq!(
        fields.first().copied(),
        Some("none"),
        "TMUX must be stripped"
    );
    assert_eq!(
        fields.get(1).copied(),
        Some("xterm-256color"),
        "TERM is ours to force"
    );
    assert_eq!(fields.get(2).copied(), Some("set"), "PATH must survive");
}

#[test]
fn extra_env_reaches_the_child() {
    let settings = SpawnSettings {
        extra_env: vec![crate::spawn_settings::EnvVar {
            key: "SKEIN_EXTRA_ENV_TEST".to_owned(),
            value: "hello".to_owned(),
        }],
        ..SpawnSettings::default()
    };
    let cmd = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "printf '<%s>' \"${SKEIN_EXTRA_ENV_TEST:-missing}\"".to_owned(),
    ];
    let raw = capture_pty(&cmd, &settings);
    assert!(raw.contains("<hello>"), "got {raw:?}");
}

#[test]
fn an_extra_env_named_path_cannot_shadow_the_merged_path() {
    // CommandBuilder::env is a map insert, so an extra variable
    // called PATH applied after the merge would silently win — and
    // the preview, the program-resolution check and the spawn log
    // would all still describe the merged value the child never got.
    let settings = SpawnSettings {
        extra_env: vec![crate::spawn_settings::EnvVar {
            key: "PATH".to_owned(),
            value: "/hijacked".to_owned(),
        }],
        ..SpawnSettings::default()
    };
    let cmd = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "printf '<%s>' \"$PATH\"".to_owned(),
    ];
    let raw = capture_pty(&cmd, &settings);
    assert!(!raw.contains("<hijacked>"), "got {raw:?}");
    assert!(!raw.contains("<{}>"), "got {raw:?}");

    let preview = env_preview(&settings);
    assert_eq!(preview.ignored_env_keys, vec!["PATH".to_owned()]);
}

#[test]
fn skein_owned_variables_cannot_be_overridden_by_extra_env() {
    let settings = SpawnSettings {
        extra_env: RESERVED_ENV_KEYS
            .iter()
            .map(|k| crate::spawn_settings::EnvVar {
                key: (*k).to_owned(),
                value: "nope".to_owned(),
            })
            .collect(),
        ..SpawnSettings::default()
    };
    let cmd = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "printf '<%s>' \"$TERM\"".to_owned(),
    ];
    assert!(capture_pty(&cmd, &settings).contains("<xterm-256color>"));
    assert_eq!(
        env_preview(&settings).ignored_env_keys.len(),
        RESERVED_ENV_KEYS.len()
    );
}

#[test]
fn disabling_capture_is_reported_as_a_setting_not_a_shell_failure() {
    // Selecting "don't ask a shell" used to surface as
    // `unsupported_shell`, telling the user their deliberate choice
    // was a compatibility problem and pointing at a fix that was
    // already in place.
    let dir = tempfile::tempdir().expect("tempdir");
    let shell = fake_shell(dir.path(), "zsh", PAYLOAD);
    let outcome = run_probe(
        shell.to_str().expect("utf-8 path"),
        CaptureMode::None,
        Duration::from_secs(5),
        dir.path(),
    );
    assert!(
        matches!(
            outcome,
            ProbeOutcome::Failed {
                reason: ProbeFailure::Disabled,
                ..
            }
        ),
        "got {outcome:?}"
    );
    let settings = SpawnSettings {
        capture: CaptureMode::None,
        ..SpawnSettings::default()
    };
    assert_eq!(describe_probe(&outcome, &settings).state, "disabled");
}

#[test]
fn a_dropped_addition_is_reported_with_its_reason() {
    let settings = SpawnSettings {
        path_prepend: vec!["/definitely/not/here".to_owned()],
        ..SpawnSettings::default()
    };
    let preview = env_preview(&settings);
    assert_eq!(preview.dropped_additions.len(), 1);
    assert_eq!(preview.dropped_additions[0].entry, "/definitely/not/here");
}

#[test]
fn a_broken_shell_setting_is_surfaced_rather_than_silently_ignored() {
    let settings = SpawnSettings {
        shell: Some("/no/such/shell".to_owned()),
        ..SpawnSettings::default()
    };
    assert_eq!(
        env_preview(&settings).shell_rejected.as_deref(),
        Some("/no/such/shell")
    );
    // A good one produces no warning.
    let ok = SpawnSettings {
        shell: Some("/bin/sh".to_owned()),
        ..SpawnSettings::default()
    };
    assert_eq!(env_preview(&ok).shell_rejected, None);
}

#[test]
fn a_non_executable_file_does_not_count_as_a_resolved_program() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("faketool");
    std::fs::write(&path, "not executable").expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let as_path = OsString::from(dir.path());
    assert_eq!(resolve_program("faketool", &as_path), None);

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(resolve_program("faketool", &as_path).is_some());
}

#[test]
fn stripping_can_be_turned_off() {
    let settings = SpawnSettings {
        strip_host_env: false,
        ..SpawnSettings::default()
    };
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = apply_env(
        &mut builder,
        &settings,
        probe_snapshot(),
        None,
        &Injection::default(),
    );
    assert!(applied.stripped.is_empty());
}

#[test]
fn path_additions_survive_a_failed_probe() {
    // The live one-condition-away outage: `augment_path` used to sit
    // inside `if let Some(path) = login_shell_path()`, so any probe
    // failure silently dropped `~/.local/bin` — which on this
    // machine is where `claude` itself lives.
    let failed = ProbeOutcome::Failed {
        reason: ProbeFailure::Timeout,
        shell: "/bin/zsh".to_owned(),
        elapsed_ms: 5000,
    };
    let base = OsString::from("/usr/bin:/bin");
    let probed = failed.path().map_or(base, OsString::from);
    let home = PathBuf::from("/home/tester");
    let merged = spawn_env::merge_path(
        &probed,
        &["~/.local/bin".to_owned()],
        Some(&home),
        &|_| None,
        &|_| true,
    );
    assert_eq!(
        std::env::split_paths(&merged.path)
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["/home/tester/.local/bin", "/usr/bin", "/bin"]
    );
}
