use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::record::{self, Observed, PidRecord};
use super::sweep::{SweepDecision, decide, sweep_orphans};
use super::{ExitNotice, OpencodeSidecarManager, SidecarState, StartRequest, process};

fn rec(pid: u32) -> PidRecord {
    PidRecord {
        harness_id: "h".into(),
        pid,
        start_time: 100,
        exe: "/bin/opencode".into(),
    }
}

fn obs(start_time: u64, exe: &str) -> Observed {
    Observed {
        start_time,
        exe: exe.into(),
    }
}

#[test]
fn decision_table() {
    let r = rec(1);
    assert_eq!(decide(&r, None), SweepDecision::Gone);
    assert_eq!(
        decide(&r, Some(&obs(100, "/bin/opencode"))),
        SweepDecision::Kill
    );
    assert_eq!(
        decide(&r, Some(&obs(101, "/bin/opencode"))),
        SweepDecision::LeaveMismatch
    );
    assert_eq!(
        decide(&r, Some(&obs(100, "/bin/other"))),
        SweepDecision::LeaveMismatch
    );
    let case = decide(&r, Some(&obs(100, "/BIN/OPENCODE")));
    assert_eq!(
        case,
        if cfg!(windows) {
            SweepDecision::Kill
        } else {
            SweepDecision::LeaveMismatch
        }
    );
}

#[test]
fn record_round_trip_and_remove() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(record::read(dir.path()).len(), 0);
    record::add(dir.path(), rec(1));
    let mut other = rec(2);
    other.harness_id = "g".into();
    record::add(dir.path(), other.clone());
    assert_eq!(record::read(dir.path()), vec![rec(1), other.clone()]);
    record::remove(dir.path(), "h", 999); // wrong pid: untouched
    assert_eq!(record::read(dir.path()).len(), 2);
    record::remove(dir.path(), "h", 1);
    assert_eq!(record::read(dir.path()), vec![other]);
    assert!(!dir.path().join("opencode-sidecars.json.tmp").exists());
}

#[test]
fn redaction_replaces_every_occurrence() {
    let pw = process::mint_password();
    assert_eq!(pw.len(), 64);
    assert!(pw.chars().all(|c| c.is_ascii_hexdigit()));
    let line = format!("auth {pw} and again {pw}");
    let out = process::redact(&line, &pw);
    assert_eq!(out, "auth <redacted> and again <redacted>");
    assert_eq!(process::redact("plain", ""), "plain");
}

fn env() -> Vec<(OsString, OsString)> {
    std::env::vars_os().collect()
}

#[cfg(windows)]
fn sleeper() -> (PathBuf, Vec<OsString>) {
    (
        "ping".into(),
        vec!["-n".into(), "30".into(), "127.0.0.1".into()],
    )
}
#[cfg(not(windows))]
fn sleeper() -> (PathBuf, Vec<OsString>) {
    ("sleep".into(), vec!["30".into()])
}
#[cfg(windows)]
fn exit_three() -> (PathBuf, Vec<OsString>) {
    ("cmd".into(), vec!["/c".into(), "exit".into(), "3".into()])
}
#[cfg(not(windows))]
fn exit_three() -> (PathBuf, Vec<OsString>) {
    ("sh".into(), vec!["-c".into(), "exit 3".into()])
}

fn request(
    id: &str,
    (exe, args): (PathBuf, Vec<OsString>),
    tx: mpsc::Sender<ExitNotice>,
) -> StartRequest {
    StartRequest {
        harness_id: id.into(),
        cwd: std::env::temp_dir(),
        env: env(),
        exe,
        port: 4242,
        lower_priority: true,
        on_exit: std::sync::Arc::new(move |n| {
            let _ = tx.send(n);
        }),
        args_override: Some(args),
    }
}

fn gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if record::observe(pid).is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn stop_kills_and_clears_record_without_exit_notice() {
    let dir = tempfile::tempdir().unwrap();
    let m = OpencodeSidecarManager::new(Some(dir.path().to_path_buf()));
    let (tx, rx) = mpsc::channel();
    let info = m.start(request("a", sleeper(), tx)).unwrap();
    assert_eq!(info.port, 4242);
    assert!(record::observe(info.pid).is_some());
    assert_eq!(record::read(dir.path()).len(), 1);
    assert_eq!(m.exited("a", info.generation), Some(SidecarState::Running));
    assert!(
        m.password_for("a", info.generation)
            .is_some_and(|p| p.len() == 64)
    );
    assert!(m.password_for("a", info.generation + 1).is_none());

    m.stop("a");
    assert!(gone(info.pid));
    assert_eq!(record::read(dir.path()).len(), 0);
    std::thread::sleep(Duration::from_millis(1200));
    assert!(rx.try_recv().is_err(), "stop must not fire on_exit");
    assert_eq!(m.exited("a", info.generation), None);
}

