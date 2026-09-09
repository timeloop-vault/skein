//! Git (and `gh`) as spawned processes.
//!
//! Everything else in this crate is libgit2. The *mutations* are not,
//! deliberately — epic #52's D9 and the `grok-build` recon (§6) both
//! land on the same conclusion: link libgit2 for reads, shell out to
//! the git binary for writes. Three reasons, in order of how much they
//! matter here:
//!
//! 1. **Credential helpers, hooks and LFS work for free.** A push
//!    through libgit2 means reimplementing the user's credential
//!    helper; a push through `git` means the one that already works
//!    keeps working.
//! 2. **The user's git config is honoured** — `merge.ff`, `core.hooksPath`,
//!    signing. Skein is not a git client and should not hold opinions
//!    the user has already expressed elsewhere.
//! 3. libgit2's status is documented as 5-10x slower than native git on
//!    large repos.
//!
//! # Nothing may block on an invisible prompt
//!
//! These processes are spawned by a desktop app, not by a terminal. A
//! credential prompt or an SSH host-key question would be written to a
//! pipe nobody is reading, and the action would hang forever with no
//! way for the user to see why. So every spawn here carries
//! [`GIT_ENV`], which turns each of those prompts into an immediate
//! failure, and every spawn has a wall-clock deadline on top of that
//! for the case no environment variable covers — a TCP connect into a
//! black hole.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::{GitError, Result};

/// The auth-suppression environment every spawned git inherits.
///
/// Each entry turns a would-be interactive prompt into an error:
/// `GIT_TERMINAL_PROMPT` covers git's own username/password prompt,
/// the two `*_ASKPASS` variables stop it delegating to a GUI helper,
/// `BatchMode=yes` makes ssh fail rather than ask about an unknown host
/// key, and `GIT_LFS_SKIP_SMUDGE` keeps an LFS checkout from reaching
/// for the network in the middle of a merge.
pub const GIT_ENV: [(&str, &str); 5] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", ""),
    ("SSH_ASKPASS", ""),
    ("GIT_LFS_SKIP_SMUDGE", "1"),
    ("GIT_SSH_COMMAND", "ssh -o BatchMode=yes"),
];

/// Ambient variables that would override `current_dir` and point the
/// spawn at a *different* repository than the one we asked about.
///
/// Not hypothetical: git exports `GIT_DIR` and `GIT_INDEX_FILE` to
/// every hook it runs, so anything launched from inside a hook inherits
/// them, and a `git status` in a temp directory then answers about the
/// hook's repository instead. Skein does not normally run from a hook —
/// but the shell that launched it may have any of these set, and the
/// consequence here is a merge or a push against the wrong repo. The
/// working directory is the only thing allowed to decide.
const GIT_UNSET: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// `gh` shells out to git, so it needs all of the above plus its own
/// two: no interactive survey prompts, no update banner on stdout.
const GH_EXTRA_ENV: [(&str, &str); 2] =
    [("GH_PROMPT_DISABLED", "1"), ("GH_NO_UPDATE_NOTIFIER", "1")];

/// Local git work — status, rev-list, merge. Generous, because a merge
/// on a large repo with hooks is legitimately slow.
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Anything that touches the network. Long enough for a real push over
/// a slow link, short enough that a dead remote surfaces as an error in
/// the same sitting.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(300);

/// How long to keep collecting output after the process itself has
/// exited. Only a grandchild holding the pipe open ever reaches this.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// What a finished process said.
///
/// A non-zero exit is *not* an error at this layer: git uses exit codes
/// to answer questions (`rev-parse --verify` on a missing ref, a merge
/// that conflicted), and the caller is the only one who knows which
/// non-zero means "no" and which means "broken".
#[derive(Debug, Clone)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout with the trailing newline gone — the shape almost every
    /// single-value git query wants.
    pub fn line(&self) -> &str {
        self.stdout.trim_end_matches(['\n', '\r'])
    }

    /// The most useful thing to show a user about a failure: git puts
    /// its diagnosis on stderr, but a few commands report on stdout,
    /// and an empty message would be worse than either.
    pub fn failure(&self) -> String {
        let stderr = self.stderr.trim();
        if !stderr.is_empty() {
            return stderr.to_owned();
        }
        let stdout = self.stdout.trim();
        if !stdout.is_empty() {
            return stdout.to_owned();
        }
        match self.code {
            Some(c) => format!("exited with code {c}"),
            None => "killed by a signal".to_owned(),
        }
    }
}

