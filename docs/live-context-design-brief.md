# Live Context — design brief

> **Design pass is complete.** For *what to build*, read
> [`docs/live-context-handover.md`](./live-context-handover.md) —
> it answers the 10 open UX questions in §6 and is authoritative
> for the v1 implementation. This brief is the input the designer
> consumed; keep it for the data inventory and the why behind the
> backend decisions, but the renderer should follow the handover.

Companion to the recon (`docs/live-context-recon.md`) and the
re-scoped issue (#80). Recon decided **what data we can extract**;
this doc translates that into **what a designer can mock** without
having to read either.

Backend work is starting from the decisions captured at the end of
this doc. The UI is open: this brief is for the design iteration.

---

## 1. What "Live Context" is, in one paragraph

The right pane of a room becomes a vertical stack of three
collapsible cards: **Diff** (top, biggest, "what's being changed
right now"), **Plan** (middle, "what every agent in the room is
trying to do, and where they are"), **Activity** (bottom, "what
each agent just did"). Per-room, not per-harness — multiple
harnesses in the same room contribute to all three cards, visually
distinguished by their chip colour.

The cards replace the current `Status` / `Files` / `Activity`
tabbed pane. The previous "Activity" tab (state-transition feed)
was the wrong substance and has been closed (PR #79).

---

## 2. The data we'll have on day 1

After backend ships, every Claude and opencode harness in a room
will persist these signals to the local SQLite DB. The designer can
assume **all of them are available**, live-updating, per-room,
per-harness, with absolute timestamps.

### 2.1 The lowest common denominator (works for every harness)

| Signal | When it lands | What's in it | Card |
|---|---|---|---|
| **Tool call** | Each time an agent calls a tool | tool name (normalized), input args (JSON), result (JSON or string), success / error / interrupted, start/end ms, optional human-readable title, optional list of files touched | Activity (every row); Diff (triggers auto-focus when the tool edits a file) |
| **Plan change** | Each TaskCreate/TaskUpdate (Claude) or todowrite (opencode) | item text, position, status (pending / in_progress / completed / cancelled), priority (opencode only) | Plan (state); Activity (change row) |
| **Patch** | When a tool edits/writes a file | list of files touched, optional `+lines` / `-lines`, optional structured diff (git-style hunks) | Diff (content + auto-focus); Activity (compact summary row) |
| **Turn cost** | End of each assistant turn | tokens (in/out/cache), USD, duration ms, model id | Activity (optional collapsible row); session footer rollup later |
| **Turn duration** | End of each turn | duration ms, message count | Activity |
| **API error / retry** | Network or API failure | HTTP status, retry-in ms, attempt number, error message | Activity (highlighted, retry countdown) |

### 2.2 Claude-only signals

These will only show up on Claude harnesses (opencode has no
direct equivalent). All trivially extracted; all going in:

| Signal | What's in it | Card / use |
|---|---|---|
| **Away summary** | Auto-generated paragraph: "We're working on epic #50 L2c-1, implementation done, next is dogfooding" — written when Claude detects the user has been away | Activity (top-pinned when present?); or as a room subtitle. **High value — Claude's own "what are we doing" text.** |
| **PR link** | PR number + URL + repository | Activity (pinned row: "session opened PR #61") |
| **Queue op** | "User typed a prompt while busy" — the queued text | Activity (distinct row glyph) |
| **User edited file outside Claude** | filename + snippet of the diff | Activity (row: "user edited `<file>` outside Claude") |
| **Slash command** | command name + output | Activity (compact row) |
| **AskUserQuestion answers** | the question + which option user picked | Activity (one row per question answered) |

### 2.3 opencode-only signals

| Signal | What's in it | Card / use |
|---|---|---|
| **Context compaction** | tokens before/after, snapshot hash | Activity (row: "context compacted — 40k → 12k tokens") |
| **Session-level diff summary** | total +lines / -lines / files across the whole session | Diff card header? Or session footer? |
| **Per-step snapshot hash** | git-tree-style hash of the worktree at each step | Out of scope v1 — points-in-time worktree diffs are a follow-up |

---

## 3. What a tool-call row actually contains (per tool, with sample data)

Designers shouldn't have to guess what an "Edit" row vs a "Bash" row
vs a "TaskCreate" row looks like. Here are real examples from
recon, normalized into the schema we'll persist:

```
tool_name: "edit",   harness: claude
input:  { file_path: "/Users/.../Cargo.toml", old_string: "...", new_string: "..." }
result: { files: ["Cargo.toml"], additions: 3, deletions: 0, ok: true,
          structured_patch: [ {oldStart:57, newStart:57, lines:[" [target...", "+[dev-dependencies]", ...]} ],
          user_modified: false }
title:  null (Claude doesn't supply one; UI can synthesize "Cargo.toml")
ms:     start=..., end=..., duration_ms=12
```

```
tool_name: "bash",   harness: claude
input:  { command: "gh issue view 50", description: "View issue #50 epic details" }
result: { stdout: "title: ...\nstate: OPEN\n...", stderr: "", exit_ok: true, interrupted: false }
title:  "View issue #50 epic details"
```

```
tool_name: "task_create",   harness: claude
input:  { subject: "Build harness_events_claude.rs adapter + tests",
          description: "...", active_form: "Building Claude JSONL adapter" }
result: { task_id: "1", subject: "Build harness_events_claude.rs adapter + tests" }
```

```
tool_name: "task_update",   harness: claude
input:  { task_id: "1", status: "in_progress" }
result: { ok: true, updated_fields: ["status"], status_change: {from: "pending", to: "in_progress"} }
```

```
tool_name: "ask_user_question",   harness: claude
input:  { questions: [{ question: "Release version?", options: [
          {label: "v0.1.7", description: "..."}, {label: "v0.2.0", description: "..."} ]}] }
result: { answers: { "Release version?": "v0.1.7" } }
title:  "Asked: Release version?"
```

```
tool_name: "agent",   harness: claude   (sub-agent invocation)
input:  { description: "Research xterm.js bug", subagent_type: "general-purpose", prompt: "...long..." }
result: { agent_id: "agent_abc", status: "completed", content: "report text...",
          duration_ms: 49712, total_tokens: 1234, total_tool_uses: 8 }
title:  "general-purpose: Research xterm.js bug"
```

```
tool_name: "edit",   harness: opencode
input:  { filePath: "/Users/.../lib.rs", oldString: "...", newString: "..." }
result: { ok: true, files: ["lib.rs"], additions: 1, deletions: 0,
          unified_diff: "Index: .../lib.rs\n@@ -19,8 +19,9 @@\n...",
          interrupted: false }
title:  "crates/dorc-agent/src/lib.rs"   (opencode supplies a title)
```

```
tool_name: "todowrite",   harness: opencode  (single call, full list)
input:  { todos: [
            { content: "Backend: ReminderScheduler", status: "pending", priority: "high" },
            { content: "Frontend: Toast", status: "pending", priority: "high" },
            ... 8 items ...
          ] }
result: { count: 8 }
title:  "8 todos"
```

```
tool_name: "bash",   harness: opencode  (error / interrupted)
input:  { command: "...", description: "..." }
result: { ok: false, error: "Tool execution aborted", interrupted: true }
```

---

## 4. Volume + timing — what the cards see in practice

Numbers from real recon sessions (one Claude session ≈ 5MB JSONL,
one opencode session ≈ 30k DB parts):

- **Tool calls per active session**: 619 (Claude, ~7-hour session)
  / 898 (opencode, similar duration). Roughly **1-2 per minute**
  averaged, with bursts of 10+ in rapid sequence during heavy
  work.
- **Plan items per session**: typically 5-15 active at any time;
  30-60 total over the session lifetime.
- **Patch events per session**: ~150-200, often clustered when an
  agent is in implementation mode.
- **Away summaries per session**: 26 over a multi-day session —
  ~one per resumed work block.
- **PR links per session**: 1-3 typically (one per PR created).
- **API errors per session**: 0-20, almost always bursty (529s
  during Anthropic capacity blips).

The Activity card therefore needs to gracefully handle:
- Long quiet stretches (nothing new for an hour)
- Burst storms (20+ rows in 30 seconds during a refactor)
- A session-lifetime backlog of thousands of rows (we keep
  everything forever per the schema decision)

---

## 5. Cross-harness display vocabulary (already in CSS)

For when multiple harnesses are in the same room:

- Claude: terracotta (`--accent: #c96442`)
- opencode: sage
- Copilot: slate-blue
- BYOH: ochre
- Chip component: `HChip` in `app/src/components.tsx`

The original design (`docs/design/skein/chats/chat1.md`) uses the
`↔` glyph for cross-harness events ("h1b flagged X in h1a's
diff"). v1 doesn't generate cross-harness events from any agent
(see §7); it can render them when they appear in the action
stream, but won't synthesize them.

---

## 6. UX questions the design should answer

These were explicitly deferred during backend planning because
they're rendering-shaped, not data-shaped:

1. **Auto-focus rule for the Diff card.** "Latest file-touching
   tool call wins" — but: latest from any harness, or latest from
   the harness whose chip is currently emphasized? Sticky on
   user click vs always auto-follow?
2. **Plan card grouping.** Currently every harness's plan items
   would interleave. Group by harness (one sub-list per chip)? By
   status? By priority?
3. **Plan card status mapping for opencode.** opencode's `todo`
   table only has `pending` / `completed` / `cancelled` — no
   `in_progress`. Show "now" as the first pending after the
   latest completed? Or live without an in-progress indicator on
   opencode rows?
4. **`away_summary` placement.** It's a paragraph of "what are we
   doing" text, ~50-200 chars, auto-generated by Claude. Goes
   where? Top of Activity card as a pinned row? Room header?
   Subtitle under the tab strip?
5. **Activity card density.** With 619 rows per session and bursts
   of 10+/30s, do we collapse consecutive same-tool rows (e.g.
   "Edit ×12 in `app/src/`")? Or render each? Or auto-collapse
   after N minutes of inactivity?
6. **Sub-agent rows.** Claude's `Agent` and opencode's `task`
   spawn sub-agents that themselves run for minutes. Render as
   a single collapsible row with a child-feed inside? Or as a
   start row + finish row with their tool calls inline?
7. **Tool result rendering.** Some results are tiny ("ok: true").
   Some are KB-sized strings (Bash stdout). Some are images. Some
   are structured (the AskUserQuestion answers map). Hover/expand
   pattern? Always-collapsed by default?
8. **Card resize / collapse.** Designer's call: keyboard
   shortcuts, drag-to-resize, save-per-room layout, ...?
9. **Error rows.** API 529, Bash exit code 1, Edit
   ambiguous-match — different error classes, same visual
   treatment or distinct?
10. **Cost rollup.** We capture per-turn token+USD. Show in the
    Activity card per turn, or as a session footer total, or
    both?

---

## 7. Explicitly NOT in v1 (already deferred)

- **Editing plan items from the right pane** — display only.
- **Cross-harness reasoning** (h1b *generates* a flag on h1a) —
  filed as #77. v1 just renders any flag that appears in the
  action stream; nothing generates them.
- **Cross-room view** ("all rooms in one stack") — filed as #76.
- **Click-to-jump from an Activity row into source location** —
  nice-to-have follow-up.
- **Point-in-time worktree diff at any past snapshot hash** —
  opencode-only and would require reverse-engineering their
  snapshot format. Follow-up spike.

---

## 8. What backend is starting on (so the designer knows the
   constraints they can rely on)

A new table `harness_actions(id, harness_id, room_id, timestamp_ms,
kind, source, payload)` populated by the existing Claude/opencode
adapters. Fifteen `kind` values capture everything in §2:

```
tool_call, plan_change, patch,
pr_link, queue_op, edited_text_file, slash_command,
away_summary, turn_duration, api_error, turn_cost,
permission_mode, ai_title, bridge_status, user_prompt
```

The last four were added during Part B implementation — they're
all cheap one-row→one-action extractions from Claude row types
not originally surfaced as v1 kinds. The principle: err on
capturing more signal, not less. (`file-history-snapshot` and
`summary` remain excluded; the former is internal Claude undo
bookkeeping with no user-facing event, the latter was never
observed in any recon session.)

Behaviour:

- **Persisted forever** (no auto-pruning; future feature if
  needed).
- **Full payload inline** in the DB (JSON column — no sidecar
  files, no truncation; SQLite handles the volumes).
- **Backfilled on attach** — when Skein opens a room, it scans
  existing Claude JSONL / opencode parts and replays history into
  `harness_actions`, then tails forward.
- **Source attribution** — every row knows which adapter event
  emitted it (mirrors L7a).
- **Live-update channel** — frontend gets notified when new rows
  land (mechanism TBD during impl, but assume "subscribe per
  room, receive rows as they're written").

Tool names normalized at the adapter boundary so the UI sees the
same name regardless of harness:

| Canonical (UI sees) | Claude | opencode |
|---|---|---|
| `edit` | `Edit` | `edit` |
| `write` | `Write` | `write` |
| `read` | `Read` | `read` |
| `bash` | `Bash` | `bash` |
| `grep` | `Grep` | `grep` |
| `glob` | (n/a) | `glob` |
| `task_create` | `TaskCreate` | (n/a — uses `todowrite`) |
| `task_update` | `TaskUpdate` | (n/a) |
| `todowrite` | (n/a) | `todowrite` |
| `agent` | `Agent` | `task` |
| `ask_user_question` | `AskUserQuestion` | `question` |
| `skill` | `Skill` | `skill` |
| `tool_search` | `ToolSearch` | (n/a) |

Input/output field names also normalized to snake_case to align
the per-harness shapes.

---

## 9. Reference

- Recon details: `docs/live-context-recon.md`
- Issue: #80
- Original prototype: `docs/design/skein/project/skein-proto.jsx`
  (ActivityFeed @ line 641, ContextStack @ line 696)
- Design source: `docs/design/skein/chats/chat1.md` (search
  "live context", "Layout B", "context stack", "flagged")
- Closed in favor of #80: #74, PR #79
