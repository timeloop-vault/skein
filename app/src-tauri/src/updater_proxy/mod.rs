//! Proxy for the in-app updater (#614).
//!
//! `tauri-plugin-updater` builds a fresh `reqwest::ClientBuilder` for every
//! check and every download. With no proxy configured reqwest falls back to
//! its system matcher: the proxy environment variables, then (macOS) the
//! manual system proxy. PAC scripts are never evaluated. A GUI app launched
//! from Finder or the Start menu usually does not carry the shell's
//! `HTTPS_PROXY`, so behind a corporate proxy "Check for updates" fails with
//! an error that names nothing.
//!
//! This module gives the updater an explicit proxy (Settings, persisted in
//! `updater.json` beside `skein.db`), falls back to the environment, and
//! finally to reqwest's own system matcher.
//!
//! **Plumbing.** The plugin's `configure_client` hook exists only on the
//! per-update `UpdaterBuilder`, and the JS `check()` command builds that
//! itself, so the plugin cannot be configured globally. The frontend instead
//! asks [`commands::updater_check_proxy`] and passes the answer as
//! `check({ proxy })`; the plugin carries it into the `Update` and uses it
//! for the download too. (`NO_PROXY` is therefore not honoured on that path;
//! [`apply`] honours it for the diagnose request.)
//!
//! The setting lives in a process-global because the plugin is registered in
//! `lib.rs` before `setup()` runs and so cannot be handed state; `setup()`
//! fills it with [`init`].

pub(crate) mod commands;
mod settings;

#[cfg(test)]
mod tests;

use std::sync::RwLock;

use url::Url;

/// Proxy variables in precedence order: the scheme-specific upper/lower
/// case pairs first (every endpoint we talk to is https), then the catch-all.
pub(crate) const PROXY_ENV_VARS: [&str; 6] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
];

const NO_PROXY_ENV_VARS: [&str; 2] = ["NO_PROXY", "no_proxy"];

/// The normalised setting from `updater.json`, `None` = not configured.
static SETTING: RwLock<Option<String>> = RwLock::new(None);

/// Where the proxy in force came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxySource {
    /// The Settings field.
    Setting,
    /// This environment variable.
    Env(&'static str),
    /// Nothing explicit: reqwest's own system matcher decides.
    System,
}

impl ProxySource {
    /// `"setting"`, `"env:HTTPS_PROXY"` or `"system"` — what the UI shows.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Setting => "setting".to_owned(),
            Self::Env(name) => format!("env:{name}"),
            Self::System => "system".to_owned(),
        }
    }
}

/// The outcome of [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectiveProxy {
    pub(crate) source: ProxySource,
    /// `None` only for [`ProxySource::System`].
    pub(crate) url: Option<Url>,
    pub(crate) no_proxy: Option<String>,
    /// Sources that held a value we could not use (`"setting"` or an env
    /// var name). Skipped, never fatal — recorded so the log can say why a
    /// lower-precedence source won.
    pub(crate) skipped: Vec<&'static str>,
}

/// Parse a user-typed proxy into a URL.
///
/// Blank means "no proxy" (`Ok(None)`); a bare `host:port` gets `http://`.
/// Only `http` and `https` proxies are supported, and a host is required.
pub(crate) fn normalize_proxy(raw: &str) -> Result<Option<Url>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let full = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    let url = Url::parse(&full).map_err(|e| format!("not a valid proxy address ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "unsupported proxy scheme \"{}\" (use http or https)",
            url.scheme()
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("proxy address has no host".to_owned());
    }
    Ok(Some(url))
}

