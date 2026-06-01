# Live Context D2 (Activity card) — build map

Distilled from the D2 mapping pass (a fan-out that cross-checked the
handover spec against the *real* backend payloads and the design
reference). This is the implementation spec for the D2 slices; the
handover (`live-context-handover.md`) remains the design source of
truth, this resolves it against what the extractors actually emit.

## The #1 trap: two-level dispatch

The handover's row-catalogue "kinds" (`edit`, `read`, `bash`,
`task_create`, `todowrite`, `ask`, `agent`, …) are **NOT** backend
`kind` values. The real `harness_actions.kind` values are a smaller
set; the row sub-types are keyed by `payload.tool`:

```
switch (row.kind) {
  tool_call   → sub-classify by payload.tool (Read/Bash/Grep/Glob/Agent/AskUserQuestion/…)
  patch       → payload.tool ∈ {Edit, Write, MultiEdit}   (the file-edit rows)
  plan_change → payload.plan_item.op ∈ {create, update} (Claude → TaskRow)
                                     | "write"            (opencode → TodoWriteRow)
  pr_link / queue_op / edited_text_file / slash_command   → 1:1 simple rows
  away_summary                                            → room subtitle (not a row; D1)
  turn_duration                                           → turn separator (D2d), no own row
  turn_cost                                               → per-turn cost hair-line (D2d), no own row
  api_error                                               → ApiErrorRow (error treatment)
  compaction                                              → CompactRow (opencode-only in practice)
  permission_mode / ai_title / bridge_status / user_prompt → quiet/simple rows
}
```

A naive `switch(row.kind)` using the handover labels matches nothing.
**Plus**: any `tool_call`/`patch`/`plan_change` row with
`payload.is_error === true` short-circuits to `ToolErrorRow` **before**
per-tool dispatch.

## Claude ↔ opencode payload divergence

Every shared kind needs harness-aware extraction. The traps:

| Concern | Claude | opencode |
|---|---|---|
| patch deltas | `patch_info.additions/deletions` derived from `structured_patch` (array of hunks); also `user_modified` | `patch_info.additions/deletions` pre-computed; `patch_info.diff` is a unified-diff **string**, no `structured_patch` |
| tool result | `result` is `toolUseResult` (object — Bash `{stdout,stderr,interrupted}`, Read `{file}`) or string | `result` is `state.output` (plain string) |
| tool-name case | CamelCase (`Edit`, `Bash`, `Grep`, `AskUserQuestion`, `Task`) | lowercase (`edit`, `bash`, `grep`, `question`, `task`) |
| plan_change | `TaskCreate`/`TaskUpdate` → `plan_item.{op,id,subject?,status_change?}` | `todowrite` → `plan_item.{op:"write",count,items:[{content,status,priority}]}` |
| turn cost | `usage.{input_tokens,output_tokens,cache_*}` (flat); **no USD** | `tokens.{total,input,output,cache:{read,write}}` (nested) + `cost` (USD) |
| user_prompt | `prompt` = typed text (~200 char) | `prompt` is **always null** (text lives in part rows) |
| ai_title | nullable; carry-forward timestamp | non-null; SSE-live only |
| turn boundary | `turn_duration` kind (ms + message_count) | **none** — no per-turn duration/boundary signal |

Normalize tool names to lowercase before dispatch *and* display.
Normalize `result` → display string per harness+tool before measuring
length / feeding a preview block.

## Per-kind row table

`displayKind` = the CSS class suffix (`.lc-row.k-<displayKind>`) and
GLYPH key. Gist/right-meta name the exact payload paths.

