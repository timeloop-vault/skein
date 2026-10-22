use super::*;
use std::process::{Child, Command, Stdio};

#[test]
fn windows_decision_table() {
    // NORMAL, ABOVE_NORMAL, HIGH, REALTIME get lowered; BELOW_NORMAL, IDLE do not.
    for (class, lower) in [
        (0x20, true),
        (0x8000, true),
        (0x80, true),
        (0x100, true),
        (0x4000, false),
        (0x40, false),
    ] {
        assert_eq!(windows_needs_lowering(class), lower, "class {class:#x}");
    }
}

#[test]
fn unix_decision_table() {
    for (nice, lower) in [(-5, true), (0, true), (9, true), (10, false), (19, false)] {
        assert_eq!(unix_needs_lowering(nice), lower, "nice {nice}");
    }
}

#[cfg(any(windows, unix))]
fn long_lived_child() -> Child {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        c
    };
    #[cfg(unix)]
    let mut cmd = {
        let mut c = Command::new("sleep");
        c.arg("30");
        c
    };
    cmd.stdout(Stdio::null()).spawn().expect("spawn child")
}

#[cfg(any(windows, unix))]
#[test]
fn lowers_a_live_child_once() {
    let mut child = long_lived_child();
    let pid = child.id();
    let result = (|| -> io::Result<()> {
        // A runner that is itself low priority hands its children the same class.
        if !is_lowered(std::process::id())? {
            assert!(!is_lowered(pid)?);
        }
        assert_eq!(lower_priority(pid)?, Outcome::Lowered);
        assert!(is_lowered(pid)?);
        assert_eq!(lower_priority(pid)?, Outcome::AlreadyLow);
        Ok(())
    })();
    child.kill().expect("kill child");
    child.wait().expect("wait child");
    result.expect("priority calls");
}
