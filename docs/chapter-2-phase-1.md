# Chapter 2 — Phase 1 execution plan

## Context

After chapter 1, the codebase still ships scaffolding from the design's
HTML-prototype era: seeded fictional sessions, static TUI panels that
imitate Claude/opencode/Copilot, the 13-step scripted tour, the macOS
traffic-light decoration, a right-pane tab strip whose tabs only mean
something with fixture data, and a settings strip that exposes
"Restore demo data" buttons. Real and fake live side-by-side. For a
user this is confusing; for the codebase it's noise that hides the
load-bearing parts behind imitations of them.

Phase 1 deletes the imitations. Every line that's left should
correspond to something that actually runs. Nothing structural changes
— the kept Live components (`LiveTerminal`, `LiveStatus`, `SessionTab`,
`HarnessTab`, `HarnessPicker`, plus the modal) keep their current
behavior.

CLAUDE.md exists at the repo root already (landed in commit
`66968bf`); phase 1 doesn't need to write it.

## Per-file changes

### `app/src/types.ts`
**Delete:**
- `TreeNode`, `DiffLineKind`, `DiffLine` (the fixture-shape one with
  `n1/n2/src: ReactNode`), `ActiveFile`, `PlanState`, `PlanItem`,
  `ActivityEvent`, `SessionData`
- `RightTab` (right-pane tab strip is going)
- The unused `import type { ReactNode }` (only the fixture types
  needed it)

**Edit:**
- `Harness`: drop the comment about "seeded demo harnesses" — there
  are no seeded harnesses any more. `live`/`cmd`/`cwd` become the
  normal shape, not the special case.
- `Session`: same — drop the "seeded demo sessions" comment on `cwd`.

**Stays:** `HarnessKind`, `Status`, `Harness`, `Session`, `Theme`,
`Density`. The DTO types in `LiveStatus.tsx` (its inline `DiffLineDto`
etc.) are unrelated and stay where they are.

### `app/src/data.tsx`
**Delete:**
- `INITIAL_SESSIONS` (entire `Session[]` array, ~150 lines)
- `SESSION_DATA` (the `Record<string, SessionData>` of fixture diffs,
  trees, plans, activity — the bulk of the file)
- Unused import: `Session`, `SessionData` from `./types.ts`

**Stays:** `HarnessKindMeta`, `HARNESS_KINDS`, `HARNESS_ORDER`. The
file shrinks from ~763 lines to ~50.

**Optional:** rename `data.tsx` → `harnesses.ts` (with import updates
at the call sites). The remaining content is just harness metadata;
"data" is misleading. Skip if the rename feels disruptive.

### `app/src/components.tsx`
**Delete (all of these are mock/fixture-only):**
- `ClaudePanel`
- `OpenCodePanel`
- `ByohPanel`
- `ByohResolvedPanel`
- `CopilotPanel`
- `CopilotErroredPanel`
- `FileTree`
- `DiffEditor`
- `PlanCard`
- `ActivityFeed`
- `FullPaneHead`
- The `// ── Terminal panels ──` and `// ── Right-pane primitives ──`
  section banners that bracket the deleted blocks
- The now-unused imports: `CSSProperties`, `ReactNode`, `ActivityEvent`,
  `DiffLine`, `PlanItem`, `SessionData`, `TreeNode`

**Stays:** `HChip`, `StatusDot`, `SessionTab`, `HarnessTab`,
`HarnessPicker`. File shrinks from 568 → ~120 lines.

### `app/src/App.tsx`
This is the big one. Split into clearly bounded deletes.

**Imports — drop:**
- `ReactNode` from `react` (only tour `body: ReactNode` needs it)
- `ActivityFeed`, `ByohPanel`, `ByohResolvedPanel`, `ClaudePanel`,
  `CopilotErroredPanel`, `CopilotPanel`, `DiffEditor`, `FileTree`,
  `FullPaneHead`, `PlanCard` from `./components.tsx`
- `INITIAL_SESSIONS`, `SESSION_DATA` from `./data.tsx`
- `RightTab`, `SessionData` from `./types.ts`

