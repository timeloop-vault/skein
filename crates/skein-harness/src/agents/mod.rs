//! Which agents a harness will accept at `--agent` spawn time.
//!
//! Both CLIs Skein drives bind an agent at launch, and Claude Code
//! cannot change it afterwards — so the list has to be known *before*
//! the process starts. Issue #246 (sub-issue A of #219).
//!
//! **Names come from the CLI, not from globbing its config dirs.**
//! Claude has no agent-list flag (`claude agents` manages *background*
//! sessions), but an unknown `--agent` makes it enumerate its own
//! resolution — builtins, the user dir, the project dir and namespaced
//! plugin agents — in one line, in 0.35 s, with no API call:
//!
//! ```text
//! $ claude --agent __skein_agent_probe__ --print x
//! --agent '__skein_agent_probe__' not found. Available agents: claude,
//! coder, explore, Explore, general-purpose, Plan,
//! pr-review-toolkit:code-reviewer, ... reviewer, scribe
//! $ echo $?
//! 1
//! ```
//!
//! opencode has a real subcommand, `opencode agent list`, printing
//! `name (mode)` per line.
//!
//! Replicating Claude's own resolution instead would mean reading
//! `installed_plugins.json` for each plugin's install path,
//! intersecting with `enabledPlugins` across the user, project and
//! local settings files, and picking among the ten cached versions a
//! daily-driven box holds for one plugin. That drifts from the CLI
//! silently, which is the one failure this list cannot have.
//!
//! So disk parsing here is **enrichment only**: it supplies the
//! `description` a picker shows as a subtitle and the `tools`
//! allowlist that decides whether an agent can see Skein's review MCP
//! tools at all (#215). Because every name is already vouched for by
//! the CLI, enrichment is free to scan too widely — an agent file that
//! matches no name is simply never read back out.
//!
//! # Layout
//!
//! Split by the question a reader has, the way `skein-review` is:
//!
//! - [`def`] — what an agent *is* once discovered, and the one rule
//!   that reads its `tools` list.
//! - [`files`] — the definition files on disk, and their frontmatter.
//!   Shared: both CLIs use `---`-fenced markdown.
//! - [`claude`] — the probe, and Claude's four sources.
//! - [`opencode`] — `agent list`, and why only primaries are offered.
//!
//! Note the sibling collision: [`claude`] here is agent *discovery*,
//! while [`crate::claude`] is the session store. Same for `opencode`.
//!
//! Everything is synchronous, read-only and local. The harnesses own
//! these files; we never write to them.

pub mod claude;
mod def;
mod files;
pub mod opencode;

pub use def::{AgentDef, AgentList, AgentSource, allows_mcp_tools};

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// A `Command` that cannot be hijacked by an ambient git environment.
///
/// Git exports `GIT_DIR`, `GIT_INDEX_FILE` and friends to every hook it
/// runs, so anything spawned from the pre-commit gate inherits them.
/// Neither CLI reads them today, but both shell out to git, and the
/// consequence when it bites is that a child silently operates on *this
/// repository* instead of the one it was pointed at — which is how a
/// `.git` folder appears to vanish (see Conventions in `CLAUDE.md`, and
/// `2bcae5a` on `feat/214-land-actions`). Stripping them costs nothing.
fn clean_command(program: &OsStr) -> Command {
    let mut cmd = Command::new(program);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_PREFIX",
    ] {
        cmd.env_remove(key);
    }
    cmd.stdin(Stdio::null());
    cmd
}
