// #625: Skein's hooks ride the same consent as the MCP entry.

use super::*;
use crate::hermes_profile::hooks;
use crate::hermes_profile::yaml_hooks::HOOK_EVENTS;

const ALLOWLIST: &str = "shell-hooks-allowlist.json";

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn off() -> HookInputs {
    HookInputs::default()
}

fn find<'a>(l: &'a ProfileListDto, name: &str) -> &'a ProfileDto {
    l.profiles.iter().find(|p| p.name == name).unwrap()
}

struct Hooked {
    dir: tempfile::TempDir,
    inputs: HookInputs,
}

impl Hooked {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("res/skein-hook.sh");
        write(dir.path(), "res/skein-hook.sh", "#!/bin/sh\n");
        write(dir.path(), "root/profiles/work/config.yaml", "model: x\n");
        Self {
            inputs: HookInputs {
                supported: true,
                script: Some(script),
            },
            dir,
        }
    }
    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }
    fn home(&self) -> PathBuf {
        self.root().join("profiles/work")
    }
    fn config(&self) -> String {
        std::fs::read_to_string(self.home().join("config.yaml")).unwrap()
    }
    fn approvals(&self) -> Vec<serde_json::Value> {
        let text = std::fs::read_to_string(self.home().join(ALLOWLIST)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["approvals"].as_array().unwrap().clone()
    }
}

#[test]
fn preview_shows_mcp_and_hooks_for_a_bare_profile() {
    let h = Hooked::new();
    let p = preview_in(&h.root(), "work", &h.inputs).unwrap();
    let text = p.lines.join("\n");
    assert!(text.contains("skein-hooks:begin"), "{text}");
    assert!(text.contains("on_session_start"), "{text}");
    assert!(!text.contains("hooks_auto_accept"), "{text}");
    assert!(text.contains("mcp_servers"), "{text}");
    let n = p.lines.len();
    let approval = &p.lines[n - 2];
    assert!(
        approval.contains("8 hook commands, and only those"),
        "{approval}"
    );
    assert!(approval.contains(ALLOWLIST), "{approval}");
    assert!(
        p.lines[n - 1].contains("skein-hook.sh"),
        "{:?}",
        p.lines[n - 1]
    );
    assert!(p.offers_mcp && p.offers_hooks);
}

#[test]
fn preview_for_an_mcp_only_profile_offers_just_the_hooks() {
    let h = Hooked::new();
    inject_in(&h.root(), "work", &off()).unwrap();
    let listed = list_profiles_in(Some(&h.root()), &h.inputs);
    let work = find(&listed, "work");
    assert_eq!(work.skein_entry, SkeinEntry::Present);
    assert!(work.hooks_pending);
    let text = preview_in(&h.root(), "work", &h.inputs)
        .unwrap()
        .lines
        .join("\n");
    assert!(text.contains("skein-hooks:begin"), "{text}");
    assert!(!text.contains("mcp_servers"), "{text}");
    // Without hooks available the profile is done, as before #625.
    assert_eq!(
        preview_in(&h.root(), "work", &off()).unwrap_err(),
        "already present"
    );
}

#[test]
fn unsupported_version_or_missing_script_offers_mcp_only() {
    let h = Hooked::new();
    for inputs in [
        HookInputs {
            supported: false,
            ..h.inputs.clone()
        },
        HookInputs {
            supported: true,
            script: None,
        },
    ] {
        let text = preview_in(&h.root(), "work", &inputs)
            .unwrap()
            .lines
            .join("\n");
        assert!(text.contains("mcp_servers"), "{text}");
        assert!(!text.contains("hooks"), "{text}");
    }
}

#[test]
fn inject_writes_hooks_without_auto_accept_and_records_exactly_our_pairs() {
    let h = Hooked::new();
    assert_eq!(inject_in(&h.root(), "work", &h.inputs).unwrap(), "added");
    let cfg = h.config();
    assert!(cfg.contains("skein-hooks:begin") && cfg.contains("mcp_servers"));
    assert!(!cfg.contains("hooks_auto_accept"), "{cfg}");
    let src = h.inputs.script.as_deref().unwrap();
    let installed = hooks::installed_path(src, &h.root()).unwrap();
    assert!(installed.is_file());
    let spec = hooks::spec_for(&installed).unwrap();
    let approvals = h.approvals();
    assert_eq!(approvals.len(), 8);
    for ev in HOOK_EVENTS {
        let entry = approvals
            .iter()
            .find(|a| a["event"] == ev)
            .unwrap_or_else(|| panic!("{ev} missing"));
        assert_eq!(entry["command"], spec.command_for(ev));
        assert!(entry["approved_at"].as_str().unwrap().ends_with('Z'));
        assert!(entry["script_mtime_at_approval"].as_str().is_some());
    }
    assert!(hooks::allowlisted(&h.home(), &spec));
    // Done now: no dialog, and a second inject changes nothing.
    assert_eq!(
        preview_in(&h.root(), "work", &h.inputs).unwrap_err(),
        "already present"
    );
    assert!(!find(&list_profiles_in(Some(&h.root()), &h.inputs), "work").hooks_pending);
    let before = (h.config(), std::fs::read(h.home().join(ALLOWLIST)).unwrap());
    assert_eq!(
        inject_in(&h.root(), "work", &h.inputs).unwrap(),
        "alreadyPresent"
    );
    assert_eq!(h.config(), before.0);
    assert_eq!(std::fs::read(h.home().join(ALLOWLIST)).unwrap(), before.1);
}

#[test]
fn inject_keeps_existing_allowlist_entries_and_unknown_keys() {
    let h = Hooked::new();
    write(
        &h.root(),
        "profiles/work/shell-hooks-allowlist.json",
        r#"{"approvals":[{"event":"pre_tool_call","command":"mine.sh","extra":[1,{"a":null}]}],"note":"keep"}"#,
    );
    inject_in(&h.root(), "work", &h.inputs).unwrap();
    let approvals = h.approvals();
    assert_eq!(approvals.len(), 9);
    let mine = approvals
        .iter()
        .find(|a| a["command"] == "mine.sh")
        .unwrap();
    assert_eq!(mine["extra"], serde_json::json!([1, {"a": null}]));
    let raw = std::fs::read_to_string(h.home().join(ALLOWLIST)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["note"], "keep");
}

#[test]
fn inject_refuses_a_corrupt_allowlist_and_touches_nothing() {
    let h = Hooked::new();
    for bad in ["{not json", "[1,2]", r#"{"approvals":{"a":1}}"#] {
        write(&h.root(), "profiles/work/shell-hooks-allowlist.json", bad);
        let err = inject_in(&h.root(), "work", &h.inputs).unwrap_err();
        assert!(err.contains("not editing it"), "{err}");
        assert_eq!(
            std::fs::read_to_string(h.home().join(ALLOWLIST)).unwrap(),
            bad
        );
        assert_eq!(h.config(), "model: x\n");
    }
}