/// Run `git` in `cwd`.
pub fn git(cwd: &Path, args: &[&str]) -> Result<Output> {
    run("git", cwd, args, &[], None, LOCAL_TIMEOUT)
}

/// Run `git` in `cwd` with a network-sized deadline.
pub fn git_networked(cwd: &Path, args: &[&str]) -> Result<Output> {
    run("git", cwd, args, &[], None, NETWORK_TIMEOUT)
}

/// Run `gh` in `cwd`, optionally feeding it stdin.
///
/// `stdin` exists for exactly one reason: `gh` has no `--body @-`, and
/// passing that string sets the PR body to the literal two characters
/// `@-`. The supported form is `--body-file -`, which reads from here.
pub fn gh(cwd: &Path, args: &[&str], stdin: Option<&str>) -> Result<Output> {
    run("gh", cwd, args, &GH_EXTRA_ENV, stdin, NETWORK_TIMEOUT)
}

/// Spawn, drain, feed, wait — with a deadline.
///
/// The ordering is not incidental: stdout and stderr are drained on
/// their own threads *before* stdin is written, because a child that
/// writes more than a pipe buffer while we are still writing its input
/// deadlocks both sides otherwise.
fn run(
    program: &str,
    cwd: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<Output> {
    let mut child = command(program, cwd, args, extra_env, stdin.is_some())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::CommandMissing(program.to_owned())
            } else {
                GitError::Io(e)
            }
        })?;

    // The readers report over channels rather than being joined
    // directly, because killing a child does not close a pipe one of
    // its own children still holds open (git spawning ssh, or a
    // credential helper). Joining such a thread would reintroduce
    // exactly the unbounded wait the deadline exists to prevent.
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    std::thread::spawn(move || out_tx.send(read_all(out.as_mut())));
    std::thread::spawn(move || err_tx.send(read_all(err.as_mut())));

    if let Some(data) = stdin {
        // A closed pipe here means the child exited early, and its exit
        // code explains that far better than an io error would.
        if let Some(mut sink) = child.stdin.take() {
            let _ = sink.write_all(data.as_bytes());
        }
    }

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let Some(status) = status else {
        return Err(GitError::Timeout {
            command: format!("{program} {}", args.join(" ")),
            secs: timeout.as_secs(),
        });
    };

    // The process has exited, so its own end of both pipes is closed
    // and the readers are finishing. `DRAIN_GRACE` bounds the one case
    // that is not true — a surviving grandchild — at the cost of
    // reporting less output than there was, which beats not returning.
    let stdout = out_rx.recv_timeout(DRAIN_GRACE).unwrap_or_default();
    let stderr = err_rx.recv_timeout(DRAIN_GRACE).unwrap_or_default();

    Ok(Output {
        code: status.code(),
        stdout,
        stderr,
    })
}

/// The command every spawn here is made from — separated so its
/// environment can be asserted on without running anything.
fn command(
    program: &str,
    cwd: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    has_stdin: bool,
) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(cwd)
        .args(args)
        .stdin(if has_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in GIT_ENV.iter().chain(extra_env) {
        cmd.env(k, v);
    }
    for k in GIT_UNSET {
        cmd.env_remove(k);
    }
    cmd
}

