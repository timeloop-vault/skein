//! The Settings preview of the resolved harness environment, and the
//! program lookup it shares with agent discovery.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use portable_pty::CommandBuilder;
use serde::Serialize;

use crate::harness_config::Injection;
use crate::spawn_env;
use crate::spawn_settings::SpawnSettings;

use super::env::apply_env;
use super::probe::{ProbeFailure, ProbeOutcome, probe_snapshot};

// ── Settings preview (#72) ─────────────────────────────────────────
//
// The resolved harness environment used to be visible nowhere: the only
// record was a log line carrying the *probe's* output, i.e. the value
// before Skein's own additions, in a daily-rotating file. That opacity
// is why #72 reads as "a noticeably shorter PATH" rather than naming a
// missing directory — you cannot debug what you cannot see.

/// Where one `PATH` entry came from.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathSource {
    /// From the user's "additional directories" list.
    Added,
    /// From the login-shell probe.
    Shell,
    /// From the environment Skein itself was launched with — on
    /// Windows, unioned with the live registry `PATH`, which
    /// `portable-pty` supplies in place of the inherited one.
    Inherited,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathEntryReport {
    pub entry: String,
    /// `false` renders greyed — a directory that isn't there is the
    /// single most common reason a `PATH` addition "doesn't work".
    pub exists: bool,
    pub source: PathSource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    /// `pending` | `captured` | `not_applicable` | `unsupported_shell`
    /// | `timeout` | `spawn_failed` | `no_payload`
    pub state: String,
    pub shell: String,
    pub elapsed_ms: u64,
    pub argv: Vec<String>,
    /// Plain-language explanation when the state isn't `captured`.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramReport {
    pub name: String,
    pub resolved: Option<String>,
}

/// One of the user's additions that didn't make it onto `PATH`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DroppedAddition {
    pub entry: String,
    pub reason: spawn_env::DropReason,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvPreview {
    pub shell: String,
    pub probe: ProbeReport,
    pub path: Vec<PathEntryReport>,
    pub programs: Vec<ProgramReport>,
    pub stripped: Vec<String>,
    pub extra_env_keys: Vec<String>,
    /// Additions that were skipped, with the reason — otherwise "I added
    /// a directory and nothing happened" is unanswerable, which is the
    /// confusion this panel exists to end.
    pub dropped_additions: Vec<DroppedAddition>,
    /// Extra-env keys Skein owns and therefore refused.
    pub ignored_env_keys: Vec<String>,
    /// Set when a shell is configured but isn't a runnable file, so the
    /// panel can say the value is being ignored instead of leaving the
    /// user staring at a field that looks accepted.
    pub shell_rejected: Option<String>,
    /// `bundled` when Skein was launched from Finder/Explorer/Dock,
    /// `terminal` when it inherited a terminal's environment. The
    /// difference is the whole reason PATH bugs survive testing: from a
    /// terminal the rc chain prepends to an already-rich `PATH`, so the
    /// bug is invisible in exactly the profile developers run.
    pub launch_context: String,
}

/// The binaries whose absence actually breaks a room.
const PROGRAMS_OF_INTEREST: &[&str] = &["claude", "opencode", "gh", "git"];