fn first_non_empty(
    names: &[&'static str],
    env: &impl Fn(&str) -> Option<String>,
) -> Option<(&'static str, String)> {
    names.iter().find_map(|&name| {
        env(name)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .map(|v| (name, v))
    })
}

/// Pick the proxy: setting, then the first usable proxy variable, then the
/// system. Pure — the environment is passed in.
pub(crate) fn resolve(
    setting: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> EffectiveProxy {
    let no_proxy = first_non_empty(&NO_PROXY_ENV_VARS, &env).map(|(_, v)| v);
    let mut skipped = Vec::new();

    if let Some(raw) = setting {
        match normalize_proxy(raw) {
            Ok(Some(url)) => {
                return EffectiveProxy {
                    source: ProxySource::Setting,
                    url: Some(url),
                    no_proxy,
                    skipped,
                };
            }
            Ok(None) => {}
            Err(_) => skipped.push("setting"),
        }
    }

    for name in PROXY_ENV_VARS {
        let Some(value) = env(name) else { continue };
        match normalize_proxy(&value) {
            Ok(Some(url)) => {
                return EffectiveProxy {
                    source: ProxySource::Env(name),
                    url: Some(url),
                    no_proxy,
                    skipped,
                };
            }
            Ok(None) => {}
            Err(_) => skipped.push(name),
        }
    }

    EffectiveProxy {
        source: ProxySource::System,
        url: None,
        no_proxy,
        skipped,
    }
}

/// Install the resolved proxy on a client builder. `System` leaves the
/// builder alone so reqwest's own matcher still applies.
pub(crate) fn apply(
    builder: reqwest::ClientBuilder,
    eff: &EffectiveProxy,
) -> reqwest::ClientBuilder {
    let Some(url) = &eff.url else {
        return builder;
    };
    match reqwest::Proxy::all(url.as_str()) {
        Ok(proxy) => {
            let no_proxy = eff
                .no_proxy
                .as_deref()
                .and_then(reqwest::NoProxy::from_string);
            builder.proxy(proxy.no_proxy(no_proxy))
        }
        Err(e) => {
            // `normalize_proxy` already vetted the URL, so this is
            // unreachable in practice; fail open to the system matcher.
            tracing::warn!(error = %e, proxy = %redact(url), "updater proxy: rejected by reqwest");
            builder
        }
    }
}

/// curl-style `NO_PROXY` match: entries are comma/whitespace separated and
/// case-insensitive; `*` matches everything; `example.com` and
/// `.example.com` match that host and any subdomain; a `:port` suffix on an
/// entry is ignored; IP literals compare exactly. CIDR ranges
/// (`10.0.0.0/8`) are not supported and never match.
pub(crate) fn no_proxy_matches(list: &str, host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    list.split(|c: char| c == ',' || c.is_whitespace())
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .any(|entry| {
            if entry == "*" {
                return true;
            }
            let entry = strip_port(&entry);
            if entry.parse::<std::net::IpAddr>().is_ok() || host.parse::<std::net::IpAddr>().is_ok()
            {
                return entry == host;
            }
            let bare = entry.trim_start_matches('.');
            !bare.is_empty()
                && (host == bare || host.strip_suffix(bare).is_some_and(|p| p.ends_with('.')))
        })
}

fn strip_port(entry: &str) -> &str {
    if let Some(inner) = entry.strip_prefix('[') {
        return inner.split(']').next().unwrap_or(inner);
    }
    // A bare IPv6 literal has several colons; leave it alone.
    match entry.matches(':').count() {
        1 => entry.split(':').next().unwrap_or(entry),
        _ => entry,
    }
}

/// Whether `NO_PROXY` sends this host direct, bypassing the chosen proxy.
pub(crate) fn bypassed(eff: &EffectiveProxy, host: &str) -> bool {
    eff.url.is_some()
        && eff
            .no_proxy
            .as_deref()
            .is_some_and(|list| no_proxy_matches(list, host))
}

/// The URL for display or logs, without credentials.
pub(crate) fn redact(url: &Url) -> String {
    let mut clean = url.clone();
    let _ = clean.set_password(None);
    let _ = clean.set_username("");
    clean.to_string()
}

/// The error's own message followed by each `source()` down the chain.
/// reqwest's top-level text ("error sending request") says nothing; the
/// cause that does (DNS, proxy refused, TLS) is a level or two down.
pub(crate) fn error_chain(err: &dyn std::error::Error) -> Vec<String> {
    let mut out = vec![err.to_string()];
    let mut cur = err.source();
    while let Some(e) = cur {
        out.push(e.to_string());
        cur = e.source();
    }
    out
}

/// Resolve against the real environment and the current setting.
pub(crate) fn current() -> EffectiveProxy {
    let setting = setting();
    resolve(setting.as_deref(), |k| std::env::var(k).ok())
}

pub(crate) fn setting() -> Option<String> {
    SETTING
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(crate) fn set_setting(value: Option<String>) {
    *SETTING
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}

/// Load `updater.json` into the global. Called from `setup()`.
pub(crate) fn init(data_dir: &std::path::Path) {
    let (loaded, warning) = settings::load(data_dir);
    set_setting(loaded.proxy);
    set_load_warning(warning);
}

/// Why `updater.json` was ignored at boot, until a save replaces it.
static LOAD_WARNING: RwLock<Option<String>> = RwLock::new(None);

/// Held across "write file, then update the global" so two saves cannot
/// leave the file and the global disagreeing.
pub(crate) static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn load_warning() -> Option<String> {
    LOAD_WARNING
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(crate) fn set_load_warning(value: Option<String>) {
    *LOAD_WARNING
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}