**`HarnessBody`** collapses to a single `<LiveTerminal>` return,
guarded on `harness.cmd && harness.cwd !== undefined`. Drop the
`resolved`, `onApprove`, `onRetry`, `onReauth` props (no consumers
left). All mock-routing branches go.

**Delete components entirely:**
- `ContextStack`
- `FilesFullPane`
- `DiffFullPane`
- `PlanFullPane`
- `RIGHT_TABS` const + `rightTab` state + setter

**Tour — delete entirely:**
- `TourActions`, `TourStep` interfaces
- `TOUR_STEPS` const (huge JSX array, ~250 lines)
- `CalloutStyle` interface
- `TourOverlay` component
- `tourIdx` state, `preTourSessions` state
- `tourActions` object construction
- `startTour` / `endTour` / `nextStep` / `prevStep` / `skipTour`
- The `useEffect` that fires step actions
- The render-time `tourStep` lookup and the conditional
  `<TourOverlay>` block
- The save-effect's `preTourSessions !== null` gate (no longer needed)
- Remove `onTour` prop from `Titlebar` and the launch button JSX

**Titlebar — simplify:**
- Drop the entire `<div className="sk-traffic">…</div>` block (3 fake
  traffic-light circles)
- **Keep `data-tauri-drag-region`** — Tauri's `tauri.conf.json` has
  `decorations: false`, so the drag region is load-bearing. Verify
  against `app/src-tauri/tauri.conf.json` if in doubt.
- Drop the `▶ Take the tour` button and `onTour` prop

**Initial state:**
- `sessions` initial state changes from `INITIAL_SESSIONS` → `[]`
- `activeSessionId` initial state changes from `"s2"` → `""`
- The DB-load effect that previously kept `INITIAL_SESSIONS` if the
  DB was empty now keeps the empty array — first-launch users land
  on the empty state, which is correct.

**Permission/error machinery — delete:**
- `permissionResolved` state + `setPermissionResolved`
- `approve` function
- `recoverError` function
- These were called only by the now-deleted mock panels.

**SettingsStrip — simplify:**
- Drop `showActivityFeed` state + setter (no activity feed left to
  toggle)
- Drop `onResetEmpty` and `onRestoreSamples` props + the "empty" /
  "restore" buttons
- Drop the activity checkbox + label
- What's left: theme toggle, density select. Keep the settings strip
  in its current position; phase 2 reworks it.

**Right pane render — simplify:**
- Drop the `<div className="sk-right-tabs">` block entirely
- Drop the `data && rightTab === …` branches
- Drop the `!data && !session.cwd` placeholder
- The right pane becomes:
  ```tsx
  <div className="sk-right">
    {session.cwd ? <LiveStatus cwd={session.cwd} /> : <SessionWithoutCwd />}
  </div>
  ```
  Where `SessionWithoutCwd` is a small inline placeholder for the rare
  case of a session with no cwd. After cleanup, every session created
  via the dialog has a cwd, so this case is mostly unreachable —
  either keep the placeholder defensively or remove if `session.cwd`
  becomes non-optional after types are tightened.

**Status bar — simplify:**
- Drop the `urgent` lookup and `UrgentToast` render (no waiting state
  on real sessions today)
- Drop the `urgent` segment
- Drop `toastDismissed` state + setter
- Drop the `UrgentToast` component definition
- What stays: harness chip + name, status dot, branch, model, tokens,
  utf-8 segment.

**HarnessBody render loop:** the `display: none` mount-all pattern
stays — that's load-bearing for chapter 1's intra-session PTY
persistence. Phase 3 of chapter 2 lifts it up to the global level;
phase 1 leaves it where it is.

### `app/src/styles.css`
Identify orphaned class families. Safe to delete:

- `.sk-traffic*` — decoration removed
- `.sk-term`, `.sk-term *`, `.term-claude`, `.term-opencode`,
  `.term-copilot`, `.term-byoh` — mock terminal styling. xterm.js
  has its own CSS via `@xterm/xterm/css/xterm.css`.
