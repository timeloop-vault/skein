//! Tauri commands for Hermes profiles (#626/#627): list them with the
//! state of Skein's MCP entry, preview the edit, and make it with the
//! user's consent. The listing is a pure function of the Hermes root so
//! it can be tested against a temp dir.

use std::path::{Path, PathBuf};

use serde::Serialize;
use skein_harness::hermes;

use super::hooks::{self, HookInputs};
use super::yaml_hooks::{HOOK_EVENTS, HookSpec, hooks_preview};
use super::{EnsureOutcome, SKEIN_PROFILE, allowlist, ensure_skein_entry, yaml, yaml_hooks};

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SkeinEntry {
    Present,
    Absent,
    Refused,
    NoConfig,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub name: String,
    pub exists: bool,
    pub skein_entry: SkeinEntry,
    pub reason: Option<String>,
    /// Skein's status hooks could be added and approved here (#625) and are
    /// not yet, whatever `skein_entry` says. The dialog asks again.
    pub hooks_pending: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileListDto {
    pub root: Option<String>,
    pub profiles: Vec<ProfileDto>,
    pub error: Option<String>,
    /// Why the listing may not be what the user expects of this root.
    pub root_note: Option<RootNoteDto>,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RootNoteKind {
    /// The root is not a directory.
    Missing,
    /// The root exists but has no `profiles/` (normal for a fresh Hermes).
    NoProfilesDir,
    /// The root itself is named `profiles` and has no `profiles/` below it:
    /// the user probably pointed at the profiles dir, not at Hermes' home.
    LooksLikeProfilesDir,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RootNoteDto {
    pub kind: RootNoteKind,
    pub suggested_root: Option<String>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EntryPreviewDto {
    pub config_path: String,
    pub lines: Vec<String>,
    /// The preview includes Skein's MCP entry.
    pub offers_mcp: bool,
    /// The preview includes Skein's status hooks (and their approval).
    pub offers_hooks: bool,
}

fn config_path(home: &Path) -> PathBuf {
    home.join("config.yaml")
}

/// Skein's hooks for `profile_home`, when they should be offered: the
/// version gate passed, the script resolved, the hooks are absent or
/// different or the allowlist does not cover them, and the config can take
/// them. The pair is (script as installed, command spec).
fn pending_hooks(
    root: &Path,
    profile_home: &Path,
    text: &str,
    inputs: &HookInputs,
) -> Option<(PathBuf, HookSpec)> {
    if !inputs.supported {
        return None;
    }
    let installed = hooks::installed_path(inputs.script.as_deref()?, root).ok()?;
    let spec = hooks::spec_for(&installed)?;
    let done = yaml_hooks::hooks_present(text, &spec) && hooks::allowlisted(profile_home, &spec);
    (!done && yaml_hooks::plan_hooks(text, &spec, false).is_ok()).then_some((installed, spec))
}

fn profile_dto(root: &Path, name: &str, inputs: &HookInputs) -> ProfileDto {
    let home = hermes::profile_home(root, Some(name));
    let exists = home.is_dir();
    let mut hooks_pending = false;
    let (skein_entry, reason) = match std::fs::read(config_path(&home)) {
        Err(e) => (SkeinEntry::NoConfig, Some(format!("{:?}", e.kind()))),
        Ok(bytes) => match String::from_utf8(bytes) {
            Err(_) => (SkeinEntry::Refused, Some("not valid UTF-8".to_string())),
            Ok(text) if yaml::skein_entry_present(&text) => {
                hooks_pending =
                    name != SKEIN_PROFILE && pending_hooks(root, &home, &text, inputs).is_some();
                (SkeinEntry::Present, None)
            }
            Ok(text) => match yaml::insert_skein_entry(&text) {
                Ok(_) => (SkeinEntry::Absent, None),
                Err(why) => (SkeinEntry::Refused, Some(why.0)),
            },
        },
    };
    ProfileDto {
        name: name.to_string(),
        exists,
        skein_entry,
        reason,
        hooks_pending,
    }
}

/// Profiles under `root`. With no root (Hermes home unresolvable) only
/// `skein` is listed, as not existing: there is nowhere for `default` to
/// live, and `error` stays null because that is a normal state for a
/// machine without Hermes.
pub(crate) fn list_profiles_in(root: Option<&Path>, inputs: &HookInputs) -> ProfileListDto {
    let Some(root) = root else {
        return ProfileListDto {
            root: None,
            profiles: vec![ProfileDto {
                name: SKEIN_PROFILE.to_string(),
                exists: false,
                skein_entry: SkeinEntry::NoConfig,
                reason: Some("NotFound".to_string()),
                hooks_pending: false,
            }],
            error: None,
            root_note: None,
        };
    };
    let mut names = vec![SKEIN_PROFILE.to_string(), "default".to_string()];
    let mut error = None;
    let mut not_found = false;
    match std::fs::read_dir(root.join("profiles")) {
        Ok(entries) => {
            let mut found: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    tracing::info!(
                        entry = ?entry.file_name(),
                        "hermes profile list: skipped an entry: not valid UTF-8 name"
                    );
                    continue;
                };
                if !entry.path().is_dir() {
                    tracing::info!(
                        profile = name,
                        "hermes profile list: skipped an entry: not a directory"
                    );
                } else if !hermes::valid_profile_name(&name) {
                    tracing::info!(
                        profile = name,
                        "hermes profile list: skipped an entry: invalid Hermes profile name"
                    );
                } else if !names.contains(&name) {
                    found.push(name);
                }
            }
            found.sort();
            names.extend(found);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => not_found = true,
        Err(e) => error = Some(format!("cannot read profiles: {:?}", e.kind())),
    }
    let root_note = root_note_for(root, not_found);
    tracing::info!(
        root = %root.display(),
        note = ?root_note.as_ref().map(|n| n.kind),
        "hermes profile list: read root"
    );
    ProfileListDto {
        root: Some(root.to_string_lossy().into_owned()),
        profiles: names.iter().map(|n| profile_dto(root, n, inputs)).collect(),
        error,
        root_note,
    }
}

/// What to tell the user about `root`, given whether `<root>/profiles`
/// was `NotFound`. A root named `profiles` without one below it is the
/// "pointed at the profiles dir" mistake, whether or not it exists.
fn root_note_for(root: &Path, profiles_not_found: bool) -> Option<RootNoteDto> {
    let named_profiles = root
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("profiles"));
    if named_profiles && !root.join("profiles").is_dir() {
        return Some(RootNoteDto {
            kind: RootNoteKind::LooksLikeProfilesDir,
            suggested_root: root.parent().map(|p| p.to_string_lossy().into_owned()),
        });
    }
    let kind = if !root.is_dir() {
        RootNoteKind::Missing
    } else if profiles_not_found {
        RootNoteKind::NoProfilesDir
    } else {
        return None;
    };
    Some(RootNoteDto {
        kind,
        suggested_root: None,
    })
}

fn profile_config(root: &Path, profile: &str) -> Result<PathBuf, String> {
    if !hermes::valid_profile_name(profile) {
        return Err(format!("invalid Hermes profile name {profile:?}"));
    }
    Ok(config_path(&hermes::profile_home(root, Some(profile))))
}

pub(crate) fn preview_in(
    root: &Path,
    profile: &str,
    inputs: &HookInputs,
) -> Result<EntryPreviewDto, String> {
    let path = profile_config(root, profile)?;
    let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "profile config not found".to_string(),
        k => format!("cannot read profile config: {k:?}"),
    })?;
    let text = String::from_utf8(bytes).map_err(|_| "config is not valid UTF-8".to_string())?;
    let home = hermes::profile_home(root, Some(profile));
    let mcp_present = yaml::skein_entry_present(&text);
    let hooks = pending_hooks(root, &home, &text, inputs);
    if mcp_present && hooks.is_none() {
        return Err("already present".to_string());
    }
    let mut lines: Vec<String> = Vec::new();
    if !mcp_present {
        let preview = yaml::entry_preview(&text).map_err(|e| e.to_string())?;
        lines.extend(preview.lines().map(str::to_string));
    }
    if let Some((installed, spec)) = &hooks {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(hooks_preview(spec, false));
        lines.push(String::new());
        lines.push(format!(
            "Skein's {} hook commands, and only those, are then approved in {}",
            HOOK_EVENTS.len(),
            home.join(allowlist::ALLOWLIST_FILE).display()
        ));
        lines.push(format!(
            "The hook script is copied to {}",
            installed.display()
        ));
    }
    Ok(EntryPreviewDto {
        config_path: path.to_string_lossy().into_owned(),
        lines,
        offers_mcp: !mcp_present,
        offers_hooks: hooks.is_some(),
    })
}

