# Live Context (#80) — progress & handover

Status snapshot for the right-pane Diff/Plan/Activity stack. Read this
first when resuming #80, then the spec trio (`live-context-recon.md`,
`live-context-design-brief.md`, `live-context-handover.md` — the
**handover wins on conflicts**) and the build map
(`live-context-d2-buildmap.md`) for slice-level detail.

_Last updated: 2026-06-05, after D2b (#92) merged to `main`._

## Where we are

The backend (action capture) is **done end-to-end**; the frontend is
mid-way through the **D2 (Activity card)** slices. Diff (D3) and Plan
(D4) card bodies are still placeholders.

### Merged to `main`

| Slice / PR | What landed |
| --- | --- |
| recon + design brief (`cf45cca`) | data inventory, handoff to design |
| part A (`1863f99`) | `harness_actions` table + `record_harness_action` |
| part B (`524c517`, #83) | Claude JSONL action extractor + backfill |
| part C (`32ba1cc`, #85) | opencode SSE+SQLite action extractor + backfill |
| design import (`8481302`, #87) | v2 prototype artifacts into `docs/design/` |
| **D1** (`e8ecbab`, #88) | card-stack chrome (collapse, drag-resize, per-room layout persist), room subtitle, `harness-action` live subscription wired to the Activity header count |
| **D2a** (`e23eb86`, #90) | Activity row grid + chip resolution + dispatcher + the 8 simple single-shape rows + all net-new CSS; `harnesses` threaded App→LiveContext→ActivityCard |
| **D2b** (`f2faecc`, #92) | tool family (Edit/Read/Search/Bash/Task/TodoWrite/Ask/Agent), harness normalization, `is_error → ToolErrorRow` + ApiErrorRow, **plus** chronological feed ordering + auto-tail + the `.sk-right` scroll fix (these were folded in from review/testing — see note below) |

> **Note on #92's scope:** the auto-tail / chronological-ordering work
> was first drafted as a separate PR (#94) branched off `main`, which
> left it missing all of D2b. We closed #94 and cherry-picked it onto
> the D2b branch (zero file overlap → clean). **Lesson, now a rule:**
> branch each slice off the *previous slice's* branch (or wait for the
> merge) — never default to `main` while a prior slice is unmerged. And
> work that comes out of reviewing/testing a PR goes back into *that*
> PR, not a new one.

### Remaining D2 slices (not started)

From `live-context-d2-buildmap.md` § Slice plan:

- **D2c** — tool-result expansion: >200-char preview blocks + size
  pill, wired onto Bash/Read/Ask/Todo rows.
- **D2d** — flattened-item layer: turn separators (from `turn_duration`,
  Claude only), per-turn cost hair-lines (toggle, **off by default**),
  backfill banner + end-divider (needs store provenance), slide-in vs
  snap animation.
- **D2e** — burst collapse: runtime fold of consecutive same-tool
  same-dir patch rows with gap < 5 s; expand/collapse; live shimmer.
- **D2f** — _partially done._ Auto-tail + jump-to-latest pill **shipped
  in #92**. Still open: the graduated error toast (needs the active-room
  focus signal, which now exists as the `visible` prop).
- **D2g** — virtualization (variable-height; reconcile with auto-tail).
  Deferred until dogfooding shows the ~5k-row frame janks.

### After D2

- **D3** — Diff card: Monaco diff + auto-focus + flicker. `DiffCard.tsx`
  is a placeholder; `LiveContext.tsx` still uses `useGitBranchWatcher`
  to keep the status-bar branch live until D3's status/diff fetch lands.
- **D4** — Plan card + sub-agent inspector. `PlanCard.tsx` placeholder.
  The `AgentRow` onOpen seam (D2b) is the entry point.

## Architecture map (what to read)

### Backend — action capture

- `app/src-tauri/src/db.rs` — `harness_actions` table (sibling to
  `harness_events`) + the `action_kind` module: **17 kinds** —
  `tool_call, plan_change, patch, pr_link, queue_op, edited_text_file,
  slash_command, away_summary, turn_duration, api_error, turn_cost,
  permission_mode, ai_title, bridge_status, user_prompt, compaction,
  reasoning`. `record_harness_action` returns the inserted row id.
- `app/src-tauri/src/harness_actions_claude.rs` — `ActionExtractor`
  tailing `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`.
- `app/src-tauri/src/harness_actions_opencode.rs` — opencode extractor:
  SSE (`/event`) live + SQLite (`~/.local/share/opencode/opencode.db`)
  backfill. **opencode user prompts are backfill-only today** — live
  prompt tailing is an open gap (#93).
- `app/src-tauri/src/harness_action_event.rs` — `EVENT_NAME =
  "harness-action"`, emitted on **live** insert only (not backfill),
  camelCase payload via `HarnessActionEvent::emit()`.

### Frontend — `app/src/liveContext/`

- `store.ts` — `useRoomActions(roomId)`: loads the room's actions, then
  listens to `harness-action`, filters by roomId, merges by id
  (`mergeById`). Rows arrive in **insertion (id) order** — *not*
  chronological.
- `LiveContext.tsx` — composition. Props: `roomId, cwd, harnesses,
  visible, onBranchChange`. `harnessKindOf` resolves a row's
  `harnessId → HarnessKind` for its chip (unknown → `"byoh"`).
- `ActivityCard.tsx` — `orderForDisplay` (sort by effective timestamp,
  carry-forward for `ts=0`, seeded so leading `ts=0` don't fly to top) +
  auto-tail (sticks to bottom while at bottom; "▼ N new" pill derived
  from a last-seen-bottom marker; re-pins when `visible` flips true,
  because hidden cards are `display:none` and can't measure scroll).
- `rows.tsx` — the dispatcher: `switch` on backend `kind`, then the
  tool family delegates to `toolRows.tsx`. Hosts the 8 simple rows.
- `toolRows.tsx` — tool family rows, sub-classified by normalized
  `payload.tool` (D2b).
- `payload.ts` — typed payload accessors (handles Claude↔opencode shape
  divergence).
- `Row.tsx` — `GLYPH` map, `formatClock`, `basename` primitives.
- `CardStack.tsx` — resizable/collapsible 3-card layout + persistence.
- `RoomSubtitle.tsx` — latest `away_summary`.
- `chrome.css` / `activity.css` — pane/card chrome; feed styles.
- `DiffCard.tsx` / `PlanCard.tsx` — placeholders (D3/D4).

### The #1 trap — two-level dispatch

Never switch on tool name alone. **First** switch on the backend `kind`,
**then** sub-classify the tool family by normalized `payload.tool`. The
same logical action has different payload shapes across the two
harnesses (patch_info shape, result shape, tool-name casing,
`plan_change` two-mode, `ts=0` rows). `payload.ts` is where that
normalization lives — add to it rather than special-casing in rows.

## Conventions / gotchas (learned the hard way)

- **Biome runs from `app/`** — `cd app && npx biome check --write .`.
  Running it from repo root with a path arg picks up no config and
  reformats files to the wrong style (bit us twice). See the
  `biome-run-from-app-dir` memory.
- **Indentation differs by language:** `.rs` = 4-space (rustfmt), `.ts/
  .tsx` = tabs (biome). Match the file when using Edit.
- **Gate before every commit:** `bash .githooks/pre-commit` (tsc,
  biome, cargo fmt/clippy `-D warnings`, cargo test). It's wired via
  `git config core.hooksPath .githooks`.
- **Live event emits on live insert only** — backfilled rows don't fire
  `harness-action`; they arrive via the initial `useRoomActions` load.
- **PR-per-slice**, branched off a *complete* base. Commit trailer
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`;
  PR trailer `🤖 Generated with Claude Code`.
- **ultracode is ON** — use the Workflow tool for substantive build/
  review tasks, and adversarially verify payload paths against the real
  extractors before shipping a row type.

## Open issues (deferred work — do NOT fold into D2 UI slices)

- **#93** — Live Context data layer: content-level dedup (dup rows in
  bridge-heavy rooms), full re-scan, continuous source tailing
  (notably opencode live-prompt tailing so old prompts stop being
  stranded behind max-ts dedup).
- **#91** — backend extractor enrichments deferred from D2 (frontend
  renders best-effort meanwhile).

## How to resume

1. `git switch main && git pull`.
2. Pick the next slice (**D2c**). Branch `feat/80-d2c-tool-results` off
   `main`.
3. Read `live-context-d2-buildmap.md` for that slice's spec + the
   per-kind row table + standing risks.
4. Verify any new payload field against the real extractor (Claude
   *and* opencode) before rendering it.
5. Gate (`bash .githooks/pre-commit`), open the PR, address review via
   the `pr-review-address` skill, merge.
