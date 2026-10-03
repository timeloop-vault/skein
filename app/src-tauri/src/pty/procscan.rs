//! Finding an opencode TUI inside one PTY (#517).
//!
//! A user can exit an opencode harness, land in the post-exit shell of the
//! same PTY pane and start `opencode` again. Skein did not spawn that
//! process, so the only way to learn its session id and port is to look at
//! the PTY child's descendants and read their argv. The parsing, the
//! descendant walk and the choice among candidates are pure; only
//! [`scan_opencode`] touches the OS.
//!
//! On Windows an `opencode.cmd` shim runs through `cmd.exe`/`node` before the
//! real `opencode.exe`; the shim's argv does not parse as opencode, so only
//! the real binary is ever a candidate.
//!
//! Matching is on argv[0]'s basename only: the npm shim's `cmd.exe`/`node`
//! hop is not matched, only the native opencode binary it execs (verified on
//! Windows); a launcher running `node …/opencode` would not be matched.

use std::collections::{HashMap, HashSet, VecDeque};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// What an opencode argv says about the session it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpencodeArgv {
    pub session_id: Option<String>,
    pub port: Option<u16>,
    /// Started with `-c` / `--continue`: it resumed *some* session whose id
    /// the argv does not name.
    pub continue_last: bool,
}

/// An opencode TUI found under a PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpencodeFound {
    pub pid: u32,
    pub session_id: Option<String>,
    pub port: Option<u16>,
    /// True when that very pid is listening on `port`.
    pub port_confirmed: bool,
    pub continue_last: bool,
}

/// Subcommands that are not the interactive TUI. `attach` connects to a
/// remote server, so it owns no local port or session worth adopting.
const NON_TUI: &[&str] = &[
    "serve",
    "run",
    "web",
    "acp",
    "mcp",
    "auth",
    "agent",
    "upgrade",
    "uninstall",
    "models",
    "stats",
    "export",
    "import",
    "github",
    "debug",
    "generate",
    "session",
    "pr",
    "attach",
];

/// Flags that take a separate value, which must not be mistaken for a
/// positional (`--model x` is not a project dir).
const VALUE_FLAGS: &[&str] = &[
    "--session",
    "-s",
    "--port",
    "--hostname",
    "--model",
    "-m",
    "--agent",
    "--prompt",
    "--log-level",
    "--mdns-domain",
];

fn is_opencode_program(program: &str) -> bool {
    let base = program.rsplit(['/', '\\']).next().unwrap_or("");
    let base = base.to_ascii_lowercase();
    base == "opencode" || base == "opencode.exe"
}

fn parse_port(value: &str) -> Option<u16> {
    value.parse::<u16>().ok().filter(|p| *p != 0)
}

/// Parse a process argv; `Some` only for an opencode TUI.
pub fn parse_opencode_argv(args: &[String]) -> Option<OpencodeArgv> {
    let (program, rest) = args.split_first()?;
    if !is_opencode_program(program) {
        return None;
    }
    let mut out = OpencodeArgv {
        session_id: None,
        port: None,
        continue_last: false,
    };
    let mut seen_positional = false;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        if arg.starts_with('-') {
            if let Some((flag, value)) = arg.split_once('=') {
                match flag {
                    "--session" | "-s" if !value.is_empty() => {
                        out.session_id = Some(value.to_owned());
                    }
                    "--port" => out.port = parse_port(value),
                    _ => {}
                }
            } else if matches!(arg.as_str(), "-c" | "--continue") {
                out.continue_last = true;
            } else if VALUE_FLAGS.contains(&arg.as_str()) {
                let value = iter.next();
                match (arg.as_str(), value) {
                    ("--session" | "-s", Some(v)) => out.session_id = Some(v.clone()),
                    ("--port", Some(v)) => out.port = parse_port(v),
                    _ => {}
                }
            }
        } else if !seen_positional {
            seen_positional = true;
            if NON_TUI.contains(&arg.as_str()) {
                return None;
            }
        }
    }
    Some(out)
}

/// Every transitive child of `root` (BFS, cycle-safe, root excluded).
/// Only processes reachable from `root` are returned, which is what keeps
/// two panes' opencodes apart.
pub fn descendants(procs: &[(u32, Option<u32>)], root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, parent) in procs {
        if let Some(parent) = parent {
            children.entry(*parent).or_default().push(*pid);
        }
    }
    let mut seen: HashSet<u32> = HashSet::from([root]);
    let mut queue = VecDeque::from([root]);
    let mut out = Vec::new();
    while let Some(pid) = queue.pop_front() {
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(*child) {
                out.push(*child);
                queue.push_back(*child);
            }
        }
    }
    out
}

/// One matching process: `(pid, start_time, argv)`.
pub type Candidate = (u32, u64, OpencodeArgv);

/// The candidate to report: newest start time, ties to the higher pid.
pub fn select_candidate(candidates: Vec<Candidate>) -> Option<Candidate> {
    candidates
        .into_iter()
        .max_by_key(|(pid, start, _)| (*start, *pid))
}

/// Whether `l` is `pid` listening on `port` over TCP (a UDP socket on
/// the same port is not opencode's server).
fn is_tcp_listener_of(l: &listeners::Listener, pid: u32, port: u16) -> bool {
    l.process.pid == pid && l.socket.port() == port && l.protocol == listeners::Protocol::TCP
}

fn pid_listens_on(pid: u32, port: u16) -> bool {
    listeners::get_all().is_ok_and(|all| all.iter().any(|l| is_tcp_listener_of(l, pid, port)))
}

#[cfg(test)]
mod listener_tests {
    use super::*;

    fn listener(pid: u32, port: u16, protocol: listeners::Protocol) -> listeners::Listener {
        listeners::Listener {
            process: listeners::Process {
                pid,
                name: String::new(),
                path: String::new(),
            },
            socket: std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            protocol,
            state: listeners::SocketState::Listen,
        }
    }

    #[test]
    fn only_a_tcp_listener_confirms_the_port() {
        assert!(is_tcp_listener_of(
            &listener(7, 4096, listeners::Protocol::TCP),
            7,
            4096
        ));
        assert!(!is_tcp_listener_of(
            &listener(7, 4096, listeners::Protocol::UDP),
            7,
            4096
        ));
        assert!(!is_tcp_listener_of(
            &listener(8, 4096, listeners::Protocol::TCP),
            7,
            4096
        ));
    }
}

fn process_table(sys: &System) -> Vec<(u32, Option<u32>)> {
    sys.processes()
        .iter()
        .map(|(pid, p)| (pid.as_u32(), p.parent().map(Pid::as_u32)))
        .collect()
}

/// Scan `root_pid`'s descendants for an opencode TUI. Takes tens of
/// milliseconds; call from a blocking thread.
pub fn scan_opencode(root_pid: u32) -> Option<OpencodeFound> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let table = process_table(&sys);
    let candidates = descendants(&table, root_pid)
        .into_iter()
        .filter_map(|pid| {
            let proc = sys.process(Pid::from_u32(pid))?;
            let argv: Vec<String> = proc
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            Some((pid, proc.start_time(), parse_opencode_argv(&argv)?))
        })
        .collect();
    let (pid, _, argv) = select_candidate(candidates)?;
    let port_confirmed = argv.port.is_some_and(|p| pid_listens_on(pid, p));
    Some(OpencodeFound {
        pid,
        session_id: argv.session_id,
        port: argv.port,
        port_confirmed,
        continue_last: argv.continue_last,
    })
}

#[cfg(test)]
mod tests;
