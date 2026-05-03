# Chapter 6 — Workspace model

After chapter 5 the prototype's resume flow is solid, but two
daily-use frictions and a lurking conceptual tangle remain:

1. **"Session" means two different things.** Skein's top-level tabs
   (folder + task + harnesses) and the agent tools' conversation ids
   both live on `Harness` — `session.id` for the workspace,
   `harness.sessionId` for the agent conversation. Chapter 5's resume
   work made the second meaning concrete and "the opencode session
   for this session" is now a real sentence we have to write.
2. **Closing a top-level tab is destructive.** `closeSession` removes
   the workspace from state and the next save wipes it from
   `skein.db`. There's no "reopen recent" surface. The worktree dir
   is still on disk afterwards, but the harness conversations are
   unrecoverable in Skein.
3. **Folder picker requires a git repo.** `NewSessionDialog` gates
   the commit button on `git_is_repo`. Non-git folders, mono-style
   parents containing several gits, scratch dirs with no repo — all
   blocked. Real workflows break out.

Chapter 6 closes those three gaps as one chapter because they share
an abstraction: **what a "workspace" actually is.** Picking clearer
names first lets persistence and git-optional lay on top with the
right vocabulary from the start, instead of compounding the overload.

## Phase 1 — Rename Session → Workspace

**Goal:** the top-level concept is called "workspace" everywhere;
the agent-tool conversation id stays `sessionId` (the only meaning
of "session" left in Skein's vocabulary).

- `app/src/types.ts`: `Session` → `Workspace`. `Harness.sessionId`
  stays.
- `app/src/App.tsx` variables: `sessions / setSessions` →
  `workspaces / setWorkspaces`, `activeSessionId / setActiveSessionId`
  → `activeWorkspaceId / setActiveWorkspaceId`, `closeSession` →
  `closeWorkspace`, `createSession` → `createWorkspace`,
  `targetSessionId` → `targetWorkspaceId`,
  `setShowNewSession` → `setShowNewWorkspace`. Big mechanical patch
  but contained — single file.
- `NewSessionDialog` → `NewWorkspaceDialog`. Empty-state copy
  ("No sessions yet" → "No workspaces yet"), command palette items
  ("New session" → "New workspace", etc.), shortcut hint comments,
  docs prose all follow.
- Rust `app/src-tauri/src/db.rs`: struct `Session` → `Workspace`.
  Tauri commands `db_load_sessions` / `db_save_sessions` →
  `db_load_workspaces` / `db_save_workspaces`; frontend `invoke`
  calls follow.
- The sqlite **table name** `sessions` stays. Renaming would need a
  migration and is purely cosmetic — JSON blobs inside don't carry
  the table name. Defer indefinitely.
- Comments split into the right meaning: `// session` (Skein concept)
  becomes `// workspace`; `// session id` referring to the
  agent-tool field stays as `sessionId` / "conversation id."

This phase is a no-op for the user — the UI looks identical, just
relabelled. It's the foundation so phases 2 / 3 don't introduce more
"session" code we'd have to rename later.

## Phase 2 — Persistent workspace history

**Goal:** closing a workspace is reversible. A small "Recent" picker
lists archived workspaces and lets you reopen them.

- New optional field on `Workspace`: `archived?: number` — the
  close timestamp (epoch ms). Absence means active. Round-trips via
  the existing JSON-blob path; Rust struct mirrors it the same way
  `sessionId` was added in chapter 5.
- `closeWorkspace` flips the workspace to `archived = Date.now()`
  instead of dropping it from state. Tab strip render and command
  palette enumeration filter `workspaces.filter(w => !w.archived)`.
- New `ReopenWorkspaceModal` (`app/src/ReopenWorkspaceModal.tsx`):
  lists archived workspaces sorted by archived-time descending.
  Each row shows folder name + repo · branch (when present) +
  close date. Click to unarchive (clear `archived`, set as
  `activeWorkspaceId`). Reuses `.sk-modal*` chrome.
- Trigger paths:
  - Command palette: new "Reopen workspace…" item — opens the modal.
  - Empty state: when there are zero active workspaces but at least
    one archived, show a "Reopen recent…" link below the "Create
    your first workspace" button.
- Resume just works: unarchiving doesn't change harness state; the
  boot-time resume flow already runs `resumeCmd` on every
  workspace's harnesses, so reopening triggers the same flow as a
  Skein restart. PTYs that were killed at close re-spawn fresh; the
  captured `sessionId`s wire them back to the same conversations.
- **Edge:** if the worktree dir under the workspace's `cwd` no
  longer exists (`rm -rf` outside Skein), reopen still works but
  harnesses error at spawn. Don't bother with a pre-flight existence
  check — `LiveTerminal` already renders spawn errors inline.

## Phase 3 — Git-optional workspace

**Goal:** any folder can be a workspace. Repos still get the
branch / worktree machinery; non-repos just use the picked folder
as `cwd` directly.

- `Workspace.repo` and `Workspace.branch` become optional. Tab strip
  renders `repo · branch` when both present, just the folder name
  otherwise.
- `NewWorkspaceDialog`:
  - Repo path (today's behaviour): branch picker + "new worktree" /
    "current branch" mode.
  - Non-repo path: hide the branch UI, treat the picked folder as
    `cwd` directly. Submit button enabled once `cwd` + `task` are
    filled. Replace today's "checking" / "not-a-repo" / "valid" /
    "empty" gating with: any picked folder is valid; the branch UI
    is conditional on `git_is_repo === true`.
- `LiveStatus` (right pane): gracefully degrade for non-git cwds.
  Skip the `git_watch_start` / `git_status` / `git_diff` round-trips;
  render a small "no git repo" placeholder so the layout stays
  consistent.
- `WatcherManager` (Rust): nothing to change — App.tsx just gates
  the `git_watch_start` invoke on `is_repo`. The existing watcher
  pool stays unused for non-git workspaces.
- **Out of scope — mono-style parents.** A folder containing several
  child repos gets treated as plain non-git cwd (no per-child
  picker). Captures the reported case; the "let me pick which child
  to tie to" workflow is parked for a future chapter.

## Out of scope for chapter 6

See [`backlog.md`](./backlog.md) — anything we considered but pushed
out has been merged there. Notably:

- **Multi-git parent picker** — phase 3 handles parents as plain
  non-git cwd; a future chapter can add the "pick a child repo or
  use the parent" UI when the friction shows up.
- **Worktree cleanup on archive.** Archiving a workspace doesn't
  `git worktree remove` the worktree dir. Left to the user; a
  "delete worktree on close" toggle is a future polish item.
- **Renaming the sqlite `sessions` table** — cosmetic, requires a
  migration, defer.
- **Settings panel proper** (API keys, default harness, permission
  mode) — still backlog.
- **Terminal embedding rework** — still chapter 7. The
  agreed-A→C→B order shifts to A → workspace-model (this) →
  terminal rework → distribution.
