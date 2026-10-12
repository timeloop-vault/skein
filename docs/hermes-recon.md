# Hermes Agent as a harness kind — hands-on recon (#621)

Epic #620 asks whether Nous Research's Hermes Agent can run as a Skein
harness kind beside Claude Code and opencode. This recon answers #621's
questions 0–7 from real runs and source reading, and ends with a
recommended strategy and the tier slices. Nothing here is production code.

**Summary**

- **Front-end: drive `hermes --tui` in a PTY.** It ran cleanly in a Skein
  ConPTY. Resize, bracketed paste and Ctrl+C all behaved. The classic
  prompt_toolkit CLI rendered fine too, but did not follow a pane resize.
  ACP works well over stdio, but it makes Hermes a non-PTY kind with no
  #238 nudge seam and needs a chat UI Skein does not have.
- **Telemetry: Hermes shell hooks, plus a read-only `state.db`.** Shell hooks
  fire from both front-ends with `session_id`, `cwd` and `turn_id`. They mark:
  - the session start;
  - every end of turn (`on_session_end`, with `completed` / `interrupted`);
  - permission opened (`pre_approval_request`);
  - permission answered (`post_approval_response`, with `choice`);
  - each tool call;
  - subagent start and stop.

  `state.db` has the rest: messages and tool calls, the model, token totals
  and a running cost estimate.
- **Session identity is simple.** Hermes mints the id itself, and there is no
  way to pre-assign it. The id reaches Skein through `on_session_start` or a
  `state.db` row, but neither exists until the first prompt is sent. By
  default compression keeps the same id (`compression.in_place: true`). When
  rotation is on, `--resume <any id in the chain>` lands on the newest child.
  So `Harness.sessionId` can hold the first id it learns, for good.