pub(super) fn describe_probe(outcome: &ProbeOutcome, settings: &SpawnSettings) -> ProbeReport {
    let argv = |shell: &str| {
        spawn_env::probe_args(shell, settings.capture)
            .map(|a| {
                let mut v = vec![shell.to_owned()];
                v.extend(a.iter().map(|s| (*s).to_owned()));
                v.push(spawn_env::PROBE_SCRIPT.to_owned());
                v
            })
            .unwrap_or_default()
    };
    match outcome {
        ProbeOutcome::Pending => ProbeReport {
            state: "pending".into(),
            shell: String::new(),
            elapsed_ms: 0,
            argv: Vec::new(),
            message: Some("Still asking your shell for its PATH.".into()),
        },
        ProbeOutcome::Captured {
            shell, elapsed_ms, ..
        } => ProbeReport {
            state: "captured".into(),
            shell: shell.clone(),
            elapsed_ms: *elapsed_ms,
            argv: argv(shell),
            message: None,
        },
        ProbeOutcome::Failed {
            reason,
            shell,
            elapsed_ms,
        } => {
            let (state, message) = match reason {
                ProbeFailure::Disabled => (
                    "disabled",
                    "PATH capture is turned off. Harnesses get the environment Skein was \
                     launched with, plus your additions below."
                        .to_owned(),
                ),
                ProbeFailure::NotApplicable => (
                    "not_applicable",
                    "Windows has no login shell to ask. Skein uses the live registry PATH \
                     (system + user, re-read on every spawn) plus your additions below."
                        .to_owned(),
                ),
                ProbeFailure::UnsupportedShell => (
                    "unsupported_shell",
                    format!(
                        "Skein has no verified way to ask {shell} for its PATH, so it uses the \
                         environment it was launched with plus your additions. Set a shell below \
                         (zsh, bash, fish, sh, ksh, dash, csh and tcsh all work) if you want a \
                         capture."
                    ),
                ),
                ProbeFailure::Timeout => (
                    "timeout",
                    format!(
                        "{shell} did not finish within 5 s and was stopped. Something in your \
                         startup files is blocking. Skein is using the environment it was \
                         launched with plus your additions."
                    ),
                ),
                ProbeFailure::SpawnFailed => {
                    ("spawn_failed", format!("Skein could not start {shell}."))
                }
                ProbeFailure::NoPayload => (
                    "no_payload",
                    format!(
                        "{shell} ran but printed no PATH Skein could read. Check the log for its \
                         raw output."
                    ),
                ),
            };
            ProbeReport {
                state: state.to_owned(),
                shell: shell.clone(),
                elapsed_ms: *elapsed_ms,
                argv: argv(shell),
                message: Some(message),
            }
        }
    }
}

/// Find `name` on `path`, the way the OS will.
///
/// Deliberately *not* `portable-pty`'s `search_path`: on Unix that one
/// also joins the cwd first, and on Windows it takes no cwd at all and
/// degrades to returning the bare name on failure. We only want the
/// honest "is this on PATH" answer.
///
/// The executable-bit check on Unix matters — `portable-pty` uses
/// `access(X_OK)`, so a present-but-non-executable file would report as
/// resolved here while the spawn refuses it.
pub(super) fn resolve_program(name: &str, path: &OsStr) -> Option<String> {
    let exts = path_exts();
    // #207: on Windows an extensionless `dir\name` is only a candidate
    // when the caller already spelled an executable extension
    // (`powershell.exe`), never as a bare fallback. `npm i -g` writes
    // three shims per binary — `opencode` (a `#!/bin/sh` script),
    // `opencode.cmd` and `opencode.ps1` — and cmd.exe runs the second,
    // never the first. `portable-pty` tries the extensionless file
    // first, which is how the sh script reached `CreateProcessW` and
    // came back as "%1 is not a valid Win32 application".
    let lower = name.to_ascii_lowercase();
    let bare_ok = !cfg!(windows) || exts.iter().any(|e| lower.ends_with(e.as_str()));
    resolve_program_in(name, path, &exts, bare_ok)
}

/// The executable extensions to try per `PATH` directory, in `PATHEXT`
/// order. Empty off Windows, where the concept doesn't exist.
fn path_exts() -> Vec<String> {
    if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    } else {
        Vec::new()
    }
}

/// The lookup itself, with the platform decisions already made so it
/// can be exercised from either OS.
pub(super) fn resolve_program_in(
    name: &str,
    path: &OsStr,
    exts: &[String],
    bare_ok: bool,
) -> Option<String> {
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if is_executable(&candidate) {
                return Some(candidate.display().to_string());
            }
        }
        let direct = dir.join(name);
        if bare_ok && is_executable(&direct) {
            return Some(direct.display().to_string());
        }
    }
    None
}

/// The absolute program to hand `CommandBuilder` in place of the
/// caller's bare name, or `None` to leave `portable-pty`'s own
/// resolution alone.
///
/// `None` off Windows, where portable-pty's lookup already matches the
/// OS's, and `None` when `program` isn't on `PATH` at all — the honest
/// failure there is portable-pty's, which names the program.
///
/// A `program` that already carries a directory needs no help: the
/// caller pointed at a specific file, and `PATH` order is not ours to
/// second-guess.
///
/// The resolved file may well be a `.cmd` — that is what `npm i -g`
/// installs — and needs no interpreter wrapper: `CreateProcessW` falls
/// back to the command interpreter for a batch extension it
/// recognises. Only the extensionless `#!/bin/sh` sibling has no such
/// fallback, and that is the one that dies as "%1 is not a valid Win32
/// application".
pub(super) fn windows_resolved_program(program: &str, path: &OsStr) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let is_path_like = program.contains('/') || program.contains('\\') || program.contains(':');
    if is_path_like {
        return None;
    }
    resolve_program(program, path)
}

