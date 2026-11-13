//! Shared fixtures for the `harness_argv` tests (the vitest helpers
//! `harness`, `room`, `ports`, `SID`, `SHELL`).

use std::collections::HashMap;

use skein_proto::rooms::{Harness, Room};

pub const SID: &str = "3c8c4693-3838-46ab-8680-3e60df6fdefe";

pub fn shell() -> Vec<String> {
    sv(&["/bin/zsh", "-l"])
}

pub fn sv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// A bare harness of `kind` (id `h1`), nothing else set.
pub fn harness(kind: &str) -> Harness {
    Harness {
        id: "h1".to_owned(),
        kind: kind.to_owned(),
        name: "H".to_owned(),
        status: "idle".to_owned(),
        model: String::new(),
        tokens: String::new(),
        live: None,
        cmd: None,
        cwd: None,
        session_id: None,
        agent: None,
        design_entry: None,
        design_device: None,
        pending_notifications: None,
        created_by: None,
        shell_claim: None,
        remote: None,
        env_profile: None,
        hermes_profile: None,
        opencode_version: None,
        launch: None,
    }
}

/// A bare harness with its stored argv and session id.
pub fn with(kind: &str, cmd: &[&str], sid: Option<&str>) -> Harness {
    Harness {
        cmd: Some(sv(cmd)),
        session_id: sid.map(str::to_owned),
        ..harness(kind)
    }
}

pub fn room(harnesses: Vec<Harness>) -> Room {
    Room {
        id: "r1".to_owned(),
        name: "R".to_owned(),
        task: String::new(),
        status: "idle".to_owned(),
        badge: 0,
        active_harness_id: harnesses.first().map(|h| h.id.clone()).unwrap_or_default(),
        harnesses,
        cwd: None,
        branch: None,
        repo: None,
        archived: None,
        repo_root: None,
        attention: None,
        created_by: None,
        closed_by: None,
        retired: None,
        repo_identity: None,
        todos: None,
    }
}

pub fn ports(entries: &[(&str, u16)]) -> HashMap<String, u16> {
    entries.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

/// A minter that must not be called.
pub fn no_mint() -> String {
    panic!("must not mint");
}

pub fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.iter().map(|p| p.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
        && s.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}
