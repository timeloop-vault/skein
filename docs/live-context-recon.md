# Live Context — adapter signal recon (issue #80)

Verified on macOS arm64 (Darwin 25.2.0) on 2026-05-26 against the
user's own working session logs:

- Claude Code 2.1.x (`~/.claude/projects/-Users-scripter-git-private-skein/*.jsonl`)
- opencode 1.14.x (`~/.local/share/opencode/opencode.db` + `storage/`)

Issue #80 calls for re-scoping the right pane around a Diff + Plan +
Activity card stack and asks the recon to: (a) confirm Claude's
TodoWrite shape in the JSONL we already read, (b) confirm whether
opencode has a plan equivalent in its SSE/db, (c) decide schema:
extend `harness_events` or add a sibling `harness_actions` table.

Headlines before the detail:

1. **TodoWrite is gone from Claude.** Zero `TodoWrite` calls across
   all of this user's sessions, ever. The current plan mechanism is
   `TaskCreate` / `TaskUpdate` / `TaskList` / `TaskGet` (with
   `TaskOutput` / `TaskStop` for background tasks). Issue #80's plan
   card spec needs updating — it's a "Tasks" card now, sourced from
   these tool calls.
2. **opencode has *two* plan stores.** A `todo` SQLite table (current
   state, joined view) *and* `todowrite` tool-call parts (per-call
   event log). Best of both worlds — current snapshot for the Plan
   card, history for the Activity card.
3. **Edit/Write returns a structured patch.** Every Claude Edit/Write
   `toolUseResult` carries `structuredPatch` (full git-style hunks)
   plus `userModified`. Every opencode edit tool result carries
   `metadata.filediff: {file, patch, additions, deletions}`. The Diff
   card does **not** need to re-derive — both adapters give it to us.
4. **Activity card is mostly already in flight.** Both adapters
   already see every tool call (`ToolUseStart { name }`); we just
   discard the payload. Persisting the full payload + result is
   ~the entire missing piece. The activity feed scaffold from PR #79
   is the right shell — different data shape, same plumbing.
5. **Recommended schema:** sibling `harness_actions` table, NOT an
   extension of `harness_events`. The existing table is two enums
   (from/to phase). Adding nullable JSON for tool name + args +
   result conflates two genuinely different event shapes; the join
   on `(harness_id, timestamp_ms)` is trivial; rationale in §4.

---

## 1. Claude JSONL — full signal inventory

The session log is JSONL, one event per line, append-only, flushed
after every write (already established in
`docs/epic-50-l2c-2-recon.md`).

### 1.1 Row types

Beyond the standard `user` / `assistant` / `system` / `summary`,
Claude writes **eight** other row types. Many of them are directly
useful for Live Context:

| `type` | Count* | What it carries | Live Context use |
|---|---|---|---|
| `assistant` | 947 | One row per streamed chunk of an assistant turn. `message.content[]` blocks: `text`, `thinking`, `tool_use`. `message.usage`, `message.model`, `message.stop_reason`, `message.id`. | **Activity card** (tool_use rows); diff trigger; cost/usage telemetry. |
| `user` | 681 | Either a typed user prompt OR a tool result (distinguish via `toolUseResult != null`). For tool results, `message.content[]` holds `tool_result` block, and a parallel structured `toolUseResult` field carries per-tool-shape detail (§1.4). | **Activity card** (tool result row). |
| `ai-title` | 121 | `aiTitle` — Claude's auto-generated session title. Updated periodically as the session evolves. | Session/room title override. Better than the user-typed slug. |
| `last-prompt` | 117 | `lastPrompt` (truncated to ~200 chars) + `leafUuid`. Rewritten on every prompt — last-write-wins. | Recent-prompt header on the room. |
| `file-history-snapshot` | 111 | `messageId` + `snapshot.trackedFileBackups` (file backup map for undo). | Not directly user-facing — Claude's own undo store. Probably skip. |
| `pr-link` | 88 | `prNumber`, `prUrl`, `prRepository`, `timestamp`. Emitted when a PR is opened or referenced. | Activity card pinned row: "session opened PR #61". |
| `permission-mode` | 77 | `permissionMode`: `default` / `acceptEdits` / `plan` / `bypassPermissions`. | Harness chip badge (`AE` / `plan`). |
| `attachment` | 67 | `.attachment.type` is one of seven subtypes (§1.5). | Several feed into the Activity card; see §1.5. |
| `bridge-session` | 27 | `bridgeSessionId` (e.g. `cse_01...`) + `lastSequenceNum`. Tracks Claude's "remote control" bridge — the `claude.ai/code/session_...` URL. | Activity row: "session bridged to remote-control". |
| `queue-operation` | 2 | `operation` (`enqueue` / `popAll`) + `content`. The user typed a prompt while the assistant was still responding. | **Activity card**: "user queued a prompt". |
| `system` | 105 | `.subtype` discriminates (§1.6). | Multiple — see §1.6. |
| `summary` | 0 in this user's sessions | Older row type; may appear on `--resume`. Ignore for v1. | — |

