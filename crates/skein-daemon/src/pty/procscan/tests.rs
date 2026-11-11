use super::*;

fn argv(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

fn parsed(s: &str) -> Option<(Option<String>, Option<u16>)> {
    parse_opencode_argv(&argv(s)).map(|a| (a.session_id, a.port))
}

fn sess(s: &str, p: Option<u16>) -> (Option<String>, Option<u16>) {
    (Some(s.to_owned()), p)
}

#[test]
fn argv_table() {
    assert_eq!(parsed("opencode"), Some((None, None)));
    assert_eq!(
        parsed("opencode --session ses_abc --port 4096"),
        Some(sess("ses_abc", Some(4096)))
    );
    assert_eq!(
        parsed("opencode --session=ses_abc --port=4096"),
        Some(sess("ses_abc", Some(4096)))
    );
    assert_eq!(parsed("opencode -s ses_x"), Some(sess("ses_x", None)));
    assert_eq!(parsed("OpenCode.EXE -s ses_x"), Some(sess("ses_x", None)));
    assert_eq!(parsed("/usr/local/bin/opencode"), Some((None, None)));
    assert_eq!(parsed("opencode ../proj"), Some((None, None)));
    assert_eq!(parsed("opencode --port 0"), Some((None, None)));
    assert_eq!(parsed("opencode --port abc"), Some((None, None)));
    assert_eq!(parsed("opencode --port=99999"), Some((None, None)));
    assert_eq!(
        parsed("opencode --model x --session y --unknown"),
        Some(sess("y", None))
    );
    // A value-taking flag's value is not a subcommand.
    assert_eq!(parsed("opencode --agent serve"), Some((None, None)));
    assert_eq!(
        parsed("opencode --hostname 127.0.0.1 --port 5"),
        Some((None, Some(5)))
    );
}

#[test]
fn argv_windows_paths() {
    for prog in [
        r"C:\Users\x\AppData\Roaming\npm\node_modules\opencode-ai\bin\opencode.exe",
        r"C:\\Users\\x\\bin\\opencode.exe",
    ] {
        let args = vec![prog.to_owned(), "--session".to_owned(), "s1".to_owned()];
        assert_eq!(
            parse_opencode_argv(&args),
            Some(OpencodeArgv {
                session_id: Some("s1".to_owned()),
                port: None,
                continue_last: false
            })
        );
    }
}

#[test]
fn argv_continue_flag() {
    let cont = |s: &str| parse_opencode_argv(&argv(s)).map(|a| a.continue_last);
    assert_eq!(cont("opencode -c"), Some(true));
    assert_eq!(cont("opencode --continue --port 5"), Some(true));
    assert_eq!(cont("opencode --port 5"), Some(false));
    // A value that happens to read `-c` is not the flag.
    assert_eq!(cont("opencode --model -c"), Some(false));
}

#[test]
fn argv_rejections() {
    for s in [
        "opencode serve --port 1",
        "opencode run hi",
        "opencode attach http://x",
        "opencode web",
        "node",
        "opencode-foo",
        "myopencode",
        "",
    ] {
        assert_eq!(parsed(s), None, "{s}");
    }
}

#[test]
fn descendants_multi_level_and_excludes_root() {
    let t = [
        (2, Some(1)),
        (3, Some(2)),
        (4, Some(3)),
        (5, Some(1)),
        (1, Some(0)),
    ];
    let mut d = descendants(&t, 1);
    d.sort_unstable();
    assert_eq!(d, vec![2, 3, 4, 5]);
}

#[test]
fn descendants_cycle_safe() {
    let t = [(1, Some(2)), (2, Some(1)), (3, Some(2))];
    let mut d = descendants(&t, 1);
    d.sort_unstable();
    assert_eq!(d, vec![2, 3]);
}

#[test]
fn descendants_isolate_panes() {
    // Two shells (10, 20), each with its own opencode.
    let t = [
        (11, Some(10)),
        (12, Some(11)),
        (21, Some(20)),
        (22, Some(21)),
    ];
    let mut a = descendants(&t, 10);
    a.sort_unstable();
    let mut b = descendants(&t, 20);
    b.sort_unstable();
    assert_eq!(a, vec![11, 12]);
    assert_eq!(b, vec![21, 22]);
}

fn cand(pid: u32, start: u64) -> Candidate {
    (
        pid,
        start,
        OpencodeArgv {
            session_id: None,
            port: None,
            continue_last: false,
        },
    )
}

#[test]
fn selection_newest_then_highest_pid() {
    assert_eq!(select_candidate(vec![]), None);
    let newest = select_candidate(vec![cand(5, 10), cand(9, 20), cand(7, 15)]);
    assert_eq!(newest.unwrap().0, 9);
    let tie = select_candidate(vec![cand(5, 10), cand(9, 10), cand(7, 10)]);
    assert_eq!(tie.unwrap().0, 9);
    let older_high_pid = select_candidate(vec![cand(99, 10), cand(1, 11)]);
    assert_eq!(older_high_pid.unwrap().0, 1);
}

/// Real OS snapshot: a child of this test process shows up as a descendant.
#[test]
fn live_descendants_contains_real_child() {
    #[cfg(windows)]
    let mut child = std::process::Command::new("cmd")
        .args(["/C", "ping", "-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    #[cfg(not(windows))]
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn");
    let child_pid = child.id();

    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let found = descendants(&process_table(&sys), std::process::id()).contains(&child_pid);
    let _ = child.kill();
    let _ = child.wait();
    assert!(found, "child {child_pid} not among descendants");
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    l.local_addr().expect("addr").port()
}

struct LiveShell {
    pid: u32,
    writer: std::sync::Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    // Held so the PTY stays open for the life of the shell.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

fn live_shell() -> LiveShell {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{Read, Write};
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    #[cfg(windows)]
    let cmd = {
        let mut c = CommandBuilder::new("powershell.exe");
        c.arg("-NoLogo");
        c.arg("-NoProfile");
        c
    };
    #[cfg(not(windows))]
    let cmd = CommandBuilder::new("sh");
    let child = pair.slave.spawn_command(cmd).expect("spawn shell");
    let pid = child.process_id().expect("pid");
    let killer = child.clone_killer();
    let mut reader = pair.master.try_clone_reader().expect("reader");
    let writer = std::sync::Arc::new(std::sync::Mutex::new(
        pair.master.take_writer().expect("writer"),
    ));
    let reply = std::sync::Arc::clone(&writer);
    // Drain so ConPTY never blocks on a full pipe.
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            // ConPTY withholds output until the host answers its cursor
            // position query, as xterm.js does in the app.
            if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
                let mut w = reply.lock().expect("lock");
                let _ = w.write_all(b"\x1b[1;1R");
                let _ = w.flush();
            }
        }
    });
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    LiveShell {
        pid,
        writer,
        killer,
        _master: pair.master,
    }
}