#[test]
fn stale_generation_is_a_no_op_and_restart_replaces() {
    let m = OpencodeSidecarManager::default();
    let (tx, _rx) = mpsc::channel();
    let first = m.start(request("a", sleeper(), tx.clone())).unwrap();
    let second = m.start(request("a", sleeper(), tx)).unwrap();
    assert!(second.generation > first.generation);
    assert!(gone(first.pid), "restart kills the old child");
    assert!(!m.stop_if_generation("a", first.generation));
    assert_eq!(
        m.exited("a", second.generation),
        Some(SidecarState::Running)
    );
    assert!(m.stop_if_generation("a", second.generation));
    assert!(gone(second.pid));
}

#[test]
fn natural_exit_fires_on_exit_once_and_clears_record() {
    let dir = tempfile::tempdir().unwrap();
    let m = OpencodeSidecarManager::new(Some(dir.path().to_path_buf()));
    let (tx, rx) = mpsc::channel();
    let info = m.start(request("a", exit_three(), tx)).unwrap();
    let n = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("exit notice");
    assert_eq!(n.harness_id, "a");
    assert_eq!(n.generation, info.generation);
    assert_eq!(n.code, Some(3));
    assert!(rx.recv_timeout(Duration::from_millis(1200)).is_err());
    assert_eq!(record::read(dir.path()).len(), 0);
    assert_eq!(
        m.exited("a", info.generation),
        Some(SidecarState::Exited(3))
    );
}

#[test]
fn a_dead_sidecar_yields_no_password() {
    let m = OpencodeSidecarManager::default();
    let (tx, rx) = mpsc::channel();
    let info = m.start(request("a", exit_three(), tx)).unwrap();
    rx.recv_timeout(Duration::from_secs(10))
        .expect("exit notice");
    assert_eq!(m.password_for("a", info.generation), None);
}

#[test]
fn sweep_kills_a_matching_orphan_and_spares_a_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, args) = sleeper();
    let spawn = || process::spawn(&exe, &args, &std::env::temp_dir(), &env(), "pw").unwrap();
    let mut victim = spawn();
    let mut bystander = spawn();
    let good = record::describe("v", victim.id()).unwrap();
    let mut bad = record::describe("b", bystander.id()).unwrap();
    bad.start_time += 7;
    record::write(dir.path(), &[good, bad]).unwrap();

    let report = sweep_orphans(dir.path());
    assert_eq!(report.killed, vec![victim.id()]);
    assert_eq!(report.mismatched, vec![bystander.id()]);
    assert_eq!(record::read(dir.path()).len(), 0);
    assert!(victim.wait().is_ok());
    assert!(
        bystander.try_wait().unwrap().is_none(),
        "mismatch left running"
    );
    bystander.kill().unwrap();
    let _ = bystander.wait();
}

#[cfg(windows)]
fn parent_of_sleeper() -> (PathBuf, Vec<OsString>) {
    (
        "cmd".into(),
        ["/c", "ping", "-n", "30", "127.0.0.1"]
            .map(OsString::from)
            .to_vec(),
    )
}
#[cfg(not(windows))]
fn parent_of_sleeper() -> (PathBuf, Vec<OsString>) {
    ("sh".into(), vec!["-c".into(), "sleep 30; true".into()])
}

#[test]
fn stop_takes_the_childs_descendants_with_it() {
    let m = OpencodeSidecarManager::default();
    let (tx, _rx) = mpsc::channel();
    let info = m.start(request("a", parent_of_sleeper(), tx)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut kids = super::tree::collect(info.pid);
    while kids.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        kids = super::tree::collect(info.pid);
    }
    assert!(!kids.is_empty(), "the parent should have a child process");
    m.stop("a");
    assert!(gone(info.pid));
    for (pid, _) in kids {
        assert!(gone(pid), "descendant {pid} outlived stop");
    }
}

#[test]
fn overlapping_starts_for_one_harness_leave_one_live_child() {
    let m = OpencodeSidecarManager::default();
    let (tx, _rx) = mpsc::channel();
    let infos: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let req = request("a", sleeper(), tx.clone());
                let m = &m;
                s.spawn(move || m.start(req).unwrap())
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let newest = infos.iter().max_by_key(|i| i.generation).unwrap();
    for i in &infos {
        if i.pid != newest.pid {
            assert!(gone(i.pid), "superseded child {} leaked", i.pid);
        }
    }
    assert!(record::observe(newest.pid).is_some());
    assert_eq!(
        m.exited("a", newest.generation),
        Some(SidecarState::Running)
    );
    m.stop("a");
    assert!(gone(newest.pid));
}