- `.sk-shell-prompt*` — mock shell prompts inside mock panels
- `.sk-term-idle*` — `IdleHarnessTerminal` is gone
- `.sk-right-tabs`, `.sk-right-tab`, `.sk-right-meta*` — right-pane
  tab strip is gone
- `.sk-context-*` — `ContextStack` is gone
- `.sk-fullpane*` — `FilesFullPane`/`DiffFullPane`/`PlanFullPane` gone
- `.sk-tree*` — `FileTree` is gone (LiveStatus uses its own list
  rendering)
- `.sk-editor*` — `DiffEditor` is gone
- `.tk-key`, `.tk-fn`, `.tk-str`, `.tk-num`, `.tk-com` — only used
  inside `SESSION_DATA`'s hand-built JSX; safe to delete
- `.sk-todo*` — `PlanCard` gone
- `.sk-activity*` — `ActivityFeed` gone
- `.sk-tour-*` — tour gone
- `.sk-permission*` — was inside `ByohPanel`
- `.sk-msg-user`, `.sk-msg-assistant`, `.sk-tool`, `.sk-composer*` —
  only referenced from mock panels

**Stays for sure (live components use):**
- `.sk-app`, density and theme classes, `.sk-titlebar`, `.sk-app-name`
  (the `.dot` child is decorative — fine to leave)
- `.sk-tabstrip`, `.sk-tab`, `.sk-tab-newbtn`, `.sk-tab-close`,
  `.tab-status`, `.tab-badge`
- `.sk-workspace`, `.sk-harness-col`, `.sk-harness-tabs`,
  `.sk-harness-tab`, `.sk-harness-add`, `.sk-harness-meta`
- `.sk-right` (LiveStatus parent)
- `.sk-statusbar`
- `.sk-empty-harness`, `.sk-harness-grid`, `.sk-harness-card` (used by
  `HarnessPicker`)
- `.sk-empty*` (used by `EmptyState`)
- `.sk-modal*`, `.sk-field`, `.sk-input`, `.sk-select`, `.sk-radio-row`,
  `.sk-radio-card` (used by `NewSessionDialog`)
- `.sk-btn` family (buttons)
- `.h-chip`, `.h-claude/opencode/copilot/byoh`,
  `.st-running/waiting/idle/error`, `@keyframes sk-pulse`
- **`.sk-line`, `.sk-code`** and the `.gutter` / `.marker` / `.src` /
  `.add` / `.del` children — used by `LiveStatus`'s `DiffView`. **Do
  not delete.**
- `.sk-toast` — `UrgentToast` is going; verify no other consumers
  before deleting (grep src/ for `sk-toast`).

Approach: rather than delete one rule at a time, group the orphaned
families into a single block and delete in one chunk. After deletion,
run `grep -nE '\bsk-[a-z-]+\b' src/*.tsx src/*.ts` to verify every
referenced class still has a matching rule.

## Order of operations

Each step should leave `npx tsc --noEmit` clean. Commits can group
several steps; the order is about staying green, not commit
boundaries.

1. **App.tsx — initial state.** `useState<Session[]>(INITIAL_SESSIONS)`
   → `useState<Session[]>([])` and `useState<string>("s2")` →
   `useState<string>("")`. Remove the `INITIAL_SESSIONS` /
   `SESSION_DATA` import. Now the empty state is the default;
   existing fixture sessions in the DB still load if present.
2. **App.tsx — delete tour.** All tour state, components, effects, and
   render branches. Remove `onTour` from Titlebar prop and the button.
3. **App.tsx — delete right-pane tab strip.** Drop `RIGHT_TABS`,
   `rightTab` state, the `<div className="sk-right-tabs">` block, all
   `data && rightTab === …` branches. Render becomes
   `session.cwd ? <LiveStatus … /> : <placeholder />`. Remove
   `ContextStack`, `FilesFullPane`, `DiffFullPane`, `PlanFullPane`
   component definitions. Drop `data` const + `SESSION_DATA` access.