\*counts from `f81beb9d-...jsonl` (~5 MB session). Other sessions
in this user's profile have the same shape with similar
distributions.

### 1.2 Assistant content blocks

Each `assistant` row's `message.content` is an array of blocks:

| Block type | Count | Key fields | Use |
|---|---|---|---|
| `tool_use` | 619 | `id`, `name`, `input` (tool-specific) | **Activity card** core row. |
| `thinking` | 168 | `thinking` (text, often empty) + `signature` (encrypted reasoning). | Optional collapsed disclosure ("Claude thought for 12s"). |
| `text` | 162 | `text` (markdown). | Activity narration / assistant message preview. |

### 1.3 Message-level metadata

`assistant.message` carries usage / cost / model telemetry:

```
model         : "claude-opus-4-7" | "<synthetic>"
stop_reason   : "tool_use" | "end_turn" | "stop_sequence"
usage         : { input_tokens, output_tokens, cache_creation,
                  cache_creation_input_tokens, cache_read_input_tokens,
                  service_tier, inference_geo, iterations,
                  server_tool_use, speed }
context_management : { ... }  // present when context is being managed
container     : { ... }       // rare
diagnostics   : { ... }       // rare
apiErrorStatus, isApiErrorMessage, error  // on failed turns
requestId     : Anthropic API request id (useful for support / dedupe)
attributionSkill : skill name that drove this turn (rare)
```

The `usage.cache_read_input_tokens` divided by `input_tokens` is the
cache hit rate — interesting telemetry, not Live-Context-critical.

### 1.4 Tool inputs and results (the gold)

