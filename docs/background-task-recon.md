# Background tasks — recon spike (#439)

Issue #439: a Claude Code harness whose only live work is a background
command (`Bash` with `run_in_background`) or an armed `Monitor` reads
idle or waiting in Skein. That looks the same as a finished or stalled
harness. This recon answers seven questions with evidence, so the fix
can be cut into slices from facts. Nothing here is production code.

**Summary**

- **It is real and reproducible.** A live repro (Q7) ends a turn with two
  120 s tasks running. The transcript shape is reproduced; by code
  reading (`AwaitingPrompt` on `end_turn`, no task awareness) Skein shows
  `waiting` at that point. The Skein phase was not observed directly.
- **It is rarer than #277 but not rare.** 111 of 663 sessions (17%) had
  a turn end with a task outstanding, against #277's 94% of sessions. As
  a secondary figure, 5.6% of main-session turns (423 of 7520).
- **The agent usually comes back on its own.** 72% of affected end of
  turns were next woken by a task notification, and 264 of 301 of those
  by one of the outstanding tasks. The other 28% were woken by the
  human first (of the 420 affected end-of-turns with a next wake-up; 3
  sessions ended first).
- **The transcript has enough to re-derive the live set.** Starts,
  terminal notifications, `TaskStop` results and Monitor deadlines are
  all on disk. The `.output` trailer is a tie-breaker.
- **Recommendation.** Track tasks as their own entity (a
  `backgroundTasks` registry beside `subagents.ts`, re-derived from disk
  in the Rust adapter). Feed the existing #277 deferral through a
  generalised outstanding-work count. Do not add a phase, and do not
  stuff tasks into `subagents.workingCount`. The ceiling policy is per
  kind: Monitor has an exact deadline, Bash gets a bounded one.
- **Claude only today.** opencode has no equivalent (Q6). Keep the
  registry kind-agnostic.

## Method and sample

Read-only mining of 4307 Claude Code transcript files on one Windows
dev machine: 663 main session files and 3644 subagent files. Tasks
dated 2026-09-06 to 2026-09-30, Claude Code 2.1.26x to 2.1.281. Nothing
older had background tasks; transcript retention starts around
2026-09-06. Plus one live repro on 2026-09-30 in a Claude Code 2.1.28x
session (Q7).

Caveats.

- One machine, one user, one Windows OS. Shell tasks here are mostly
  `Bash` and some `PowerShell`. Nothing was sampled from macOS or Linux.
- Counts are over transcripts that still exist. A session whose file was
  pruned is invisible.
- End of turn is the `system` row with subtype `turn_duration`, one per
  turn. Assistant `end_turn` rows are duplicated per content block, so
  they were not used for counting.
- Monitors drop out of the "outstanding" set after `start + timeout_ms`.
- Correlation starts from `toolUseResult` on the start row and pairs
  terminals by task id.

Terms. A **task** is a background shell command or a Monitor. A
background `Agent` (subagent) is **not** a task here: #276 already
covers it.

## Q1. What rows does a background task leave in the transcript?

### Ids

- Shell and Monitor tasks: `b` plus 8 characters (`bXXXXXXXX`).
- Background `Agent` tasks: `a` plus 16 hex characters.
- Agent task notifications share the same `<task-notification>`
  envelope, with an extra `<note>`. A parser must key on id and tool,
  not on the envelope.

### Start: explicit background Bash

Assistant `tool_use` `Bash` (or `PowerShell`):

```json
{"name":"Bash","input":{"command":"<cmd>","description":"<desc>","timeout":120000,"run_in_background":true}}
```

User `tool_result` text:

```text
Command running in background with ID: bXXXXXXXX. Output is being written to: <tmp>\tasks\bXXXXXXXX.output. You will be notified when it completes...
```

with

```json
{"toolUseResult":{"stdout":"","stderr":"","interrupted":false,"isImage":false,"noOutputExpected":false,"backgroundTaskId":"bXXXXXXXX","backgroundCwdHint":"<cwd>"}}
```

`timeout` and `backgroundCwdHint` are optional.

### Start: auto-backgrounded Bash