/// Read a pipe to exhaustion, lossily. Git output is usually UTF-8 but
/// a commit message or a path need not be, and losing the whole message
/// to an encoding error would be the worse trade.
fn read_all(pipe: Option<&mut impl Read>) -> String {
    let Some(pipe) = pipe else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = pipe.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_auth_suppression_env_covers_every_prompt_a_spawn_can_hit() {
        // These five are the contract with #214: a prompt that reaches
        // a pipe nobody reads hangs the action forever, so each has to
        // turn its prompt into an immediate failure instead.
        let keys: Vec<&str> = GIT_ENV.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"GIT_TERMINAL_PROMPT"));
        assert!(keys.contains(&"GIT_ASKPASS"));
        assert!(keys.contains(&"SSH_ASKPASS"));
        assert!(keys.contains(&"GIT_LFS_SKIP_SMUDGE"));
        assert!(keys.contains(&"GIT_SSH_COMMAND"));
        assert_eq!(GIT_ENV[0].1, "0");
        assert!(GIT_ENV[4].1.contains("BatchMode=yes"));
    }

    #[test]
    fn a_missing_program_names_itself_rather_than_reading_as_an_io_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = run(
            "skein-no-such-program",
            tmp.path(),
            &[],
            &[],
            None,
            LOCAL_TIMEOUT,
        )
        .unwrap_err();
        assert!(
            matches!(&err, GitError::CommandMissing(p) if p == "skein-no-such-program"),
            "got {err:?}"
        );
    }

    #[test]
    fn a_non_zero_exit_is_output_and_not_an_error() {
        // git answers questions with exit codes; only the caller knows
        // which non-zero means "no" and which means "broken".
        let tmp = tempfile::TempDir::new().unwrap();
        let out = git(tmp.path(), &["rev-parse", "--verify", "HEAD"]).unwrap();
        assert!(!out.ok());
        assert!(!out.failure().is_empty());
    }

    #[test]
    fn an_ambient_git_dir_is_stripped_so_the_working_directory_decides() {
        // The test above found this as a real bug: run from the
        // pre-commit hook, which git hands a `GIT_DIR`, it answered
        // about Skein's own repository from inside an empty temp
        // directory. In production the same inheritance would aim a
        // merge or a push at a repository nobody asked about.
        //
        // Asserted on the built command rather than by setting the
        // variable for real, because `set_var` is process-global (and
        // `unsafe` in edition 2024, which this crate forbids).
        let tmp = tempfile::TempDir::new().unwrap();
        let cmd = command("git", tmp.path(), &["status"], &[], false);
        let removed: Vec<&str> = cmd
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .filter_map(|(k, _)| k.to_str())
            .collect();
        for key in GIT_UNSET {
            assert!(
                removed.contains(&key),
                "{key} must be cleared, not inherited"
            );
        }
    }

    #[test]
    fn stdout_reaches_the_caller() {
        let tmp = tempfile::TempDir::new().unwrap();
        let out = git(tmp.path(), &["--version"]).unwrap();
        assert!(out.ok(), "{}", out.failure());
        assert!(out.line().starts_with("git version"));
    }

    #[test]
    fn a_process_that_will_not_end_is_killed_at_the_deadline() {
        // The environment variables cover prompts; this covers what
        // they cannot — a spawn that simply never returns.
        //
        // Both shells here outlive their own child, which is the point:
        // killing the process does not close a pipe a *grandchild*
        // still holds, so a runner that joined its reader threads would
        // hang here for the full 30 seconds despite the deadline.
        let tmp = tempfile::TempDir::new().unwrap();
        #[cfg(windows)]
        let (program, args): (&str, Vec<&str>) =
            ("cmd", vec!["/c", "ping", "-n", "30", "127.0.0.1"]);
        #[cfg(not(windows))]
        let (program, args): (&str, Vec<&str>) = ("sh", vec!["-c", "sleep 30"]);

        let started = Instant::now();
        let err = run(
            program,
            tmp.path(),
            &args,
            &[],
            None,
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(matches!(err, GitError::Timeout { .. }), "got {err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline has to actually end it"
        );
    }
}