The 619 tool_use blocks in this session split across 13 tool names
across all sessions: `Bash`, `Edit`, `Read`, `Write`, `Grep`,
`TaskCreate`, `TaskUpdate`, `TaskList`, `TaskGet`, `TaskOutput`,
`TaskStop`, `Skill`, `ToolSearch`, `Agent`, `AskUserQuestion`,
`Monitor`. (No `TodoWrite`, ever — see headline #1.)

`tool_use.input` shape is per-tool. Representative samples:

| Tool | Input shape | Result shape (`toolUseResult`) |
|---|---|---|
| `Bash` | `{command, description}` | `{stdout, stderr, interrupted, isImage, noOutputExpected[, returnCodeInterpretation, backgroundTaskId, backgroundedByUser, staleReadFileStateHint]}` — or `string` for legacy |
| `Edit` | `{file_path, old_string, new_string, replace_all}` | `{filePath, oldString, newString, originalFile, replaceAll, structuredPatch, userModified}` |
| `Write` | `{file_path, content}` | `{filePath, originalFile, content, structuredPatch, type, userModified}` |
| `Read` | `{file_path}` | `{file, type}` (minimal) |
| `Grep` | `{pattern, path, output_mode, ...}` | string |
| `TaskCreate` | `{subject, description, activeForm}` | `{task: {id, subject}}` |
| `TaskUpdate` | `{taskId, status[, subject, description, activeForm, owner, ...]}` | `{success, taskId, updatedFields, statusChange: {from, to}}` |
| `TaskList` | `{}` | array of tasks |
| `TaskGet` | `{taskId}` | full task object |
| `TaskOutput` | `{task_id, block, timeout}` | `{retrieval_status, task}` |
| `TaskStop` | `{task_id}` | `{command, message, task_id, task_type}` |
| `Skill` | `{skill}` (skill slug) | `{commandName, success}` |
| `ToolSearch` | `{query, max_results}` | `{matches, query, total_deferred_tools}` |
| `Agent` (sub-agent spawning) | `{description, subagent_type, prompt[, model, isolation, mode, ...]}` | `{agentId, agentType, status, content, prompt, totalDurationMs, totalTokens, totalToolUseCount, toolStats, usage}` |
| `AskUserQuestion` | `{questions: [{question, header, options, multiSelect}]}` | `{questions, answers: {<question>: <choice>}}` |

**`structuredPatch`** is an array of hunks, each
`{oldStart, oldLines, newStart, newLines, lines}` (git-diff style).
This is everything the Diff card needs — we do not have to compute
diffs from the worktree, the patch is already in the result.

**`userModified`** flags whether the user edited the file *after*
the agent edited it (it was different on disk when we re-read).
That's an explicit "user touched the agent's work" signal — would
be tedious to derive otherwise.

**`statusChange: {from, to}`** on TaskUpdate is a phase transition
ready to render: "Task #3 status: pending → in_progress".

**`AskUserQuestion.answers`** captures what the user chose — perfect
for "user picked v0.1.7" activity rows.

### 1.5 Attachment subtypes

`attachment` row's `.attachment.type` distinguishes:

| Subtype | Count | Carries | Use |
|---|---|---|---|
| `task_reminder` | 47 | `{content: [], itemCount}` — system reminders about open tasks | Internal nudge; skip. |
| `edited_text_file` | 7 | `{filename, snippet}` — user edited a file *outside* Claude mid-turn | Activity row: "user edited `<file>` outside Claude". |
| `deferred_tools_delta` | 5 | `{addedNames[], addedLines[], removedNames[], readdedNames[], pendingMcpServers[]}` | Activity row: "MCP server `X` added 12 tools". |
| `date_change` | 5 | `{newDate}` | Visual day-separator in feed. |
| `skill_listing` | 1 | `{content, skillCount, isInitial}` | Available-skills snapshot. |
| `plan_mode_exit` | 1 | `{planFilePath, planExists}` | Activity row: "exited plan mode (plan saved to X)". |
| `command_permissions` | 1 | `{allowedTools[]}` | Snapshot of permission grants. |

### 1.6 System subtypes

`system` row's `.subtype` distinguishes:

| Subtype | Count | Carries | Use |
|---|---|---|---|
| `turn_duration` | 55 | `{durationMs, messageCount}` | Activity row: "turn took 63 s, 32 messages". |
| `away_summary` | 26 | `{content}` — auto-generated "what we're doing right now" | **Room subtitle** when user re-opens after time away. High value. |
| `api_error` | 20 | `{error: {status, headers, requestID, type, error}, level, maxRetries, retryAttempt, retryInMs}` | Activity row + waiting-on-retry phase. |
| `informational` | 2 | `{content, level}` (e.g. "Remote Control failed to connect") | Activity row. |
| `local_command` | 1 | `{content}` — output of `/slash-command`s wrapped in `<local-command-stdout>` | Activity row for slash commands. |
| `bridge_status` | 1 | `{content, url}` — remote-control connection status | Activity row + the `claude.ai/code/session_...` URL. |

### 1.7 Common metadata on every "real" row

Every user/assistant/system/attachment row carries the same envelope:
`uuid`, `parentUuid`, `sessionId`, `cwd`, `gitBranch`, `timestamp`
(ISO 8601), `version` (Claude binary version), `entrypoint` (`cli`),
`userType`, `isSidechain`, `isMeta`. The `parentUuid` chain forms
the conversation DAG.

Tool results join to their assistant call via
`sourceToolAssistantUUID` (on the user row) → `uuid` (on the
assistant row). Multiple tool_use blocks within one assistant turn
share the same assistant uuid.

### 1.8 What we already extract today

`harness_events_claude.rs` already parses every row and emits
`ClaudeEvent::{AssistantTurn, ToolUseStart{name}, ToolUseResult,
UserPrompt, AwaitingPrompt, Attachment, SessionEnd}` over a Tauri
Channel. The pipeline works; the payload is what's missing. Adding
the full input + result to `ToolUseStart{...}` / `ToolUseResult{...}`
is straightforward and bounded.

---

## 2. opencode — full signal inventory

opencode persists everything to SQLite (`opencode.db`) plus a
`storage/` tree of JSON blobs. We've already done HTTP/SSE recon
in `docs/epic-50-l2c-2-recon.md` — this section focuses on what
we can *additionally* pull from the persisted state.

### 2.1 SQLite schema (curated)

```
session   : id, project_id, parent_id, slug, directory, title, version,
            share_url, summary_additions, summary_deletions,
            summary_files, summary_diffs, revert, permission,
            time_created, time_updated, time_compacting, time_archived,
            workspace_id, path, agent, model
message   : id, session_id, time_created, time_updated, data  (JSON)
part      : id, message_id, session_id, time_created, time_updated, data  (JSON)
todo      : session_id, content, status, priority, position,
            time_created, time_updated  (PK = session_id+position)
permission: project_id, time_created, time_updated, data  (JSON, was empty)
event_sequence + event : event-sourcing tables (were empty —
                          events are SSE-only; see L2c-2 recon)
project, workspace, account, session_share, control_account ...
```

Counts in this user's DB: `session=141`, `message=8502`, `part=30576`,
`todo=72`.

### 2.2 The `todo` table (plan card source)

```
session_id  : foreign key
content     : todo text
status      : 'pending' | 'completed' | 'cancelled'  (no 'in_progress' observed,
              but opencode's tool input allows it)
priority    : 'high' | 'medium' | 'low'
position    : ordering within the session
time_created, time_updated : epoch ms
```

This is opencode's *current* plan state — a clean current-state
projection. The Plan card reads this for "what's the agent's plan
right now."

### 2.3 Session row (rich room metadata)

```
title             : opencode's session title (~ ai-title equivalent)
agent             : agent kind ('build' / 'plan' / ...)
model             : JSON {id, providerID} — e.g. {"id":"claude-opus-4.6",
                    "providerID":"github-copilot"}
parent_id         : non-null if this is a sub-agent session
summary_additions : aggregate +lines across all patches in this session
summary_deletions : aggregate -lines
summary_files     : aggregate # files touched
summary_diffs     : JSON array of {file, patch}
revert            : revert state JSON
permission        : per-project permission JSON
time_compacting, time_archived : lifecycle timestamps
```

`parent_id` is the sub-agent link. When opencode spawns a sub-agent
via the `task` tool, the sub-agent runs in its own session row
linked back via `parent_id`. Sub-agent fan-out visualisation is
two joins away.

### 2.4 Message row (turn-level metadata)

`message.data` is JSON with shape:

```
role          : "user" | "assistant"
mode          : "build" | "plan" | ...
agent         : agent kind
parentID      : threading
path          : {cwd, root}
model         : {providerID, modelID}    (assistant only)
time          : {created, completed}
cost          : USD (cents-fraction, 0 when free)
tokens        : {total, input, output, reasoning, cache: {write, read}}
finish        : "tool-calls" | "stop" | ... (assistant only)
summary       : { diffs: [{file, patch}] }  (sometimes; aggregate)
error         : present on failed turns
```

Each assistant turn gets a `finish` reason directly — the opencode
equivalent of Claude's `stop_reason`. `summary.diffs` is sometimes
populated with per-turn patch summaries.

### 2.5 Part row (the per-step events)

`part.data` is JSON with shape `{type, ...type-specific...}`. Per
this user's most active session:

| Part type | Count | Key fields | Use |
|---|---|---|---|
| `step-start` | 920 | `{snapshot}` — git-tree-style content hash | Turn-step boundary; snapshot for diff. |
| `step-finish` | 918 | `{reason, snapshot, tokens, cost}` | Turn-step boundary; carries cost/tokens. |
| `tool` | 898 | full tool call lifecycle (§2.6) | **Activity card** core row. |
| `text` | 544 | `{text}` — assistant text block | Narration. |
| `patch` | 204 | `{hash, files[]}` — snapshot hash + list of paths changed | **Diff card** trigger: "patch at hash H touched these files". |
| `reasoning` | 6 | `{text, time, metadata.copilot.reasoningOpaque}` | Optional "Claude thought for N s" disclosure. |
| `compaction` | 6 | context compaction events | Activity row. |
| `file` | 3 | `{mime, filename}` — user attachment | Activity row. |

### 2.6 Tool part — the lifecycle in a single object

```
{
  type: "tool",
  tool: "edit" | "bash" | "read" | "write" | "grep" | "glob" |
        "todowrite" | "task" | "skill" | "question" | "github_*" |
        "agent" | ...,
  callID: "toolu_vrtx_...",
  state: {
    status: "completed" | "error" | (live: "running" | "pending"),
    input: { ... tool-specific ... },
    output: "<string body>"   // can be huge; truncated flag in metadata
    metadata: {
      truncated: bool,
      // per-tool extras, e.g.
      // edit:    { diff, filediff: {file, patch, additions, deletions}, diagnostics }
      // bash:    { output, exit, description, truncated }
      // task:    { sessionId, model, truncated }
      // question:{ answers: [[choice, ...], ...], truncated }
      // skill:   { name, dir, truncated }
      // todowrite: { todos: [...current full list...], truncated }
    },
    title: "human-friendly label, ready for display",
    time: { start, end },
    error: "string"          // when status=error
    raw: "string"            // raw model payload on parse errors
  },
  metadata: {
    copilot: { reasoningOpaque: "..." }   // sometimes
  }
}
```

Tool-name variation between opencode and Claude:

| Concept | Claude | opencode |
|---|---|---|
| Edit | `Edit` | `edit` (lowercase) |
| Shell | `Bash` | `bash` |
| Read | `Read` | `read` |
| Write | `Write` | `write` |
| Grep | `Grep` | `grep` |
| Glob | (no direct) | `glob` |
| Sub-agent | `Agent` | `task` |
| Ask user | `AskUserQuestion` | `question` |
| Skill | `Skill` | `skill` |
| Todo | (gone — uses `TaskCreate`/`TaskUpdate`) | `todowrite` (single call, full list) |

Input/output key casing also differs (`file_path` vs `filePath`,
`old_string` vs `oldString`). The Live Context layer needs a small
normalizer.

### 2.7 What we already extract today

`harness_events_opencode.rs` subscribes to the HTTP `/event` SSE
stream and emits `OpencodeEvent::ToolUseStart { name }` for
`message.part.updated` rows where `part.type == "tool"`. Same
situation as Claude — name captured, full state ignored.

### 2.8 Other on-disk artefacts

| Path | Format | Use |
|---|---|---|
| `~/.local/share/opencode/snapshot/<hash>/<inner-hash>` | content-addressed; need to confirm format (the inner file is the snapshot blob — likely a git-pack-style or tar). | Look up the worktree state at any `step-start.snapshot` hash. Out of scope for v1 unless we want point-in-time diffs across the session. |
| `~/.local/share/opencode/tool-output/<callID>` | Plain text / HTML body | Overflow for large tool outputs (e.g. WebFetch HTML). Read on-demand if Activity-card row clicks open a detail pane. |
| `~/.local/share/opencode/log/` | Plain text log | opencode's own debug log; not user-facing. |

---

## 3. Side-by-side: what we can extract per harness kind

| Signal | Claude (JSONL) | opencode (DB + SSE) |
|---|---|---|
| Tool call name | `assistant.message.content[].tool_use.name` | `part.tool` (lowercase) |
| Tool input args | `assistant.message.content[].tool_use.input` | `part.state.input` |
| Tool result (success) | `user.toolUseResult` (per-tool shape) | `part.state.output` + `part.state.metadata` |
| Tool result (error) | `user.message.content[].tool_result` with `is_error:true` and `<tool_use_error>` body | `part.state.status == "error"`, `part.state.error`, `part.state.metadata.interrupted` |
| Pre-computed file diff | `Edit.toolUseResult.structuredPatch[]` (hunks) | `edit.state.metadata.filediff.patch` (unified) + `additions`/`deletions` |
| Files touched | derive from patch | `part.type == "patch"` row gives `files[]` directly |
| Plan / todo current state | derive from cumulative `TaskCreate`+`TaskUpdate` calls | read `todo` table |
| Plan change event | `TaskCreate` / `TaskUpdate` tool_use blocks | `tool` part with `tool="todowrite"` (full list each call) |
| Sub-agent spawn | `Agent` tool_use + result with full telemetry | `task` tool — sub-agent runs as own session w/ `parent_id` link |
| Sub-agent output | `Agent.toolUseResult.content` | follow `task.metadata.sessionId` → session row |
| Awaiting user | `assistant.message.stop_reason == end_turn` / `last-prompt` row / `AskUserQuestion` tool_use | `message.data.finish == "stop"` / `question` tool with no `state.metadata.answers` yet |
| User-prompt-while-busy | `queue-operation` row (enqueue) | (need to check SSE — likely a message.queued event) |
| Session title | `ai-title.aiTitle` | `session.title` |
| Recent prompt | `last-prompt.lastPrompt` | last `user`-role `message` |
| "What are we doing" summary | `system.subtype == "away_summary"` (auto on user idle) | (no direct equivalent — could derive from session title + last todo) |
| Per-turn duration | `system.subtype == "turn_duration"` | `message.time.completed - message.time.created` |
| Per-turn tokens / cost | `assistant.message.usage` | `message.tokens` + `message.cost` |
| Permission mode | `permission-mode.permissionMode` | (session `permission` JSON; per-project `permission` row) |
| Model | `assistant.message.model` | `session.model` + `message.modelID` |
| PR opened in session | `pr-link` row | (no direct equivalent; derive from `github_*` tool calls) |
| Slash command | `system.subtype == "local_command"` | (no direct equivalent) |
| Files user edited outside | `attachment.type == "edited_text_file"` | (no direct equivalent) |
| API error / retry | `system.subtype == "api_error"` | `message.error` |
| Context compaction | (none observed) | `part.type == "compaction"` |

The two adapters expose roughly the same surface, but each has
genuinely-unique signals worth keeping:

- Claude-only: `away_summary`, `pr-link`, `queue-operation`,
  `edited_text_file`, `bridge_status`, `local_command`, structured
  `AskUserQuestion.answers`.
- opencode-only: `compaction`, content-addressed `snapshot` per step
  (point-in-time diff is one filesystem read away), `parent_id`
  threading for sub-agents, `summary_additions` /
  `summary_deletions` / `summary_files` pre-aggregated on the
  session.

Lowest common denominator (what we can render uniformly across
*every* harness from day 1):

- Tool call: `{harness, room, tool_name (normalized), tool_input (raw
  JSON), tool_result_summary (string), status (ok / error /
  interrupted), started_at, ended_at, duration_ms, title (optional
  human label), files_touched[] (when derivable)}`
- Plan item: `{harness, room, position, text, status (pending /
  in_progress / completed / cancelled), priority?, updated_at}`
- Patch event: `{harness, room, files[], additions?, deletions?,
  ts}` — for the Diff card auto-focus signal.

Everything else is per-adapter "richer if available" garnish.

---

## 4. Schema implication: `harness_actions` over `harness_events`

The existing `harness_events` table is two enums and a couple of
flags:

```sql
CREATE TABLE harness_events (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    harness_id   TEXT NOT NULL,
    room_id      TEXT NOT NULL,
    from_phase   TEXT NOT NULL,
    to_phase     TEXT NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    has_user_input INTEGER NOT NULL,
    source       TEXT
);
```

A new tool call needs: harness_id, room_id, ts, tool_name,
tool_input (JSON), tool_result (JSON), status, duration_ms,
title, files_touched (JSON array). Plan changes need: harness_id,
room_id, ts, position, text, status, priority. Patch events need:
harness_id, room_id, ts, files (JSON array), additions, deletions.

**Three options:**

**A. Extend `harness_events`** with a JSON `payload` column and a
new `kind` discriminator that tags the row as `phase_transition`,
`tool_call`, `plan_change`, `patch`. Cheap up front; collapses
two genuinely different shapes onto one table; every query needs
a `WHERE kind = ...` filter or it returns garbage; phase-transition
consumers (already shipping) have to learn the new column.

**B. Sibling table per shape** — `harness_events` stays as-is for
phase transitions; add `harness_actions` (tool calls + plan
changes + patches; with a discriminator), or even three siblings
(`harness_tool_calls`, `harness_plans`, `harness_patches`). Clean
schemas; no impact on existing consumers; the Activity card
queries the right table; one extra join when we want a unified
timeline.

**C. Just one big sibling — `harness_actions`** with `kind` +
JSON `payload`. All non-phase signal lives here. One table to
query for the Activity card; the JSON shape varies by `kind`.

**Recommendation: C.** Reasons:

- The phase machine is a tiny, well-defined finite-state ledger.
  Don't touch it.
- The new shapes (tool call, plan change, patch, attachment,
  cross-harness flag, …) will continue to grow. A discriminator
  + JSON payload is the right shape for "many kinds, schema-
  flexible, queried together as a timeline."
- Two tables is the cheapest split that respects the difference;
  three is overkill.
- The unified timeline (Activity card) is one table scan, no
  joins. Adding a new `kind` is zero schema work.

Concrete:

```sql
CREATE TABLE harness_actions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    harness_id   TEXT NOT NULL,
    room_id      TEXT NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    kind         TEXT NOT NULL,   -- 'tool_call' | 'plan_change'
                                  -- | 'patch' | 'attachment'
                                  -- | 'pr_link' | 'queue_op' | ...
    source       TEXT,            -- adapter event id (mirrors L7a)
    payload      TEXT NOT NULL    -- JSON, shape-per-kind
);
CREATE INDEX idx_harness_actions_harness ON harness_actions(harness_id, timestamp_ms);
CREATE INDEX idx_harness_actions_room    ON harness_actions(room_id, timestamp_ms);
CREATE INDEX idx_harness_actions_kind    ON harness_actions(room_id, kind, timestamp_ms);
```

Then the Activity card is `SELECT ... FROM harness_actions WHERE
room_id = ? ORDER BY timestamp_ms DESC LIMIT N`. The Plan card is
`WHERE kind = 'plan_change'` (or read opencode's `todo` table
directly when the harness kind is opencode — projection-vs-event
question we can decide in implementation).

---

## 5. Open questions / things to decide during implementation

1. **Plan card source for opencode**: read the `todo` table (clean
   projection, no rebuild logic) or replay `todowrite` tool calls
   (uniform with Claude). The table is opencode-only; the tool
   calls work for both. Probably read the table directly for
   opencode and replay tool calls for Claude — keep harness-kind
   branches where the harnesses are genuinely different rather
   than forcing one shape on both.
2. **TaskCreate / TaskUpdate as Plan**: Claude's `Task*` tools were
   originally designed for in-session task tracking (the same
   surface as the old `TodoWrite`), but the user can also use them
   for genuinely-background scheduled work via `TaskOutput` /
   `TaskStop`. We should treat all rows the same for the Plan card —
   any TaskCreate/Update is a plan item; the "is it backgrounded"
   distinction can live on the row as a tag.
3. **Tool-name normalization**: a small per-harness map
   (`edit` ↔ `Edit`, `bash` ↔ `Bash`, `question` ↔ `AskUserQuestion`,
   `task` ↔ `Agent`, `todowrite` ↔ (Claude has no equivalent)).
   Doable in the adapter or in the consumer; recommend adapter so
   the persisted `payload` is already normalized.
4. **Snapshot hash → diff** for opencode: the `~/.local/share/
   opencode/snapshot/<hash>/<inner>` files store full worktree
   snapshots. Reading them needs format reverse-engineering (the
   inner file isn't a git object dir — it's a single blob of
   unknown format). Not blocking — we have `metadata.filediff`
   from the edit tool already. Worth a follow-up spike if we
   want point-in-time diffs across the session.
5. **Auto-focus rule for Diff card**: "most recent tool call that
   touched a file." For Claude → look at the latest `Edit` /
   `Write` toolUseResult.filePath (or structuredPatch). For
   opencode → latest `patch` part's first file, or latest
   `edit`/`write` tool with non-empty filediff. Both cases give
   us a clean single-file focus signal.
6. **Sub-agent fan-out**: out of scope for v1 per #80 ("cross-room
   view" is #76, "cross-harness reasoning" is #77). But: Claude's
   `Agent` tool result and opencode's `task` tool sessionId
   linkage give us enough to render a "spawned sub-agent →
   completed in 32s, used 12 tools" row in the Activity card.
   Trivial v1 surface.
7. **Cost / token telemetry**: every assistant turn from either
   harness reports cost + tokens. Not Live-Context-prescribed but
   trivial to capture (one extra `kind = 'turn_cost'` row per
   turn). User decision whether to surface in the UI.
8. **`away_summary` placement**: it's auto-generated when the
   user has been away for a while and is some of the highest-
   value text in the whole log — Claude's own summary of "what
   we're doing." Strong candidate for the room header or as the
   top-pinned row in the Activity card when present.
9. **Plan-card `in_progress` for opencode**: the `todo` table's
   current values are `pending` / `completed` / `cancelled` only.
   No `in_progress` rows in 72 todos. Claude's TaskUpdate, by
   contrast, frequently moves `pending → in_progress → completed`
   (61 TaskUpdate calls in one session, ~half are status flips).
   If we want a unified "now" indicator, opencode may need to
   derive it from "first pending todo after the latest completed
   one" or similar.

---

## 6. What this changes about issue #80

Adjustments to the issue's three-card spec based on what's
actually extractable:

- **Diff card** is unchanged. Both adapters give us a structured
  diff; the file-watcher pipeline is already shipping.
- **Plan card** — rename mental model: it's a **Tasks** card.
  Source for Claude is the `Task*` tool family (not TodoWrite —
  TodoWrite is gone). Source for opencode is the `todo` table
  *and* the `todowrite` tool history. The display shape (done /
  now / next) maps cleanly to both. `by:` chip is per-harness
  (terracotta / sage / ...). Cross-harness ownership of plan
  items (`h1b owns h1a's plan item`) requires cross-harness
  reasoning (#77) and is out of scope.
- **Activity card** is broader than originally scoped — beyond
  tool calls, we can include `pr-link`, `queue-operation`,
  `attachment.edited_text_file`, `system.api_error`,
  `system.turn_duration`, `bridge_status`, AskUserQuestion
  answers, plan-mode entry/exit, slash-command output, cost
  rollups. All of it slots into the same `harness_actions`
  table with different `kind` values. The user can decide later
  what to filter in vs out of the rendered feed.

Implementation order remains as #80 sketched: **A (persist
actions) → B (Activity card UI) → C (Plan card UI) → Diff card
auto-focus refinement.** Schema is now concrete (§4); both
adapter sides have a clear payload shape (§3).
