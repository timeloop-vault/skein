//! #568: argv for a `remote` harness, a tool run inside tmux on a remote
//! host over ssh (port of the argv half of `app/src/remoteCmd.ts`;
//! record building, `buildRemoteSpec` and `remoteSessionName`, stays in
//! TypeScript for now). Built from the harness record only.

use skein_proto::rooms::RemoteSpec;

/// POSIX single-quote a string.
#[must_use]
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A remote directory as a shell word. tmux never expands `~` and the
/// quotes would block it anyway, so a leading `~/` (or a bare `~`) becomes
/// a double-quoted "$HOME" that the remote login shell expands.
fn dir_word(dir: &str) -> String {
    if dir == "~" {
        return "\"$HOME\"".to_owned();
    }
    if let Some(rest) = dir.strip_prefix("~/") {
        return format!("\"$HOME\"/{}", sh_quote(rest));
    }
    sh_quote(dir)
}

/// JS `\s` plus `[\u0000-\u001f\u007f]`: the characters a host may not
/// contain. Written out because Rust's `is_whitespace` differs from JS
/// at U+0085 (Rust only) and U+FEFF (JS only).
fn is_forbidden_in_host(c: char) -> bool {
    let js_space = (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}';
    js_space || c <= '\u{1f}' || c == '\u{7f}'
}

/// An error message, or `None` when the host is usable. Rejects a leading
/// "-" so a host can never be parsed by ssh as an option
/// (`-oProxyCommand=...`).
#[must_use]
pub fn validate_remote_host(host: &str) -> Option<&'static str> {
    let h = host.trim();
    if h.is_empty() {
        return Some("Host is required.");
    }
    if h.chars().any(is_forbidden_in_host) {
        return Some("Host must not contain whitespace.");
    }
    if h.starts_with('-') {
        return Some("Host must not start with '-'.");
    }
    None
}

/// The ssh argv for a remote harness, or `None` when the host is invalid
/// or the tool is empty.
///
/// The remote command is wrapped in `exec "$SHELL" -lc '...'`: a
/// non-login ssh command sees no Homebrew / ~/.local PATH (measured on a
/// macOS host: tmux and opencode not found without it). `new-session -A`
/// attaches when the session already exists, so a Skein restart
/// reattaches. The tool is ONE tmux shell-command argument, which tmux
/// runs through the remote shell, so `~` and flags inside it work.
/// `status off` hides tmux's status line so the TUI looks native and its
/// clock doesn't keep resetting the L2a quiet timer. `--` stops ssh
/// option parsing before the host.
#[must_use]
pub fn remote_argv(spec: &RemoteSpec) -> Option<Vec<String>> {
    if validate_remote_host(&spec.host).is_some() {
        return None;
    }
    if spec.tool.trim().is_empty() {
        return None;
    }
    let session = sh_quote(&spec.session);
    let mut inner = format!("tmux new-session -A -s {session}");
    if let Some(dir) = spec.dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        inner.push_str(" -c ");
        inner.push_str(&dir_word(dir));
    }
    let inner = format!(
        "{inner} {} \\; set-option -t {session} status off",
        sh_quote(&spec.tool)
    );
    Some(vec![
        "ssh".to_owned(),
        "-t".to_owned(),
        "--".to_owned(),
        spec.host.trim().to_owned(),
        format!("exec \"$SHELL\" -lc {}", sh_quote(&inner)),
    ])
}