4. **App.tsx — collapse `HarnessBody`.** Down to a single
   `<LiveTerminal>` return. Delete the `permissionResolved` /
   `approve` / `recoverError` / `resolved` machinery and props.
5. **App.tsx — drop urgent toast + activity feed toggle.** Remove
   `toastDismissed` state, `urgent` lookup, `UrgentToast` render,
   `UrgentToast` component definition, `showActivityFeed` state and
   prop chain. Trim `SettingsStrip` to theme + density only; drop
   `onResetEmpty` / `onRestoreSamples` props.
6. **App.tsx — Titlebar.** Drop the `sk-traffic` block. Keep
   `data-tauri-drag-region`. Drop the tour launch button.
7. **components.tsx — delete mock panels.** All terminal panels,
   `FileTree`, `DiffEditor`, `PlanCard`, `ActivityFeed`,
   `FullPaneHead`. After App.tsx's imports are already pruned, this
   change is referentially safe.
8. **types.ts — prune fixture types.** `TreeNode`, fixture-shape
   `DiffLine` / `DiffLineKind`, `ActiveFile`, `PlanItem`, `PlanState`,
   `ActivityEvent`, `SessionData`, `RightTab`, the `ReactNode`
   import. Remove the "Phase 1" / "Phase 2" framing comments.
9. **data.tsx — delete fixtures.** `INITIAL_SESSIONS`,
   `SESSION_DATA`, the `Session` / `SessionData` imports. File is
   now ~50 lines of harness metadata. Optionally rename to
   `harnesses.ts` (defer if it complicates review).
10. **styles.css — drop orphan classes.** Identified families above.
    Run the grep before and after to confirm no live class lost its
    rule.
11. **Verify.** See section below.

## Critical files

- `app/src/App.tsx` — main tree, biggest delta
- `app/src/components.tsx` — component library, large deletion
- `app/src/types.ts` — type cleanup
- `app/src/data.tsx` — fixture removal
- `app/src/styles.css` — orphan class removal
- `app/src-tauri/tauri.conf.json` — confirm `decorations: false` so
  the titlebar drag region stays load-bearing

## Verification

1. `cd app && npx tsc --noEmit` — no type errors after each step.
2. `cd app && npx biome check .` — no lint errors.
3. `cd app/src-tauri && cargo fmt --check && cargo clippy --tests -- -D warnings`
   — Rust side is untouched, but pre-commit will run anyway.
4. `cd app && npx vite build` — bundle still builds.
5. **Manual smoke test** (`cd app && npm run tauri dev`):
   - First launch with empty DB lands on the empty state (no seeded
     sessions). To force-test, delete
     `%APPDATA%\com.timeloop-vault.skein\skein.db`.
   - "Create your first session" opens the dialog. Pick a folder
     with a git repo, create. Session appears, harness spawns, status
     pane shows real worktree status.
   - `+ harness` opens the picker, picking a kind spawns into the
     same worktree.
   - Tab × on a session prompts and removes.
   - Theme toggle and density select still work in the settings strip.
   - No "Take the tour" button anywhere. No traffic-light dots in the
     titlebar. No right-pane tab strip.
6. **Commit message.** Group steps 1–10 into one commit:
   `Phase 1: strip design scaffolding`. Or split into two if the
   diff is unwieldy: one for App.tsx + components.tsx, one for
   types/data/styles. Either is fine; pre-commit chain runs on each.

## What's deliberately NOT changed in phase 1

- `LiveTerminal.tsx`, `LiveStatus.tsx` — load-bearing, untouched.
- `app/src-tauri/` — Rust backend unaffected.
- `crates/skein-git/` — unaffected.
- The all-harnesses-mounted-at-once pattern within a session — phase 3
  lifts this to the global level; phase 1 leaves it as-is.
- Font size and pane resize — that's phase 2.
- The settings strip's visual style — phase 2 reworks it.

After phase 1: roughly 1500 lines of TS/CSS deleted, no behavior
regressions for real sessions, and the codebase actually matches the
prototype it claims to be.