The input has no `run_in_background`. The result has the same shape, but
`toolUseResult.timedOutAfterMs` is `120000` and the text reads:

```text
Command did not complete within its 120s timeout and was moved to the background (ID: bXXXXXXXX)
```

This is about as common as the explicit form (table in Q2).

### Start: Monitor

```json
{"name":"Monitor","input":{"description":"<desc>","timeout_ms":600000,"command":"<cmd>"}}
```

`persistent` is an optional input; it was always false in the sample.
Result text: `Monitor started (task bXXXXXXXX, expires in 10m unless the source ends first; ...)` with

```json
{"toolUseResult":{"taskId":"bXXXXXXXX","timeoutMs":600000,"persistent":false}}
```

### Monitor, per event

One notification per output line, with no `<status>` and no
`<tool-use-id>`:

```text
<task-notification>
<task-id>bXXXXXXXX</task-id>
<summary>Monitor event: "<desc>"</summary>
<event><line></event>
If this event is something the user would act on now, send a PushNotification...
</task-notification>
```

### Terminal: Bash

```text
<task-notification>
<task-id>bXXXXXXXX</task-id>
<tool-use-id><start tool_use id></tool-use-id>
<output-file><tmp>\tasks\bXXXXXXXX.output</output-file>
<status>completed</status>
<summary>Background command "<desc>" completed (exit code 0)</summary>
</task-notification>
```

`<status>` is one of `completed`, `failed`, `killed`, `stopped`. The
summary forms are `... completed (exit code 0)`, `... failed with exit
code N` (status `failed`) and `... was stopped` (status `killed`). With
no description, the summary quotes the raw command, which may span
several lines. The `<tool-use-id>` equals the start's `tool_use` id.

### Terminal: Monitor

Three forms.

| Form | Status tag | Summary | Event |
| --- | --- | --- | --- |
| Stream ended | `completed` | `Monitor "<d>" stream ended` | the last output line rides on the end notice |
| No output | `completed` | `Monitor "<d>" ended without producing output (exit N)` | none |
| Expiry | none | `Monitor event: "<d>"` | `[Monitor expired after 10m with no events delivered. Re-arm it...]` |

The expiry form has no status tag. It is recognisable only by the event
text, so the parser depends on a string Claude Code may reword. The expiry
text was reconfirmed on 2.1.286 with a longer wording (`[Monitor expired
after 5m with no events delivered. Re-arm it if you still need the watch
— and widen the filter if silence was unexpected.]`). S1 pins both forms
in fixtures.

### TaskStop

```json
{"name":"TaskStop","input":{"task_id":"bXXXXXXXX"}}
```

```json
{"toolUseResult":{"message":"Successfully stopped task: bXXXXXXXX (<desc>)","task_id":"bXXXXXXXX","task_type":"local_bash","command":"<cmd>"}}
```

`task_type` is `local_bash`, `local_agent` or `in_process_teammate`.
`TaskStop` produces **no notification and no terminal row**. `KillShell`
and `KillBash` never appear. `TaskOutput {task_id, block, timeout}`
exists (6 uses) and is not a lifecycle event.

### Session-restart synthetic

When a session is resumed, Claude Code writes its own terminal for every
task the old process left behind, as an ordinary user row:

```text
<status>stopped</status><summary>Background shell command didn't finish before the previous session ended
```

(or `No completion record was found...`), carrying the original
`<tool-use-id>`.

### How one notification is carried

The text is the same in each carrier. A notification always starts as a
`queue-operation` row, and then takes one of two paths.

The enqueue (always; 4124 in main files, no agent attribution):

```json
{"type":"queue-operation","operation":"enqueue","timestamp":"<ts>","sessionId":"<sid>","content":"<task-notification>\n<task-id>bXXXXXXXX</task-id>..."}
```

**Idle path** (1293 rows). `queue-operation` `dequeue` (no content),
then about 30 ms after the enqueue a user row whose content is the raw
string:

```json
{"type":"user","message":{"role":"user","content":"<task-notification>..."},
 "origin":{"kind":"task-notification","producer":"session-task"},
 "turnOrigin":"task_notification","promptSource":"system"}
```

