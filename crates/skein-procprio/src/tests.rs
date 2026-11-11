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
        use std::os::windows::process::CommandExt;
        // NORMAL_PRIORITY_CLASS: a runner that is itself below normal (a hook
        // run from a Skein harness shell, #713) would otherwise hand the child
        // its own class, and there would be nothing left to lower.
        const NORMAL_PRIORITY_CLASS: u32 = 0x20;
        let mut c = Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"])
            .creation_flags(NORMAL_PRIORITY_CLASS);
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
        // Windows always starts the child at Normal (see long_lived_child). On
        // unix an unprivileged runner cannot hand out a lower nice than its
        // own, so a child of an already-niced runner (#713) starts low and the
        // first call must leave it alone.
        let starts_low = is_lowered(pid)?;
        #[cfg(windows)]
        assert!(!starts_low, "child should start at Normal");
        let expected = if starts_low {
            Outcome::AlreadyLow
        } else {
            Outcome::Lowered
        };
        assert_eq!(lower_priority(pid)?, expected);
        assert!(is_lowered(pid)?);
        assert_eq!(lower_priority(pid)?, Outcome::AlreadyLow);
        Ok(())
    })();
    child.kill().expect("kill child");
    child.wait().expect("wait child");
    result.expect("priority calls");
}
