//! opencode v2: `opencode debug agents --standalone`, a JSON array.
//!
//! `--standalone` is load-bearing. Without it the command spawns a
//! detached `opencode serve --service` that inherits our stdout pipe, so
//! waiting for EOF hangs and a process is left behind. The run is still
//! bounded by a timeout so a future CLI change cannot hang the picker.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::disk::definitions;
use super::{INTERNAL, Mode};
use crate::agents::clean_command;
use crate::agents::def::{AgentDef, AgentList, AgentSource};

const TIMEOUT: Duration = Duration::from_secs(20);

/// One entry of `debug agents`, reduced to what the picker needs. The
/// `name` field is deliberately absent: it is the display name
/// (`Build`), while `id` (`build`) is what `--agent` / POST
/// /api/session take verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct V2Agent {
    pub id: String,
    pub description: Option<String>,
    /// `None` for a mode we do not recognise.
    pub mode: Option<Mode>,
    pub hidden: bool,
}

/// The agents opencode v2 will accept for a harness spawned in `cwd`.
/// Same contract as [`super::list`].
pub fn list_v2(program: &OsStr, path_env: Option<&OsStr>, home: &Path, cwd: &Path) -> AgentList {
    let enrich = definitions(home, cwd);
    match probe(program, path_env, cwd, TIMEOUT) {
        Ok(agents) => AgentList::new(
            agents
                .into_iter()
                .map(|a| {
                    if let Some(def) = enrich.get(&a.id) {
                        return def.clone();
                    }
                    let mut def = AgentDef::bare(a.id, AgentSource::Builtin);
                    def.description = a.description;
                    def
                })
                .collect(),
            None,
        ),
        Err(why) => AgentList::new(enrich.into_values().collect(), Some(why)),
    }
}

/// Parse the `debug agents` JSON. Entries without a string `id` are
/// skipped; anything that is not a JSON array is an error.
pub(super) fn parse_debug_agents(stdout: &str) -> Result<Vec<V2Agent>, String> {
    let json: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("the opencode CLI printed unreadable agent JSON: {e}"))?;
    let Some(items) = json.as_array() else {
        return Err("the opencode CLI did not print an agent array".to_owned());
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            Some(V2Agent {
                id: id.to_owned(),
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                mode: item
                    .get("mode")
                    .and_then(Value::as_str)
                    .and_then(Mode::parse),
                hidden: item.get("hidden").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

/// Entries a picker should offer: visible, primary or all.
pub(super) fn selectable_v2(agents: Vec<V2Agent>) -> Vec<V2Agent> {
    agents
        .into_iter()
        .filter(|a| !a.hidden && a.mode.is_some_and(Mode::selectable))
        .filter(|a| !INTERNAL.contains(&a.id.as_str()))
        .collect()
}

fn probe(
    program: &OsStr,
    path_env: Option<&OsStr>,
    cwd: &Path,
    timeout: Duration,
) -> Result<Vec<V2Agent>, String> {
    let text = run(program, path_env, cwd, timeout)?;
    let agents = selectable_v2(parse_debug_agents(&text)?);
    if agents.is_empty() {
        return Err(
            "the opencode CLI listed no agents — the output format may have changed".to_owned(),
        );
    }
    Ok(agents)
}

/// Run `<program> debug agents --standalone` and return its stdout.
/// Stdout is drained on a thread so a large listing cannot fill the
/// pipe while we poll for exit; on timeout the child is killed.
fn run(
    program: &OsStr,
    path_env: Option<&OsStr>,
    cwd: &Path,
    timeout: Duration,
) -> Result<String, String> {
    run_args(
        program,
        path_env,
        cwd,
        &["debug", "agents", "--standalone"],
        timeout,
    )
}

pub(super) fn run_args(
    program: &OsStr,
    path_env: Option<&OsStr>,
    cwd: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut cmd = clean_command(program);
    cmd.current_dir(cwd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(path) = path_env {
        cmd.env("PATH", path);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not run the opencode CLI: {e}"))?;
    let mut pipe = child
        .stdout
        .take()
        .ok_or("the opencode CLI has no stdout")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                // On Windows a `.cmd` shim means kill() reaches only cmd.exe;
                // the grandchild may linger holding the pipe. The reader
                // thread is bounded by the 5 s recv_timeout, so the caller
                // never hangs.
                let _ = child.kill();
                let _ = child.wait();
                return Err("the opencode CLI timed out listing agents".to_owned());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("could not wait for the opencode CLI: {e}"));
            }
        }
    };
    if !status.success() {
        return Err(format!("the opencode CLI exited with {status}"));
    }
    // The pipe closes with the process; a stray grandchild holding it
    // open must not hang us either.
    let buf = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the opencode CLI did not close its output".to_owned())?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}
