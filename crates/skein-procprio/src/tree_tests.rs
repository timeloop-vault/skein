//! Tests for [`ProcessTree`].
//!
//! Proof that a grandchild died needs no process table: the parent is spawned
//! with a piped stdout, so the grandchild inherits the write end, and the read
//! side reaches EOF only once every holder of that end is gone.

#![cfg(any(windows, unix))]

use super::*;
use std::io::Read;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(10);

/// Ends and reaps the child on drop, so a failed assertion leaks nothing.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Msg {
    /// The first bytes arrived on the pipe.
    Started,
    /// Every holder of the write end is gone.
    Eof,
}

/// Reads the pipe on a helper thread so a failure cannot hang the suite.
fn watch(mut stdout: ChildStdout) -> Receiver<Msg> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0_u8; 256];
        let mut started = false;
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Msg::Eof);
                    return;
                }
                Ok(_) if !started => {
                    started = true;
                    let _ = tx.send(Msg::Started);
                }
                Ok(_) => {}
            }
        }
    });
    rx
}

fn spawn_piped(mut cmd: Command) -> (Guard, Receiver<Msg>) {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn parent");
    let stdout = child.stdout.take().expect("piped stdout");
    (Guard(child), watch(stdout))
}

fn expect_msg(rx: &Receiver<Msg>, want: &Msg, bound: Duration) {
    let deadline = Instant::now() + bound;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(m) if &m == want => return,
            Ok(_) => {}
            Err(e) => panic!("did not see {want:?} within {bound:?}: {e}"),
        }
    }
}

fn expect_child_exit(guard: &mut Guard) {
    let deadline = Instant::now() + BOUND;
    while Instant::now() < deadline {
        if guard.0.try_wait().expect("try_wait").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("direct child still running after {BOUND:?}");
}

#[cfg(windows)]
fn windows_tree() -> (Guard, Receiver<Msg>) {
    // The first ping gives time to attach; its output goes to NUL, so the
    // first bytes on the pipe come from the second, which is started after the
    // job exists and inherits stdout.
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", "ping -n 3 127.0.0.1 >NUL & ping -n 60 127.0.0.1"]);
    spawn_piped(cmd)
}

#[cfg(windows)]
#[test]
fn kill_ends_the_whole_tree() {
    let (mut guard, rx) = windows_tree();
    let tree = ProcessTree::attach(guard.0.id()).expect("attach");
    expect_msg(&rx, &Msg::Started, BOUND);
    tree.kill().expect("kill");
    expect_msg(&rx, &Msg::Eof, BOUND);
    expect_child_exit(&mut guard);
}

#[cfg(windows)]
#[test]
fn dropping_the_tree_ends_it_too() {
    let (mut guard, rx) = windows_tree();
    let tree = ProcessTree::attach(guard.0.id()).expect("attach");
    expect_msg(&rx, &Msg::Started, BOUND);
    drop(tree);
    expect_msg(&rx, &Msg::Eof, BOUND);
    expect_child_exit(&mut guard);
}

#[cfg(windows)]
#[test]
fn kill_twice_is_ok() {
    let (mut guard, rx) = windows_tree();
    let tree = ProcessTree::attach(guard.0.id()).expect("attach");
    expect_msg(&rx, &Msg::Started, BOUND);
    tree.kill().expect("first kill");
    tree.kill().expect("second kill");
    expect_msg(&rx, &Msg::Eof, BOUND);
    expect_child_exit(&mut guard);
}

#[cfg(windows)]
#[test]
fn attach_to_a_gone_process_errors() {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", "exit 0"]).stdout(Stdio::null());
    let mut child = cmd.spawn().expect("spawn");
    let pid = child.id();
    child.wait().expect("wait");
    assert!(ProcessTree::attach(pid).is_err());
}

#[cfg(unix)]
fn unix_group(script: &str) -> (Guard, Receiver<Msg>) {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("sh");
    cmd.args(["-c", script]).process_group(0);
    spawn_piped(cmd)
}

#[cfg(unix)]
#[test]
fn kill_ends_the_whole_group() {
    let (mut guard, rx) = unix_group("echo started; sleep 60 & wait");
    let tree = ProcessTree::attach(guard.0.id()).expect("attach");
    expect_msg(&rx, &Msg::Started, BOUND);
    tree.kill().expect("kill");
    expect_msg(&rx, &Msg::Eof, BOUND);
    expect_child_exit(&mut guard);
    guard.0.wait().expect("reap");
}

#[cfg(unix)]
#[test]
fn sigkill_escalation_ends_a_group_that_ignores_sighup() {
    let (mut guard, rx) = unix_group("trap '' HUP; echo started; sleep 60 & wait");
    let tree = ProcessTree::attach(guard.0.id()).expect("attach");
    expect_msg(&rx, &Msg::Started, BOUND);
    tree.kill().expect("kill");
    expect_msg(
        &rx,
        &Msg::Eof,
        crate::tree::UNIX_KILL_GRACE + Duration::from_secs(5),
    );
    expect_child_exit(&mut guard);
    guard.0.wait().expect("reap");
}

#[cfg(unix)]
#[test]
fn attach_rejects_a_pid_that_is_not_a_group_leader() {
    let mut cmd = Command::new("sleep");
    cmd.arg("60").stdout(Stdio::null());
    let guard = Guard(cmd.spawn().expect("spawn"));
    let err = ProcessTree::attach(guard.0.id()).expect_err("not a leader");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[cfg(unix)]
#[test]
fn attach_rejects_pid_1() {
    let err = ProcessTree::attach(1).expect_err("pid 1");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}
