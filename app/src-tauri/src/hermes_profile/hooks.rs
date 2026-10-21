//! Skein's Hermes shell hooks for harness status (#625): installing the
//! hook script at a stable path, writing the hooks into the profile Skein
//! owns, and deciding whether a spawn actually has working hook telemetry.
//!
//! Hermes' hook allowlist is keyed on the exact `(event, command)` string,
//! and the bundled resource directory differs per Skein build profile and
//! moves on update. So the script is copied to `<hermes root>/skein-hooks/`
//! and the command names that copy: it only changes when the script's
//! location convention does, not on every Skein update.
//!
//! Hooks are written to the `skein` profile only. A user's own profile is
//! never edited here (consent is a separate step); for one we only report.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::allowlist::ALLOWLIST_FILE;
use super::yaml_hooks::{
    HOOK_EVENTS, HookSpec, auto_accept_blocks, auto_accept_value, hooks_present, plan_hooks,
    quote_arg,
};
use super::{read_existing, write_config_atomic};

/// Directory under the Hermes root that holds the installed script.
pub(crate) const SCRIPT_DIR: &str = "skein-hooks";
/// A hook is cut off after this long; status updates are fire-and-forget.
pub(crate) const TIMEOUT_SECS: u32 = 5;

/// What the spawn path knows about hooks before touching any profile.
#[derive(Debug, Clone, Default)]
pub(crate) struct HookInputs {
    /// The Hermes version is one the payloads were verified against.
    pub supported: bool,
    /// The bundled script for this platform, when it resolved.
    pub script: Option<PathBuf>,
}

/// Where [`install_script`] puts `src`, without touching the disk.
pub(crate) fn installed_path(src: &Path, root: &Path) -> Result<PathBuf, String> {
    let name = src
        .file_name()
        .ok_or_else(|| format!("{} has no file name", src.display()))?;
    Ok(root.join(SCRIPT_DIR).join(name))
}

/// Copy `src` to `<root>/skein-hooks/<name>`, rewriting only when the
/// bytes differ (atomic: temp file + rename). Returns the installed path.
pub(crate) fn install_script(src: &Path, root: &Path) -> Result<PathBuf, String> {
    let dest = installed_path(src, root)?;
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let dir = root.join(SCRIPT_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let current = read_existing(&dest)?;
    if current.as_deref() != Some(bytes.as_slice())
        && let Err(e) = write_config_atomic(&dest, current.as_deref(), &bytes)
        // The rename can fail while Python has the script open (Windows),
        // or lose a race with another Skein build profile that just wrote
        // the same bytes: the destination is then already what we want.
        && read_existing(&dest).ok().flatten().as_deref() != Some(bytes.as_slice())
    {
        return Err(e);
    }
    Ok(dest)
}

/// The command prefix for the installed script: a `.py` runs by itself
/// (Windows), anything else through `sh`. `None` when the path cannot be
/// quoted for Hermes' command splitter.
pub(crate) fn spec_for(script: &Path) -> Option<HookSpec> {
    let quoted = quote_arg(&script.to_string_lossy())?;
    let prefix = if script.extension().is_some_and(|e| e == "py") {
        quoted
    } else {
        format!("sh {quoted}")
    };
    Some(HookSpec {
        prefix,
        timeout_secs: TIMEOUT_SECS,
    })
}

/// Whether every hook event of `spec` is approved in the profile's
/// allowlist. A missing or corrupt file reads as `false`.
pub(crate) fn allowlisted(profile_home: &Path, spec: &HookSpec) -> bool {
    let Ok(bytes) = std::fs::read(profile_home.join(ALLOWLIST_FILE)) else {
        return false;
    };
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let Ok(serde_json::Value::Object(map)) = serde_json::from_slice(bytes) else {
        return false;
    };
    let Some(serde_json::Value::Array(approvals)) = map.get("approvals") else {
        return false;
    };
    HOOK_EVENTS.iter().all(|ev| {
        let want = spec.command_for(ev);
        approvals.iter().any(|a| {
            a.get("event").and_then(|v| v.as_str()) == Some(ev)
                && a.get("command").and_then(|v| v.as_str()) == Some(want.as_str())
        })
    })
}

/// Whether the profile at `profile_home` has working hook telemetry:
/// our hooks are in its config and Hermes will run them without asking.
fn status(config_text: &str, profile_home: &Path, spec: &HookSpec) -> bool {
    hooks_present(config_text, spec)
        && (auto_accept_value(config_text) == Some(true) || allowlisted(profile_home, spec))
}

/// Install the script, write the hooks when `write` (the Skein-owned
/// profile with its switch on), and report whether the profile has
/// working telemetry. Failures are logged, never returned: they cost the
/// harness its status hooks, not its spawn.
pub(crate) fn prepare(
    root: &Path,
    profile_home: &Path,
    config: &Path,
    write: bool,
    inputs: &HookInputs,
) -> bool {
    if !inputs.supported {
        return false;
    }
    let Some(src) = inputs.script.as_deref() else {
        return false;
    };
    // The script is only installed when something needs it: the hooks are
    // about to be written (the Skein-owned profile), or the profile already
    // carries Skein's hooks (a consented user profile, refreshed in place).
    // Never for a user profile that has not consented.
    let Ok(dest) = installed_path(src, root) else {
        return false;
    };
    let Some(spec) = spec_for(&dest) else {
        tracing::warn!(path = %dest.display(), "hermes hooks: the script path cannot be quoted");
        return false;
    };
    let text = std::fs::read_to_string(config).ok();
    if write && text.as_deref().is_some_and(auto_accept_blocks) {
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "hermes hooks: the skein profile sets hooks_auto_accept to something other than true; Hermes would ask on every start, so the hooks are not written"
            );
        }
        return false;
    }
    let carries = text.as_deref().is_some_and(|t| hooks_present(t, &spec));
    if !write && !carries {
        return false;
    }
    if let Err(e) = install_script(src, root) {
        tracing::warn!(error = %e, "hermes hooks: installing the hook script failed");
        return false;
    }
    if write && let Err(e) = write_hooks(config, &spec, true) {
        tracing::warn!(error = %e, "hermes hooks: writing the hooks failed");
    }
    std::fs::read_to_string(config).is_ok_and(|t| status(&t, profile_home, &spec))
}

/// Write Skein's hooks into `config`; `auto_accept` adds
/// `hooks_auto_accept: true` (only ever for the profile Skein owns).
pub(crate) fn write_hooks(config: &Path, spec: &HookSpec, auto_accept: bool) -> Result<(), String> {
    let old = read_existing(config)?;
    let text = match &old {
        Some(b) => String::from_utf8(b.clone())
            .map_err(|_| format!("{} is not valid UTF-8; not editing it", config.display()))?,
        None => String::new(),
    };
    if auto_accept && auto_accept_blocks(&text) {
        return Err(format!(
            "{} sets hooks_auto_accept to something other than true; not writing hooks",
            config.display()
        ));
    }
    if let Some(new) = plan_hooks(&text, spec, auto_accept).map_err(|e| e.to_string())? {
        write_config_atomic(config, old.as_deref(), new.as_bytes())?;
        tracing::info!("hermes hooks: wrote Skein's hooks to a profile");
    }
    Ok(())
}