/// The consented write: Skein's MCP entry, and (when offered) its hooks in
/// `config.yaml` without `hooks_auto_accept` plus Skein's own pairs in the
/// profile's Hermes allowlist. The allowlist goes first: a file we refuse
/// to edit stops the whole thing before the config is touched.
pub(crate) fn inject_in(
    root: &Path,
    profile: &str,
    inputs: &HookInputs,
) -> Result<&'static str, String> {
    let path = profile_config(root, profile)?;
    let home = hermes::profile_home(root, Some(profile));
    // Same lock as a spawn's profile preparation: the script, the allowlist
    // and the config are written together or not at all.
    let _guard = super::prepare::PREPARE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let text = std::fs::read(&path)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let hooks = pending_hooks(root, &home, &text, inputs);
    if let (Some((_, spec)), Some(src)) = (&hooks, inputs.script.as_deref()) {
        let installed = hooks::install_script(src, root)?;
        allowlist::record_approvals(&home, spec, &installed)?;
    }
    let outcome = ensure_skein_entry(&path)?;
    let mut changed = outcome == EnsureOutcome::Added;
    if let Some((_, spec)) = &hooks {
        hooks::write_hooks(&path, spec, false)?;
        changed = true;
    }
    Ok(if changed {
        tracing::info!(profile, "added Skein's entries to a Hermes profile");
        "added"
    } else {
        "alreadyPresent"
    })
}