/// The executable and `PATH` a harness spawn would use right now,
/// without spawning anything.
///
/// #246 runs the harness CLI itself to ask which agents it accepts, and
/// a picker that consulted a *different* `claude` than the spawn will
/// use would be worse than no picker — it would be confidently wrong.
/// So the agent probe goes through the same merge and the same #207
/// `PATHEXT` resolution as `spawn`, and returns `None` for the program
/// when `PATH` holds nothing by that name: the honest failure is the
/// probe's, which names it.
///
/// Uses the *non-waiting* probe read, like `env_preview`: the caller is
/// answering a picker, not starting work, and must not block the event
/// loop for `PROBE_WAIT` (#171/#177).
pub(crate) fn harness_program_lookup(
    program: &str,
    settings: &SpawnSettings,
) -> (Option<String>, OsString) {
    let mut builder = CommandBuilder::new("skein-agent-probe");
    let applied = apply_env(
        &mut builder,
        settings,
        probe_snapshot(),
        None,
        &Injection::default(),
    );
    (resolve_program(program, &applied.path), applied.path)
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Build the environment a harness would get right now, without spawning
/// anything.
pub(crate) fn env_preview(settings: &SpawnSettings) -> EnvPreview {
    // The program name doesn't affect `get_base_env`, and we never spawn
    // this builder — we only read the environment back off it.
    let mut builder = CommandBuilder::new("skein-preview");
    // The *non-waiting* read on purpose: `spawn_env_preview` is a sync
    // Tauri command, so waiting here would block the event loop for up
    // to `PROBE_WAIT`, and the panel already knows how to follow a
    // `pending` state until it settles.
    let applied = apply_env(
        &mut builder,
        settings,
        probe_snapshot(),
        None,
        &Injection::default(),
    );

    let added: Vec<String> = applied.added;
    let from_shell = applied.probe.path().is_some();
    let path = std::env::split_paths(&applied.path)
        .map(|p| {
            let entry = p.display().to_string();
            let source = if added.iter().any(|a| Path::new(a) == p) {
                PathSource::Added
            } else if from_shell {
                PathSource::Shell
            } else {
                PathSource::Inherited
            };
            PathEntryReport {
                exists: p.is_dir(),
                entry,
                source,
            }
        })
        .collect();

    let programs = PROGRAMS_OF_INTEREST
        .iter()
        .map(|name| ProgramReport {
            name: (*name).to_owned(),
            resolved: resolve_program(name, &applied.path),
        })
        .collect();

    let shell_rejected = settings
        .shell
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && settings.valid_shell().is_none())
        .map(ToOwned::to_owned);

    EnvPreview {
        probe: describe_probe(&applied.probe, settings),
        shell: applied.shell,
        path,
        programs,
        stripped: applied.stripped,
        extra_env_keys: settings
            .extra_env
            .iter()
            .map(|v| v.key.trim().to_owned())
            .filter(|k| !k.is_empty())
            .collect(),
        dropped_additions: applied
            .dropped
            .into_iter()
            .map(|(entry, reason)| DroppedAddition { entry, reason })
            .collect(),
        ignored_env_keys: applied.ignored_env_keys,
        shell_rejected,
        launch_context: launch_context(),
    }
}

/// Whether Skein inherited a terminal's environment or a stripped
/// GUI-launch one. Read from the *process* environment, deliberately
/// before any stripping, because it is diagnostic rather than inherited.
fn launch_context() -> String {
    let from_terminal = std::env::var_os("TERM_PROGRAM").is_some()
        || std::env::var_os("TMUX").is_some()
        || std::env::var_os("VSCODE_INJECTION").is_some()
        || std::env::var_os("WT_SESSION").is_some();
    if from_terminal { "terminal" } else { "bundled" }.to_owned()
}
