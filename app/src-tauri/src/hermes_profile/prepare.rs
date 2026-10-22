//! Getting Skein's own Hermes profile ready for a spawn (#626).
//!
//! Hermes reads MCP servers from the active profile's `config.yaml`,
//! and has no session-scoped switch like Claude's `--plugin-dir`. So,
//! unlike the other harnesses, this injection lives in a file — but
//! only in the profile Skein owns (`skein`). The user's root config and
//! their own profiles are never written here; adding the entry to one of
//! those is the separate, consented step (#627).
//!
//! Root resolution: the consent/listing/resume paths read
//! [`super::effective_root`] (Settings env additions, else Skein's own
//! environment). The spawn uses the final builder env instead, which also
//! carries a per-harness env profile (#615); a `HERMES_HOME` set only there
//! is seen by the spawn alone, which logs a warning when the roots differ.
//! A `HERMES_HOME` starting with `~` is expanded here with the same helper
//! as `effective_root`, and written back to the builder so Hermes itself
//! receives an absolute path.
//!
//! Some Hermes versions don't write a `config.yaml` on `profile create`,
//! so Skein writes the profile's config itself when it is missing (only
//! ever for `skein`, and only if the file is still absent at write time).
//!
//! The decision is a pure function ([`plan`]); the I/O around it takes
//! the profile-creating step as a closure so tests never run Hermes.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use skein_harness::hermes::{profile_home, valid_profile_name};

use super::hooks::{self, HookInputs};
use super::{
    EnsureOutcome, SKEIN_PROFILE, ensure_skein_entry_creating, remove_skein_entry_owned, yaml,
};

/// How long `hermes profile create` may take before it is abandoned.
const CREATE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait, after the child exited or was killed, for it to be
/// reaped and for each output pipe to reach EOF. A launcher's grandchild
/// can keep a pipe open past the kill, so nothing here joins unboundedly.
const REAP_GRACE: Duration = Duration::from_secs(2);

/// Preparing the profile is one step at a time, process-wide: two Hermes
/// harnesses restored together would otherwise both see `profiles/skein`
/// missing and both run `profile create`, and the loser would fail.
pub(super) static PREPARE_LOCK: Mutex<()> = Mutex::new(());

/// The profile named by the Hermes argv (`-p x`, `--profile x`,
/// `--profile=x`); `None` when there is none, or `-p` has no value.
pub(crate) fn argv_profile(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--profile=") {
            return (!v.is_empty()).then(|| v.to_owned());
        }
        if a == "-p" || a == "--profile" {
            return it.next().filter(|v| !v.is_empty()).cloned();
        }
    }
    None
}

fn is_skein_profile(profile: Option<&str>) -> bool {
    profile.is_some_and(|p| p.trim().eq_ignore_ascii_case(SKEIN_PROFILE))
}

/// What to do to Skein's own profile for one spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Plan {
    pub create: bool,
    pub ensure: bool,
    pub remove: bool,
}

/// The decision table. Anything but the `skein` profile gets the empty
/// plan: Skein writes nothing to a profile it does not own.
///
/// | switch | profile dir | plan |
/// | :-- | :-- | :-- |
/// | on | missing | create, then ensure the entry |
/// | on | present | ensure the entry |
/// | off | missing | nothing (never creates) |
/// | off | present | remove the entry |
pub(crate) fn plan(switch_on: bool, profile: Option<&str>, profile_dir_exists: bool) -> Plan {
    if !is_skein_profile(profile) {
        return Plan::default();
    }
    if switch_on {
        Plan {
            create: !profile_dir_exists,
            ensure: true,
            remove: false,
        }
    } else {
        Plan {
            create: false,
            ensure: false,
            remove: profile_dir_exists,
        }
    }
}

fn config_path(root: &Path, profile: Option<&str>) -> PathBuf {
    profile_home(root, profile).join("config.yaml")
}

/// Whether Skein's MCP entry is in the effective profile's config,
/// read from the file.
fn entry_present(root: &Path, profile: Option<&str>) -> bool {
    std::fs::read_to_string(config_path(root, profile)).is_ok_and(|t| yaml::skein_entry_present(&t))
}

/// What a spawn learns from preparing the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Prepared {
    /// Skein's MCP entry is in the effective profile's config.
    pub injected: bool,
    /// Skein's status hooks are in it, approved, and the version is
    /// supported (#625).
    pub hooks: bool,
}

/// [`prepare_with_hooks`] without hook telemetry: the MCP entry only.
#[cfg(test)]
pub(crate) fn prepare_in(
    root: &Path,
    args: &[String],
    switch_on: bool,
    create: impl FnOnce() -> Result<(), String>,
) -> bool {
    prepare_with_hooks(root, args, switch_on, &HookInputs::default(), create).injected
}

