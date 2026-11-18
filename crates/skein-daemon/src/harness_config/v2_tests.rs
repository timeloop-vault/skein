//! The opencode v2 config file (#695): which spawn gets which file, and
//! the shape of the two files actually shipped.

use super::tests::{bundle, identity};
use super::*;

fn config_value(injection: &Injection) -> &str {
    let (key, value) = injection.env.first().expect("an OPENCODE_CONFIG entry");
    assert_eq!(key, OPENCODE_CONFIG_VAR);
    value
}

fn call(
    kind: HarnessKind,
    program: &str,
    config: &HarnessConfig,
    settings: &SpawnSettings,
    shape: OpencodeConfigShape,
) -> Injection {
    injection_for(
        kind,
        program,
        Some(config),
        settings,
        Some(&identity()),
        shape,
    )
}

#[test]
fn a_v2_spawn_gets_the_v2_file_and_a_v1_spawn_the_v1_file() {
    let (_tmp, config) = bundle();
    let settings = SpawnSettings::default();
    // The program matches (the TUI) and the post-exit shell (#517).
    for program in ["opencode", "zsh"] {
        let v1 = call(
            HarnessKind::Opencode,
            program,
            &config,
            &settings,
            OpencodeConfigShape::V1,
        );
        let v2 = call(
            HarnessKind::Opencode,
            program,
            &config,
            &settings,
            OpencodeConfigShape::V2,
        );
        assert!(config_value(&v1).ends_with("opencode.json"), "{program}");
        assert!(config_value(&v2).ends_with("opencode.v2.json"), "{program}");
    }
}

#[test]
fn a_v2_spawn_without_the_v2_file_falls_back_to_the_v1_file() {
    let (tmp, _) = bundle();
    std::fs::remove_file(tmp.path().join(BUNDLE_DIR).join(OPENCODE_CONFIG_V2_FILE))
        .expect("remove v2 file");
    let config = HarnessConfig::resolve(tmp.path());
    assert!(config.opencode_config.is_some(), "v1 stays resolved");
    assert!(config.opencode_config_v2.is_none());
    let error = config.error.as_deref().expect("the miss is reported");
    assert!(error.contains("opencode.v2.json"), "{error}");
    for program in ["opencode", "zsh"] {
        let v2 = call(
            HarnessKind::Opencode,
            program,
            &config,
            &SpawnSettings::default(),
            OpencodeConfigShape::V2,
        );
        assert!(config_value(&v2).ends_with("opencode.json"), "{program}");
    }
}

#[test]
fn the_opencode_toggle_switches_off_both_shapes() {
    let (_tmp, config) = bundle();
    let off = SpawnSettings {
        inject_opencode_config: false,
        ..SpawnSettings::default()
    };
    for shape in [OpencodeConfigShape::V1, OpencodeConfigShape::V2] {
        for program in ["opencode", "zsh"] {
            assert!(call(HarnessKind::Opencode, program, &config, &off, shape).is_empty());
        }
    }
}

/// Reads the two files Skein actually ships, so a drift between them and
/// the code's expectations fails here. Each pinned key matters: v1's MCP
/// schema is strict (`additionalProperties: false`), so `codemode` cannot
/// go in its file; v2 defaults `codemode` to true, which hides the tools
/// behind a generic `execute` tool, so v2's file must say `false`, and its
/// server list lives under `mcp.servers`, not `mcp.<name>`.
#[test]
fn the_shipped_opencode_files_have_the_shape_each_version_reads() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../app/src-tauri")
        .join(BUNDLE_DIR)
        .join("opencode");
    let read = |name: &str| -> serde_json::Value {
        let path = dir.join(name);
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        serde_json::from_str(&raw).expect("valid JSON")
    };
    let v1 = read("opencode.json");
    let v2 = read("opencode.v2.json");

    let s1 = &v1["mcp"]["skein"];
    assert_eq!(s1["type"], "remote");
    assert_eq!(s1["enabled"], true);
    assert!(s1.get("codemode").is_none(), "v1's schema would reject it");
    assert!(v1["mcp"].get("servers").is_none());

    let s2 = &v2["mcp"]["servers"]["skein"];
    assert_eq!(s2["type"], "remote");
    assert_eq!(s2["disabled"], false);
    assert_eq!(s2["codemode"], false, "v2 defaults to Code Mode");
    assert!(v2["mcp"].get("skein").is_none());

    assert_eq!(s1["url"], s2["url"]);
    assert_eq!(
        s1["headers"]["Authorization"],
        s2["headers"]["Authorization"]
    );
    assert_eq!(
        s1["headers"]["X-Skein-Harness"],
        s2["headers"]["X-Skein-Harness"]
    );
    assert_eq!(s2["url"], "{env:SKEIN_REVIEW_URL}");
    assert_eq!(
        s2["headers"]["Authorization"],
        "Bearer {env:SKEIN_REVIEW_TOKEN}"
    );
    assert_eq!(s2["headers"]["X-Skein-Harness"], "{env:SKEIN_HARNESS_ID}");
}
