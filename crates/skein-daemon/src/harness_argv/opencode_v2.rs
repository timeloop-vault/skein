//! #676: opencode v2 attaches its TUI to a Skein-owned server (port of
//! `opencodeV2Argv` / `isOpencodeV2` / `opencodeSpawnPlan`).

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::OpencodeVersion;

/// Which spawn path an opencode harness takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpencodeMode {
    V1,
    V2,
}

/// The v2 argv. The URL is per spawn, so this is derived at spawn time
/// and never stored in `Harness.cmd` (part of the pane's mountKey). No
/// `--agent`: the v2 TUI rejects it; the agent is applied when the
/// session is created.
#[must_use]
pub fn opencode_v2_argv(server_url: &str, session_id: &str) -> Vec<String> {
    ["opencode", "--server", server_url, "--session", session_id]
        .map(str::to_owned)
        .to_vec()
}

/// Major version 2 or later. `None` / unknown reads as v1 (#675).
#[must_use]
pub fn is_opencode_v2(version: Option<&OpencodeVersion>) -> bool {
    version.is_some_and(|v| v.major >= 2)
}

/// v2 only for an opencode harness on a v2 CLI whose argv is still
/// opencode's own (argv[0], the same ownership test as `resume_harness`);
/// a post-exit shell stays as is.
#[must_use]
pub fn opencode_spawn_plan(
    cmd: &[String],
    kind: HarnessKind,
    version: Option<&OpencodeVersion>,
) -> OpencodeMode {
    if kind == HarnessKind::Opencode
        && is_opencode_v2(version)
        && cmd.first().map(String::as_str) == HarnessKind::Opencode.program()
    {
        OpencodeMode::V2
    } else {
        OpencodeMode::V1
    }
}
