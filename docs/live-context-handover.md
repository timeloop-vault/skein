# Live Context — implementation handover

Companion to:
- `uploads/live-context-design-brief.md` (data, recon, decisions backend made)
- `Live Context.html` (design canvas — every row kind, every state)
- `Live Context Prototype.html` (interactive demo — live tailing, drag-resize, sub-agent inspector, history-on-attach)
- `live-context-rows.jsx` (row taxonomy — the canonical row component set)
- `live-context-cards.jsx` (Diff / Plan / Activity card chrome)
- `lc-proto.jsx` (the interactive prototype's app + tape)

This document tells Claude Code **what to build**. It does not re-derive
**why** — the brief did that. Where this doc and the brief conflict, this
doc wins (it's the post-design follow-up).

---

## 1 · Goal

Replace the room's current right pane (`Status` / `Files` / `Activity`
tabbed; PR #79 closed) with a vertical stack of three resizable cards:
**Diff**, **Plan**, **Activity**. Per-room, not per-harness. Updates
live as the backend writes new rows to `harness_actions`. Renders all
15 backend `kind` values plus burst-collapse and turn-grouping that the
backend does *not* materialise (those are pure rendering).

## 2 · Definition of done

Ship when all of the following are true for the Live Context pane in a
real Skein room:

1. The three cards render in order Diff / Plan / Activity, full pane
   height, divider-resizable, sizes persist per room via the existing
   per-room settings store.
2. The pane subscribes to `harness_actions` for the active room and
   the cards tail forward as new rows land.
3. Every `kind` listed in §6 of this doc renders with the specified
   row treatment.
4. Backfill on attach (5 MB Claude JSONL / 30 k opencode parts in
   recon-scale rooms) lands as one batch with the "backfilled" banner
   and *no* slide-in animations, then live tailing resumes underneath.
5. The five state variations in §10 render correctly (empty, long
   quiet, burst storm in progress, errored, permission as a row).
6. Clicking a sub-agent row opens the inspector sheet; pressing `Esc`
   or clicking the scrim dismisses it.
7. Clicking a burst row expands it to its constituent rows; clicking
   "collapse" re-folds it.
8. The diff card auto-focuses the focused harness's latest edit (see
   §5.1). Other-harness edits on different files appear as a thin
   pulsing accent line on their tab and never steal focus.
9. The Activity card auto-scrolls to the bottom when the user is
   within 30 px of the bottom; if the user has scrolled up, a "▼ N
   new" pill appears bottom-right, click to resume tailing.
10. The room subtitle bar shows the latest Claude `away_summary` (if
    any) or "Idle — created N ago" when empty.
11. Dark and light themes both render legibly with the tokens in §3.

## 3 · Design system (tokens, type, colour)

All colour decisions are captured in CSS custom properties. Lift these
directly from `Live Context.html` and `Live Context Prototype.html` —
the values are identical. Both files declare them on a `.lc` ancestor
so they're scoped (don't leak to the rest of Skein's CSS).

### 3.1 Tokens (dark)

```
--bg-0      #0d0e10   app shell
--bg-1      #131418   pane
--bg-2      #181a1f   elevated
--bg-3      #20232a   hover
--bg-card   #14161b   card surface
--line      rgba(255,255,255,0.06)
--line-strong rgba(255,255,255,0.12)
--fg-0      #e8e6df   primary
--fg-1      #b6b3aa   secondary
--fg-2      #7a786f   tertiary
--fg-3      #4a4944   placeholder / dim
--accent    #c96442   warm terracotta — single accent
--ok        #7aa37f   success / additions
--warn      #d4a657   bash / warn
--err       #c97163   error / deletions
--waiting   #8a96c9   waiting / permission / inferred
--diff-add  rgba(122,163,127,0.18)
--diff-del  rgba(201,113,99,0.16)
```

Light theme values are in the prototype CSS. The token names are the
same; only the values change.

### 3.2 Harness colour assignments

```
claude    #c96442  (same hue as --accent, used only inside the chip)
opencode  #5b8a72  sage
copilot   #4a6b9a  slate-blue
byoh      #8a7a4a  ochre
```

These are the *only* places harness colour appears: the chip, the
diff-tab flicker accent, the per-row left-edge tint when relevant, and
the activity row's `by` column. Nowhere else.

### 3.3 Type

- Mono everywhere in the right pane: **JetBrains Mono**, 400/500/600/700.
- Sans only for body text inside the sub-agent inspector's final
  report block: **Inter**, 400/500/600.
- No icons. No emoji. Glyph characters from the JetBrains Mono set
  only (✎ ◌ ⌕ $ ◇ ☰ ? ✦ ⤴ ✕ ⏵ ✋ / ⤓ ▸ ⇄ ❝ ›). See §6 for the
  per-kind assignments.

## 4 · The room subtitle

A thin always-visible bar between the room's top tab strip and the
right pane. Single line. Hosts the most recent Claude `away_summary`
verbatim; ellipses if longer than the bar's width.

Layout: `[AT] · summary text · 2m ago · Claude`

- `AT` glyph in accent, mono caps, 10 px, 0.04em letter-spacing.
- Text in `--fg-1`, 11.5 px.
- Right-aligned age + author in `--fg-3`, mono 9.5 px.
- Empty state: glyph `IDLE`, text "No agent has worked here yet" in
  italic `--fg-3`.

The subtitle is **read-only**. Clicking it does nothing in v1.

## 5 · The card stack

Three cards stacked vertically with 6 px drag dividers between. Each
card has:

- A 30 px head with chevron, all-caps mono label, and right-aligned
  meta. Click the head to collapse; collapsed cards take only the head
  height.
- A scroll-only body.
- Equal flex weight by default. Drag a divider to redistribute.
  Persist per room (key by room ID, JSON-encode `[diff, plan, activity]`
  flex weights to the existing room-settings store).

### 5.1 Diff card

Header: `Diff` · `● auto-follow · focused: claude` (the harness name
mirrors the active tab; `●` is a small ok-coloured pulse dot to signal
the card is live-following).

Body has two sub-regions:

**Tab bar (28 px).** Horizontal scroll if needed. Each tab is
`[chip] file.ext [+N] [−N]`, mono 10.5 px. Active tab has an accent
underline. Tabs from another harness on a non-focused file get a thin
pulsing accent line at the top of the tab (`@keyframes lc-flicker`,
1.4 s). Click any tab to switch focus; this clears that tab's flicker.

**Diff body.** Monaco-rendered unified diff of the active tab's file
in the worktree. The prototype draws raw `<div>` lines for demo
purposes — use Monaco in production. Body has a 700 ms accent-glow
animation (`@keyframes lc-refocus`) whenever auto-focus jumps to a new
file, so the user notices the swap.

**Auto-focus rule.** The Diff card follows the *focused harness's*
latest file-touching tool call. The focused harness is whatever harness
the user is currently chatting with in the harness pane (Skein
already tracks this). When a non-focused harness edits a *different*
file, that file appears as a new (or existing) tab with the flicker
animation — never as the active diff. When a non-focused harness
edits the *same* file the active diff is showing, no flicker is needed
(the diff naturally updates).

**Sticky / jump-to-latest.** If the user clicks anywhere in the diff
body, treat as "sticky": stop auto-focusing for ~30 s. While sticky,
new edits update the tab `+N/−N` but don't switch the active tab. A
small "→ latest" pill in the upper-right of the body offers to resume
following. Hour-glassing this for now in the prototype — implement
post-v1 if it ever feels needed; ship without it first.

### 5.2 Plan card

Header: `Plan · 2 now · 4/8`

- `N now` counts items across all harnesses with `status in_progress`
  (Claude) or inferred-now (opencode).
- `4/8` is `done / total`.

Body is one **sub-list per harness**. Each sub-list has:
- A small group head: `[chip] Harness Name · count`.
- Plan rows in the order received from the backend.

Row layout: `[box] text [priority pill]`

- `box` is the status indicator: empty square (pending/next), `◆` in
  accent (now), `✓` filled in `--ok` (done), `×` filled in `--fg-3`
  (cancelled).
- Row colour by status: `--fg-1` (pending), `--fg-0` bold (now),
  `--fg-3` with strike-through (done), `--fg-3` strike-through (cancelled).
- Priority pill (opencode only): `high` (err background), `med` (warn
  background). No pill if absent. Render exactly what the backend
  emits — don't synthesize.
- **Inferred opencode "now"**: opencode's todo state has only
  `pending / completed / cancelled`. Synthesize "now" as the first
  `pending` row that follows the most recent `completed` row within
  that harness's sub-list. Render with `font-style: italic` on the
  trailing `· inferred` annotation (CSS handles this — class
  `lc-plan-row.now.inferred`). Don't render the annotation for
  Claude-driven rows even if their state is `in_progress`.

Empty state: dim mono "no plan items yet — agents will populate this
as they work" with a small `·` glyph.

### 5.3 Activity card

Header: `Activity · 619 events · $1.42 · 18.4k tok`

- Event count is `harness_actions` row count for the room.
- Cost is the session-total sum of `turn_cost.usd` so far.
- Tokens is the sum of `turn_cost.tokens` so far.
- Right of those, a 6 px pulse dot in `--ok` when tailing, `--fg-3`
  when paused.

Body renders, in DB order:

1. **Backfill banner** (if currently in a backfilled view; see §11).
2. **Backfill end marker** (matching the banner; only when there are
   live events after backfill).
3. **Turn separators** — a 1 px hair-line with `turn · 14:01:54` on the
   left and `54s` (turn duration) on the right when the turn ends.
   Emit one when the backend has a row with `kind = turn_duration`.
4. **Activity rows** — see §6 for the full catalogue.
5. **Per-turn cost rows** — rendered as a small hair-line under the
   turn separator (mono 9.5 px, `--fg-3`). User-toggleable; off by
   default in compact mode (see Tweaks). Backend gives us
   `turn_cost.tokens`, `turn_cost.usd`, `turn_duration.ms` — combine.
6. **Tail line** — a "tailing — new rows slide in" sentinel at the
   bottom with a pulsing dot. Becomes "idle" with a static dot when
   the room has been silent for N seconds (N = 90 s suggestion).

**Row animation.** Newly-arrived rows slide in from below (8 px → 0,
opacity 0 → 1, 320 ms ease-out — `@keyframes lc-slide-in`). Backfilled
rows do *not* animate (they snap in).

**Auto-tail behaviour.** If the user is within 30 px of the bottom of
the activity card body when a new row lands, scroll to keep the new
row visible. If they've scrolled up further, do not scroll; instead
show a floating accent pill `▼ N new` in the bottom-right of the
activity body, click to jump to live and resume tailing.

## 6 · Row catalogue

Every backend `kind` has a row component. Names below match what's
exported from `live-context-rows.jsx`. The reference render of all
rows is in `Live Context.html` § "Activity · every row kind".

Common row layout: `[time] [chip] [glyph] [gist] [right-meta]`

- `time` is `--fg-3` mono 9.5 px (absolute clock, "14:02:18").
- `chip` is a two-letter harness chip (12 px).
- `glyph` is one mono character coloured per row kind.
- `gist` carries the row's main content; the only place colour mixing
  happens at all.
- `right-meta` is `--fg-3` mono 9.5 px, right-aligned.

Per kind:

| kind | glyph | component | gist | right-meta |
|---|---|---|---|---|
| `tool_call · edit` / `· write` | `✎` (ok) | `EditRow` | `edit <file>` | `+N −N` |
| `tool_call · read` | `◌` (fg-3) | `ReadRow` | `read <file>` | `<N> ln` |
| `tool_call · grep` / `· glob` | `⌕` (fg-2) | `SearchRow` | `grep "<pattern>"` | `<N> matches` |
| `tool_call · bash` | `$` (warn) | `BashRow` | `bash <title or command>` | `<ms>` |
| `tool_call · task_create` / `· task_update` | `◇` (accent) | `TaskRow` | `+ task <text>` / `update <text>` | `pending → in_progress` |
| `tool_call · todowrite` | `☰` (accent) | `TodoWriteRow` | `todowrite <N> todos` | `replaced plan` |
| `tool_call · ask_user_question` | `?` (waiting) | `AskRow` | `asked <Q> → <A>` | `user chose` |
| `tool_call · agent` / `· task` (sub-agent) | `✦` (waiting) | `AgentRow` | `sub-agent <title>` | `<ms>` |
| `pr_link` | `⤴` (ok) | `PrRow` | `opened PR #<N> — <title>` | `<repo>` |
| `queue_op` | `⏵` (fg-2) | `QueueRow` | `queue "<text>"` | `queued` |
| `edited_text_file` | `✋` (warn) | `UserFileRow` | `noticed <file> edited outside` | `user edited` |
| `slash_command` | `/` (accent) | `SlashRow` | `slash /<name>` | `<output length>c` |
| `tool_call · compact` (opencode) | `⤓` (fg-2) | `CompactRow` | `compacted context` | `40k → 12k` |
| `turn_cost` | `$` (fg-3) | `CostRow` | `turn <tokens> tok · $<usd>` | `<ms>` |
| `api_error` | `✕` (err) | `ApiErrorRow` | `api error <status>` | `attempt N` |
| tool error (non-api) | `✕` (err) | `ToolErrorRow` | `<tool> <message>` | — |
| `permission_mode = ask…→` (BYOH while paused) | `⏵` (waiting) | `PermissionRow` | `permission bash · <command> · jump to harness ↗` | `awaiting you` |
| `user_prompt` | `›` (accent) | `UserPromptRow` | `user "<text>"` | — |
| `permission_mode` (config change) | `⏵` (waiting) | `PermissionModeRow` | `permission mode` | `ask → always_for_session` |
| `ai_title` | `❝` (fg-3) | `AiTitleRow` | `title "<title>"` | `harness titled` |
| `bridge_status` | `⇄` (ok/warn) | `BridgeStatusRow` | `bridge <status>` | `<detail>` |
| burst (rendering-only) | `▸` (fg-0) | `BurstRow` | `<tool> ×<N> <scope>` | `+N −N · <window>` |

Burst rows are **rendering-only** — the backend never emits them. The
frontend folds consecutive `tool_call · edit` (or `· write`) rows
where `harness`, `tool`, and `dirname(file)` are equal and the
inter-row gap is < 5 s into a single burst row. Clicking the row
expands it to the constituent rows; clicking again folds back. The
right-meta carries the cumulative `+N/−N` and `<live>` window.

**Tool-result rendering.** Smart:

- Results ≤ 200 chars: inline in the row's gist as `· "<truncated…>"`
  if useful, else hidden.
- Results > 200 chars: collapsed by default; the row gets a `▸` cue
  on hover and clicking expands a 6-line preview block under the row
  with a size pill (`4.2 KB · click to expand`). Bash output, Read
  full file contents, AskUserQuestion answers map.
- Images: not in v1; defer (#80 follow-up).

**Error row treatment is graduated:**

1. Always rendered inline as the standard error row (red left edge,
   inline preview block for the message).
2. If the room is not the active room when an error lands, surface as
   a toast in the bottom-right of the right pane, anchored to the
   error row (the prototype's `.lc-toast` style).
3. If the toast is dismissed without the error being resolved, the
   error's existence persists as the urgent segment in the bottom
   status bar of the room shell (this lives in `Skein Prototype.html`,
   not in the Live Context pane itself).

## 7 · Sub-agent inspector

When a user clicks a sub-agent row (`AgentRow`), open a right-side
sheet 80 % of the pane's width with a low-opacity scrim (25 %, light
blur). The parent activity remains visible behind the sheet — this is
deliberate; the inspector is a *drawer*, not a *new room*.

Header: `[chip] [title]` + a `sub-agent` kind pill, then a meta row
with duration, tokens, tool count, status.

Body sections in order:

1. **Prompt**: the sub-agent's instruction text in a left-bordered
   block (accent left edge).
2. **Tool calls (N)**: the full ordered list of tool calls the
   sub-agent made, rendered as standard Activity rows but with
   *agent-time* offsets in the `time` slot (`0.2s`, `2.4s`, etc.)
   instead of clock time.
3. **Final report**: the sub-agent's returned text. Inter (sans) for
   readability since this is prose, not metadata. Render markdown.

Close: `Esc`, click scrim, or click the × in the header.

If the sub-agent is still running (`status != completed`), the body
shows tool calls as they land and the header reads `running…` with a
pulsing accent dot. The Final Report block is hidden until completion.

## 8 · Cross-cutting behaviours

- **Drag-resize.** Pointer-events on the 6-px divider; `cursor:
  row-resize`. Dragging redistributes flex between adjacent cards
  (min flex 8 % of total). Persist `[diff, plan, activity]` weights
  to the room settings store on `pointerup`. Restore on mount.
- **Card collapse.** Clicking a card head toggles `.collapsed` which
  forces the card to `flex: 0 0 <head-height>`. Sibling cards expand
  to fill. Persist alongside the flex weights.
- **Density.** Three modes (compact / regular / comfy) controlled by
  a single CSS variable `--row-py` (1.5 / 3 / 5 px) and `--card-head-h`
  (26 / 30 / 34 px). User-level setting, not per room.
- **Theme.** Dark default. Light is a class toggle on the `.lc` root.
- **Keyboard.** Out of scope for v1; defer.

## 9 · State updates from the backend

Subscribe (mechanism per backend's choice — websocket / Tauri event
channel / SQLite trigger + polling) to `harness_actions` rows for the
active room. Each new row:

1. Dispatch to the appropriate reducer based on `kind`:
   - `tool_call.edit` / `.write`: append Activity row; bump the
     matching Diff tab's `+N/−N` and maybe flicker; refresh diff body
     if active tab.
   - `tool_call.read` / `.grep` / `.glob` / `.bash`: append.
   - `tool_call.task_*` / `.todowrite`: append; update Plan card.
   - `plan_change` (if backend emits as a separate kind): update Plan
     only.
   - `patch`: update active diff body if file matches active tab.
   - `away_summary`: replace room subtitle text + age.
   - `pr_link` / `queue_op` / `edited_text_file` / `slash_command` /
     `compact` / `cost` / `turn_duration` / `api_error` / `ai_title` /
     `permission_mode` / `bridge_status` / `user_prompt`: append.
2. Within ~300 ms of last append in a same-tool same-file streak,
   start folding into a burst row instead of appending individually.
   Maintain the burst's running totals.

## 10 · State variations to verify

Map to the artboards in `Live Context.html` § "State variations":

- **Empty**: brand-new room. No rows. Diff says "when an agent
  edits…", Plan says "no plan items yet…", Activity shows just the
  idle tail dot.
- **Long quiet**: last row was 2 h+ ago. Tail shows "idle"; header
  meta shows `· idle 2h 14m`.
- **Burst storm**: a recent burst row with shimmer and a `LIVE`
  pill; cumulative `+N/−N` in the right meta.
- **Errored harness**: an `api_error` row inline, with bridge
  reconnection rows surrounding it. Toast in the bottom-right if the
  room is not the focused one.
- **Permission as row**: a passive deep-link permission row in
  waiting-blue, clickable to jump to the harness terminal where the
  agent is actually paused.

## 11 · History-on-attach

When the user opens a room, the backend **backfills** by scanning
existing Claude JSONL and opencode parts and replaying them into
`harness_actions`. The UI should signal the boundary between
"backfilled from disk" and "live tailing":

1. While the backfill batch is being inserted, the Activity card body
   may show all rows at once **without slide-in animation**.
2. The first row of the backfill is preceded by a banner:
   `↩ backfilled from disk · N events · 09:14 – 13:51` (start–end of
   the backfilled window). Banner has a waiting-blue tint and a top+
   bottom line. CSS class: `.lc-backfill-banner`.
3. The last row of the backfill is followed by a divider:
   `─ resume tailing — live below ─`. CSS class: `.lc-backfill-end`.
4. After the divider, live tailing resumes; new rows slide in
   normally underneath.

For very large backfills (recon-scale rooms have ≥ 6 k rows),
**virtualize the Activity card body** — the prototype doesn't, and
that's fine for ~50 rows. Use the project's existing virtualization
helper (`react-virtual` or whatever the codebase already uses).

Diff and Plan cards do not need a backfill banner — they always show
current state, regardless of how it was populated.

## 12 · Open questions Claude Code may decide

- **Per-turn cost rows on by default?** Prototype has them on with a
  Tweak to hide. Reasonable to flip if user testing wants quieter.
- **Burst-fold threshold** (currently 5 s same-tool same-dir). Tune
  during dogfooding.
- **Tail-idle threshold** (currently 90 s). Tune.
- **Sticky-on-click duration** (currently 30 s). Tune.

## 13 · Explicitly NOT in v1

Repeating from the brief so it doesn't get re-asked:

- Editing plan items from the right pane.
- Cross-harness reasoning (h1b *generates* a flag on h1a). v1 renders
  flags that appear in the stream; nothing synthesises them.
- Cross-room view ("all rooms in one stack").
- Click-to-jump from an Activity row into source location.
- Point-in-time worktree diff at any past snapshot hash.
- Keyboard shortcuts.
- Search/filter within Activity (despite ≥ 6 k row sessions — defer).
- Image tool results.
- Sub-agent live-streaming (open the inspector after completion only,
  v1; partial-while-running is a follow-up).

## 14 · Migration / cleanup

The current right-pane code path is the tabbed `Status` / `Files` /
`Activity` thing closed in PR #79. Delete it and any orphan helpers
it imported. The harness pane to the left of the right pane is
unchanged.

The `ContextStack` component in `docs/design/skein/project/skein-proto.jsx`
is the *old* design and should not be referenced for implementation —
treat it as a historical artefact. The new implementation lives in
`live-context-rows.jsx`, `live-context-cards.jsx`, and the prototype
shell in `lc-proto.jsx`.

## 15 · Source files in this handover

| File | What |
|---|---|
| `Live Context — Implementation Handover.md` | this doc |
| `uploads/live-context-design-brief.md` | the original brief (data, recon, deferred decisions) |
| `Live Context.html` | design canvas — static reference for every row kind and state |
| `Live Context Prototype.html` | interactive demo — live tailing, drag, inspector, backfill |
| `live-context-rows.jsx` | row component taxonomy (one component per kind) |
| `live-context-cards.jsx` | Diff / Plan / Activity card chrome |
| `lc-proto.jsx` | the interactive prototype's app + event tape (do not ship as-is; reference for behaviour) |
| `design-canvas.jsx` | only the canvas wrapper used by Live Context.html; not needed in app |
| `tweaks-panel.jsx` | only the demo's tweak panel; not needed in app |

When the canvas / prototype disagree with this doc, treat this doc as
authoritative. The artefacts are reference; the doc captures intent.
