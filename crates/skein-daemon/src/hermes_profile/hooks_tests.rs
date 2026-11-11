use std::path::{Path, PathBuf};

use super::hooks::{HookInputs, allowlisted, install_script, spec_for};
use super::prepare::{Prepared, prepare_with_hooks};
use super::yaml_hooks::{HOOK_EVENTS, hooks_present};

const BASE: &str = "model:\n  default: x\n";

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

struct Fixture {
    dir: tempfile::TempDir,
    script: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let res = dir.path().join("res");
        std::fs::create_dir_all(&res).expect("mkdir");
        let script = res.join("skein-hook.sh");
        std::fs::write(&script, "#!/bin/sh\necho v1\n").expect("write");
        for p in ["skein", "work"] {
            let h = dir.path().join("root/profiles").join(p);
            std::fs::create_dir_all(&h).expect("mkdir");
            std::fs::write(h.join("config.yaml"), BASE).expect("write");
        }
        Self { dir, script }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    fn inputs(&self, supported: bool) -> HookInputs {
        HookInputs {
            supported,
            script: Some(self.script.clone()),
        }
    }

    fn run(&self, profile: &str, supported: bool) -> Prepared {
        prepare_with_hooks(
            &self.root(),
            &args(&["--tui", "-p", profile]),
            true,
            &self.inputs(supported),
            || panic!("no create"),
        )
    }

    fn config(&self, profile: &str) -> PathBuf {
        self.root()
            .join("profiles")
            .join(profile)
            .join("config.yaml")
    }

    fn installed(&self) -> PathBuf {
        self.root().join("skein-hooks").join("skein-hook.sh")
    }
}

fn approve(home: &Path, script: &Path, events: &[&str]) {
    let spec = spec_for(script).expect("spec");
    let approvals: Vec<_> = events
        .iter()
        .map(|e| serde_json::json!({"event": e, "command": spec.command_for(e)}))
        .collect();
    std::fs::write(
        home.join("shell-hooks-allowlist.json"),
        serde_json::json!({ "approvals": approvals }).to_string(),
    )
    .expect("write");
}

#[test]
fn spec_for_picks_the_launcher_by_script_type() {
    let py = spec_for(Path::new(
        r"C:\Users\Some One\root\skein-hooks\skein-hook.py",
    ))
    .unwrap();
    assert_eq!(
        py.command_for("pre_llm_call"),
        r#""C:\Users\Some One\root\skein-hooks\skein-hook.py" pre_llm_call"#
    );
    let sh = spec_for(Path::new("/home/u/.hermes/skein-hooks/skein-hook.sh")).unwrap();
    assert_eq!(
        sh.command_for("on_session_end"),
        "sh /home/u/.hermes/skein-hooks/skein-hook.sh on_session_end"
    );
    assert_eq!(py.timeout_secs, 5);
}

#[test]
fn skein_profile_gets_hooks_when_supported_and_auto_accept_makes_it_working() {
    let f = Fixture::new();
    let r = f.run("skein", true);
    assert!(r.injected && r.hooks);
    let text = std::fs::read_to_string(f.config("skein")).expect("read");
    let spec = spec_for(&f.installed()).expect("spec");
    assert!(hooks_present(&text, &spec));
    assert!(text.contains("hooks_auto_accept: true"));
    assert!(f.installed().is_file());
}

#[test]
fn unsupported_version_writes_no_hooks_and_installs_nothing() {
    let f = Fixture::new();
    let r = f.run("skein", false);
    assert!(r.injected && !r.hooks);
    assert!(
        !std::fs::read_to_string(f.config("skein"))
            .unwrap()
            .contains("skein-hooks")
    );
    assert!(!f.root().join("skein-hooks").exists());
}

#[test]
fn missing_script_means_no_hooks() {
    let f = Fixture::new();
    let r = prepare_with_hooks(
        &f.root(),
        &args(&["-p", "skein"]),
        true,
        &HookInputs {
            supported: true,
            script: None,
        },
        || panic!("no create"),
    );
    assert!(r.injected && !r.hooks);
}

#[test]
fn switch_off_does_not_write_hooks() {
    let f = Fixture::new();
    let r = prepare_with_hooks(
        &f.root(),
        &args(&["-p", "skein"]),
        false,
        &f.inputs(true),
        || panic!("no create"),
    );
    assert!(!r.hooks);
    assert!(
        !std::fs::read_to_string(f.config("skein"))
            .unwrap()
            .contains("hooks:")
    );
}

