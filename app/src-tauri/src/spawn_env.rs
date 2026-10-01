//! Spawn-environment policy — what `PATH`, shell and environment a
//! harness PTY actually gets.
//!
//! Split out of `pty.rs` so every decision is a *pure* function with the
//! world injected (`HOME`, "does this directory exist", the probe's raw
//! stdout, the environment lookup). `pty.rs` had zero tests; `PATH` is
//! load-bearing — `portable-pty` refuses to spawn at all when `cmd[0]`
//! isn't findable (`cmdbuilder.rs:406-432`), so a merge regression is a
//! total harness outage, not a degraded one.
//!
//! `unsafe_code = "forbid"` plus edition 2024 means `std::env::set_var`
//! is unavailable, so ambient state cannot be swapped out in a test.
//! Injecting it as parameters is not a style preference here — it is the
//! only way these functions are testable at all.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::spawn_settings::CaptureMode;

/// Sentinel wrapping the probe's `PATH` payload. A login+interactive
/// shell prints startup noise to stdout (nvm banners, `brew shellenv`
/// echoes, motd), so the value has to be delimited rather than assumed
/// to be the whole of stdout.
///
/// Windows never runs the probe (`pty::prewarm_probe`), so outside the
/// test build these are unreachable there — not unused.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) const PATH_PROBE_START: &str = "___SKEIN_PATH_BEGIN___";
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) const PATH_PROBE_END: &str = "___SKEIN_PATH_END___";

/// The one-liner the probe shell runs. `printf` rather than `echo`
/// because `echo`'s escape handling differs across shells.
/// (The sentinels are spelled out because `concat!` only takes
/// literals; a test pins them against the constants above.)
pub(crate) const PROBE_SCRIPT: &str =
    "printf '%s%s%s' '___SKEIN_PATH_BEGIN___' \"$PATH\" '___SKEIN_PATH_END___'";

/// Flags that make a given shell source its rc chain and then run one
/// command. `None` means "we can't drive this shell" — the caller must
/// skip the probe rather than emit an argv that is guaranteed to fail.
///
/// Deliberately *separate* arguments, never the bundled `-ilc`: tcsh
/// parses `-ilc` as one unknown option and dies with a usage message
/// (measured), which the old probe then rejected as a non-zero exit —
/// turning every tcsh user's harness `PATH` into the launchd stub.
///
/// Only shells verified by hand on a real machine are listed. Anything
/// else falls through to `None`, which is safe: the caller keeps the
/// inherited `PATH` and still applies the user's additions.
pub(crate) fn probe_args(shell: &str, mode: CaptureMode) -> Option<&'static [&'static str]> {
    if mode == CaptureMode::None {
        return None;
    }
    let name = Path::new(shell).file_name()?.to_str()?;
    // Take the part before any version separator, so `zsh-5.9` and
    // `bash-5.2` still identify as zsh and bash. A suffix with no
    // separator (`bash5`) is not handled and falls through to `None`,
    // which is the safe direction: no probe, inherited PATH, additions
    // still applied.
    let base = name.split(['-', '.']).next().unwrap_or(name);
    match base {
        // Verified: accept `-l`/`-i`/`-c` and print a colon-joined PATH.
        // fish is included on purpose — its path-flagged variables join
        // with ':' in a quoted expansion, so the sentinel payload parses
        // exactly like a POSIX shell's.
        "sh" | "bash" | "zsh" | "ksh" | "dash" | "fish" => Some(match mode {
            CaptureMode::Login => &["-l", "-c"],
            _ => &["-l", "-i", "-c"],
        }),
        // Verified: reject `-l` alongside `-c`, accept `-i -c`. So csh
        // gets the same argv in both modes, and `.login` — where csh
        // users conventionally set `path` — is never sourced. csh is
        // un-broken here, not fixed.
        "csh" | "tcsh" => Some(&["-i", "-c"]),
        // nu / xonsh / elvish / pwsh: no compatible flag set, or no
        // `$PATH` string to print. Skip rather than guess.
        _ => None,
    }
}

/// Pull the `PATH` payload out of the probe shell's stdout.
///
/// Returns `None` when the payload is absent or truncated — the shape a
/// killed-on-deadline shell leaves behind. Returning `None` rather than
/// a best-effort prefix matters: a truncated `PATH` that still parses
/// would silently drop the tail of the user's entries.
///
/// Unreachable on Windows outside the test build — see
/// `PATH_PROBE_START`.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) fn extract_probe_path(stdout: &str) -> Option<String> {
    let start = stdout.find(PATH_PROBE_START)? + PATH_PROBE_START.len();
    let rest = &stdout[start..];
    let end = rest.find(PATH_PROBE_END)?;
    let value = &rest[..end];
    if value.is_empty() {
        return None;
    }
    Some(value.to_owned())
}

