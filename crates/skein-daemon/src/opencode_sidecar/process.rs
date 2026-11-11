//! Spawning the child and draining its pipes.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;

pub(super) const STDERR_TAIL_LINES: usize = 20;

/// Git variables a hook may have exported; none may reach the child.
const GIT_ENV: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// 64 hex chars. `Uuid::new_v4` draws from the OS CSPRNG (via `getrandom`),
/// and two of them give 244 random bits.
pub(super) fn mint_password() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Replace every occurrence of the password in `line`.
pub(super) fn redact(line: &str, password: &str) -> String {
    if password.is_empty() {
        line.to_owned()
    } else {
        line.replace(password, "<redacted>")
    }
}

pub(super) fn spawn(
    exe: &Path,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
    password: &str,
) -> std::io::Result<Child> {
    let mut cmd = Command::new(exe);
    cmd.args(args).current_dir(cwd).env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.env("OPENCODE_PASSWORD", password);
    for k in GIT_ENV {
        cmd.env_remove(k);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn()
}

/// Drain one pipe on a background thread: redacted `debug!` lines, and
/// (for stderr) the last few lines kept in `tail`.
pub(super) fn drain<R: Read + Send + 'static>(
    pipe: R,
    harness_id: String,
    stream: &'static str,
    password: String,
    tail: Option<Arc<Mutex<VecDeque<String>>>>,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&buf);
            let line = redact(text.trim_end(), &password);
            tracing::debug!(harness = %harness_id, stream, "opencode serve: {line}");
            if let Some(tail) = &tail {
                let mut t = tail.lock();
                if t.len() == STDERR_TAIL_LINES {
                    t.pop_front();
                }
                t.push_back(line);
            }
        }
    });
}
