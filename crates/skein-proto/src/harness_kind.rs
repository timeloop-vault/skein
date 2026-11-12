//! The harness kinds Skein knows how to spawn (issue #116, absorbing
//! #240). This is the Rust half of the registry whose TS half is
//! `HARNESS_KINDS` in `app/src/data.tsx` (id ↔ variant, `program` ↔
//! `program()`, `capabilities.agents` ↔ `takes_agent()`); `ALL` mirrors
//! the `HarnessKind` union in `app/src/types.ts`. Deserializing at the
//! Tauri boundary (`pty_spawn`, `list_harness_agents`) means an unknown
//! kind string now fails the invoke instead of silently falling through
//! every `match` arm to "do nothing" — the drift #240 was filed about.
//! The `agreement` test below parses both TS files so the two halves
//! cannot quietly diverge again.

use std::fmt;

/// One of the harness kinds a room can spawn. `Files` is the one
/// exception (#184/#185) — a non-PTY harness that never reaches
/// `pty_spawn` at all, but it still needs a place in this registry
/// because agent discovery and config injection both gate on kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessKind {
    Claude,
    Opencode,
    Copilot,
    Byoh,
    Files,
    /// #433: a non-PTY live preview of the worktree's HTML.
    Design,
    /// #568: `ssh -t -- <host> …tmux…` in a local PTY.
    Remote,
    /// #623: Hermes Agent's TUI (`hermes --tui`) in a plain PTY.
    Hermes,
}

// Every kind, for the agent API's kind check (so it cannot drift from
// the enum) and for this module's own tests.
pub const ALL: [HarnessKind; 8] = [
    HarnessKind::Claude,
    HarnessKind::Opencode,
    HarnessKind::Copilot,
    HarnessKind::Byoh,
    HarnessKind::Files,
    HarnessKind::Design,
    HarnessKind::Remote,
    HarnessKind::Hermes,
];

