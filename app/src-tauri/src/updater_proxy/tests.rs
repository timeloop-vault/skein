use std::collections::HashMap;
use std::fmt;

use super::*;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |k| map.get(k).cloned()
}

fn norm(raw: &str) -> String {
    normalize_proxy(raw).unwrap().unwrap().to_string()
}

#[test]
fn bare_host_port_gets_http() {
    assert_eq!(norm("127.0.0.1:3128"), "http://127.0.0.1:3128/");
    assert_eq!(norm("  proxy.corp:8080  "), "http://proxy.corp:8080/");
}

#[test]
fn explicit_schemes_and_trailing_slash() {
    assert_eq!(norm("http://p.corp:80"), "http://p.corp/");
    assert_eq!(norm("https://p.corp:8443"), "https://p.corp:8443/");
    assert_eq!(norm("http://p.corp:3128/"), "http://p.corp:3128/");
}

#[test]
fn port_is_optional() {
    assert_eq!(norm("p.corp"), "http://p.corp/");
}

#[test]
fn userinfo_is_kept() {
    assert_eq!(norm("user:pw@p.corp:3128"), "http://user:pw@p.corp:3128/");
}

#[test]
fn blank_is_none() {
    assert_eq!(normalize_proxy(""), Ok(None));
    assert_eq!(normalize_proxy("  \t "), Ok(None));
}

#[test]
fn unsupported_scheme_is_named() {
    let err = normalize_proxy("socks5://p.corp:1080").unwrap_err();
    assert!(err.contains("socks5"), "{err}");
}

#[test]
fn garbage_and_hostless_are_errors() {
    assert!(normalize_proxy("not a url").is_err());
    assert!(normalize_proxy("http://").is_err());
    assert!(normalize_proxy("http://:3128").is_err());
}

#[test]
fn setting_beats_env() {
    let eff = resolve(
        Some("a.corp:1"),
        env_of(&[("HTTPS_PROXY", "http://b.corp:2")]),
    );
    assert_eq!(eff.source, ProxySource::Setting);
    assert_eq!(eff.url.unwrap().as_str(), "http://a.corp:1/");
}

#[test]
fn env_precedence_order() {
    let all = [
        ("all_proxy", "h6:6"),
        ("ALL_PROXY", "h5:5"),
        ("http_proxy", "h4:4"),
        ("HTTP_PROXY", "h3:3"),
        ("https_proxy", "h2:2"),
        ("HTTPS_PROXY", "h1:1"),
    ];
    for (i, expect) in PROXY_ENV_VARS.iter().enumerate() {
        // Keep only the variables at or below this rank.
        let subset: Vec<_> = all
            .iter()
            .copied()
            .filter(|(k, _)| PROXY_ENV_VARS.iter().position(|n| n == k).unwrap() >= i)
            .collect();
        let eff = resolve(None, env_of(&subset));
        assert_eq!(eff.source, ProxySource::Env(expect));
    }
}

#[test]
fn empty_values_are_skipped() {
    let eff = resolve(
        Some("  "),
        env_of(&[
            ("HTTPS_PROXY", ""),
            ("https_proxy", "  "),
            ("ALL_PROXY", "h:9"),
        ]),
    );
    assert_eq!(eff.source, ProxySource::Env("ALL_PROXY"));
    assert_eq!(eff.skipped, Vec::<&str>::new());
}

#[test]
fn invalid_env_value_is_skipped_and_recorded() {
    let eff = resolve(
        None,
        env_of(&[("HTTPS_PROXY", "socks5://x:1"), ("http_proxy", "h:8")]),
    );
    assert_eq!(eff.source, ProxySource::Env("http_proxy"));
    assert_eq!(eff.skipped, vec!["HTTPS_PROXY"]);
}

#[test]
fn invalid_setting_falls_through() {
    let eff = resolve(Some("socks5://x:1"), env_of(&[("HTTP_PROXY", "h:8")]));
    assert_eq!(eff.source, ProxySource::Env("HTTP_PROXY"));
    assert_eq!(eff.skipped, vec!["setting"]);
}

#[test]
fn nothing_is_system() {
    let eff = resolve(None, env_of(&[]));
    assert_eq!(eff.source, ProxySource::System);
    assert!(eff.url.is_none());
    assert_eq!(eff.source.label(), "system");
}

