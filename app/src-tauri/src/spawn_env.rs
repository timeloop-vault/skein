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
mod tests {
    use super::*;

    /// Split a joined `PATH` back into comparable pieces, so assertions
    /// don't hardcode `:` vs `;`.
    fn parts(joined: &OsStr) -> Vec<String> {
        std::env::split_paths(joined)
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    /// The resolved PATH of a merge, as comparable pieces.
    fn merged_parts(m: &MergedPath) -> Vec<String> {
        parts(&m.path)
    }

    fn reasons(m: &MergedPath) -> Vec<(String, DropReason)> {
        m.dropped.clone()
    }

    fn join(entries: &[&str]) -> OsString {
        std::env::join_paths(entries.iter().map(Path::new)).expect("test paths are joinable")
    }

    fn no_vars(_: &str) -> Option<String> {
        None
    }

    fn home() -> PathBuf {
        PathBuf::from("/home/tester")
    }

    // ── probe_args ────────────────────────────────────────────────

    #[test]
    fn probe_args_posix_shells_use_separate_flags() {
        // Pinned as three entries on purpose: the bundled "-ilc" form is
        // what broke csh/tcsh, and a "tidy-up" back to one string would
        // silently reintroduce it.
        for shell in [
            "/bin/sh",
            "/bin/bash",
            "/bin/zsh",
            "/bin/ksh",
            "/bin/dash",
            "/opt/homebrew/bin/fish",
        ] {
            assert_eq!(
                probe_args(shell, CaptureMode::LoginInteractive),
                Some(&["-l", "-i", "-c"][..]),
                "{shell}"
            );
            assert_eq!(
                probe_args(shell, CaptureMode::Login),
                Some(&["-l", "-c"][..]),
                "{shell}"
            );
        }
    }

    #[test]
    fn probe_args_csh_family_drops_the_login_flag() {
        for shell in ["/bin/csh", "/bin/tcsh"] {
            for mode in [CaptureMode::LoginInteractive, CaptureMode::Login] {
                assert_eq!(probe_args(shell, mode), Some(&["-i", "-c"][..]), "{shell}");
            }
        }
    }

    #[test]
    fn probe_args_unknown_shells_are_skipped_not_guessed() {
        for shell in [
            "/usr/bin/nu",
            "/usr/bin/xonsh",
            "/usr/bin/elvish",
            "/usr/local/bin/pwsh",
            "powershell.exe",
            "",
        ] {
            assert_eq!(
                probe_args(shell, CaptureMode::LoginInteractive),
                None,
                "{shell}"
            );
        }
    }

    #[test]
    fn probe_args_tolerates_versioned_names() {
        let m = CaptureMode::LoginInteractive;
        assert_eq!(
            probe_args("/usr/bin/bash-5.2", m),
            Some(&["-l", "-i", "-c"][..])
        );
        assert_eq!(
            probe_args("/usr/bin/zsh-5.9", m),
            Some(&["-l", "-i", "-c"][..])
        );
    }

    #[test]
    fn capture_mode_none_skips_every_shell() {
        // The escape hatch: "don't ask a shell at all". It has to hold
        // even for shells we know how to drive.
        for shell in ["/bin/zsh", "/bin/bash", "/bin/tcsh"] {
            assert_eq!(probe_args(shell, CaptureMode::None), None, "{shell}");
        }
    }

    // ── extract_probe_path ────────────────────────────────────────

    #[test]
    fn probe_script_uses_the_declared_sentinels() {
        assert!(PROBE_SCRIPT.contains(PATH_PROBE_START));
        assert!(PROBE_SCRIPT.contains(PATH_PROBE_END));
    }

    #[test]
    fn extract_probe_path_ignores_surrounding_shell_noise() {
        let out =
            format!("Now using node v22\n{PATH_PROBE_START}/usr/bin:/bin{PATH_PROBE_END}\nbye\n");
        assert_eq!(extract_probe_path(&out).as_deref(), Some("/usr/bin:/bin"));
    }

    #[test]
    fn extract_probe_path_rejects_a_truncated_payload() {
        // The shape a shell killed on the deadline leaves behind. A
        // best-effort prefix here would silently drop the tail of the
        // user's PATH, which is worse than having no probe result.
        let out = format!("{PATH_PROBE_START}/usr/bin:/opt/homebrew/b");
        assert_eq!(extract_probe_path(&out), None);
    }

    #[test]
    fn extract_probe_path_rejects_missing_or_empty_payloads() {
        assert_eq!(extract_probe_path(""), None);
        assert_eq!(extract_probe_path("no sentinels at all"), None);
        assert_eq!(
            extract_probe_path(&format!("{PATH_PROBE_END}/usr/bin")),
            None
        );
        assert_eq!(
            extract_probe_path(&format!("{PATH_PROBE_START}{PATH_PROBE_END}")),
            None
        );
    }

    #[test]
    fn extract_probe_path_takes_the_first_complete_payload() {
        let out = format!(
            "{PATH_PROBE_START}/first{PATH_PROBE_END}{PATH_PROBE_START}/second{PATH_PROBE_END}"
        );
        assert_eq!(extract_probe_path(&out).as_deref(), Some("/first"));
    }

    // ── expand_entry ──────────────────────────────────────────────

    #[test]
    fn expand_entry_expands_leading_tilde() {
        assert_eq!(
            expand_entry("~/.local/bin", Some(&home()), &no_vars).as_deref(),
            Some("/home/tester/.local/bin")
        );
        assert_eq!(
            expand_entry("~", Some(&home()), &no_vars).as_deref(),
            Some("/home/tester")
        );
    }

    #[test]
    fn expand_entry_refuses_tilde_user() {
        // Needs a passwd lookup we don't do; emitting it literally would
        // put a directory that cannot exist into PATH.
        assert_eq!(expand_entry("~root/bin", Some(&home()), &no_vars), None);
    }

    #[test]
    fn expand_entry_expands_variable_forms() {
        assert_eq!(
            expand_entry("$HOME/bin", Some(&home()), &no_vars).as_deref(),
            Some("/home/tester/bin")
        );
        assert_eq!(
            expand_entry("${HOME}/bin", Some(&home()), &no_vars).as_deref(),
            Some("/home/tester/bin")
        );
        assert_eq!(
            expand_entry("%USERPROFILE%/bin", Some(&home()), &no_vars).as_deref(),
            Some("/home/tester/bin")
        );
    }

    #[test]
    fn expand_entry_uses_the_injected_lookup() {
        let lookup = |k: &str| (k == "SDK").then(|| "/opt/sdk".to_owned());
        assert_eq!(
            expand_entry("$SDK/bin", None, &lookup).as_deref(),
            Some("/opt/sdk/bin")
        );
        assert_eq!(
            expand_entry("%SDK%/bin", None, &lookup).as_deref(),
            Some("/opt/sdk/bin")
        );
    }

    #[test]
    fn expand_entry_drops_entries_referencing_unset_variables() {
        // Better a missing entry than a literal "%LOCALAPPDATA%\bin" in
        // PATH, which silently matches nothing forever.
        assert_eq!(expand_entry("$NOPE/bin", Some(&home()), &no_vars), None);
        assert_eq!(expand_entry("%NOPE%/bin", Some(&home()), &no_vars), None);
        assert_eq!(expand_entry("~/x", None, &no_vars), None);
    }

    #[test]
    fn expand_entry_leaves_stray_sigils_literal() {
        assert_eq!(
            expand_entry("/opt/100%/bin", Some(&home()), &no_vars).as_deref(),
            Some("/opt/100%/bin")
        );
        assert_eq!(
            expand_entry("/opt/a$/bin", Some(&home()), &no_vars).as_deref(),
            Some("/opt/a$/bin")
        );
    }

    #[test]
    fn expand_entry_ignores_blank_input() {
        assert_eq!(expand_entry("", Some(&home()), &no_vars), None);
        assert_eq!(expand_entry("   ", Some(&home()), &no_vars), None);
    }

    #[test]
    fn expand_entry_handles_multibyte_content() {
        assert_eq!(
            expand_entry("~/kläder/bin", Some(&home()), &no_vars).as_deref(),
            Some("/home/tester/kläder/bin")
        );
    }

    // ── merge_path ────────────────────────────────────────────────
    //
    // Split by platform (#202). `merge_path` is platform-generic —
    // `split_paths`/`join_paths` for the separator, a cfg'd
    // `PATH_ENTRY_FORBIDDEN` — but its *fixtures* cannot be: `/usr/bin`
    // is not absolute on Windows (an absolute path there needs a prefix,
    // not just a root), so a Unix-shaped base is entirely discarded and
    // the assertions would be wrong to pass. The cases below are the
    // Unix half; `windows_merge_path` mirrors each one, and anything
    // genuinely platform-neutral (`merge_path_is_idempotent`,
    // `concat_paths_joins_with_the_platform_separator`) stays ungated.

    #[test]
    #[cfg(unix)]
    fn merge_path_prepends_existing_directories_in_order() {
        let base = join(&["/usr/bin", "/bin"]);
        let exists = |p: &Path| p.starts_with("/home/tester");
        let out = merge_path(
            &base,
            &["~/.local/bin".into(), "~/bin".into()],
            Some(&home()),
            &no_vars,
            &exists,
        );
        assert_eq!(
            merged_parts(&out),
            vec![
                "/home/tester/.local/bin",
                "/home/tester/bin",
                "/usr/bin",
                "/bin"
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_skips_directories_that_do_not_exist() {
        let base = join(&["/usr/bin"]);
        let exists = |p: &Path| p != Path::new("/home/tester/bin");
        let out = merge_path(
            &base,
            &["~/.local/bin".into(), "~/bin".into()],
            Some(&home()),
            &no_vars,
            &exists,
        );
        assert_eq!(
            merged_parts(&out),
            vec!["/home/tester/.local/bin", "/usr/bin"]
        );
    }

    #[test]
    fn merge_path_is_idempotent() {
        let base = join(&["/usr/bin", "/bin"]);
        let prepends = vec!["~/.local/bin".to_owned(), "~/bin".to_owned()];
        let exists = |_: &Path| true;
        let once = merge_path(&base, &prepends, Some(&home()), &no_vars, &exists);
        let twice = merge_path(&once.path, &prepends, Some(&home()), &no_vars, &exists);
        assert_eq!(merged_parts(&once), merged_parts(&twice));
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_collapses_duplicates_including_trailing_separators() {
        let base = join(&["/usr/bin", "/home/tester/bin/"]);
        let out = merge_path(
            &base,
            &["~/bin".into(), "~/bin".into()],
            Some(&home()),
            &no_vars,
            &|_| true,
        );
        assert_eq!(merged_parts(&out), vec!["/usr/bin", "/home/tester/bin/"]);
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_never_emits_an_empty_entry() {
        // An empty PATH element means *the current directory* on Unix.
        // Inherited into an agent harness that runs what it finds, that
        // is a real hazard — and it is exactly what splitting an empty
        // PATH produces.
        let out = merge_path(
            OsStr::new(""),
            &["~/bin".into()],
            Some(&home()),
            &no_vars,
            &|_| true,
        );
        assert_eq!(merged_parts(&out), vec!["/home/tester/bin"]);
        assert!(!merged_parts(&out).iter().any(String::is_empty));

        let with_hole = join(&["/usr/bin", "", "/bin"]);
        let out = merge_path(&with_hole, &[], Some(&home()), &no_vars, &|_| true);
        assert!(!merged_parts(&out).iter().any(String::is_empty));
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_rejects_an_entry_containing_the_separator() {
        // Must be dropped whole, never split into two entries — and it
        // must not fail the merge for everything else.
        let sneaky = "/a:/b".to_owned();
        let base = join(&["/usr/bin"]);
        let out = merge_path(&base, &[sneaky], Some(&home()), &no_vars, &|_| true);
        assert_eq!(merged_parts(&out), vec!["/usr/bin"]);
    }

    #[test]
    fn concat_paths_joins_with_the_platform_separator() {
        // Compiled and exercised on every platform even though only the
        // Windows branch calls it — a cfg'd union would be verifiable
        // only on the machine that can't easily run these tests.
        let a = join(&["/one", "/two"]);
        let b = join(&["/three"]);
        assert_eq!(
            parts(&concat_paths(Some(a.clone()), Some(b.clone()))),
            vec!["/one", "/two", "/three"]
        );
        assert_eq!(
            parts(&concat_paths(Some(a.clone()), None)),
            vec!["/one", "/two"]
        );
        assert_eq!(parts(&concat_paths(None, Some(b))), vec!["/three"]);
        assert_eq!(concat_paths(None, None), OsString::new());
        // Overlap is left to merge_path's dedupe, not silently dropped
        // here, so the two responsibilities stay separable.
        assert_eq!(
            parts(&concat_paths(Some(a.clone()), Some(a))),
            vec!["/one", "/two", "/one", "/two"]
        );
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_refuses_relative_additions() {
        // Same hazard as an empty entry, and this is the side the user
        // controls *and* the side that goes first: a `.` here makes
        // every agent command prefer whatever is in the worktree.
        let base = join(&["/usr/bin"]);
        let out = merge_path(
            &base,
            &[".".into(), "..".into(), "bin".into()],
            Some(&home()),
            &no_vars,
            &|_| true,
        );
        assert_eq!(merged_parts(&out), vec!["/usr/bin"]);
        assert_eq!(
            reasons(&out),
            vec![
                (".".to_owned(), DropReason::NotAbsolute),
                ("..".to_owned(), DropReason::NotAbsolute),
                ("bin".to_owned(), DropReason::NotAbsolute),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_reports_why_each_addition_was_dropped() {
        // "I added a directory and nothing happened" is the confusion
        // the whole settings panel exists to end, so every skip has to
        // be attributable to a reason the UI can show.
        let base = join(&["/usr/bin", "/home/tester/bin"]);
        let sep = "/a:/b";
        let out = merge_path(
            &base,
            &[
                "$NOPE/bin".into(),
                sep.to_owned(),
                "/does/not/exist".into(),
                "~/bin".into(),
            ],
            Some(&home()),
            &no_vars,
            &|p| p != Path::new("/does/not/exist"),
        );
        assert_eq!(
            reasons(&out),
            vec![
                ("$NOPE/bin".to_owned(), DropReason::Unresolved),
                (sep.to_owned(), DropReason::Separator),
                ("/does/not/exist".to_owned(), DropReason::Missing),
                ("~/bin".to_owned(), DropReason::Duplicate),
            ]
        );
        assert!(out.added.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_dedupes_the_base_too() {
        // Real shells hand back PATHs with repeated entries — this
        // machine's own does — and each duplicate is a wasted stat on
        // every command lookup for the life of the harness.
        let base = join(&["/usr/bin", "/bin", "/usr/bin", "/bin/"]);
        let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
        assert_eq!(merged_parts(&out), vec!["/usr/bin", "/bin"]);
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_drops_relative_entries_from_the_base() {
        let base = join(&["/usr/bin", ".", "relative/dir"]);
        let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
        assert_eq!(merged_parts(&out), vec!["/usr/bin"]);
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_reports_what_it_accepted() {
        let base = join(&["/usr/bin"]);
        let out = merge_path(
            &base,
            &["~/.local/bin".into(), "/does/not/exist".into()],
            Some(&home()),
            &no_vars,
            &|p| p != Path::new("/does/not/exist"),
        );
        // `added` must reflect the merge's own filters, not a
        // re-expansion of the raw list — the preview labels PATH rows
        // from it, and crediting Skein for an entry it dropped would
        // make the panel disagree with reality.
        assert_eq!(out.added, vec!["/home/tester/.local/bin".to_owned()]);
    }

    #[test]
    #[cfg(unix)]
    fn merge_path_with_no_additions_returns_the_base_unchanged() {
        let base = join(&["/usr/bin", "/bin"]);
        let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
        assert_eq!(merged_parts(&out), parts(&base));
    }

    /// The Windows half of the `merge_path` cases above (#202).
    ///
    /// Same behaviours, Windows-shaped fixtures: `C:\…` bases, `;` as
    /// the forbidden in-entry separator, `\` as the trailing separator
    /// that must collapse. Two cases have no Unix counterpart because
    /// the behaviour only exists here — case-insensitive dedupe and
    /// `%VAR%` expansion — and one Unix case (the empty PATH element
    /// meaning "current directory") is asserted for the same reason it
    /// is there: an inherited empty entry is a real hazard for a harness
    /// that runs what it finds.
    #[cfg(windows)]
    mod windows_merge_path {
        use super::{DropReason, Path, PathBuf, join, merge_path, merged_parts, no_vars, reasons};
        use std::ffi::OsStr;

        /// A Windows `HOME` for `~` expansion. `merge_path` only ever
        /// reads it as a prefix, so it needs to exist as a path shape,
        /// not on disk.
        fn home() -> PathBuf {
            PathBuf::from(r"C:\Users\tester")
        }

        #[test]
        fn prepends_existing_directories_in_order() {
            let base = join(&[r"C:\Windows\System32", r"C:\Windows"]);
            let exists = |p: &Path| p.starts_with(r"C:\Users\tester");
            let out = merge_path(
                &base,
                &[r"~\.local\bin".into(), r"~\bin".into()],
                Some(&home()),
                &no_vars,
                &exists,
            );
            assert_eq!(
                merged_parts(&out),
                vec![
                    r"C:\Users\tester\.local\bin",
                    r"C:\Users\tester\bin",
                    r"C:\Windows\System32",
                    r"C:\Windows",
                ]
            );
        }

        #[test]
        fn skips_directories_that_do_not_exist() {
            let base = join(&[r"C:\Windows\System32"]);
            let exists = |p: &Path| p != Path::new(r"C:\Users\tester\bin");
            let out = merge_path(
                &base,
                &[r"~\.local\bin".into(), r"~\bin".into()],
                Some(&home()),
                &no_vars,
                &exists,
            );
            assert_eq!(
                merged_parts(&out),
                vec![r"C:\Users\tester\.local\bin", r"C:\Windows\System32"]
            );
        }

        #[test]
        fn collapses_duplicates_including_trailing_separators() {
            let base = join(&[r"C:\Windows\System32", r"C:\Users\tester\bin\"]);
            let out = merge_path(
                &base,
                &[r"~\bin".into(), r"~\bin".into()],
                Some(&home()),
                &no_vars,
                &|_| true,
            );
            assert_eq!(
                merged_parts(&out),
                vec![r"C:\Windows\System32", r"C:\Users\tester\bin\"]
            );
        }

        /// Windows-only: `dedupe_key` lowercases, so entries differing
        /// only in case are the same directory and must collapse.
        /// Getting this wrong costs a redundant stat on every command
        /// lookup for the life of the harness.
        #[test]
        fn dedupes_case_insensitively() {
            let base = join(&[
                r"C:\Windows\System32",
                r"c:\windows\system32\",
                r"C:\Windows",
            ]);
            let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
            assert_eq!(
                merged_parts(&out),
                vec![r"C:\Windows\System32", r"C:\Windows"]
            );
        }

        #[test]
        fn never_emits_an_empty_entry() {
            let out = merge_path(
                OsStr::new(""),
                &[r"~\bin".into()],
                Some(&home()),
                &no_vars,
                &|_| true,
            );
            assert_eq!(merged_parts(&out), vec![r"C:\Users\tester\bin"]);
            assert!(!merged_parts(&out).iter().any(String::is_empty));

            let with_hole = join(&[r"C:\Windows\System32", "", r"C:\Windows"]);
            let out = merge_path(&with_hole, &[], Some(&home()), &no_vars, &|_| true);
            assert!(!merged_parts(&out).iter().any(String::is_empty));
        }

        /// `;` is the Windows separator, so an entry containing one must
        /// be dropped whole rather than split into two — and must not
        /// take the rest of the merge down with it.
        #[test]
        fn rejects_an_entry_containing_the_separator() {
            let base = join(&[r"C:\Windows\System32"]);
            let out = merge_path(
                &base,
                &[r"C:\a;C:\b".to_owned()],
                Some(&home()),
                &no_vars,
                &|_| true,
            );
            assert_eq!(merged_parts(&out), vec![r"C:\Windows\System32"]);
        }

        #[test]
        fn refuses_relative_additions() {
            let base = join(&[r"C:\Windows\System32"]);
            let out = merge_path(
                &base,
                &[
                    ".".into(),
                    "..".into(),
                    "bin".into(),
                    r"\rooted-no-prefix".into(),
                ],
                Some(&home()),
                &no_vars,
                &|_| true,
            );
            assert_eq!(merged_parts(&out), vec![r"C:\Windows\System32"]);
            assert_eq!(
                reasons(&out),
                vec![
                    (".".to_owned(), DropReason::NotAbsolute),
                    ("..".to_owned(), DropReason::NotAbsolute),
                    ("bin".to_owned(), DropReason::NotAbsolute),
                    // Root without a drive prefix is *not* absolute on
                    // Windows — the case that makes the Unix fixtures
                    // collapse to nothing here.
                    (r"\rooted-no-prefix".to_owned(), DropReason::NotAbsolute),
                ]
            );
        }

        #[test]
        fn reports_why_each_addition_was_dropped() {
            let base = join(&[r"C:\Windows\System32", r"C:\Users\tester\bin"]);
            let out = merge_path(
                &base,
                &[
                    r"%NOPE%\bin".into(),
                    r"C:\a;C:\b".into(),
                    r"C:\does\not\exist".into(),
                    r"~\bin".into(),
                ],
                Some(&home()),
                &no_vars,
                &|p| p != Path::new(r"C:\does\not\exist"),
            );
            assert_eq!(
                reasons(&out),
                vec![
                    (r"%NOPE%\bin".to_owned(), DropReason::Unresolved),
                    (r"C:\a;C:\b".to_owned(), DropReason::Separator),
                    (r"C:\does\not\exist".to_owned(), DropReason::Missing),
                    (r"~\bin".to_owned(), DropReason::Duplicate),
                ]
            );
            assert!(out.added.is_empty());
        }

        /// Windows-only: `%VAR%` is the native form, and an addition
        /// that resolves through it must be credited like any other.
        #[test]
        fn expands_percent_vars_in_additions() {
            let base = join(&[r"C:\Windows\System32"]);
            let lookup = |name: &str| {
                (name == "LOCALAPPDATA").then(|| r"C:\Users\tester\AppData\Local".to_owned())
            };
            let out = merge_path(
                &base,
                &[r"%LOCALAPPDATA%\Programs\bin".into()],
                Some(&home()),
                &lookup,
                &|_| true,
            );
            assert_eq!(
                out.added,
                vec![r"C:\Users\tester\AppData\Local\Programs\bin".to_owned()]
            );
            assert_eq!(
                merged_parts(&out),
                vec![
                    r"C:\Users\tester\AppData\Local\Programs\bin",
                    r"C:\Windows\System32",
                ]
            );
        }

        #[test]
        fn dedupes_the_base_too() {
            let base = join(&[
                r"C:\Windows\System32",
                r"C:\Windows",
                r"C:\Windows\System32",
                r"C:\Windows\",
            ]);
            let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
            assert_eq!(
                merged_parts(&out),
                vec![r"C:\Windows\System32", r"C:\Windows"]
            );
        }

        #[test]
        fn drops_relative_entries_from_the_base() {
            let base = join(&[r"C:\Windows\System32", ".", r"relative\dir"]);
            let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
            assert_eq!(merged_parts(&out), vec![r"C:\Windows\System32"]);
        }

        #[test]
        fn reports_what_it_accepted() {
            let base = join(&[r"C:\Windows\System32"]);
            let out = merge_path(
                &base,
                &[r"~\.local\bin".into(), r"C:\does\not\exist".into()],
                Some(&home()),
                &no_vars,
                &|p| p != Path::new(r"C:\does\not\exist"),
            );
            assert_eq!(out.added, vec![r"C:\Users\tester\.local\bin".to_owned()]);
        }

        #[test]
        fn with_no_additions_returns_the_base_unchanged() {
            let base = join(&[r"C:\Windows\System32", r"C:\Windows"]);
            let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
            assert_eq!(merged_parts(&out), super::parts(&base));
        }
    }

    // ── host-terminal identity ────────────────────────────────────

    #[test]
    fn every_listed_host_terminal_var_is_stripped() {
        for key in HOST_TERMINAL_ENV_VARS {
            assert!(is_host_terminal_var(key), "{key} should be stripped");
        }
    }

    #[test]
    fn the_keep_list_always_survives() {
        // The single most important assertion in this module. A careless
        // `SSH_*` prefix rule would eat the agent's ssh-agent socket and
        // break `git push` in a way that looks like a git bug.
        for key in HOST_TERMINAL_ENV_KEEP {
            assert!(!is_host_terminal_var(key), "{key} must be kept");
        }
    }

    #[test]
    fn variables_the_harness_depends_on_are_untouched() {
        for key in [
            "PATH",
            "HOME",
            "USERPROFILE",
            "SHELL",
            "USER",
            "LANG",
            "LC_ALL",
            "TMPDIR",
            "XDG_RUNTIME_DIR",
            "ANTHROPIC_API_KEY",
            "GITHUB_TOKEN",
            "NVM_DIR",
            "JAVA_HOME",
            "SSH_AUTH_SOCK",
            "CLAUDE_CODE_GIT_BASH_PATH",
        ] {
            assert!(!is_host_terminal_var(key), "{key} must be kept");
        }
    }

    #[test]
    fn vendor_prefixes_match_their_families() {
        for key in [
            "KITTY_WINDOW_ID",
            "WEZTERM_PANE",
            "WEZTERM_UNIX_SOCKET",
            "ALACRITTY_SOCKET",
            "GHOSTTY_RESOURCES_DIR",
            "VSCODE_INJECTION",
            "VSCODE_GIT_IPC_HANDLE",
            "WT_SESSION",
            "ZELLIJ_SESSION_NAME",
            "ITERM_PROFILE",
            "LC_TERMINAL_VERSION",
            "VTE_VERSION",
            "KONSOLE_VERSION",
            "GNOME_TERMINAL_SCREEN",
            "MSYSTEM",
            "ANSICON_DEF",
        ] {
            assert!(is_host_terminal_var(key), "{key} should be stripped");
        }
    }

    #[test]
    fn matching_is_case_insensitive() {
        // Windows environment keys are case-insensitive, and portable-pty
        // lowercases them in its own map.
        assert!(is_host_terminal_var("tmux"));
        assert!(is_host_terminal_var("Term_Program"));
        assert!(is_host_terminal_var("wt_session"));
        assert!(!is_host_terminal_var("ssh_auth_sock"));
    }

    #[test]
    fn the_lists_contain_no_duplicates() {
        for list in [
            HOST_TERMINAL_ENV_VARS,
            HOST_TERMINAL_ENV_PREFIXES,
            HOST_TERMINAL_ENV_KEEP,
        ] {
            let mut seen: Vec<String> = list.iter().map(|k| k.to_ascii_uppercase()).collect();
            seen.sort_unstable();
            let before = seen.len();
            seen.dedup();
            assert_eq!(before, seen.len(), "duplicate entry in {list:?}");
        }
    }
}
