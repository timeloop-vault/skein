//! The Tauri boundary for the updater proxy (#614) and nothing else.

use std::time::Duration;

use serde::Serialize;
use tauri::Manager;

use super::{
    EffectiveProxy, apply, bypassed, current, error_chain, normalize_proxy, redact, settings,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EffectiveProxyDto {
    /// `"setting"`, `"env:<VAR>"` or `"system"`.
    pub source: String,
    /// Credentials removed.
    pub url: Option<String>,
    pub no_proxy: Option<String>,
    /// `NO_PROXY` matches the update endpoint's host, so the request goes
    /// direct (to the system matcher) instead of through `url`.
    pub bypassed_by_no_proxy: bool,
    /// The endpoint host the `NO_PROXY` decision used.
    pub host: Option<String>,
    /// Sources ignored as invalid: `"setting"` or an env var name.
    pub skipped: Vec<String>,
}

impl EffectiveProxyDto {
    fn new(eff: &EffectiveProxy, host: &str) -> Self {
        Self {
            source: eff.source.label(),
            url: eff.url.as_ref().map(redact),
            no_proxy: eff.no_proxy.clone(),
            bypassed_by_no_proxy: bypassed(eff, host),
            host: (!host.is_empty()).then(|| host.to_owned()),
            skipped: eff.skipped.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

/// Host of the first updater endpoint ("" if unparseable).
fn endpoint_host(endpoint: &str) -> String {
    url::Url::parse(endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn host_of_config(app: &tauri::AppHandle) -> String {
    first_endpoint(app)
        .map(|e| endpoint_host(&e))
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdaterProxyDto {
    /// The stored setting, normalised, with credentials stripped.
    pub setting: Option<String>,
    pub effective: EffectiveProxyDto,
    /// `updater.json` existed but could not be read or parsed (the
    /// message only, never file contents). Cleared by a successful save.
    pub load_warning: Option<String>,
}

fn snapshot(host: &str) -> UpdaterProxyDto {
    UpdaterProxyDto {
        setting: super::setting().map(|s| url::Url::parse(&s).map_or(s, |u| redact(&u))),
        load_warning: super::load_warning(),
        effective: EffectiveProxyDto::new(&current(), host),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri extracts the handle by value
pub(crate) fn updater_proxy_get(app: tauri::AppHandle) -> UpdaterProxyDto {
    snapshot(&host_of_config(&app))
}

/// Validate, persist and activate the proxy. Invalid input saves nothing.
/// `None` or blank clears it.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri extracts the handle by value
pub(crate) fn updater_proxy_set(
    app: tauri::AppHandle,
    proxy: Option<String>,
) -> Result<UpdaterProxyDto, String> {
    let normalized = match proxy {
        Some(raw) => normalize_proxy(&raw)?.map(String::from),
        None => None,
    };
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let guard = super::SAVE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    settings::save(
        &dir,
        &settings::UpdaterSettings {
            proxy: normalized.clone(),
        },
    )?;
    super::set_setting(normalized);
    super::set_load_warning(None);
    drop(guard);
    let dto = snapshot(&host_of_config(&app));
    tracing::info!(
        source = %dto.effective.source,
        proxy = dto.effective.url.as_deref().unwrap_or("-"),
        "updater proxy: setting saved"
    );
    Ok(dto)
}

/// The decision shared by the real check and diagnose: no proxy when
/// `NO_PROXY` matches the endpoint host.
pub(super) fn check_proxy(eff: &EffectiveProxy, host: &str) -> Option<String> {
    if bypassed(eff, host) {
        return None;
    }
    eff.url.as_ref().map(ToString::to_string)
}

/// The proxy URL to hand to the plugin's `check({ proxy })`, credentials
/// included (the plugin needs them). `None` = leave it to reqwest's system
/// matcher.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri extracts the handle by value
pub(crate) fn updater_check_proxy(app: tauri::AppHandle) -> Option<String> {
    check_proxy(&current(), &host_of_config(&app))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdaterDiagnosisDto {
    pub url: String,
    pub effective: EffectiveProxyDto,
    pub ok: bool,
    pub status: Option<u16>,
    /// Display of the error and each `source()`, outermost first.
    pub chain: Vec<String>,
}

fn first_endpoint(app: &tauri::AppHandle) -> Result<String, String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "no updater endpoint in the app config".to_owned())
}

/// GET the first updater endpoint the way the plugin would (same proxy
/// resolution) and report what happened, including the whole error chain.
#[tauri::command]
pub(crate) async fn updater_diagnose(app: tauri::AppHandle) -> Result<UpdaterDiagnosisDto, String> {
    let url = first_endpoint(&app)?;
    let eff = current();
    let effective = EffectiveProxyDto::new(&eff, &endpoint_host(&url));
    crate::install_rustls_provider();

    let builder = reqwest::Client::builder()
        .user_agent(concat!(
            "skein-updater-diagnose/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(Duration::from_secs(20));
    let builder = if bypassed(&eff, &endpoint_host(&url)) {
        builder
    } else {
        apply(builder, &eff)
    };
    let result = match builder.build() {
        Ok(client) => client.get(&url).send().await.map_err(|e| error_chain(&e)),
        Err(e) => Err(error_chain(&e)),
    };

    let (ok, status, chain) = match result {
        Ok(resp) => {
            let status = resp.status();
            tracing::info!(
                %url, status = status.as_u16(),
                source = %effective.source,
                proxy = effective.url.as_deref().unwrap_or("-"),
                "updater diagnose"
            );
            let chain = if status.is_success() {
                Vec::new()
            } else {
                vec![format!("endpoint returned HTTP {}", status.as_u16())]
            };
            (status.is_success(), Some(status.as_u16()), chain)
        }
        Err(chain) => {
            tracing::warn!(
                %url,
                source = %effective.source,
                proxy = effective.url.as_deref().unwrap_or("-"),
                chain = %chain.join(" → "),
                "updater diagnose failed"
            );
            (false, None, chain)
        }
    };
    Ok(UpdaterDiagnosisDto {
        url,
        effective,
        ok,
        status,
        chain,
    })
}
