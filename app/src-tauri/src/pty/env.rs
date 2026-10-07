//! The environment a harness PTY is spawned with: `PATH` merging,
//! host-terminal identity stripping and the reserved keys.

use std::ffi::OsString;
use std::path::Path;

use portable_pty::CommandBuilder;

use crate::agent_api::state::HarnessIdentity;
use crate::harness_config::Injection;
use crate::spawn_env;
use crate::spawn_env::login_env::{login_env_overlay, merge_no_proxy};
use crate::spawn_settings::SpawnSettings;

use super::probe::ProbeOutcome;
#[cfg(not(target_os = "windows"))]
use super::probe::probe_shell;

/// What `apply_env` did, for logging and for the Settings preview.
pub(crate) struct AppliedEnv {
    pub path: OsString,
    pub stripped: Vec<String>,
    pub probe: ProbeOutcome,
    pub shell: String,
    /// The expanded additions that actually made it in, in order.
    pub added: Vec<String>,
    /// The additions that didn't, and why.
    pub dropped: Vec<(String, spawn_env::DropReason)>,
    /// Extra-env keys Skein refused because it owns them itself.
    pub ignored_env_keys: Vec<String>,
    /// Names (never values) of the login-shell variables that reached the
    /// child: overlaid, not stripped afterwards, not Skein-reserved.
    pub login_env_keys: Vec<String>,
}

/// Environment variables the user cannot set through "extra environment
/// variables", because Skein writes them after that layer and would
/// silently win.
///
/// `PATH` is the one that matters: allowing it would mean the resolved
/// PATH the preview shows, the program-resolution check, and the spawn
/// log all describe a value the child never receives. The other three
/// are ours by design — `TERM`/`COLORTERM` describe how xterm.js
/// renders, and `SHELL` has to agree with the shell we actually probed.
pub(crate) const RESERVED_ENV_KEYS: &[&str] = &[
    "PATH",
    "TERM",
    "COLORTERM",
    "SHELL",
    // #213: the room's review endpoint and its bearer token. Letting a
    // user pin these by hand would point one harness at another room's
    // review — the one thing the token model exists to prevent.
    "SKEIN_REVIEW_URL",
    "SKEIN_REVIEW_TOKEN",
    "SKEIN_ROOM_ID",
    "SKEIN_HARNESS_ID",
    // #535: the build this harness runs under; always set by Skein.
    "SKEIN_VERSION",
];

/// The four variables `pty_spawn` sets from a `HarnessIdentity` — and
/// removes when it has none (#243), so a nested Skein never hands its
/// harnesses the outer room's review.
pub(super) const AGENT_IDENTITY_ENV_KEYS: &[&str] = &[
    "SKEIN_REVIEW_URL",
    "SKEIN_REVIEW_TOKEN",
    "SKEIN_ROOM_ID",
    "SKEIN_HARNESS_ID",
];