/// What the version gate and the bundled script allow, for the profile
/// commands. A Hermes that cannot be found or run reads as unsupported.
fn hook_inputs(
    settings: &crate::spawn_settings::SpawnSettings,
    config: &crate::harness_config::HarnessConfig,
) -> HookInputs {
    let (resolved, path) = crate::pty::harness_program_lookup("hermes", settings);
    let supported = resolved.is_some_and(|program| {
        let mut env: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("PATH"))
            .collect();
        env.push(("PATH".to_string(), path.to_string_lossy().into_owned()));
        super::version_probe::telemetry_ok(
            &program,
            env.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        )
    });
    let script = if cfg!(windows) {
        config.hermes_hook_py()
    } else {
        config.hermes_hook_sh()
    };
    HookInputs {
        supported,
        script: script.map(Path::to_path_buf),
    }
}

fn root_or_err(settings: &crate::spawn_settings::SpawnSettings) -> Result<PathBuf, String> {
    super::effective_root(settings).ok_or_else(|| "Hermes home not found".to_string())
}

/// Hermes profiles and the state of Skein's MCP entry in each.
#[tauri::command]
pub async fn hermes_list_profiles(
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<ProfileListDto, String> {
    let settings = spawn_env.snapshot();
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = super::effective_root(&settings);
        let inputs = if root.is_some() {
            hook_inputs(&settings, &config)
        } else {
            HookInputs::default()
        };
        list_profiles_in(root.as_deref(), &inputs)
    })
    .await
    .map_err(|e| e.to_string())
}

/// What adding Skein's entry to `profile` would insert.
#[tauri::command]
pub async fn hermes_profile_entry_preview(
    profile: String,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<EntryPreviewDto, String> {
    let settings = spawn_env.snapshot();
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = root_or_err(&settings)?;
        preview_in(&root, &profile, &hook_inputs(&settings, &config))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn hermes_profile_inject(
    profile: String,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<&'static str, String> {
    let settings = spawn_env.snapshot();
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = root_or_err(&settings)?;
        inject_in(&root, &profile, &hook_inputs(&settings, &config))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