| backend kind (+tool) | displayKind | glyph / color | gist | right-meta |
|---|---|---|---|---|
| patch (Edit) | `edit` | ✎ ok | `edit <basename(files[0])>` | `+adds −dels` from `patch_info.{additions,deletions}` (gated ≠ null) |
| patch (Write/MultiEdit) | `write` | ✎ ok | `write <file>` | same |
| tool_call (Read) | `read` | ◌ fg-3 | `read <file>` | `<N> ln` — DERIVE (no field): claude `result.file.numLines` or newline-count; opencode newline-count; else omit |
| tool_call (Grep/Glob) | `grep`/`glob` | ⌕ fg-2 | `grep <input.pattern>` (class `arg`) | `<N> matches` — DERIVE from result; else omit |
| tool_call (Bash) | `bash` | $ warn | `bash <title ?? input.command>` + ` · exit 1` if error | `<duration_ms>ms` |
| plan_change (op create/update) | `task` | ◇ accent | create: `+ task <plan_item.subject>`; update: `update <subject?from-plan-lookup or id>` | update: `<status_change.from> → <status_change.to>` |
| plan_change (op write) | `todowrite` | ☰ accent | `todowrite <plan_item.count> todos` | `replaced plan` |
| tool_call (AskUserQuestion/question) | `ask` | ? waiting | `asked <input.question> → <chosen from result>` | `user chose` |
| tool_call (Task/agent) | `agent` | ✦ waiting | `sub-agent <input.description/title>` (clickable→D4) | `<duration_ms>` |
| pr_link | `pr` | ⤴ ok | `opened PR #<pr_number>` (**drop title** — no field) | `<pr_repository>` |
| queue_op | `queue` | ⏵ fg-2 | `queue "<content>"` | `queued` |
| edited_text_file | `userfile` | ✋ warn | `noticed <basename(filename)> edited outside` | `user edited`; preview = `snippet` |
| slash_command | `slash` | / accent | `slash /<name>` — PARSE name from `content` (`<command-name>` tags) | omit char-count (no `output` field) |
| compaction | `compact` | ⤓ fg-2 | `compacted context` (+ `auto`/manual) | **drop** `40k→12k` (no token-delta field) |
| turn_cost | `cost` | $ fg-3 | (D2d cost hair-line, not a row) | tokens · $ (opencode only) · ms (claude only) |
| api_error | `error` | ✕ err | `api error <error.status>` | `attempt <retry_attempt>`; preview countdown from `retry_in_ms` |
| (any tool w/ is_error) | `error` | ✕ err | `<tool> <message>` | — |
| user_prompt | `user` | › accent | `user <prompt>` (opencode null → placeholder) | — |
| permission_mode | `perm-mode` | ⏵ waiting | `permission mode` | `→ <permission_mode>` (**new value only** — no prior to make from→to) |
| ai_title | `title` | ❝ fg-3 | `title "<ai_title>"` (italic) | `harness titled` |
| bridge_status | `bridge` | ⇄ fg-2 | `bridge <bridge_session_id>` (**flat notice** — no up/down status field) | — |
| burst (rendering-only) | `burst` | ▸ fg-0 | `<tool> ×<N> <scope>` (frontend-folded) | cumulative `+N −N` |

## Resolved v1 decisions (user-confirmed)

- **Missing-data fields** (PR title, compaction tokens, bridge up/down,
  opencode prompt text, Claude turn-cost $, permission-mode transition):
  **omit / best-effort, frontend-only.** No backend changes; file
  follow-ups if wanted.
- **PermissionRow** (passive "agent paused · jump to harness ↗"):
  **deferred.** No backing `harness_actions` event exists; would need a
  live harness-permission signal. Out of D2.
- **Per-turn cost rows:** **off by default**, revealed by a pref/Tweak.
- **Slicing:** granular (D2a–D2g).

## Slice plan

- **D2a** — Row grid + chip resolution + dispatcher + the simple
  single-shape rows (user_prompt, ai_title, permission_mode,
  bridge_status, pr_link, queue_op, edited_text_file, slash_command) +
  the full net-new CSS. Tool family renders via a minimal generic
  fallback (correct glyph/color) pending D2b. Threads `harnesses`
  App→LiveContext→ActivityCard.
- **D2b** — the tool_call/patch/plan_change family with harness
  normalization: EditRow, ReadRow, SearchRow, BashRow, TaskRow,
  TodoWriteRow, AskRow, AgentRow (+ onOpen seam to D4), and the
  `is_error → ToolErrorRow` short-circuit + ApiErrorRow.
- **D2c** — tool-result expansion (>200-char preview blocks + size
  pill), wired onto Bash/Read/Ask/Todo.
- **D2d** — flattened-item layer: turn separators (from `turn_duration`,
  Claude only), per-turn cost hair-lines (toggle, off by default),
  backfill banner + end-divider (needs store provenance), slide-in vs
  snap.
- **D2e** — burst collapse (runtime fold of consecutive same-tool
  same-dir patch rows, gap < 5 s; expand/collapse; live shimmer).
- **D2f** — auto-tail + jump-to-latest pill + graduated error toast
  (needs active-room focus signal from App).
- **D2g** — virtualization (variable-height; reconcile with auto-tail).
  Deferred until dogfooding shows the ~5 k-row frame janks.

## Standing risks (carry into every slice)

- Dispatch must be two-level (kind → normalized tool); the handover
  labels are not kinds.
- Every shared kind needs a Claude/opencode branch or silent blank
  fields result.
- `timestampMs` can be 0 (timestamp-less Claude rows; some opencode
  live rows) — guard `formatClock(0)` and turn-start tracking.
- Auto-tail unseen-counter must count the **flattened rendered-item**
  array, not raw `actions.length` (bursts collapse N→1; separators/cost
  add items).
- opencode emits `turn_cost` per step-finish (many/turn) vs Claude once
  per terminal turn — aggregate opencode costs per turn or the feed
  spams.
- Glyph/color collisions (`⏵` queue/perm/perm-mode; `$` bash/cost) are
  resolved only by the per-kind `k-*` CSS class — the `is_error → k-error`
  short-circuit must override the per-tool class.

## Backend follow-ups (tracked: #91)

The "omit / best-effort, frontend-only" decision leaves several payload
fields the design wants but the extractors don't emit. These are
captured in **issue #91** (backend extractor enrichments) so the
frontend's placeholders can be upgraded later without re-deriving the
gaps: slash-command name, opencode `user_prompt` text, `bridge_status`
connection state, `compaction` token delta, `pr_link` title, Claude
`turn_cost` USD, and the PermissionRow backing signal.