impl HarnessKind {
    /// The kind named by `name` (the `as_str` form), or `None` for
    /// anything unknown — for strings that came over a boundary and
    /// must not be stored unchecked.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "claude" => Self::Claude,
            "opencode" => Self::Opencode,
            "copilot" => Self::Copilot,
            "byoh" => Self::Byoh,
            "files" => Self::Files,
            "design" => Self::Design,
            "remote" => Self::Remote,
            "hermes" => Self::Hermes,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Opencode => "opencode",
            Self::Copilot => "copilot",
            Self::Byoh => "byoh",
            Self::Files => "files",
            Self::Design => "design",
            Self::Remote => "remote",
            Self::Hermes => "hermes",
        }
    }

    /// The program Skein spawns for this kind, where Skein chooses it.
    /// Mirrors `HARNESS_KINDS[kind].program` in `app/src/data.tsx`
    /// (formerly `managedProgram` in `harnessCmd.ts`): `byoh` runs the
    /// user's own shell and `files` runs nothing, so neither has one.
    pub fn program(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("claude"),
            Self::Opencode => Some("opencode"),
            Self::Copilot => Some("gh"),
            Self::Remote => Some("ssh"),
            Self::Hermes => Some("hermes"),
            Self::Byoh | Self::Files | Self::Design => None,
        }
    }

    /// Whether the CLI takes `--agent <name>` and binds it at launch
    /// (#247). Mirrors `HARNESS_KINDS[kind].capabilities.agents` in
    /// `app/src/data.tsx`: `copilot` is a managed program with no agent
    /// concept, which is exactly the distinction `program().is_some()`
    /// alone would lose.
    pub fn takes_agent(self) -> bool {
        matches!(self, Self::Claude | Self::Opencode)
    }

    /// Whether the underlying tool can resume its conversation across
    /// Skein restarts (drives the boot/reopen argv rewrite). Mirrors
    /// `HARNESS_KINDS[kind].capabilities.resume` in `app/src/data.tsx`.
    pub fn resumes(self) -> bool {
        matches!(
            self,
            Self::Claude | Self::Opencode | Self::Hermes | Self::Remote
        )
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn each_kind_round_trips_its_lowercase_string() {
        for kind in ALL {
            let json = serde_json::to_string(&kind).expect("serialize");
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let back: HarnessKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn from_name_inverts_as_str_and_rejects_unknown() {
        for kind in ALL {
            assert_eq!(HarnessKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(HarnessKind::from_name("something-new"), None);
    }

    /// Pinned by hand as well as by the TS agreement test below, so the
    /// daemon's argv builders have a fixed table to lean on.
    #[test]
    fn program_and_resumes_per_kind() {
        let table = [
            (HarnessKind::Claude, Some("claude"), true),
            (HarnessKind::Opencode, Some("opencode"), true),
            (HarnessKind::Copilot, Some("gh"), false),
            (HarnessKind::Byoh, None, false),
            (HarnessKind::Files, None, false),
            (HarnessKind::Design, None, false),
            (HarnessKind::Remote, Some("ssh"), true),
            (HarnessKind::Hermes, Some("hermes"), true),
        ];
        assert_eq!(table.len(), ALL.len());
        for (kind, program, resumes) in table {
            assert_eq!(kind.program(), program, "{kind} program");
            assert_eq!(kind.resumes(), resumes, "{kind} resumes");
        }
    }

    #[test]
    fn an_unknown_kind_string_fails_to_deserialize() {
        let result: Result<HarnessKind, _> = serde_json::from_str("\"something-new\"");
        assert!(result.is_err(), "an unknown kind must be rejected");
    }

    /// Parses `export type HarnessKind = "a" | "b" | ...;` out of
    /// `types.ts` and pulls `program`/`capabilities.agents` out of each
    /// `HARNESS_KINDS` entry in `data.tsx`, so a kind added to one
    /// language and not the other (or a `program`/`agents` value that
    /// disagrees) fails loudly here instead of silently at spawn time.
    #[test]
    fn agrees_with_the_typescript_registry() {
        // Paths are relative to this file: crates/skein-daemon/src/
        // harness_kind.rs → ../../../app/src/{types.ts,data.tsx} is
        // app/src/{types.ts,data.tsx}.
        let types_ts = include_str!("../../../app/src/types.ts");
        let data_tsx = include_str!("../../../app/src/data.tsx");

        let ts_kinds = parse_harness_kind_union(types_ts);
        let rust_kinds: BTreeSet<&str> = ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(
            ts_kinds, rust_kinds,
            "types.ts `HarnessKind` union and harness_kind::ALL have drifted apart"
        );

        for kind in ALL {
            let (program, agents, resume) = parse_data_tsx_entry(data_tsx, kind.as_str())
                .unwrap_or_else(|| panic!("data.tsx: no HARNESS_KINDS entry for {kind}"));
            assert_eq!(
                program,
                kind.program().map(str::to_owned),
                "data.tsx `program` for {kind} disagrees with HarnessKind::program()"
            );
            assert_eq!(
                agents,
                kind.takes_agent(),
                "data.tsx `capabilities.agents` for {kind} disagrees with HarnessKind::takes_agent()"
            );
            assert_eq!(
                resume,
                kind.resumes(),
                "data.tsx `capabilities.resume` for {kind} disagrees with HarnessKind::resumes()"
            );
        }
    }

    fn parse_harness_kind_union(src: &str) -> BTreeSet<&str> {
        let marker = "export type HarnessKind =";
        let start = src
            .find(marker)
            .unwrap_or_else(|| panic!("types.ts: no `{marker}` found"))
            + marker.len();
        let end = src[start..]
            .find(';')
            .unwrap_or_else(|| panic!("types.ts: unterminated HarnessKind union"))
            + start;
        src[start..end]
            .split('|')
            .map(|part| part.trim().trim_matches('"'))
            // Biome breaks a long union onto lines with a leading `|`,
            // which leaves an empty first part.
            .filter(|part| !part.is_empty())
            .collect()
    }

    /// Pulls one `HARNESS_KINDS` entry's `program` and
    /// `capabilities.agents` fields out of the raw source, from its
    /// `id: "<kind>"` line up to the next entry's `id: "` (or EOF).
    /// Small and string-based on purpose (#116): a kind whose value the
    /// parser can't find panics with the kind's name rather than
    /// silently passing.
    fn parse_data_tsx_entry(src: &str, kind: &str) -> Option<(Option<String>, bool, bool)> {
        let marker = format!("id: \"{kind}\"");
        let entry_start = src.find(&marker)?;
        let rest = &src[entry_start + marker.len()..];
        let entry_end = rest.find("id: \"").unwrap_or(rest.len());
        let entry = &rest[..entry_end];

        let program = if let Some(idx) = entry.find("program: \"") {
            let after = &entry[idx + "program: \"".len()..];
            let end = after
                .find('"')
                .unwrap_or_else(|| panic!("data.tsx: {kind} has an unterminated program string"));
            Some(after[..end].to_owned())
        } else if entry.contains("program: null") {
            None
        } else {
            panic!("data.tsx: {kind} entry has no `program` field");
        };

        let agents = if entry.contains("agents: true") {
            true
        } else if entry.contains("agents: false") {
            false
        } else {
            panic!("data.tsx: {kind} entry has no `agents` field");
        };

        let resume = if entry.contains("resume: true") {
            true
        } else if entry.contains("resume: false") {
            false
        } else {
            panic!("data.tsx: {kind} entry has no `resume` field");
        };

        Some((program, agents, resume))
    }
}