/// Run the plan against `root`, then report whether Skein's entry is in
/// the effective profile's `config.yaml`. Errors are logged and never
/// returned: a failed preparation must not fail the spawn (Hermes shows
/// its own error for a missing profile).
///
/// The answer comes from the file, not from the switch: a user profile
/// the user consented to (#627) carries the entry whatever the switch
/// says, and a failed removal leaves the tools reachable.
///
/// Hook telemetry (#625) rides along: the script is installed under the
/// root and, for the `skein` profile with the switch on, the hooks are
/// written; `hooks` reports whether the effective profile has them
/// working. A user's own profile is only ever read.
pub(crate) fn prepare_with_hooks(
    root: &Path,
    args: &[String],
    switch_on: bool,
    hook_inputs: &HookInputs,
    create: impl FnOnce() -> Result<(), String>,
) -> Prepared {
    let _guard = PREPARE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let profile = argv_profile(args);
    if let Some(p) = profile.as_deref()
        && !valid_profile_name(p)
    {
        return Prepared::default();
    }
    let skein_dir = root.join("profiles").join(SKEIN_PROFILE);
    let todo = plan(switch_on, profile.as_deref(), skein_dir.is_dir());
    let cfg = skein_dir.join("config.yaml");
    if todo.create
        && let Err(e) = create()
    {
        // Another process may have created it meanwhile; if the config is
        // there now, carry on with the ensure step.
        if cfg.is_file() {
            tracing::info!(error = %e, "hermes profile: create failed but the profile exists now");
        } else {
            tracing::warn!(error = %e, "hermes profile: creating the skein profile failed");
            return Prepared::default();
        }
    }
    if todo.ensure {
        // `todo.ensure` is only ever set for the Skein-owned profile (see
        // `plan`), so creating its config here never touches a user's file.
        match ensure_skein_entry_creating(&cfg) {
            Ok(EnsureOutcome::Added) => tracing::info!("hermes profile: added skein MCP entry"),
            Ok(EnsureOutcome::AlreadyPresent) => {}
            Err(e) => {
                tracing::warn!(error = %e, "hermes profile: adding the skein MCP entry failed");
            }
        }
    }
    if todo.remove {
        // `todo.remove` is only set for the Skein-owned profile (see `plan`).
        match remove_skein_entry_owned(&cfg) {
            Ok(true) => tracing::info!("hermes profile: removed skein MCP entry"),
            Ok(false) => {
                if entry_present(root, profile.as_deref()) {
                    tracing::warn!(
                        "hermes profile: Skein's MCP entry has no Skein markers (Hermes may have rewritten the file); it was left in place"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "hermes profile: removing the skein MCP entry failed");
            }
        }
    }
    let hooks = hooks::prepare(
        root,
        &profile_home(root, profile.as_deref()),
        &config_path(root, profile.as_deref()),
        todo.ensure,
        hook_inputs,
    );
    Prepared {
        injected: entry_present(root, profile.as_deref()),
        hooks,
    }
}

/// The spawn-path entry: prepare the profile for the finished command.
/// `launch` is the program as the PTY will start it, `args` the argv
/// after it, and `builder` carries the merged environment — so the
/// create step sees the same `PATH` and `HERMES_HOME` the TUI will.
/// Blocking (it may run Hermes); the caller is already off the UI thread.
pub(crate) fn prepare_for_spawn(
    builder: &mut portable_pty::CommandBuilder,
    launch: &str,
    args: &[String],
    settings: &crate::spawn_settings::SpawnSettings,
    harness_config: Option<&crate::harness_config::HarnessConfig>,
) -> Prepared {
    let from_env = builder
        .get_env("HERMES_HOME")
        .and_then(|h| h.to_str())
        .filter(|h| !h.is_empty())
        .map(str::to_string);
    let root = match from_env {
        Some(raw) if raw.starts_with('~') => {
            let expanded = skein_harness::hermes::expand_tilde(&raw);
            if expanded.as_os_str() != raw.as_str() {
                builder.env("HERMES_HOME", &expanded);
            }
            Some(expanded)
        }
        Some(raw) => Some(PathBuf::from(raw)),
        None => skein_harness::hermes::hermes_home(),
    };
    let Some(root) = root else {
        return Prepared::default();
    };
    let seen_elsewhere = super::effective_root(settings);
    if seen_elsewhere.as_deref() != Some(root.as_path()) {
        tracing::warn!(
            spawn_root = %root.display(),
            settings_root = ?seen_elsewhere,
            "hermes profile: this spawn's HERMES_HOME differs from the one consent, the profile list and the resume probe read"
        );
    }
    let hook_inputs = HookInputs {
        supported: super::version_probe::telemetry_ok(launch, builder.iter_full_env_as_str()),
        script: harness_config.and_then(|c| {
            let script = if cfg!(windows) {
                c.hermes_hook_py()
            } else {
                c.hermes_hook_sh()
            };
            script.map(Path::to_path_buf)
        }),
    };
    prepare_with_hooks(
        &root,
        args,
        settings.inject_hermes_config,
        &hook_inputs,
        || create_skein_profile(launch, builder.iter_full_env_as_str()),
    )
}

fn tail(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let s = s.trim();
    let start = s.len().saturating_sub(800);
    let start = (start..=s.len())
        .find(|i| s.is_char_boundary(*i))
        .unwrap_or(s.len());
    s[start..].to_owned()
}

/// Run `<program> -p default profile create skein --no-alias --no-skills`
/// with exactly `env`, no console window on Windows, bounded by
/// [`CREATE_TIMEOUT`].
pub(crate) fn create_skein_profile<'a>(
    program: &str,
    env: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<(), String> {
    let mut cmd = Command::new(program);
    cmd.args([
        "-p",
        "default",
        "profile",
        "create",
        SKEIN_PROFILE,
        "--no-alias",
        "--no-skills",
    ])
    .env_clear()
    .envs(env)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    let drain = |mut r: Box<dyn Read + Send>| {
        let (tx, rx) = mpsc::channel();
        // Detached on purpose: if a grandchild keeps the pipe open past
        // the kill, this thread ends whenever it finally does.
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        rx
    };
    let out = child.stdout.take().map(|s| drain(Box::new(s)));
    let err = child.stderr.take().map(|s| drain(Box::new(s)));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) if started.elapsed() > CREATE_TIMEOUT => {
                let _ = child.kill();
                let reaped = Instant::now();
                while child.try_wait().ok().flatten().is_none() && reaped.elapsed() < REAP_GRACE {
                    std::thread::sleep(Duration::from_millis(20));
                }
                break Err(format!("timed out after {}s", CREATE_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => break Err(e.to_string()),
        }
    };
    let collect = |rx: Option<mpsc::Receiver<Vec<u8>>>| {
        rx.and_then(|rx| rx.recv_timeout(REAP_GRACE).ok())
            .unwrap_or_default()
    };
    let (out, err) = (collect(out), collect(err));
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!(
            "exited with {s}; stdout: {}; stderr: {}",
            tail(&out),
            tail(&err)
        )),
        Err(e) => Err(format!(
            "{e}; stdout: {}; stderr: {}",
            tail(&out),
            tail(&err)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hermes_profile::ensure_skein_entry;
    use std::cell::Cell;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn argv_profile_spellings() {
        assert_eq!(
            argv_profile(&args(&["--tui", "-p", "skein"])).as_deref(),
            Some("skein")
        );
        assert_eq!(
            argv_profile(&args(&["--profile", "work", "--tui"])).as_deref(),
            Some("work")
        );
        assert_eq!(
            argv_profile(&args(&["--profile=work"])).as_deref(),
            Some("work")
        );
        assert_eq!(argv_profile(&args(&["--tui", "--resume", "s1"])), None);
        assert_eq!(argv_profile(&args(&["--tui", "-p"])), None);
    }

    #[test]
    fn plan_table() {
        let none = Plan::default();
        assert_eq!(
            plan(true, Some("skein"), false),
            Plan {
                create: true,
                ensure: true,
                remove: false
            }
        );
        assert_eq!(
            plan(true, Some("Skein"), true),
            Plan {
                create: false,
                ensure: true,
                remove: false
            }
        );
        assert_eq!(
            plan(false, Some("skein"), true),
            Plan {
                create: false,
                ensure: false,
                remove: true
            }
        );
        assert_eq!(plan(false, Some("skein"), false), none);
        for p in [None, Some("default"), Some("work")] {
            assert_eq!(plan(true, p, true), none);
            assert_eq!(plan(false, p, true), none);
        }
    }

    const BASE: &str = "model:\n  default: x\n";

    fn root_with_skein_profile() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("profiles").join("skein");
        std::fs::create_dir_all(&p).expect("mkdir");
        std::fs::write(p.join("config.yaml"), BASE).expect("write");
        dir
    }

    fn skein_cfg(root: &Path) -> String {
        std::fs::read_to_string(root.join("profiles/skein/config.yaml")).expect("read")
    }

    #[test]
    fn on_adds_the_entry_and_reports_injected_then_off_removes_it() {
        let root = root_with_skein_profile();
        let a = args(&["--tui", "-p", "skein"]);
        assert!(prepare_in(root.path(), &a, true, || panic!(
            "must not create"
        )));
        assert!(yaml::skein_entry_present(&skein_cfg(root.path())));
        assert!(!prepare_in(root.path(), &a, false, || panic!(
            "must not create"
        )));
        assert_eq!(skein_cfg(root.path()), BASE);
    }

    #[test]
    fn on_with_missing_profile_creates_it_through_the_closure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ran = Cell::new(false);
        let root = dir.path().to_path_buf();
        let injected = prepare_in(&root, &args(&["-p", "skein"]), true, || {
            ran.set(true);
            let p = root.join("profiles/skein");
            std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
            std::fs::write(p.join("config.yaml"), BASE).map_err(|e| e.to_string())
        });
        assert!(ran.get() && injected);
    }

    #[test]
    fn failed_create_and_off_with_missing_profile_report_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!prepare_in(
            dir.path(),
            &args(&["-p", "skein"]),
            true,
            || Err("boom".into())
        ));
        assert!(!prepare_in(
            dir.path(),
            &args(&["-p", "skein"]),
            false,
            || panic!("no create")
        ));
        assert!(!dir.path().join("profiles").exists());
    }

    #[test]
    fn other_profiles_are_never_written_but_report_by_file() {
        let root = root_with_skein_profile();
        let work = root.path().join("profiles/work");
        std::fs::create_dir_all(&work).expect("mkdir");
        std::fs::write(work.join("config.yaml"), BASE).expect("write");
        for sw in [true, false] {
            assert!(!prepare_in(
                root.path(),
                &args(&["-p", "work"]),
                sw,
                || panic!("no create")
            ));
            assert!(!prepare_in(root.path(), &args(&["--tui"]), sw, || panic!(
                "no create"
            )));
        }
        assert_eq!(
            std::fs::read_to_string(work.join("config.yaml")).expect("read"),
            BASE
        );
        // A consented entry counts, whatever the switch says.
        ensure_skein_entry(&work.join("config.yaml")).expect("ensure");
        assert!(prepare_in(
            root.path(),
            &args(&["-p", "work"]),
            false,
            || panic!("no create")
        ));
    }

    #[test]
    fn a_failed_create_that_another_process_won_still_proceeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let injected = prepare_in(&root, &args(&["-p", "skein"]), true, || {
            let p = root.join("profiles/skein");
            std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
            std::fs::write(p.join("config.yaml"), BASE).map_err(|e| e.to_string())?;
            Err("already exists".into())
        });
        assert!(injected);
    }

    #[test]
    fn off_with_an_unmarked_entry_leaves_it_and_still_reports_present() {
        let root = root_with_skein_profile();
        let cfg = root.path().join("profiles/skein/config.yaml");
        std::fs::write(
            &cfg,
            "mcp_servers:
  skein:
    url: https://example.test
",
        )
        .expect("write");
        assert!(prepare_in(
            root.path(),
            &args(&["-p", "skein"]),
            false,
            || panic!("no create")
        ));
    }

    #[test]
    fn skein_profile_without_a_config_gets_one_with_just_the_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("profiles/skein")).expect("mkdir");
        let a = args(&["-p", "skein"]);
        assert!(prepare_in(dir.path(), &a, true, || panic!("no create")));
        let first = std::fs::read(dir.path().join("profiles/skein/config.yaml")).expect("read");
        let text = String::from_utf8(first.clone()).expect("utf8");
        assert!(yaml::skein_entry_present(&text));
        assert!(text.starts_with("mcp_servers:") || text.starts_with("# skein:begin"));
        assert!(text.ends_with('\n') && !text.contains('\r') && !text.starts_with('\u{feff}'));
        // Second run: already present, file untouched.
        assert!(prepare_in(dir.path(), &a, true, || panic!("no create")));
        assert_eq!(
            std::fs::read(dir.path().join("profiles/skein/config.yaml")).expect("read"),
            first
        );
        assert_eq!(
            ensure_skein_entry_creating(&dir.path().join("profiles/skein/config.yaml")),
            Ok(EnsureOutcome::AlreadyPresent)
        );
    }

    #[test]
    fn create_that_writes_no_config_still_gets_the_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let injected = prepare_in(&root, &args(&["-p", "skein"]), true, || {
            std::fs::create_dir_all(root.join("profiles/skein")).map_err(|e| e.to_string())
        });
        assert!(injected);
        assert!(yaml::skein_entry_present(&skein_cfg(&root)));
    }

    #[test]
    fn user_profile_without_a_config_is_never_given_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = dir.path().join("profiles/work");
        std::fs::create_dir_all(&work).expect("mkdir");
        for sw in [true, false] {
            assert!(!prepare_in(
                dir.path(),
                &args(&["-p", "work"]),
                sw,
                || panic!("no create")
            ));
        }
        assert!(!work.join("config.yaml").exists());
        assert!(ensure_skein_entry(&work.join("config.yaml")).is_err());
        assert!(!work.join("config.yaml").exists());
    }

    #[test]
    fn invalid_profile_names_do_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!prepare_in(
            dir.path(),
            &args(&["-p", "../x"]),
            true,
            || panic!("no create")
        ));
    }
}
