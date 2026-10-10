# ssh + tmux spike (#568)

Phase 0 of the planned skeind epic (epic to follow). Question: how far does a
dumb remote — `ssh` plus `tmux`, no daemon — get us before a daemon is worth
building? This is a spike, written to be removable.

## What it is

A `remote` harness kind. Skein spawns `ssh` in a local PTY, and the remote end
runs the tool inside a named tmux session. The record
`Harness.remote {host, tool, session, dir?}` is the authority; the argv is
rebuilt from it at every boot and reopen, never matched against the previous
argv (the `harnessCmd.ts` rule, #153/#170). `remoteCmd.ts` is the pure builder.

For `{host: "user@devbox", tool: "claude", session: "skein-r1-h1", dir: "~/work"}`:

    ["ssh", "-t", "--", "user@devbox", "exec \"$SHELL\" -lc <INNER>"]

where `<INNER>` is the command below, POSIX single-quoted as one word. Unwrapped,
the command the remote login shell runs is:

    tmux new-session -A -s 'skein-r1-h1' -c "$HOME"/'work' 'claude' \; set-option -t 'skein-r1-h1' status off

## How to use

`+ harness` -> Remote (ssh+tmux), then three fields:

- **host**: anything `ssh` accepts, including `~/.ssh/config` aliases. Use
  key-based auth; there is no password handling beyond the terminal itself.
- **tool**: a remote shell command line. Default `claude`; e.g. `opencode`,
  or a full path such as `~/.local/bin/claude` when the CLI is not on the
  login PATH.
- **dir** (optional): absolute or `~/...`.

Not offered in New room. The agent API's `create_room` / `open_harness`
refuse the kind.

Requirements on the host: `tmux`, the tool, and reachable by `ssh` from the
Skein machine. To end a session for good, exit the tool inside tmux, or run
`tmux kill-session -t skein-...` on the host.

Removing the spike: the `remote` entry in `data.tsx` / `types.ts` /
`harness_kind.rs`, `remoteCmd.ts`, `RemoteHostStep.tsx` / `.css`, and the
`remote` field in `db/rooms.rs` (an `Option`, so old blobs keep parsing).

## Design choices and why

- **Login-shell wrap** (`exec "$SHELL" -lc`). On the test host (a macOS
  machine) a non-login `ssh host cmd` found neither `tmux` nor `opencode` on
  PATH (Homebrew), and `claude` lived in `~/.local/bin`, on no PATH at all.
- **`new-session -A`** is attach-or-create, so a restart reattaches. The
  session name comes from room + harness ids, is minted once and stored, and
  is sanitised to `[A-Za-z0-9_-]` (tmux forbids `.` and `:`).
- **`status off`**: native look, and tmux's status clock would otherwise keep
  resetting the L2a quiet timer.
- **`--` before the host, plus host validation** (no leading `-`, no
  whitespace or control characters) against ssh option injection such as
  `-oProxyCommand=...`. The tool is deliberately a raw command line run by
  the user's own remote shell; it is the user's own command.
- **Capabilities**: `pty` yes; `resume` = tmux reattach (same argv); `notify`
  on, heuristic only; `agents` no; no #215 config injection.

## What works (verified)

- Reattach across a full Skein quit and relaunch: lands in the same tmux
  session with the TUI and conversation intact (manual check by the
  maintainer on the dev build).
- The generated command run against the test host: the session survives the
  ssh disconnect, `status off` is applied, `~/dir` resolves, and a tool with
  quotes and `;` reaches tmux intact.

## What doesn't work / what is lost

- **Phase.** No transcript or SSE exists locally, so there is no L2c adapter:
  phase is L2a idle / L2b regex only. No `permission` phase, no subagent or
  background-task deferral, end of turn is not authoritative. Expected from
  the design; not specifically observed in the manual check.
- **Nudge and mail delivery** (#238/#329) are gated on a proven adapter, so
  unavailable.
- **Review.** No #215 injection remotely, so the remote agent has no Skein
  MCP API. And the room's worktree is local while the agent edits the remote
  filesystem, so the review pane shows nothing of its work.
- **Activity feed, Plan card, cost**: empty (no `harness_actions` rows).
- **Post-exit shell.** When ssh exits (tool quit, network drop) the pane
  offers the LOCAL shell, not a remote one. Restart harness, or the next
  boot, reconnects via the record.
- **Agents** (`--agent`) are not offered.
- **Resize** is "a bit janky", similar to the current release's local Claude
  resize behaviour. main has a Claude-specific resize fix that may not apply
  here. tmux sits between xterm and the TUI, so a resize goes xterm -> ssh
  window-change -> tmux -> TUI.
- **Shell syntax error in the tool** makes the pane exit at once and the tmux
  session vanish (seen with an unbalanced quote in a probe).

## Observations for the skeind design

1. Reattach is cheap and already good with dumb tmux. The daemon's value is
   not keeping the process alive but the telemetry: the transcript / SSE
   lives on the remote host, so the daemon must tail it where it lives and
   ship events, not just bytes.
2. PATH and environment on the remote are the first thing that breaks. The
   daemon should own the spawn env remotely (cf. #565 locally).
3. A second resize layer (tmux) adds jank. A daemon owning the PTY directly
   removes a hop.
4. Review needs the worktree where the agent works: either the room's
   worktree is remote, or review reads through the daemon.
5. The agent API must be reachable from the remote (tunnel / port-forward, or
   daemon-relayed MCP) for review tools, mail and nudge to work.