**Mid-turn path** (2774 removes, 2508 attachments). The notification is
absorbed into the running turn:

```json
{"type":"queue-operation","operation":"remove","reason":"absorbed_mid_turn","commandUuid":"<id>"}
{"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification>...",
 "commandMode":"task-notification","origin":{"kind":"task-notification"},"source_uuid":"<id>"}}
```

### Subagents launch tasks too

The start and the delivery (an attachment, or a user row with
`isSidechain: true` and `agentId`) land in
`<sid>/subagents/agent-*.jsonl`. The enqueue lands in the **main** file
with no agent id. Remove reasons there: `delivered_to_agent` (33) and
`agent_stopped` (1). Ownership therefore has to come from the start row.

### On disk

`<tmp>/claude/<encoded-cwd>/<sid>/tasks/<id>.output`. Only `.output`
files exist, with no status or meta file.

- The last line is `[exited with code N]` or `[killed]`. A missing
  trailer means still running, or crashed (unverified).
- `TaskStop` and Monitor expiry both leave `[killed]`.
- Files are never cleaned up (files from 2026-09-07 are still present).
- Agent task `.output` files are empty hard links.
- The path is printed in the Bash start `tool_result` and in the
  terminal `<output-file>`. The directory is under the OS temp dir, so
  it differs per OS (on macOS, under `$TMPDIR`). Read it from the row;
  do not reconstruct it.

## Q2. Can the live set be re-derived from disk, as #276 does?