/// Apply Skein's environment policy to a `CommandBuilder`.
///
/// Shared verbatim by `spawn` and by the Settings preview, so "what the
/// preview shows is what the child gets" is a property of the code
/// rather than a claim in a doc comment.
pub(super) fn apply_env(
    builder: &mut CommandBuilder,
    settings: &SpawnSettings,
    probe: ProbeOutcome,
    agent: Option<&HarnessIdentity>,
    injection: &Injection,
) -> AppliedEnv {
    // `CommandBuilder::new` already seeds the child env from this
    // process's environment — and on Windows it additionally merges the
    // *live* `HKLM` + `HKCU` registry PATH, i.e. the user's current
    // PATH rather than whatever stale block Skein happened to inherit at
    // launch. We used to copy `std::env::vars()` over the top of that,
    // which on Windows overwrote the registry merge with the stale value
    // (portable-pty lowercases env keys there, so our `PATH` collided
    // with its `Path`) — the one platform where nothing else
    // compensated. There is nothing to re-copy: let the base env stand
    // and override only what we own.

    // The login shell's full environment (rc-file exports such as API
    // keys, proxy settings, JAVA_HOME): a GUI-launched Skein inherits
    // only launchd's. Overlaid before the strip below so a TERM_PROGRAM an
    // rc file exports is stripped too, and before PATH/extra env so those
    // keep winning. `PATH` itself is never in the overlay (merged below).
    let overlay = login_env_overlay(probe.login_env().unwrap_or_default());
    for (key, value) in &overlay {
        builder.env(key, value);
    }

    // #192: strip host-terminal identity. Skein inherits markers like
    // TERM_PROGRAM / TMUX / VSCODE_* when it is itself launched from a
    // terminal, and the agent CLIs sniff them — adopting the *host*
    // terminal's key and clipboard quirks instead of xterm.js's.
    // `iter_full_env_as_str` borrows the builder, so collect the keys
    // before removing any.
    let mut stripped: Vec<String> = if settings.strip_host_env {
        builder
            .iter_full_env_as_str()
            .map(|(k, _)| k.to_owned())
            .filter(|k| spawn_env::is_host_terminal_var(k))
            .collect()
    } else {
        Vec::new()
    };
    stripped.sort_unstable();
    for key in &stripped {
        builder.env_remove(key);
    }
    // Skein overwrites the reserved keys itself, so they are not "from the
    // login shell" as far as the child is concerned; nor are names the
    // user's additions or this spawn's injection override.
    let mut login_env_keys: Vec<String> = overlay
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| !stripped.iter().any(|s| s == k))
        .filter(|k| !RESERVED_ENV_KEYS.iter().any(|r| r.eq_ignore_ascii_case(k)))
        .filter(|k| {
            !settings
                .extra_env
                .iter()
                .any(|v| v.key.trim().eq_ignore_ascii_case(k))
        })
        .filter(|k| !injection.env.iter().any(|(n, _)| n.eq_ignore_ascii_case(k)))
        .map(str::to_owned)
        .collect();
    login_env_keys.sort_unstable();

    // PATH: the login-shell probe when we have one, otherwise the
    // inherited (on Windows, registry-merged) PATH — and the user's
    // additions on top either way. Applying additions only on the
    // probe's success path, which is what the code did before, meant a
    // probe failure silently took `~/.local/bin` with it, and that is
    // where `claude` itself is installed.
    let probed = probe
        .path()
        .map_or_else(|| base_path(builder), OsString::from);
    let home = crate::home_dir();
    let lookup = |k: &str| std::env::var(k).ok();
    let dir_exists = |p: &Path| p.is_dir();
    let merged = spawn_env::merge_path(
        &probed,
        &settings.path_prepend,
        home.as_deref(),
        &lookup,
        &dir_exists,
    );
    builder.env("PATH", &merged.path);

    // The user's own KEY=VALUE additions, last of the inherited layers
    // so they win over anything the base env carried, but before the
    // TERM/COLORTERM force below, which is ours to own.
    //
    // #215 adds to the reserved set *dynamically*: a key this spawn is
    // about to inject (`OPENCODE_CONFIG`) is ours for this spawn only.
    // Reserving it unconditionally would mean that turning the
    // injection off in Settings — the documented way to reclaim the
    // variable for your own config file — still left the user unable to
    // set it.
    let mut ignored_env_keys = Vec::new();
    for var in &settings.extra_env {
        let key = var.key.trim();
        if key.is_empty() {
            continue;
        }
        // `CLAUDE_CODE_PLUGIN_DIRS` is the exception (#318): the user's
        // own list is merged with ours below, never ignored.
        let reserved = RESERVED_ENV_KEYS
            .iter()
            .any(|r| r.eq_ignore_ascii_case(key))
            || injection.env.iter().any(|(k, _)| {
                k.eq_ignore_ascii_case(key) && k != crate::harness_config::CLAUDE_PLUGIN_DIRS_VAR
            });
        if reserved {
            ignored_env_keys.push(key.to_owned());
            continue;
        }
        builder.env(key, &var.value);
    }

    // Unix only: portable-pty otherwise fills SHELL from the passwd
    // database, which can disagree with the shell we actually probed and
    // with the one a shell harness runs. Windows has no meaningful
    // $SHELL, and setting one confuses Git-Bash-aware tools that read it.
    #[cfg(not(target_os = "windows"))]
    let shell = probe_shell(settings);
    #[cfg(target_os = "windows")]
    let shell = settings.valid_shell().unwrap_or_default().to_owned();
    #[cfg(not(target_os = "windows"))]
    builder.env("SHELL", &shell);

    // TERM / COLORTERM are about how *we* render, not about what the
    // user configured, so they are forced last and unconditionally.
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");

    // #535: which Skein build spawned this harness. Not secret and not
    // tied to the agent API, so it is set even with no identity — a hook
    // can read it when the server never bound. Overwrites an inherited
    // value, so a nested Skein reports its own version, not the outer's.
    builder.env("SKEIN_VERSION", crate::build_info::VERSION);

    // #213: how this harness reaches its room's review. After the
    // host-terminal strip and after the user's extra env, because these
    // are Skein's to own — they are also in `RESERVED_ENV_KEYS`, so a
    // user who sets one by hand is told it was ignored rather than
    // having it silently overwritten here.
    //
    // Both harnesses expand `${VAR}` inside their MCP config, so the
    // URL never has to be a number anyone agreed on in advance.
    //
    // #243: and when there is no identity, *remove* them rather than
    // leave the base env alone. Skein (dev) is dogfooded from a harness
    // inside the release Skein, so the base env carries the OUTER
    // room's URL and token; an inner harness that inherited them would
    // read and answer another room's review — the one thing the token
    // model exists to prevent.
    if let Some(agent) = agent {
        builder.env("SKEIN_REVIEW_URL", &agent.url);
        builder.env("SKEIN_REVIEW_TOKEN", &agent.token);
        builder.env("SKEIN_ROOM_ID", &agent.room_id);
        builder.env("SKEIN_HARNESS_ID", &agent.harness_id);
    } else {
        for key in AGENT_IDENTITY_ENV_KEYS {
            builder.env_remove(key);
        }
    }

    // #215: the environment half of the config injection — today only
    // opencode's `OPENCODE_CONFIG`. After the variables above, because
    // the file it points at interpolates them.
    for (key, value) in &injection.env {
        if key == crate::harness_config::CLAUDE_PLUGIN_DIRS_VAR {
            // #318: additive — keep the user's own plugin dirs (from the
            // inherited env or their extra env) and append ours.
            let existing = builder
                .get_env(key)
                .map(|v| v.to_string_lossy().into_owned());
            let merged = crate::harness_config::merge_plugin_dirs(existing.as_deref(), value);
            builder.env(key, merged);
        } else {
            builder.env(key, value);
        }
    }

    apply_no_proxy(builder);

    AppliedEnv {
        path: merged.path,
        stripped,
        probe,
        shell,
        added: merged.added,
        dropped: merged.dropped,
        ignored_env_keys,
        login_env_keys,
    }
}