- **Two blockers for the agent API (tier D).**
  - **Skein's MCP version.** Skein's `/mcp` rejects Hermes' MCP client.
    Hermes sends `MCP-Protocol-Version: 2025-11-25` (MCP SDK 2.0.0), and
    Skein accepts `2024-11-05` through `2025-06-18` only. That is a Skein fix,
    needed whichever injection route is chosen.
  - **No per-session config.** Hermes has no per-session config flag or env
    var. MCP servers and shell hooks live only in `<HERMES_HOME>/config.yaml`
    (or a profile's). Every route either writes outside the worktree or gives
    Hermes a separate identity. See Q3.
- **Hooks are injected the same way as MCP.** So tiers C and D share one
  injection decision; they are not independent.
- **Profiles are not agents (tier E).** A profile is a whole separate
  HERMES_HOME. It is not a system prompt plus a tool allowlist, and nothing
  lists profiles in a machine-readable form. Do not map them onto
  `capabilities.agents`.
- **Subagents and background work are visible (tier F).** Delegations are
  visible in hooks and on disk. `async_delegations` holds a `running` row
  while a background child outlives its parent's turn. Background shell
  processes appear in `processes.json`.
- **Upstream moves fast.** There were 14 releases in 60 days, and the version
  scheme moved from CalVer to semver. Couple to the hooks' documented
  payloads first, and to `state.db` columns second.

## Method and evidence legend

- One Windows 11 machine. Hermes was installed with the official
  `install.ps1` into a dedicated folder (`-HermesHome
  %LOCALAPPDATA%\hermes-skein-recon -NonInteractive -SkipBrowser
  -SkipComputerUse`), not the default `%LOCALAPPDATA%\hermes`.
- Version: `Hermes Agent v0.21.7+184.g79fe847 (2026.9.24)`, checkout
  `79fe847`, Python 3.14.7, MCP SDK 2.0.0.
- Every experiment ran in a **scratch HERMES_HOME** under that folder. No real
  Hermes setup was touched. Two scratch homes were used: one plain, one with
  logging shell hooks on every relevant event.
- Model: `anthropic/claude-haiku-4.5` via OpenRouter. The key was entered by
  the human and never written into this repo.
- Interactive runs (classic `--cli` and `--tui`) were driven by a person in a
  Skein shell (`byoh`) harness, so the PTY was Skein's ConPTY. A hook logger
  and read-only `state.db` queries were watched live.
- Scripted runs covered the rest:
  - oneshot `-z`;
  - `chat -q`;
  - `--resume`;
  - an ACP client over stdio;
  - forced compression;
  - sync and background `delegate_task`;
  - a profile overlay.

Every claim carries one of these marks:

- **[run]** — verified by a run on the version above.
- **[src]** — read in the source at checkout `79fe847`, not run.
- **[docs]** — upstream docs or release notes only.

## Q0. Which front-end

| | classic `hermes --cli` | `hermes --tui` | `hermes acp` |
|---|---|---|---|
| Built on | prompt_toolkit REPL [src] | Node/Ink UI spawning a Python `tui_gateway` child [src] | JSON-RPC over stdio, no UI [run] |
| Rendering in a Skein PTY | clean [run] | clean [run] | n/a |
| Pane resize | did **not** reflow [run] | reflowed correctly [run] | n/a |
| Multi-line paste | one message, newlines kept [run] | one message, newlines kept (41 lines) [run] | n/a |
| Ctrl+C while streaming | interrupts the turn and stays open; hook reports `interrupted: true`; partial text is saved [run] | same [run] | `session/cancel` returns `stopReason: cancelled` [run] |
| `/exit` | ends the session: row gets `end_reason=cli_close`, and `on_session_finalize` fires [run] | no finalize hook, and `ended_at` stays NULL [run]. The UI hard-kills its gateway child; a later launch reaps the row after a 6 h TTL as `startup_orphan_reap` [src] |  closing stdin gives `end_reason=acp_disconnect` [run] |
| Resume footer printed on exit | `hermes --resume <id>` / `-c "<title>"` [run] | `hermes --tui --resume <id>` / `--tui -c "<title>"` [run] | n/a |
| Fits Skein's PTY model and #238 nudge seam | yes | yes | no: non-PTY like `files`, and needs a chat surface |

**Choose `--tui`.** It is what upstream invests in: the v0.21.3 release notes
add a typed wire contract for the TUI/Desktop gateway [docs]. It is also the
one that resizes properly here. The TUI's exit gap does not matter if
telemetry ends on `on_session_end` (per turn) rather than on finalize. Skein
already knows when the PTY exits. **Spawn with `--tui` explicitly.** The
front-end otherwise depends on `display.interface` in the user's config and
on whether stdin is a TTY [src].

ACP is the strongest structured surface. It provides:
- permission requests to the client;
- cancel;
- `session/load` and `session/resume` with history replay;
- per-session `mcpServers`;
- shared `state.db` (a session started over ACP resumes in the CLI and the
  other way round) [run].

But a Skein kind built on it would be a new non-PTY chat surface, not a
harness tier. Park it for a later epic if a chat UI ever exists.

## Q1. Session identity

- **Format.** `YYYYMMDD_HHMMSS_<6 hex>` (local time) for CLI, TUI and oneshot
  sessions [run]. ACP sessions are plain UUIDs [run].
- **No pre-assignment.** There is no flag or env var to pre-assign the id
  [src]. `HERMES_SESSION_ID` is exported into the agent's own tool and hook
  processes, not read from the caller [run + src]. `--pass-session-id` only
  writes a "Session ID:" line into the model's system prompt [src]. It tells
  the host nothing, so it is irrelevant to discovery.
- **When the id becomes visible.** The id is minted at launch (its timestamp
  is the launch time), but nothing visible exists until the first prompt.
  Then the `sessions` row is inserted and `on_session_start` fires [run]. In
  one run, launch at 21:52:28 and first prompt about 70 s later produced the
  row and the hook at the first prompt.
- **How Skein learns the id.**
  - **With hooks:** `on_session_start` payload `session_id` [run], filtered to
    the harness by `SKEIN_HARNESS_ID`. Spawn env vars reach hook processes
    [run].
  - **Without hooks:** query `state.db` for `sessions` where `cwd` equals the
    room's worktree, `source = 'tui'`, and `started_at >= spawn time`. Every
    row carries `cwd`, `source`, `started_at` and `model` [run].
    Same-worktree races between two Hermes harnesses are possible. This needs
    a new claiming rule: take the newest row no other harness has claimed.
    Skein has no such rule today. opencode's post-exit-shell follow works
    from process scans, not from store rows.
- **Resume.** `--resume <id>` reopens the same row and appends to it. No new
  `on_session_start` fires [run]. `--resume` also accepts an id prefix or a
  title, and `-c` takes the latest session [docs]. The TUI form is
  `hermes --tui --resume <id>` [run].
- **Compression.** By default (`compression.in_place: true`) compaction keeps
  the same session id, marks old message rows `active=0` / `compacted=1`, and
  writes no child [run].
  - **Rotation.** With `in_place: false`, each compression ends the current
    row (`end_reason=compression`) and starts a child with `parent_session_id`
    set [run].
  - **Several rotations per turn.** Two rotations can happen inside one turn
    [run].
  - **Resume follows the chain.** `--resume <root>` or `--resume <middle>`
    both land on the newest child (`resolve_resume_session_id` follows the
    chain) [run].
  - **Session list.** `sessions list` shows only the newest child [run].
  - **Hooks.** The child id first appears in `pre_llm_call` after
    compression. `on_session_start` does not fire for children, and
    intermediate ids appear in no hook [run]. `turn_id` keeps the root id as
    its prefix [run].
- **What `Harness.sessionId` should hold.** The first id Skein learns, kept
  for good. `--resume` on it lands on the newest child either way, so
  following rotations is optional. It is only cosmetic, for display.
- **Existence probe for `resume.rs`.** Open `<HERMES_HOME>/state.db`
  read-only and check `SELECT 1 FROM sessions WHERE id = ? AND archived = 0`
  (or use `hermes sessions list`). A read-only open is safe: readers attach
  read-only since v0.21.3 [docs], and the WAL readers here never blocked a
  running session [run]. Note that `HERMES_HOME` can be a profile dir, whose
  sessions live in that profile's own `state.db` [run].

## Q2. Telemetry source

### `state.db`

- **Mode.** SQLite in WAL mode [run]. Each write is its own committed
  transaction [src]. Message rows appeared about 0.2 s after they streamed
  over ACP, at a 200 ms poll [run].
- **`sessions`.** Columns include `id`, `source`, `model`,
  `parent_session_id`, `started_at`, `ended_at`, `end_reason`,
  `message_count`, `tool_call_count`, `input/output/cache_*/reasoning_tokens`,
  `cwd`, `git_branch`, `estimated_cost_usd`, `actual_cost_usd`, `title`,
  `last_activity_at`, `profile_name`, `archived` and `hidden` [run].
- **`messages`.** Columns include `role`, `content`, `tool_calls` (JSON),
  `tool_name`, `tool_call_id`, `timestamp`, `finish_reason`, `reasoning`,
  `active` and `compacted` [run]. The model is per session, plus
  `session_model_usage` per (session, model) with tokens and cost [run].
- **Totals.** The running cost and token totals are updated during the session
  [run]. A finished classic session showed tokens and about USD 0.04.
- **Other tables.** `async_delegations` holds background delegations (see
  Q5).
- **Not in `state.db`.**
  - Permission prompts: an open approval leaves no row.
  - A clean "turn ended" marker. `finish_reason` on the last assistant row is
    the nearest thing, and this was not checked turn by turn [src].
- **Stability.** The sessions page documents `state.db` only as "key tables",
  with no schema-version promise [docs]. v0.21.0–v0.21.2 had a state.db
  robustness regression and a patch release about it [docs].

### Shell hooks (`config.yaml` → `hooks:`)

- **Configuration.**
  - Each event takes a list of `{command, matcher?, timeout?}` [src].
  - The command runs without a shell (`shlex.split`) with a JSON payload on
    stdin: `{hook_event_name, tool_name, tool_input, session_id, cwd, profile,
    extra}` [run].
  - The process inherits the Hermes process environment, including spawn-time
    `SKEIN_*` variables and `HERMES_SESSION_ID` [run].
- **Events seen firing from the CLI, the TUI and oneshot** [run]:
  - `on_session_start` (`extra.model`, `extra.platform`);
  - `pre_llm_call` / `post_llm_call`;
  - `pre_tool_call` / `post_tool_call` (`tool_name`, `tool_call_id`; post
    carries the result);
  - **`pre_approval_request`** (`description`, `surface`: `cli` for classic,
    `gateway` for the TUI);
  - **`post_approval_response`** (`choice`: `once` / `session` / `always` /
    `deny`);
  - **`on_session_end`**, which fires **at the end of every turn**, not at
    exit, with `completed`, `interrupted` and `turn_exit_reason`;
  - `on_session_finalize` (classic exit only);
  - `subagent_start` / `subagent_stop`;
  - `pre_auxiliary_call` / `post_auxiliary_call` (with
    `aux_task: "compression"`).

  `config.yaml`'s own comment omits the approval events, but they work [run].
- **No dialog id in the shell payload.** The shell payload's `extra` has no
  approval `request_id`. Correlate a dialog by `turn_id` + `description`
  [run]. The Python plugin hook gets a `request_id` [src].
- **Consent.** Each `(event, command)` pair is approved once on a TTY and
  stored in that home's `shell-hooks-allowlist.json`. Non-interactive runs
  skip unapproved hooks [src]. `hooks_auto_accept: true` in config,
  `--accept-hooks`, or `HERMES_ACCEPT_HOOKS=1` approve without a prompt [run
  for the config key, src for the rest]. The env var would also auto-approve
  the user's own unapproved hooks in that process, so prefer the config key
  in a Skein-owned config.
- **Blocking.** Only `pre_tool_call` can block. Everything else is an observer
  that fails open on error or timeout [src]. A Skein hook can never stall the
  agent beyond its timeout.
- **Fragile config.** Hermes refuses to start on a malformed `config.yaml`
  and keeps a `.corrupt` copy [run]. Anything Skein writes must round-trip
  UTF-8 exactly.
- **Stability.** Hooks are the best-documented surface. Payloads are keyword
  sets designed for forward compatibility and carry
  `telemetry_schema_version: "hermes.observer.v1"` [docs + run].

### ACP

ACP gives these for free: phase (`session/prompt` returns a `stopReason`),
permission (`session/request_permission` with five options), tool-call and
usage streaming, and per-session `mcpServers` [run]. See Q0 for why it is not
the recommended front-end.

**Run notes.**
- **No MCP capability advertised.** `initialize` advertises no
  `mcpCapabilities`, but `session/new` accepts an `http` server with headers
  as a `[{name, value}]` list [run].
- **Unlinked permission id.** The permission request's `toolCallId` differs
  from the preceding `tool_call`'s id [run].
- **No hooks over ACP.** ACP ran no shell hooks in these runs [run].

### One source per signal

| Signal | Source | Fallback without hooks |
|---|---|---|
| Session id | `on_session_start` [run] | `state.db` by cwd + source + start time [run] |
| Running | `pre_llm_call` / `pre_tool_call` [run] | L2a output heuristic |
| Waiting (end of turn) | `on_session_end` with `completed` or `interrupted` [run] | `finish_reason` on the last assistant row (unverified, see Open questions), else L2a idle |
| Permission opened / answered | `pre_approval_request` / `post_approval_response` [run] | L2b on the approval prompt text |
| Activity rows | `state.db` `messages` (`tool_calls`, `tool_name`, content) [run] | same |
| Model and cost | `state.db` `sessions` + `session_model_usage` [run] | same |
| Subagents / background | `subagent_start`/`stop` hooks + `async_delegations` + `processes.json` [run] | `state.db` + `processes.json` |

## Q3. MCP and agent API injection (#215)

**Prerequisite, Skein-side.** Hermes' MCP client sends
`MCP-Protocol-Version: 2025-11-25` on initialize [src: the installed
`mcp_types/version.py` `HANDSHAKE_PROTOCOL_VERSIONS`].
- **Skein rejects it** with HTTP 400 on the very first `POST /mcp`, the
  `initialize` call [run]. Hermes sets the header on every request,
  initialize included [src: `tools/mcp_tool_transport.py` ~601].
- **Which check rejects it.** The 400 comes from the header gate
  `protocol_ok` (`app/src-tauri/src/agent_api/http.rs:276`, message at
  `:141`). Its supported list ends at `2025-06-18` (`agent_api/mcp.rs:61`).
  `initialize`'s own body negotiation would already fall back to Skein's
  version, but the request never reaches it.
- **Hermes then gives up.** It retries over SSE (`GET /mcp` → 405), parks the
  server, and the session still starts, with no Skein tools and no error
  surfaced to the user [run].
- **No downgrade.** Hermes does not fall back to an older protocol version
  [src].
- **Fix.** Skein must accept and negotiate `2025-11-25` before any route
  below can work.

**Where Hermes reads MCP servers** [src]:
- `mcp_servers` in `<HERMES_HOME>/config.yaml`, with `${VAR}` interpolation of
  url and headers;
- servers contributed by enabled plugins (an `mcp.json`, no interpolation);
- ACP `session/new` `mcpServers`.

There is no env var, no CLI flag, and no project-level file. Shell hooks have
the same constraint: config only.

The routes, measured against #215's rules (session-scoped, additive, nothing
in the worktree, the user's own config untouched):

| Route | Session-scoped | User config untouched | Shares the user's sessions, memory, key | Cost |
|---|---|---|---|---|
| **Skein-owned profile** (`<root>/profiles/skein`, its own `config.yaml` with hooks + MCP) | per profile, not per session | yes: root `config.yaml` and `.env` stayed byte-identical [run] | **no**: own `state.db`, memories and skills; the root `.env` key is not inherited, so the key must come from the spawn env; resume does not cross between profile and root [run] | small: shares the root runtime, about 6 MB per profile [run] |
| **One-time opt-in entries in the user's `config.yaml`** (MCP `skein` server with `${SKEIN_REVIEW_URL}`; hooks running a Skein script that no-ops when `SKEIN_*` is unset) | no: active in every Hermes run, inert outside Skein | **no**: the user consents once | yes | none at spawn; needs a careful UTF-8 round-trip writer |
| **Per-session `HERMES_HOME`** | yes | yes | no | **about 630 MB per home**: each data root builds its own Python env and cache, and nothing lets two roots share one [run + src] |
| **ACP `mcpServers`** | yes | yes | yes | needs the non-PTY front-end (Q0) |
| `HERMES_BUNDLED_PLUGINS` / project plugins | — | — | — | dead end: the env var replaces the bundled plugins, project plugins live in the worktree, and plugins still need `plugins.enabled` in config [src] |

No route meets all four rules. This is a user decision, recorded on the
tier D issue. The two realistic options are:
- **Skein profile:** clean isolation, and Hermes-in-Skein becomes its own
  identity, with separate memory and sessions.
- **Opt-in user-config entries:** one identity, at the cost of Skein editing a
  file the user owns once, with consent.

## Q4. Profiles and personalities

- **Profiles.** `-p <name>` selects `<root>/profiles/<name>` as the whole
  HERMES_HOME. That covers its own config, `.env`, memories, skills, and
  `state.db` [run].
  - **Listing.** `hermes profile list` is plain text with no JSON option
    [src].
  - **Creating.** `hermes profile create <name> --no-alias --no-skills` works.
    The bundled `hermes-agent` skill is seeded anyway [run].
  - **Deleting.** `profile delete` left an "identity settlement pending" step
    (`purge-identity`) [run].
- **Personalities.** These are prompt overlays: 14 built-ins plus the user's
  own. They are selected by the config key `display.personality` or
  `/personality` mid-session, and there is no launch flag [src].
- **No named agent exists.** There is no system prompt plus tool allowlist
  bound at launch [src].

**Verdict.** Neither concept fits `capabilities.agents` (#246/#247).
- A profile is a different identity with different credentials and history,
  closer to a "Hermes home" setting than to an agent.
- A personality is a tone, switchable mid-session, so nothing needs binding
  at spawn.

Ship tier E as at most a per-harness "profile" field (argv `-p <name>`) once
tier D has decided whether Skein owns a profile itself. Do not reuse the
agents picker.

## Q5. Subagents and background work

**`delegate_task`, synchronous** (`-z`/`-q` always run it sync) [run]:
- **Hook sequence.** `subagent_start` (`child_session_id`, `child_role`,
  `child_goal`, `parent_turn_id`), then the child's own `on_session_start` /
  `pre_llm_call` / tool hooks / `on_session_end` (under the child id,
  `platform: subagent`), then `subagent_stop` (`child_status`,
  `child_summary`, `duration_ms`, `tool_call_history`), then the parent's
  `post_tool_call`.
- **Child row.** The child gets its own `sessions` row: `source = subagent`,
  `parent_session_id` set.
- **Ordering.** The child finished before the parent's turn ended.

**`delegate_task`, background.** Seen over ACP, where it detaches [run]:
- **Disk state while it runs:**
  - an `async_delegations` row with `state=running`, an `owner_pid`, and
    `parent_session_id`;
  - the child session row with `ended_at` NULL;
  - `cache/delegation/live/deleg_<id>/manifest.json` plus `task-<n>.log`.
- **Outlives the turn.** The parent's turn ended about 44 s before the child
  completed. On completion the row moved to `state=completed` with
  `completed_at` and `result_json`.
- **Orphans.** If the parent process dies, the row stays `running` with a dead
  `owner_pid`. A later startup sweeps it to `unknown` [src].
- **Unverified.** Whether the interactive TUI detaches top-level delegations
  the same way. Source says top-level delegations always try background
  [src].

**Background shell processes** (terminal tool, background mode) [run]:
- **While running.** An entry in `<HERMES_HOME>/processes.json` with `pid`,
  `started_at`, `cwd`, and `parent_session_id` equal to the main session.
- **On finish.** The entry is removed.
- **Outlives the turn.** The main turn ended while it ran.
- **No re-wake.** With `notify_on_complete: false` it never woke the session
  again.

**Enough for the #277/#446 deferral.** Outstanding work is:
- `subagent_start` minus `subagent_stop` (hooks); or, from disk,
  `async_delegations.state = 'running'` with a live `owner_pid`;
- plus live `processes.json` entries for the session.

Like Claude's, this is re-derivable from disk on attach. Upstream exposes no
"turn ended but children still running" flag.

## Q6. Turn state on screen (L2b fallback)

Prompt text, as read in the source [src]. The approval prompt and the hook
events were seen in the runs; the exact glyphs were not captured on screen.
- **Classic idle prompt.** `❯ `, with the profile name prefixed when it isn't
  the default. State glyphs are `⚠` approval, `?` clarify and `☤` running.
- **Classic approval prompt.** A header `⚠️  DANGEROUS COMMAND: <description>`,
  then `[o]nce | [s]ession | [a]lways | [d]eny`, then `Choice [o/s/a/D]: `.
- **TUI approval box.** A box titled `⚠ approval required · <description>`
  with options `1. Allow once` … `4. Deny`. Its footer reads
  `↑/↓ select · Enter confirm · 1-N quick pick · Esc/Ctrl+C deny`.
- **TUI idle prompt.** `❯` with a rotating placeholder. While busy it shows
  `Ctrl+C to interrupt…`.
- **Text comes from locale files.** All of it comes from `locales/en.yaml` /
  `ui-tui/src/i18n` and changes with the user's locale.

**L2b verdict.**
- **The approval header is a usable fallback** for the plain tier (A): the
  classic `Choice [o/s/a/D]:` and the TUI `approval required` box both end
  the visible output while blocked.
- **The idle `❯` is not:** the TUI redraws it constantly, and the placeholder
  rotates.

Tier A should ship one or two English-locale approval patterns. It should not
ship an idle pattern. The L2a idle heuristic covers the rest until hooks
(tier C) are authoritative.

## Q7. Platform, version, and stability

- **Windows native.** Works in a Skein ConPTY with `--tui` and `--cli` [run].
- **Installer footprint.** The installer adds `<home>\bin` to the user PATH,
  installs a pinned Git into its tool store even when Git is present, and has
  uv fetch a managed CPython 3.14 [run]. Upstream lists Windows and macOS
  (Apple Silicon) as Tier 1 [docs]. Open upstream items mention ConPTY
  hard-wrapping at 80 columns and Defender quarantining a regenerated
  `hermes.exe` [docs]. Neither was seen here.
- **Version detection.** `hermes --version` prints
  `Hermes Agent v<semver>[+<n>.g<sha>] (<release date>)` on its first line
  [run].
- **Cadence.**
  - **Releases.** 14 releases in the 60 days before 2026-10-12, the newest
    being v0.21.7. Tags moved from
    CalVer (`v2026.9.24`) to semver (`v0.21.6`, `v0.21.7`), with patch notes
    deferred to the next minor release [docs].
  - **Channels.** Source installs follow the latest release by default;
    `hermes update --set-channel main` follows every commit [docs].
- **Relied-on surfaces, most to least stable:**
  1. Shell-hook events and payloads: documented, forward-compatible keyword
     sets.
  2. `--resume` / `-c` / `--tui`: documented CLI.
  3. `state.db` columns: documented as key tables only, recently reworked.
  4. Prompt text: locale strings.

  Pin the minimum Hermes version Skein supports, check it with
  `hermes --version` at spawn, and degrade to tier A behaviour on an unknown
  major or minor version.

### Needs a Mac

Nothing was run on macOS. Check before shipping beyond Windows:
- `install.sh` into a non-default home;
- `--tui` rendering, resize, and paste in a Skein PTY;
- Ctrl+C;
- whether the TUI's `/exit` still skips finalize (POSIX signal handlers exist
  in `tui_gateway/entry.py` [src], so the row may close properly there);
- hook execution and the env passthrough;
- `state.db` path `~/.hermes`;
- the known macOS TUI keyboard issues upstream (modified Enter, Cmd+K) [docs].

## Recommendation

1. **Fix Skein's MCP version gate first.** Accept and negotiate `2025-11-25`
   on `/mcp`. This is small, independent, and needed by every injection
   route.
2. **Tier A (plain kind)** — `hermes --tui` in a PTY:
   - spawn argv `[hermes, --tui]`;
   - capabilities `pty` + `notify`, with no `resume` or `agents` yet;
   - English approval L2b patterns only;
   - `hermes` resolved through the existing PATHEXT logic (the Windows
     launcher is `hermes.exe`).

   This ships alone, like `remote` (#568). Its notifications are best-effort,
   from L2a idle plus the approval-only L2b patterns, until tier C makes the
   phase authoritative.
3. **Tier B (resume)** — learn the id from `state.db` by cwd + `source='tui'`
   + start time after the first prompt. Then:
   - resume with `hermes --tui --resume <id>`;
   - add a read-only `state.db` existence probe in `resume.rs` and the store
     reader in `crates/skein-harness`.

   No compression following is needed.
4. **Decide the injection route** (Q3: Skein profile vs opt-in user-config
   entries) as the first step of tier C/D. Both tiers use it.
5. **Tier C (authoritative telemetry)** — a Skein-shipped hook script that
   POSTs the payloads to Skein, keyed by `SKEIN_HARNESS_ID`.
   - **Reuse the existing routes first.** Three of the four signals map onto
     routes that already exist for Claude's plugin hooks
     (`docs/agent-api.md`): `on_session_start` → `POST
     /api/harness/session-start` (which also lets a harness leave
     `spawning`), the two approval events → `POST /api/harness/permission`.
     Whether `POST /api/harness/session-end` fits is an open question: it
     means "the session ended" (#318), while Hermes'
     `on_session_end` fires per turn.
   - **One new route at most:** for the per-turn signals (turn start, end of
     turn, subagent start/stop).
   - **`state.db`:** tailing gives activity rows, model and cost.
6. **Tier D (agent API)** — the `skein` MCP server, with the bearer header
   from `${SKEIN_REVIEW_TOKEN}`, via the chosen route. The mail nudge (#329)
   also needs tier C: the #238 gate requires a proven authoritative adapter.
   With both, it works through the #238 seam, since the TUI accepts
   bracketed paste.
7. **Tier F (subagents and background)** — `subagent_start`/`stop` plus
   `async_delegations` and `processes.json` feed the outstanding-work count.
   It depends on C.
8. **Tier E (profile)** — a per-harness profile name, not
   `capabilities.agents`. Low priority, and shaped by the tier D decision.

## Follow-up issues

| Issue | Slice |
|---|---|
| #622 | Accept MCP protocol `2025-11-25` on `/mcp` (prerequisite for D) |
| #623 | Tier A: plain PTY kind running `hermes --tui` |
| #624 | Tier B: learn the session id from `state.db` and resume |
| #625 | Tier C: authoritative phase, permission and activity (hooks + `state.db`) |
| #626 | Tier D: agent API, starting with the injection-route decision |
| #627 | Tier E: profile selection per harness |
| #628 | Tier F: delegations and background processes into the deferral |

## Open questions

- Does the interactive TUI detach top-level `delegate_task`, and do
  `subagent_stop` and the completion re-entry fire there as they do in
  ACP/sync runs?
- Is `finish_reason` on the last assistant row reliable enough to stand in
  for `on_session_end` when hooks are off?
- Does a Skein profile need the root's `auth.json` (OAuth providers such as
  Nous Portal)? Source says profiles fall back to it read-only; this was not
  run.
- The exact on-screen bytes of the approval prompts in a ConPTY, so the L2b
  regex is pinned to a fixture rather than to the locale file.
