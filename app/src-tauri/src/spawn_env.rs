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

/// Sentinel wrapping the probe's `PATH` payload. A login+interactive
/// shell prints startup noise to stdout (nvm banners, `brew shellenv`
/// echoes, motd), so the value has to be delimited rather than assumed
/// to be the whole of stdout.
pub(crate) const PATH_PROBE_START: &str = "___SKEIN_PATH_BEGIN___";
pub(crate) const PATH_PROBE_END: &str = "___SKEIN_PATH_END___";

/// The one-liner the probe shell runs. `printf` rather than `echo`
/// because `echo`'s escape handling differs across shells.
/// (The sentinels are spelled out because `concat!` only takes
/// literals; a test pins them against the constants above.)
pub(crate) const PROBE_SCRIPT: &str =
    "printf '%s%s%s' '___SKEIN_PATH_BEGIN___' \"$PATH\" '___SKEIN_PATH_END___'";

/// Flags that make a given shell source its full rc chain and then run
/// one command. `None` means "we don't know how to drive this shell" —
/// the caller must skip the probe entirely rather than emit an argv that
/// is guaranteed to fail.
///
/// Deliberately *separate* arguments, never the bundled `-ilc`: tcsh
/// parses `-ilc` as one unknown option and dies with a usage message
/// (measured), which the old probe then rejected as a non-zero exit —
/// turning every tcsh user's harness `PATH` into the launchd stub.
///
/// Only shells verified by hand on a real machine are listed. Anything
/// else falls through to `None`, which is safe: the caller keeps the
/// inherited `PATH` and still applies the user's additions.
pub(crate) fn probe_args(shell: &str) -> Option<&'static [&'static str]> {
    let name = Path::new(shell).file_name()?.to_str()?;
    // Strip a trailing version suffix the way `bash5`/`zsh-5.9` appear
    // on some distros; the base name is what identifies the dialect.
    let base = name.split(['-', '.']).next().unwrap_or(name);
    match base {
        // Verified: accept `-l -i -c` and print a colon-joined PATH.
        // fish is included on purpose — its path-flagged variables join
        // with ':' in a quoted expansion, so the sentinel payload parses
        // exactly like a POSIX shell's.
        "sh" | "bash" | "zsh" | "ksh" | "dash" | "fish" => Some(&["-l", "-i", "-c"]),
        // Verified: reject `-l` alongside `-c`, accept `-i -c`. Note
        // this means `.login` is never sourced, which is where csh users
        // conventionally set `path` — csh is un-broken here, not fixed.
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

/// Build the child's `PATH`: the user's additions first, then `base`.
///
/// - Additions are expanded, dropped when the directory doesn't exist,
///   and deduped against `base` and each other (first occurrence wins,
///   so the user's stated order is preserved).
/// - Empty entries in `base` are dropped. An empty `PATH` element means
///   *the current directory* on Unix — inherited into an agent harness
///   that runs whatever it finds, that is a real hazard, and it is
///   exactly what a naive split of an empty `PATH` produces.
/// - Prepend-only by design (issue #3): we never replace what the shell
///   or the OS reported, because a mistyped replacement is an
///   unrecoverable-from-inside-the-app state.
pub(crate) fn merge_path(
    base: &OsStr,
    prepends: &[String],
    home: Option<&Path>,
    lookup: &dyn Fn(&str) -> Option<String>,
    dir_exists: &dyn Fn(&Path) -> bool,
) -> OsString {
    let existing: Vec<PathBuf> = std::env::split_paths(base)
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    let mut seen: Vec<String> = existing.iter().map(|p| dedupe_key(p)).collect();

    let mut ordered: Vec<PathBuf> = Vec::with_capacity(existing.len() + prepends.len());
    for entry in prepends {
        let Some(expanded) = expand_entry(entry, home, lookup) else {
            continue;
        };
        let path = PathBuf::from(&expanded);
        // `join_paths` rejects an entry containing the separator (and a
        // quote on Windows). Filtering here keeps the join infallible in
        // practice instead of silently discarding the whole merge.
        if expanded.contains(PATH_ENTRY_FORBIDDEN) {
            continue;
        }
        if !dir_exists(&path) {
            continue;
        }
        let key = dedupe_key(&path);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        ordered.push(path);
    }
    ordered.extend(existing);

    std::env::join_paths(&ordered).unwrap_or_else(|_| base.to_owned())
}

/// Characters that cannot appear inside a single `PATH` entry —
/// `join_paths` rejects them, which would fail the whole merge.
#[cfg(windows)]
const PATH_ENTRY_FORBIDDEN: &[char] = &[';', '"'];
#[cfg(not(windows))]
const PATH_ENTRY_FORBIDDEN: &[char] = &[':'];

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
            assert_eq!(probe_args(shell), Some(&["-l", "-i", "-c"][..]), "{shell}");
        }
    }

    #[test]
    fn probe_args_csh_family_drops_the_login_flag() {
        for shell in ["/bin/csh", "/bin/tcsh"] {
            assert_eq!(probe_args(shell), Some(&["-i", "-c"][..]), "{shell}");
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
            assert_eq!(probe_args(shell), None, "{shell}");
        }
    }

    #[test]
    fn probe_args_tolerates_versioned_names() {
        assert_eq!(
            probe_args("/usr/bin/bash-5.2"),
            Some(&["-l", "-i", "-c"][..])
        );
        assert_eq!(
            probe_args("/usr/bin/zsh-5.9"),
            Some(&["-l", "-i", "-c"][..])
        );
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

    #[test]
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
            parts(&out),
            vec![
                "/home/tester/.local/bin",
                "/home/tester/bin",
                "/usr/bin",
                "/bin"
            ]
        );
    }

    #[test]
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
        assert_eq!(parts(&out), vec!["/home/tester/.local/bin", "/usr/bin"]);
    }

    #[test]
    fn merge_path_is_idempotent() {
        let base = join(&["/usr/bin", "/bin"]);
        let prepends = vec!["~/.local/bin".to_owned(), "~/bin".to_owned()];
        let exists = |_: &Path| true;
        let once = merge_path(&base, &prepends, Some(&home()), &no_vars, &exists);
        let twice = merge_path(&once, &prepends, Some(&home()), &no_vars, &exists);
        assert_eq!(parts(&once), parts(&twice));
    }

    #[test]
    fn merge_path_collapses_duplicates_including_trailing_separators() {
        let base = join(&["/usr/bin", "/home/tester/bin/"]);
        let out = merge_path(
            &base,
            &["~/bin".into(), "~/bin".into()],
            Some(&home()),
            &no_vars,
            &|_| true,
        );
        assert_eq!(parts(&out), vec!["/usr/bin", "/home/tester/bin/"]);
    }

    #[test]
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
        assert_eq!(parts(&out), vec!["/home/tester/bin"]);
        assert!(!parts(&out).iter().any(String::is_empty));

        let with_hole = join(&["/usr/bin", "", "/bin"]);
        let out = merge_path(&with_hole, &[], Some(&home()), &no_vars, &|_| true);
        assert!(!parts(&out).iter().any(String::is_empty));
    }

    #[test]
    fn merge_path_rejects_an_entry_containing_the_separator() {
        // Must be dropped whole, never split into two entries — and it
        // must not fail the merge for everything else.
        let sneaky = if cfg!(windows) {
            "/a;/b".to_owned()
        } else {
            "/a:/b".to_owned()
        };
        let base = join(&["/usr/bin"]);
        let out = merge_path(&base, &[sneaky], Some(&home()), &no_vars, &|_| true);
        assert_eq!(parts(&out), vec!["/usr/bin"]);
    }

    #[test]
    fn merge_path_with_no_additions_returns_the_base_unchanged() {
        let base = join(&["/usr/bin", "/bin"]);
        let out = merge_path(&base, &[], Some(&home()), &no_vars, &|_| true);
        assert_eq!(parts(&out), parts(&base));
    }
}