/// Make sure loopback is never proxied, whatever the user's proxy setup.
///
/// A harness talks to Skein's agent API on `127.0.0.1:<port>`, and an
/// rc-exported `HTTP_PROXY` would send that through the proxy. The user's
/// own `NO_PROXY` entries are merged, never refused, which is why neither
/// name is in `RESERVED_ENV_KEYS`. Last of all, so it sees the final
/// user/rc values. Applied on every platform: a proxied Windows machine
/// has the same loopback problem.
fn apply_no_proxy(builder: &mut CommandBuilder) {
    let get = |b: &CommandBuilder, k: &str| b.get_env(k).map(|v| v.to_string_lossy().into_owned());
    let merged = merge_no_proxy(
        get(builder, "NO_PROXY").as_deref(),
        get(builder, "no_proxy").as_deref(),
    );
    builder.env("NO_PROXY", &merged);
    // Unix env keys are case-sensitive and tools disagree on which they
    // read (curl wants lowercase). On Windows portable-pty lowercases keys,
    // so the two names are one entry and setting both would be redundant.
    if !cfg!(windows) {
        builder.env("no_proxy", &merged);
    }
}

/// The `PATH` to build on, before the probe and the user's additions.
///
/// On Unix this is simply the inherited `PATH`, which is what
/// `CommandBuilder::new` seeded from `vars_os()`.
///
/// Windows needs a union. `portable-pty` *replaces* the inherited
/// `PATH` with a live `HKLM` + `HKCU` registry read — a real win,
/// because the registry is the user's current `PATH` while the inherited
/// block can be an arbitrarily stale snapshot of Explorer's. But it is a
/// replacement, not a merge, so taking it alone loses everything a
/// launching shell added *in-process*: a Developer Command Prompt's MSVC
/// toolchain, an activated venv or conda env, `npm run`'s
/// `node_modules\.bin`. Those used to survive via the env-forward loop.
/// Registry first (it is the more current answer), inherited appended;
/// `merge_path` dedupes.
fn base_path(builder: &CommandBuilder) -> OsString {
    let from_builder = builder.get_env("PATH").map(OsString::from);
    // `cfg!` rather than `#[cfg]` so both arms type-check on every
    // platform: the Windows arm is the one that cannot be compiled on
    // the machine this is usually developed on.
    if cfg!(windows) {
        spawn_env::concat_paths(from_builder, std::env::var_os("PATH"))
    } else {
        from_builder.unwrap_or_default()
    }
}