/// Expand `~`, `$VAR` / `${VAR}` and `%VAR%` in a user-supplied `PATH`
/// entry.
///
/// `None` means "this entry can't be resolved" (an unset variable, or a
/// `~user` form we deliberately don't support) — the caller drops it
/// rather than passing a literal `%LOCALAPPDATA%` into the child's
/// `PATH`, where it would silently never match anything.
///
/// `%VAR%` is expanded on every platform, not just Windows, so the
/// behaviour is uniform and testable anywhere. A `%` that isn't part of
/// a well-formed `%NAME%` (alphanumerics and `_`) stays literal, so a
/// Unix directory with a `%` in its name survives.
pub(crate) fn expand_entry(
    entry: &str,
    home: Option<&Path>,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(entry.len());
    let mut rest = entry;

    // Leading `~` only. A `~` anywhere else is a literal character, and
    // `~user` needs a passwd lookup we don't do — drop the entry rather
    // than emit a path that cannot exist.
    if let Some(after) = rest.strip_prefix('~') {
        if after.is_empty() || after.starts_with('/') || after.starts_with('\\') {
            out.push_str(home?.to_str()?);
            rest = after;
        } else {
            return None;
        }
    }

    // `skip_to` is a byte offset: variable references consume more bytes
    // than the single char the iterator yields.
    let mut skip_to = 0usize;
    for (i, ch) in rest.char_indices() {
        if i < skip_to {
            continue;
        }
        match ch {
            '$' => {
                let tail = &rest[i + 1..];
                let (name, consumed) = if let Some(braced) = tail.strip_prefix('{') {
                    match braced.find('}') {
                        // `${NAME}` — 2 delimiters plus the name.
                        Some(close) => (&braced[..close], close + 3),
                        None => ("", 0),
                    }
                } else {
                    let len = tail
                        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .unwrap_or(tail.len());
                    (&tail[..len], len + 1)
                };
                if name.is_empty() {
                    out.push('$');
                    continue;
                }
                out.push_str(&resolve_var(name, home, lookup)?);
                skip_to = i + consumed;
            }
            '%' => {
                let tail = &rest[i + 1..];
                let name = match tail.find('%') {
                    Some(close) => &tail[..close],
                    None => "",
                };
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push('%');
                    continue;
                }
                out.push_str(&resolve_var(name, home, lookup)?);
                skip_to = i + name.len() + 2;
            }
            _ => out.push(ch),
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// `HOME` / `USERPROFILE` answer from the injected home directory so a
/// test can pin them without touching process state; everything else
/// goes to the injected lookup.
fn resolve_var(
    name: &str,
    home: Option<&Path>,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if (name.eq_ignore_ascii_case("HOME") || name.eq_ignore_ascii_case("USERPROFILE"))
        && let Some(h) = home
    {
        return h.to_str().map(ToOwned::to_owned);
    }
    lookup(name)
}

/// Key two `PATH` entries compare equal under. Windows paths are
/// case-insensitive and tolerate a trailing separator; Unix paths are
/// neither, but a trailing slash is still the same directory.
fn dedupe_key(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let trimmed = raw.trim_end_matches(['/', '\\']);
    let base = if trimmed.is_empty() {
        raw.as_ref()
    } else {
        trimmed
    };
    if cfg!(windows) {
        base.to_lowercase()
    } else {
        base.to_owned()
    }
}

/// Concatenate two `PATH` values, first then second.
///
/// Split-and-rejoin rather than a hand-written separator so it is
/// correct on every platform, and so it compiles and is tested
/// everywhere even though only Windows needs it. `merge_path` dedupes
/// afterwards, so overlap between the two is free.
pub(crate) fn concat_paths(first: Option<OsString>, second: Option<OsString>) -> OsString {
    match (first, second) {
        (Some(a), Some(b)) => {
            let joined = std::env::split_paths(&a).chain(std::env::split_paths(&b));
            // Only fails if an entry contains the separator, which
            // cannot happen for values that came *from* a PATH.
            std::env::join_paths(joined).unwrap_or(a)
        }
        (Some(v), None) | (None, Some(v)) => v,
        (None, None) => OsString::new(),
    }
}

/// Why one of the user's `PATH` additions didn't make it in.
///
/// Reported rather than silently swallowed: "I added a directory and
/// nothing happened" is the exact confusion this whole feature exists
/// to end, and a dropped entry with no explanation reproduces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DropReason {
    /// References a variable that isn't set, or a `~user` form.
    Unresolved,
    /// Relative. A relative entry — `.` above all — makes an agent run
    /// whatever happens to be in the working directory.
    NotAbsolute,
    /// Contains a character that can't appear in a single `PATH` entry.
    Separator,
    /// The directory doesn't exist.
    Missing,
    /// Already on the `PATH`.
    Duplicate,
}

/// The outcome of building a child's `PATH`.
pub(crate) struct MergedPath {
    pub path: OsString,
    /// Additions that actually made it in, expanded, in order.
    pub added: Vec<String>,
    /// Additions that didn't, paired with why. The entry text is the
    /// user's original spelling so the UI can point at the right row.
    pub dropped: Vec<(String, DropReason)>,
}

/// Build the child's `PATH`: the user's additions first, then `base`.
///
/// - Additions are expanded, required to be absolute, dropped when the
///   directory doesn't exist, and deduped against `base` and each other
///   (first occurrence wins, so the user's stated order is preserved).
/// - `base` is deduped too. Real shells hand back `PATH`s with repeated
///   entries — this machine's own has two — and every duplicate is a
///   wasted `stat` on every command lookup for the life of the harness.
/// - Empty and relative entries are dropped from *both* sides. An empty
///   `PATH` element means *the current directory* on Unix; inherited
///   into an agent harness that runs what it finds, that is a real
///   hazard, and `.` spelled out is the same hazard.
/// - Prepend-only by design (issue #3): we never replace what the shell
///   or the OS reported, because a mistyped replacement is a state you
///   cannot recover from inside the app.
pub(crate) fn merge_path(
    base: &OsStr,
    prepends: &[String],
    home: Option<&Path>,
    lookup: &dyn Fn(&str) -> Option<String>,
    dir_exists: &dyn Fn(&Path) -> bool,
) -> MergedPath {
    let mut seen: Vec<String> = Vec::new();
    let mut existing: Vec<PathBuf> = Vec::new();
    for entry in std::env::split_paths(base) {
        if entry.as_os_str().is_empty() || !entry.is_absolute() {
            continue;
        }
        let key = dedupe_key(&entry);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        existing.push(entry);
    }

    let mut added: Vec<String> = Vec::new();
    let mut dropped: Vec<(String, DropReason)> = Vec::new();
    let mut ordered: Vec<PathBuf> = Vec::with_capacity(existing.len() + prepends.len());

    for entry in prepends {
        let original = entry.clone();
        let Some(expanded) = expand_entry(entry, home, lookup) else {
            dropped.push((original, DropReason::Unresolved));
            continue;
        };
        // `join_paths` rejects an entry containing the separator, which
        // would fail the whole merge rather than this one entry.
        if expanded.contains(PATH_ENTRY_FORBIDDEN) {
            dropped.push((original, DropReason::Separator));
            continue;
        }
        let path = PathBuf::from(&expanded);
        if !path.is_absolute() {
            dropped.push((original, DropReason::NotAbsolute));
            continue;
        }
        if !dir_exists(&path) {
            dropped.push((original, DropReason::Missing));
            continue;
        }
        let key = dedupe_key(&path);
        if seen.contains(&key) {
            dropped.push((original, DropReason::Duplicate));
            continue;
        }
        seen.push(key);
        added.push(expanded);
        ordered.push(path);
    }
    ordered.extend(existing);

    let path = std::env::join_paths(&ordered).unwrap_or_else(|_| base.to_owned());
    MergedPath {
        path,
        added,
        dropped,
    }
}

/// Characters Skein refuses inside a single `PATH` entry.
///
/// On Unix `join_paths` errors on `:`, so this is forced. On Windows it
/// errors only on `"` and would *quote* an entry containing `;` — we
/// drop those anyway, because a quoted `PATH` element is fragile for
/// whatever child has to re-split it.
#[cfg(windows)]
const PATH_ENTRY_FORBIDDEN: &[char] = &[';', '"'];
#[cfg(not(windows))]
const PATH_ENTRY_FORBIDDEN: &[char] = &[':'];

// ── Host-terminal identity (issue #192) ────────────────────────────
//
// Skein forwards the environment it was launched with. When Skein is
// itself started from tmux, VS Code's terminal, iTerm or Windows
// Terminal, those markers reach claude / opencode / copilot — and every
// one of them sniffs the terminal. The child then adopts the *host*
// terminal's key, clipboard and rendering quirks instead of xterm.js's.
//
// The dangerous direction here is over-stripping, not under-stripping:
// removing something the agent needs fails silently and looks like a
// bug somewhere else entirely. So `KEEP` is checked first and wins over
// both the exact list and the prefixes, and prefixes are only used for
// families that are unambiguously one vendor's namespace.

/// Exact variable names to drop. Sorted; one comment per group.
pub(crate) const HOST_TERMINAL_ENV_VARS: &[&str] = &[
    // Generic terminal identification.
    "COLORFGBG",
    "TERMINAL_EMULATOR",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    // Multiplexers. `STY` is GNU screen; `TMUX_PANE` addresses a pane
    // that doesn't exist from the child's point of view.
    "STY",
    "TMUX",
    "TMUX_PANE",
    // Editors that host a terminal and expect to be talked back to.
    "EMACS",
    "INSIDE_EMACS",
    "NVIM",
    "NVIM_LISTEN_ADDRESS",
    "NVIM_LOG_FILE",
    // Credential helpers wired to a host-process IPC socket. `GIT_ASKPASS`
    // is the highest-value entry in this whole list: VS Code points it at
    // a Node shim unreachable from a Skein PTY, so a `git push` that needs
    // credentials hangs the agent with no tty prompt to fall back to.
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "SSH_ASKPASS_REQUIRE",
    // Inbound-SSH session identity. NOT `SSH_AUTH_SOCK` — see `KEEP`.
    "SSH_CLIENT",
    "SSH_CONNECTION",
    "SSH_TTY",
    // Terminal geometry. Stale values make a TUI lay out for the wrong
    // size before its first resize event lands.
    "COLUMNS",
    "LINES",
    // Desktop launch handoff tokens — single-use, and already consumed.
    "DESKTOP_STARTUP_ID",
    "XDG_ACTIVATION_TOKEN",
    // Claude Code's own session markers. A nested `claude` must not think
    // it is the outer one. `CLAUDE_CODE_GIT_BASH_PATH` is deliberately
    // absent — see `KEEP`.
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    // Windows / MSYS shell identity.
    "MSYSTEM",
    "PROMPT",
    "WSLENV",
];

/// Vendor namespaces where every member is host-terminal identity.
/// Used only where the prefix cannot plausibly collide with something
/// the agent needs.
pub(crate) const HOST_TERMINAL_ENV_PREFIXES: &[&str] = &[
    "ALACRITTY_",
    "ANSICON",
    "CONEMU",
    "GHOSTTY_",
    "GNOME_TERMINAL_",
    "ITERM_",
    "KITTY_",
    "KONSOLE_",
    "LC_TERMINAL",
    "VSCODE_",
    "VTE_",
    "WEZTERM_",
    "WT_",
    "ZELLIJ",
];

/// Never stripped, whatever the lists above say. Each entry is here
/// because removing it breaks something real.
pub(crate) const HOST_TERMINAL_ENV_KEEP: &[&str] = &[
    // The agent's ssh identity: the one variable a Finder-launched macOS
    // bundle inherits that a harness genuinely depends on (launchd also
    // supplies USER/LOGNAME/TMPDIR/XPC_*/__CF*, none of which matter
    // here). Dropping it breaks every `git push` over ssh. A reference
    // PTY test harness can afford to strip it; a production IDE cannot.
    "SSH_AUTH_SOCK",
    // Claude Code on native Windows requires Git Bash and finds it here.
    // No current rule matches it; this is a guard against someone later
    // "tidying" the three `CLAUDE_CODE_*` names above into a prefix.
    "CLAUDE_CODE_GIT_BASH_PATH",
    // Colour preference is user intent, not host identity.
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "FORCE_COLOR",
    "NO_COLOR",
    // Terminal capability databases — about what the child can render,
    // not who launched it.
    "TERMINFO",
    "TERMINFO_DIRS",
];

/// Whether `key` is host-terminal identity that must not reach a
/// harness. Case-insensitive: Windows environment keys are, and
/// `portable-pty` lowercases them there anyway.
pub(crate) fn is_host_terminal_var(key: &str) -> bool {
    if HOST_TERMINAL_ENV_KEEP
        .iter()
        .any(|k| k.eq_ignore_ascii_case(key))
    {
        return false;
    }
    if HOST_TERMINAL_ENV_VARS
        .iter()
        .any(|k| k.eq_ignore_ascii_case(key))
    {
        return true;
    }
    let upper = key.to_ascii_uppercase();
    HOST_TERMINAL_ENV_PREFIXES
        .iter()
        .any(|p| upper.starts_with(p))
}

#[cfg(test)]
mod tests;