#[test]
fn no_proxy_precedence() {
    let eff = resolve(None, env_of(&[("NO_PROXY", "a"), ("no_proxy", "b")]));
    assert_eq!(eff.no_proxy.as_deref(), Some("a"));
    let eff = resolve(None, env_of(&[("NO_PROXY", " "), ("no_proxy", "b")]));
    assert_eq!(eff.no_proxy.as_deref(), Some("b"));
    assert_eq!(resolve(None, env_of(&[])).no_proxy, None);
}

#[test]
fn labels() {
    assert_eq!(ProxySource::Setting.label(), "setting");
    assert_eq!(ProxySource::Env("HTTPS_PROXY").label(), "env:HTTPS_PROXY");
}

#[test]
fn redact_strips_credentials() {
    let url = Url::parse("http://user:secret@p.corp:3128").unwrap();
    let shown = redact(&url);
    assert_eq!(shown, "http://p.corp:3128/");
    assert!(!shown.contains("secret") && !shown.contains("user"));
}

#[test]
fn apply_system_leaves_builder_usable() {
    crate::install_rustls_provider();
    let eff = resolve(None, env_of(&[]));
    assert!(apply(reqwest::Client::builder(), &eff).build().is_ok());
    let eff = resolve(
        Some("u:p@p.corp:3128"),
        env_of(&[("NO_PROXY", "localhost")]),
    );
    assert!(apply(reqwest::Client::builder(), &eff).build().is_ok());
}

#[derive(Debug)]
struct Layer(&'static str, Option<Box<Layer>>);

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Layer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.1.as_deref().map(|e| e as _)
    }
}

#[test]
fn error_chain_walks_sources() {
    let err = Layer(
        "error sending request",
        Some(Box::new(Layer(
            "connect",
            Some(Box::new(Layer("refused", None))),
        ))),
    );
    assert_eq!(
        error_chain(&err),
        vec!["error sending request", "connect", "refused"]
    );
    assert_eq!(error_chain(&Layer("solo", None)), vec!["solo"]);
}

#[test]
fn settings_round_trip_and_defaults() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        settings::load(dir.path()).0,
        settings::UpdaterSettings::default()
    );

    let s = settings::UpdaterSettings {
        proxy: Some("http://p.corp:3128/".to_owned()),
    };
    settings::save(dir.path(), &s).unwrap();
    let raw = std::fs::read_to_string(settings::file_path(dir.path())).unwrap();
    assert!(raw.contains("\"proxy\""));
    assert_eq!(settings::load(dir.path()).0, s);

    // Unknown/missing fields and garbage both degrade to defaults.
    std::fs::write(settings::file_path(dir.path()), "{}").unwrap();
    assert_eq!(settings::load(dir.path()).0.proxy, None);
    std::fs::write(settings::file_path(dir.path()), "{nope").unwrap();
    assert_eq!(settings::load(dir.path()).0.proxy, None);
}

#[test]
fn no_proxy_matcher_semantics() {
    assert!(no_proxy_matches("*", "github.com"));
    assert!(no_proxy_matches("github.com", "github.com"));
    assert!(no_proxy_matches("GitHub.com", "api.github.com"));
    assert!(no_proxy_matches(".github.com", "github.com"));
    assert!(no_proxy_matches(
        " a.org , .github.com:443	b.org",
        "x.github.com"
    ));
    assert!(!no_proxy_matches("hub.com", "github.com"));
    assert!(!no_proxy_matches("api.github.com", "github.com"));
    assert!(!no_proxy_matches("", "github.com"));
    assert!(no_proxy_matches("10.0.0.1", "10.0.0.1"));
    assert!(!no_proxy_matches("0.0.1", "10.0.0.1"));
    assert!(no_proxy_matches("::1", "::1"));
}

#[test]
fn check_and_diagnose_agree_on_bypass() {
    let host = "github.com";
    let hit = resolve(
        Some("p.corp:3128"),
        env_of(&[("NO_PROXY", "localhost,.github.com")]),
    );
    assert!(bypassed(&hit, host));
    assert_eq!(commands::check_proxy(&hit, host), None);
    let miss = resolve(Some("p.corp:3128"), env_of(&[("NO_PROXY", "internal")]));
    assert!(!bypassed(&miss, host));
    assert_eq!(
        commands::check_proxy(&miss, host).as_deref(),
        Some("http://p.corp:3128/")
    );
    // System has no proxy of ours to bypass.
    assert!(!bypassed(
        &resolve(None, env_of(&[("NO_PROXY", "*")])),
        host
    ));
}
