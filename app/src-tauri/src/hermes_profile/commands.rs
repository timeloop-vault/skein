//! Tauri commands for Hermes profiles (#626/#627): list them with the
//! state of Skein's MCP entry, preview the edit, and make it with the
//! user's consent. The listing is a pure function of the Hermes root so
//! it can be tested against a temp dir.

use std::path::{Path, PathBuf};

use serde::Serialize;
use skein_harness::hermes;

use super::{EnsureOutcome, SKEIN_PROFILE, ensure_skein_entry, yaml};

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
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileListDto {
    pub root: Option<String>,
    pub profiles: Vec<ProfileDto>,
    pub error: Option<String>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EntryPreviewDto {
    pub config_path: String,
    pub lines: Vec<String>,
}

fn config_path(home: &Path) -> PathBuf {
    home.join("config.yaml")
}

fn profile_dto(root: &Path, name: &str) -> ProfileDto {
    let home = hermes::profile_home(root, Some(name));
    let exists = home.is_dir();
    let (skein_entry, reason) = match std::fs::read(config_path(&home)) {
        Err(e) => (SkeinEntry::NoConfig, Some(format!("{:?}", e.kind()))),
        Ok(bytes) => match String::from_utf8(bytes) {
            Err(_) => (SkeinEntry::Refused, Some("not valid UTF-8".to_string())),
            Ok(text) if yaml::skein_entry_present(&text) => (SkeinEntry::Present, None),
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
    }
}

/// Profiles under `root`. With no root (Hermes home unresolvable) only
/// `skein` is listed, as not existing: there is nowhere for `default` to
/// live, and `error` stays null because that is a normal state for a
/// machine without Hermes.
pub(crate) fn list_profiles_in(root: Option<&Path>) -> ProfileListDto {
    let Some(root) = root else {
        return ProfileListDto {
            root: None,
            profiles: vec![ProfileDto {
                name: SKEIN_PROFILE.to_string(),
                exists: false,
                skein_entry: SkeinEntry::NoConfig,
                reason: Some("NotFound".to_string()),
            }],
            error: None,
        };
    };
    let mut names = vec![SKEIN_PROFILE.to_string(), "default".to_string()];
    let mut error = None;
    match std::fs::read_dir(root.join("profiles")) {
        Ok(entries) => {
            let mut found: Vec<String> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| hermes::valid_profile_name(n) && n.as_str() != "default")
                .filter(|n| !names.contains(n))
                .collect();
            found.sort();
            names.extend(found);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => error = Some(format!("cannot read profiles: {:?}", e.kind())),
    }
    ProfileListDto {
        root: Some(root.to_string_lossy().into_owned()),
        profiles: names.iter().map(|n| profile_dto(root, n)).collect(),
        error,
    }
}

fn profile_config(root: &Path, profile: &str) -> Result<PathBuf, String> {
    if !hermes::valid_profile_name(profile) {
        return Err(format!("invalid Hermes profile name {profile:?}"));
    }
    Ok(config_path(&hermes::profile_home(root, Some(profile))))
}

pub(crate) fn preview_in(root: &Path, profile: &str) -> Result<EntryPreviewDto, String> {
    let path = profile_config(root, profile)?;
    let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "profile config not found".to_string(),
        k => format!("cannot read profile config: {k:?}"),
    })?;
    let text = String::from_utf8(bytes).map_err(|_| "config is not valid UTF-8".to_string())?;
    if yaml::skein_entry_present(&text) {
        return Err("already present".to_string());
    }
    let preview = yaml::entry_preview(&text).map_err(|e| e.to_string())?;
    Ok(EntryPreviewDto {
        config_path: path.to_string_lossy().into_owned(),
        lines: preview.lines().map(str::to_string).collect(),
    })
}

pub(crate) fn inject_in(root: &Path, profile: &str) -> Result<&'static str, String> {
    let path = profile_config(root, profile)?;
    let outcome = ensure_skein_entry(&path)?;
    Ok(match outcome {
        EnsureOutcome::Added => {
            tracing::info!(profile, "added Skein's MCP entry to a Hermes profile");
            "added"
        }
        EnsureOutcome::AlreadyPresent => "alreadyPresent",
    })
}

fn root_or_err(settings: &crate::spawn_settings::SpawnSettings) -> Result<PathBuf, String> {
    super::effective_root(settings).ok_or_else(|| "Hermes home not found".to_string())
}

/// Hermes profiles and the state of Skein's MCP entry in each.
#[tauri::command]
pub async fn hermes_list_profiles(
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
) -> Result<ProfileListDto, String> {
    let settings = spawn_env.snapshot();
    tauri::async_runtime::spawn_blocking(move || {
        list_profiles_in(super::effective_root(&settings).as_deref())
    })
    .await
    .map_err(|e| e.to_string())
}

/// What adding Skein's entry to `profile` would insert.
#[tauri::command]
pub async fn hermes_profile_entry_preview(
    profile: String,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
) -> Result<EntryPreviewDto, String> {
    let settings = spawn_env.snapshot();
    tauri::async_runtime::spawn_blocking(move || preview_in(&root_or_err(&settings)?, &profile))
        .await
        .map_err(|e| e.to_string())?
}

/// The consented write: add Skein's entry to `profile`'s `config.yaml`.
#[tauri::command]
pub async fn hermes_profile_inject(
    profile: String,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
) -> Result<&'static str, String> {
    let settings = spawn_env.snapshot();
    tauri::async_runtime::spawn_blocking(move || inject_in(&root_or_err(&settings)?, &profile))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