fn send(s: &LiveShell, text: &str) {
    use std::io::Write;
    let mut w = s.writer.lock().expect("lock");
    w.write_all(text.as_bytes()).expect("write");
    w.flush().expect("flush");
}

fn poll_scan(
    pid: u32,
    secs: u64,
    done: impl Fn(&Option<OpencodeFound>) -> bool,
) -> Option<OpencodeFound> {
    let start = std::time::Instant::now();
    loop {
        let t = std::time::Instant::now();
        let found = scan_opencode(pid);
        println!(
            "  scan({pid}) at +{:?} took {:?}: {found:?}",
            start.elapsed(),
            t.elapsed()
        );
        if done(&found) || start.elapsed().as_secs() >= secs {
            return found;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Real-process check for #517: needs `opencode` on PATH. Run with
/// `cargo test --manifest-path app/src-tauri/Cargo.toml live_opencode -- --ignored --nocapture`.
#[test]
#[ignore = "needs opencode on PATH"]
fn live_opencode_shell_scan() {
    let mut a = live_shell();
    let mut b = live_shell();
    let (p, q) = (free_port(), free_port());
    let mut spawned = vec![a.pid, b.pid];
    std::thread::sleep(std::time::Duration::from_secs(2));

    send(&a, &format!("opencode --port {p}\r"));
    let found = poll_scan(a.pid, 30, |f| {
        f.as_ref()
            .is_some_and(|f| f.port == Some(p) && f.port_confirmed)
    });
    {
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        for d in descendants(&process_table(&sys), a.pid) {
            if let Some(pr) = sys.process(Pid::from_u32(d)) {
                println!("  A descendant {d}: {:?} {:?}", pr.name(), pr.cmd());
            }
        }
    }
    let b_during = scan_opencode(b.pid);
    let verdict_a = found
        .as_ref()
        .is_some_and(|f| f.port == Some(p) && f.port_confirmed);

    // Bogus-session variant in shell B: report only.
    send(
        &b,
        &format!("opencode --session ses_doesnotexist --port {q}\r"),
    );
    println!("bogus-session variant (shell B):");
    let _ = poll_scan(b.pid, 5, |_| false);

    // Cleanup: only descendants of our own shells.
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let table = process_table(&sys);
    for root in [a.pid, b.pid] {
        for d in descendants(&table, root) {
            spawned.push(d);
            if let Some(pr) = sys.process(Pid::from_u32(d)) {
                println!("  cleanup kill {d} ({:?})", pr.name());
                pr.kill();
            }
        }
    }
    let _ = a.killer.kill();
    let _ = b.killer.kill();

    assert!(verdict_a, "shell A scan: {found:?}");
    assert!(b_during.is_none(), "shell B attributed: {b_during:?}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let left: Vec<u32> = spawned
        .into_iter()
        .filter(|p| sys.process(Pid::from_u32(*p)).is_some())
        .collect();
    println!("spawned pids still alive: {left:?}");
}