Yes, with two inputs #276 does not need. See CLAUDE.md "Subagent
awareness (#276)" for the existing model: the adapter re-derives the set
from transcripts on every attach, so `record` is idempotent.

There is no explicit "running" state in a transcript. The live set is:

```text
live = starts
     - terminal notifications
     - TaskStop results
     - Monitors past (start + timeout_ms)
```

Correlation results, starts counted from `toolUseResult`, terminals
deduplicated on enqueue:

| Kind / where | Starts | Completed | Failed | Killed | Stopped (synthetic) | Monitor end / expiry | TaskStop, no row | Orphan, unexplained |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Bash `run_in_background`, main | 235 | 201 | 3 | 6 | 1 | – | 23 | 1 |
| Bash auto-bg 120 s, main | 28 | 14 | 1 | 0 | 4 | – | 9 | 0 |
| Bash `run_in_background`, subagent | 142 | 126 | 14 | 0 | 0 | – | 2 | 0 |
| Bash auto-bg 120 s, subagent | 222 | 146 | 17 | 8 | 1 | – | 12 | 38 |
| Monitor, main | 65 | – | – | 1 | 1 | 30 end, 9 expiry | 21 | 3 |

Start and terminal pair exactly on task id. A notification's
`<tool-use-id>` equals the start's `tool_use` id. Monitor notices carry
none, so Monitors pair on task id only.

Where the non-terminal ones go: 109 starts have no terminal
notification. The `TaskStop` column (23 + 9 + 2 + 12 + 21) accounts for
67 of the 109. The unexplained column (1 + 0 + 0 + 38 + 3) is the other
42. The 38 are the subagent auto-bg row: tasks that outlived their
subagent (7 of their `.output` files end `[killed]`). Of the 3
unexplained Monitor orphans, 2 had passed their expiry with no notice.

Analysis (the author's reasoning, not measured).

- **Extra inputs.** `TaskStop` results and Monitor deadlines. Without
  the first, 47 tasks would read as running forever.
- **Restart.** PTYs die with Skein (CLAUDE.md "PTYs"), so after a Skein
  restart every task outstanding at attach belongs to a dead process. It
  is an orphan by construction. That is exactly the #277
  `SubagentStart.initial` / `fromAttach` situation. Reuse the trick:
  count only starts seen live.
- **Same-process re-attach.** The tail can resume after a transcript
  reappears (#425). There the process may still be alive, so the
  `.output` trailer is the tie-breaker. It is also the only disk record
  of a `TaskStop` that happened while Skein was not tailing.
- **Subagent-owned tasks.** Drop them when their subagent ends (the 38
  orphans), unless the trailer says the task is still going.

## Q3. How often does a turn end with a task outstanding, and how long until the agent returns?

### Frequency

| Measure | Value |
| --- | --- |
| Main-session turns | 7520 over 663 sessions |
| Turns ending with at least one task outstanding | 423 (5.6%) in 111 sessions |
| Affected end of turns, Bash bg / auto-bg / Monitor | 277 / 20 / 140 |
| Sessions affected, Bash bg / auto-bg / Monitor | 94 / 10 / 18 |
| Outstanding per end of turn | median 1, p90 1, max 4 |

The kind rows sum to more than 423 because one turn can end with
several kinds outstanding.

### What woke the session next

| Next wake-up | n | Gap median | p90 | Max |
| --- | --- | --- | --- | --- |
| A task notification | 301 | 89 s | 880 s | 58246 s |
| A human prompt | 119 | 154 s | 1736 s | 190366 s |
| Session ended first | 3 | – | – | – |

Of the 301 notification wake-ups, 264 came from one of the outstanding
tasks and 37 from another notification. The 264 split as 103 + 151 + 10
below.

| Woken by | n | Median | p90 | Max |
| --- | --- | --- | --- | --- |
| A Monitor | 103 | 22 s | 274 s | 1788 s |
| Bash bg | 151 | 215 s | 1189 s | 43734 s |
| Auto-bg Bash | 10 | not measured | – | – |

### How long tasks run (start to terminal)

| Kind / outcome | n | Median | p90 | Max |
| --- | --- | --- | --- | --- |
| Bash bg completed | 327 | 143 s | 914 s | 18013 s |
| Bash bg failed | 17 | 33 s | 541 s | 1367 s |
| Auto-bg completed | 160 | 105 s | 1626 s | 32721 s |
| Monitor stream ended | 30 | 40 s | 488 s | 921 s |
| Monitor expiry | 9 | 1800 s | – | – |

Monitor timeouts seen: 1 to 1800 s. Events per Monitor: median 1, p90 3,
max 29.

### Contrast with #277

From CLAUDE.md "End-of-turn deferral (#277)": subagents affected 94% of
sessions, and the wake-up followed a subagent's terminal row every time
(1009 delegations, 27 ms median, 54.1 s max). Background tasks affect 111
of 663 sessions (17%), and 5.6% of turns. And "the agent will be back" is much less certain. 28% of
affected end of turns were woken by the human first, and gaps are long.
The Bash p90 of about 20 minutes is above #277's 15 minute ceiling.

Two caveats on that comparison.

- **#440.** A subagent that ends with a `SubagentHandback` `tool_use`
  plus its `tool_result` never writes a terminal `stop_reason`, so the
  #276 terminal-`stop_reason` check never sees it finish. Another room
  is fixing that, and this doc proposes no fix. It means #277's measured
  "the wake-up always followed a subagent's terminal row" predates or
  excludes hand-back exits, so treat the contrast above as approximate.
- **Agent notifications.** The same `<task-notification>` envelope also
  fires for background `Agent` tasks: `a` ids,
  `<status>completed</status>`, summary `Agent "<desc>" finished`, and a
  `<note>` saying it can fire more than once if the agent is resumed. A
  notification parser built for S1 would therefore also see subagent
  completions in the main transcript. That is a possible independent
  cross-check for subagent exit. It is not a replacement, because the
  note says it can repeat. An option, not a plan.

### What Skein does today

- `ClaudeEvent` has 10 variants
  (`app/src-tauri/src/harness_events_claude.rs:102-161`): `AssistantTurn`,
  `ToolUseStart`, `ToolUseResult`, `UserPrompt`, `Attachment`,
  `SessionEnd`, `AwaitingPrompt`, `SubagentStart`, `SubagentToolResult`,
  `SubagentEnd`. Nothing parses task ids, `queue-operation`,
  `backgroundTaskId` or Monitor inputs.
- A task-notification user row is emitted as `UserPrompt` (around line
  2852), classified like a typed prompt. Moving the phase to `running` is
  correct. Counting it as a human submit is not.
- `AwaitingPrompt` comes from `stop_reason` `end_turn`, `stop_sequence`
  or `max_tokens` (around lines 2835-2843).
- Subagent re-derivation: `build_adapter` (around 1012), the seeding
  loop (around 1127-1177), `initial: true` (around 1157).
- Frontend: `awaitingPromptFromAdapter`
  (`app/src/harnessActivity.ts:400`) checks `subagents.workingCount`
  (`app/src/subagents.ts:141-147`, which excludes `fromAttach`).
  `DELEGATION_SETTLE_MS` and `DELEGATION_CEILING_MS` are at
  `harnessActivityConstants.ts:68` and `:78`. `statusLabel` is at
  `harnessActivityLabels.ts:47-60`, `delegationSummary` at `:72`.
- The supervisor invariant `ended_turn_not_waiting`
  (`app/src/supervisor/invariants.ts:99-121`) is guarded by
  `subagentsWorking === 0` and `delegationDeferredAt === null` (lines
  108-109).
- `get_room` and `list_harnesses` (`agent_api/verbs.rs`, around 3343 and
  3433) expose only a `phase` string.
- `subagent_end` row: produced at `harness_events_claude.rs` around
  2501-2534, rendered at `app/src/liveContext/rows.tsx:243-257`.

## Q4. Phase, separate entity, or an extension of the subagent count?

**Recommendation:** a separately tracked entity that feeds the existing
#277 deferral through a generalised outstanding-work count.

1. **Same meaning as #277.** The turn ended and the work it launched
   will wake it (264 of 301 notification wake-ups came from the
   outstanding task itself). `waiting` means "your turn", so it is the
   same lie, and the deferral is the right shape.
2. **A new phase ripples.** Every phase consumer would change: badges,
   toasts, OS notifications, supervisor invariants, `canSendPrompt`'s
   `phase === waiting` gate, the agent API. #381's mail and nudge
   delivery during a deferral would have to be redone. The deferral
   inherits all of that.
3. **Tasks are not subagents.** Different identity (a task id, no
   transcript file of its own, no `stop_reason`). Different terminal
   signals (notification, `TaskStop`, Monitor deadline, `.output`
   trailer). Different owner (main or a subagent). Different display
   (command, last event line, deadline). So they get their own registry.
4. **The policy must be per kind.** Monitor has an exact ceiling
   (`start + timeout_ms`). Bash has none and is silent by nature, so
   #277's "15 minutes of subagent silence" ceiling does not transfer.
   Because 28% of affected end of turns were woken by the human first,
   and some tasks never end (servers, tasks stopped later), silence is
   worse than noise. Bash deferral gets a bounded ceiling measured from
   the end of turn. Propose reusing 15 minutes to start, flagged as a
   number to tune (the Bash-woken p90 is about 20 minutes). After the
   ceiling the harness goes `waiting`, and the notification says the
   task is still running, never that it finished. The registry keeps
   showing the task after the ceiling, independent of phase.

| | Distinct phase | Separate entity + deferral (recommended) | Extend `workingCount` |
| --- | --- | --- | --- |
| Honesty of label | Best: a named state | Good: "waiting on N tasks" in place of "waiting" | Poor: "agents" counts a shell command |
| Blast radius | Large: every phase consumer | Small: one count and one registry | Smallest, but it leaks |
| Restart safety | Needs its own `initial` handling | Reuses `fromAttach` per registry | Shares #277's, but mixes kinds |
| Display richness | Rich, but phase-only | Rich: command, event, deadline per task | None: a count, no per-task data |
| Ceiling policy | Free choice | Per kind, per entry | One policy for two unlike things |

## Q5. What should the user see?

Exact wording is for Stefan to agree.

- **Status label.** "waiting on 1 background task", or combined with
  #277: "delegating · 2 agents, 1 task".
- **Popover.** One line per task: kind (bash or monitor), description
  (fallback: the command), elapsed time. For a Monitor, add the last
  event line and the time left to its deadline.
- **Live Context.** One `background_end` `harness_actions` row per
  terminal: description, status (completed, failed, killed, stopped,
  expired), exit code, duration. Model it on `subagent_end`. Monitor
  per-event rows are probably noise (median 1 event, max 29). That is an
  open question.
- **Agent API.** `get_room` and `list_harnesses` later gain a structured
  count field alongside `phase`.

What the transcript provides for this: the description and command on
the start `tool_use`, `timeout_ms`, the event line, the summary text and
the exit code inside the summary.

## Q6. Does opencode have an equivalent?

No, as of v1.18.33 (2026-09-28). The repo is now `anomalyco/opencode`.

- The core `bash` tool (`packages/core/src/tool/bash.ts`) takes
  `command`, `workdir` and `timeout` (default 2 min, max 10 min). It has
  no background parameter. Its source carries TODOs to re-add a
  model-facing background launch with get, wait and cancel tools and
  restart recovery.
- Monitor PRs #33685 and #33806 were auto-closed unmerged.
- A generic `BackgroundJob` service exists and appears to back
  background subagents (the `task` tool). Unverified.
- A maintainer said in #38070 (July 2026) that "v2" would support this.
  It has not shipped.

The fix is Claude-only today. Keep the registry kind-agnostic so an
opencode source could feed it later. opencode registers no subagents
today either (CLAUDE.md #277), and the mechanism keys only on a count.

Sources:

- https://github.com/anomalyco/opencode/blob/dev/packages/core/src/tool/bash.ts
- https://github.com/anomalyco/opencode/pull/33806
- https://github.com/anomalyco/opencode/issues/38070

## Q7. Live repro

2026-09-30, one Claude Code 2.1.28x session. Times are relative to
the first launch.

| Time | Event |
| --- | --- |
| 00:00.0 | `Monitor` running `sleep 120; echo done`, `timeout_ms` 300000. Result at 00:01.9 with `{taskId, timeoutMs:300000, persistent:false}`. |
| 00:00.6 | `Bash` with `run_in_background`, same command. Result at 00:01.9 with `backgroundTaskId`. |
| 00:08.6 | A third Monitor (`echo tick1; sleep 60; echo late`, `timeout_ms` 15000) delivers `tick1`. It arrives mid-turn: enqueue, then remove `absorbed_mid_turn`, then an attachment `queued_command`. |
| 00:09.5 | `TaskStop` on a `sleep 300; echo never` background task. |
| 00:22.7 | Expiry notice `[Monitor expired after 15s with 1 event delivered...]`. No status tag. Also absorbed mid-turn. |
| 00:44.3 | Turn ends (`system` `turn_duration`) with both 120 s tasks running. |
| 02:02.1 | Monitor completion: enqueue, dequeue, then a user row (idle path). |
| 02:02.4 | Bash completion enqueued. Delivered as a user row at 02:04.5, after the previous turn's `turn_duration`. |

The transcript shape of #439 is reproduced. By code reading
(`AwaitingPrompt` on `end_turn`, no task awareness), Skein shows
`waiting` at 00:44.3. The Skein phase was not observed directly. The gap from end of turn to wake-up was about 78 s for a
120 s task. An unrelated subagent hand-back also woke the session
briefly around 00:37, which is why "woken" has to be judged from the
notification, not from any row.

The `TaskStop` result carried `{message, task_id, task_type:"local_bash",
command}`. No notification ever arrived for it, and its `.output` file
held 10 bytes.

Completion on the idle path (Monitor):

```text
<status>completed</status><summary>Monitor "<desc>" stream ended</summary><event>done</event>
```

with `<tool-use-id>` and `<output-file>` after it. The row had
`origin.kind` `task-notification`, `turnOrigin` `task_notification` and
`promptSource` `system`, and it started a new turn.

## Recommendation

1. Add a `backgroundTasks` registry (frontend) and a matching
   re-derivation in the Rust adapter. Mirror `subagents.ts`, including
   `fromAttach`.
2. Generalise #277's deferral to key on outstanding work: subagents
   plus tasks. Keep one deferral, one set of timers, no new phase.
3. Make ceilings per kind. Monitor with a timeout: its own deadline.
   `persistent: true` Monitors (unbounded) get the Bash ceiling, not a
   deadline. Bash: a bounded
   ceiling from the end of turn (15 minutes to start, to be tuned),
   after which the phase goes `waiting` and the notice says the task is
   still running.
4. Show tasks in the status label and popover, and log a
   `background_end` row per terminal.
5. Stop classifying task-notification user rows as a human prompt where
   "a human submitted" matters (the supervisor's `lastSubmitAt`).
6. Claude only for now.

## Proposed slices

Each is a separate issue and PR.

- **S1. Parsers, `crates/skein-harness`.** Pure functions: background
  start (Bash and PowerShell bg, auto-bg, Monitor), the task-notification
  envelope (id, tool-use-id, status, summary, event, expiry recognition),
  the `TaskStop` result, and the `.output` trailer reader. Table tests on
  anonymised fixtures.
- **S2. Adapter, `harness_events_claude.rs`.** Tail main and subagent
  files for these rows. New `ClaudeEvent` variants
  `BackgroundStart { initial }` and `BackgroundEnd`. Attach
  re-derivation with `initial = true` for tasks outstanding at attach.
  Trailer check. Drop subagent-owned tasks on `SubagentEnd`. Emit a
  `background_end` `harness_actions` row. Also review how the
  task-notification `UserPrompt` is classified.
- **S3. Frontend deferral.** `backgroundTasks.ts` registry (mirrors
  `subagents.ts`, with `fromAttach`). Deferral keyed on outstanding work
  (subagents plus tasks). Per-kind ceilings. Update the supervisor guard
  in `invariants.ts` to match.
- **S4. Display.** `statusLabel` and popover wording (agree with
  Stefan), and the `background_end` feed row.
- **S5. Optional.** Structured counts on `get_room` and
  `list_harnesses`.

## Open questions / how to resolve

| Question | How to resolve |
| --- | --- |
| Bash ceiling value (15 min is a guess; Bash-woken p90 is about 20 min) | Ship the constant, then read real deferral outcomes from the `harness_events` log and tune. |
| Do Monitor events belong in the feed? | Decide with Stefan from S4 screenshots; default no (noise). |
| Is the `.output` trailer written on a hard crash? | Answered 2026-09-30: no. Tested on Claude Code 2.1.286 (Windows). The `claude` process was force-killed while a background Bash `sleep 90; echo ...` ran. The `.output` stayed empty with no trailer for over 100 s. The background child survived the kill and ran to completion (its side-effect file was written). Control: when a `-p` session exits cleanly, the task's `.output` gets `[killed]`. So a missing trailer does not mean running. After a crash it can belong to an orphan that is still running, or to one that has finished. That is why S2 seeds such tasks with `initial = true` and trusts a trailer only when one is present. |
| Do `persistent: true` Monitors (unbounded) behave like Bash? | Partly answered 2026-09-30. A persistent Monitor could not be armed headless: the models sent `"persistent":"true"` as a string, and every start result came back `persistent:false`. Still open for a real persistent Monitor. Observed for non-persistent Monitors (2.1.286): a command that exits ends with `<status>completed</status>` and `Monitor "<desc>" stream ended`, with the last line in `<event>`, and the trailer is `[exited with code 0]`. A command that never exits, on expiry, writes the `[Monitor expired after 5m with no events delivered. Re-arm it if you still need the watch — and widen the filter if silence was unexpected.]` event with no status tag in the transcript, and the trailer is `[killed]`. A `claude -p` session stayed alive past its end of turn until the Monitor expired. A Monitor armed when `claude` was hard-killed got no trailer, and `--resume` wrote no synthetic terminal for it. (A background Bash task in the same run did get the "didn't finish before the previous session ended" `stopped` synthetic.) So after a crash only the deadline ends a Monitor. S2's deadline sweep covers that. Until a persistent one is seen, they get the Bash ceiling, not a deadline (as in the Recommendation). |
| Who owns a subagent's task after the subagent ends? | 38 orphans, 7 of them with a `[killed]` trailer. Verify by trailer on a live sample. |
| macOS temp path | Confirm from a Mac transcript's start row. The design reads the path from the row, so it should not matter. |
| Does the expiry text stay stable across Claude Code versions? | It is the only expiry marker. Pin it in an S1 fixture and treat an unrecognised form as "unknown, not running". |