#[test]
fn user_profile_is_untouched_and_reports_by_state() {
    let f = Fixture::new();
    let r = f.run("work", true);
    assert_eq!(r, Prepared::default());
    assert_eq!(std::fs::read_to_string(f.config("work")).unwrap(), BASE);

    // The user consented elsewhere: hooks present, but not approved yet.
    let spec = spec_for(&f.installed()).expect("spec");
    let with = super::yaml_hooks::plan_hooks(BASE, &spec, false)
        .unwrap()
        .unwrap();
    std::fs::write(f.config("work"), &with).expect("write");
    assert!(!f.run("work", true).hooks);
    assert_eq!(std::fs::read_to_string(f.config("work")).unwrap(), with);

    // Approved in the allowlist for every event.
    let home = f.root().join("profiles/work");
    approve(&home, &f.installed(), &HOOK_EVENTS[..7]);
    assert!(!f.run("work", true).hooks, "seven of eight is not enough");
    approve(&home, &f.installed(), &HOOK_EVENTS);
    assert!(f.run("work", true).hooks);
    assert!(!f.run("work", false).hooks);
    assert_eq!(std::fs::read_to_string(f.config("work")).unwrap(), with);
}

#[test]
fn explicit_auto_accept_false_in_the_skein_profile_blocks_the_hooks() {
    let f = Fixture::new();
    std::fs::write(f.config("skein"), "hooks_auto_accept: false\nmodel: x\n").expect("write");
    assert!(!f.run("skein", true).hooks);
    approve(
        &f.root().join("profiles/skein"),
        &f.installed(),
        &HOOK_EVENTS,
    );
    // Not written, so not reported: Hermes would ask [y/N] in the PTY.
    assert!(!f.run("skein", true).hooks);
}

#[test]
fn second_prepare_writes_nothing() {
    let f = Fixture::new();
    f.run("skein", true);
    let cfg = std::fs::read(f.config("skein")).expect("read");
    let script_mtime = std::fs::metadata(f.installed())
        .unwrap()
        .modified()
        .unwrap();
    let cfg_mtime = std::fs::metadata(f.config("skein"))
        .unwrap()
        .modified()
        .unwrap();
    assert!(f.run("skein", true).hooks);
    assert_eq!(std::fs::read(f.config("skein")).unwrap(), cfg);
    assert_eq!(
        std::fs::metadata(f.config("skein"))
            .unwrap()
            .modified()
            .unwrap(),
        cfg_mtime
    );
    assert_eq!(
        std::fs::metadata(f.installed())
            .unwrap()
            .modified()
            .unwrap(),
        script_mtime
    );
}

#[test]
fn script_is_reinstalled_only_when_bytes_differ() {
    let f = Fixture::new();
    let root = f.root();
    let dest = install_script(&f.script, &root).expect("install");
    assert_eq!(dest, f.installed());
    let m1 = std::fs::metadata(&dest).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    install_script(&f.script, &root).expect("again");
    assert_eq!(std::fs::metadata(&dest).unwrap().modified().unwrap(), m1);

    std::fs::write(&f.script, "#!/bin/sh\necho v2\n").expect("write");
    install_script(&f.script, &root).expect("update");
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "#!/bin/sh\necho v2\n"
    );
    let leftovers = std::fs::read_dir(root.join("skein-hooks")).unwrap().count();
    assert_eq!(leftovers, 1, "no temp files left behind");
}

#[test]
fn allowlist_missing_corrupt_or_bom_prefixed() {
    let f = Fixture::new();
    let spec = spec_for(&f.installed()).expect("spec");
    let home = f.dir.path().join("h");
    std::fs::create_dir_all(&home).expect("mkdir");
    assert!(!allowlisted(&home, &spec));
    std::fs::write(home.join("shell-hooks-allowlist.json"), "{not json").expect("write");
    assert!(!allowlisted(&home, &spec));
    std::fs::write(home.join("shell-hooks-allowlist.json"), "[]").expect("write");
    assert!(!allowlisted(&home, &spec));
    approve(&home, &f.installed(), &HOOK_EVENTS);
    let mut bytes = b"\xef\xbb\xbf".to_vec();
    bytes.extend(std::fs::read(home.join("shell-hooks-allowlist.json")).unwrap());
    std::fs::write(home.join("shell-hooks-allowlist.json"), bytes).expect("write");
    assert!(allowlisted(&home, &spec));
}

#[test]
fn skein_profile_with_a_non_true_auto_accept_gets_no_hooks() {
    let f = Fixture::new();
    std::fs::write(
        f.config("skein"),
        format!(
            "hooks_auto_accept: false
{BASE}"
        ),
    )
    .expect("write");
    let r = f.run("skein", true);
    assert!(r.injected && !r.hooks);
    let text = std::fs::read_to_string(f.config("skein")).expect("read");
    assert!(!text.contains("on_session_start"), "{text}");
    assert!(!f.root().join("skein-hooks").exists());
}

#[test]
fn a_user_profile_that_has_not_consented_gets_no_script_installed() {
    let f = Fixture::new();
    assert!(!f.run("work", true).hooks);
    assert!(!f.root().join("skein-hooks").exists());
}

#[test]
fn install_treats_a_failed_rewrite_as_success_when_the_bytes_are_there() {
    let f = Fixture::new();
    let dest = install_script(&f.script, &f.root()).expect("install");
    // Same bytes already there: Ok whatever the filesystem would say to a
    // rewrite (a Windows rename over a file Python holds open).
    assert_eq!(install_script(&f.script, &f.root()).expect("again"), dest);
}
