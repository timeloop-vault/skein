---
name: worktree-sweep
description: Remove every git worktree and local branch whose work has already landed on main, in one pass. Use when the user asks to clean up worktrees, prune branches, or tidy the checkout after a run of merged PRs. Squash-merge aware (git branch --merged is wrong here), refuses dirty worktrees, and asks Skein (find_rooms_for_path) so no open room loses its folder. For removing ONE worktree interactively, the generic worktree-remove skill is the better fit.
---

# Worktree Sweep

Skein rooms create one worktree per task under the sibling folder
`<repo>-wt/<slug>` (`propose_worktree_path` in `crates/skein-git`), on
a branch named `skein/<slug>`. Every PR in this repo is
**squash-merged**, so after a busy week the checkout holds a dozen
worktrees and twenty branches whose content is all on `main` but which
git still reports as unmerged. This skill removes them in one pass,
with the three checks that make that safe.

## When to Use

- "clean up worktrees", "prune branches", "tidy the checkout"
- After a run of merged PRs, when `git worktree list` is long
- Before a release, to make sure nothing unlanded is hiding in a
  worktree

Do **not** use it to remove a single worktree the user is still
deciding about — use the generic `worktree-remove` skill for that.

## Why `git branch --merged` is the wrong test

A squash-merge replaces the branch's commits with one new commit on
`main`. The branch tip is not an ancestor of `main`, so
`git branch --merged main` lists none of them, and `git branch -d`
refuses every one. Two tests that do work:

1. **The PR merged.** `gh pr list --state all --head <branch>` says
   `MERGED`. Authoritative for any branch that went through a PR.
2. **The content is on main by patch id.** `git cherry origin/main
   <branch>` prints `-` for a commit whose diff already exists on
   `main`, `+` for one that does not. Use this for the `skein/<slug>`
   room branches, which usually had no PR of their own — the PR came
   from a `feat/…` branch cut inside the room.

A branch passes if **every** commit ahead of `origin/main` is covered by
one of the two. Zero commits ahead passes trivially.

## Instructions

### 1. Refresh and inventory

```bash
git fetch --prune origin
git worktree list --porcelain      # one `worktree <path>` line each, unpadded
git branch --format='%(refname:short)'
```

Then, per branch other than `main`:

```bash
git rev-list --count origin/main..<branch>          # commits ahead
gh pr list --state all --head <branch> --limit 1 \
  --json number,state --jq '.[0] | "\(.number) \(.state)"'
git cherry origin/main <branch>                     # only if ahead > 0 and no MERGED PR
```

And per worktree:

```bash
git -C <path> status --porcelain | wc -l            # dirty files
```

### 2. Check Skein's rooms

A worktree may be the folder of a Skein room. Removing it under an
**open** room breaks that room; under an **archived** room it only
means the reopen flow reports the folder as missing, which it already
handles. Ask Skein with the `find_rooms_for_path` tool, once per
candidate worktree, using the path exactly as its `worktree <path>`
line in the porcelain listing gives it:

```
find_rooms_for_path { path: "<worktree path>" }
```

One query per worktree, rather than one on the `<repo>-wt` parent,
because then every room in the answer bears on that worktree — its
folder is the worktree (`cwd`), sits below it (`contains_room`), or
sits above it (`inside_room`, such as a room on the main checkout
when the worktree is nested inside it) — and there is no second path
comparison to get wrong. It also covers a worktree that lives outside
`<repo>-wt`.

Decide on `safe_to_remove`, never on `match` — `match` only says why a
room came back. **Any room with `safe_to_remove: false` excludes its
worktree from the sweep**; say so and leave it. A worktree whose rooms
are all archived (`safe_to_remove: true`), or that has none, passes;
name the archived rooms in the plan, since reopening them will report
the folder as missing.

Two cases stop the sweep before step 3, with nothing removed:

- **`unreadable_rooms` is not 0.** Skein holds rooms it could not read,
  so a worktree missing from `rooms` may still be an open room's
  folder. Report the count and that the sweep cannot be decided safely.
- **The tool is not there, or a call fails.** Outside Skein (or on a
  Skein build older than the verb) there is no `find_rooms_for_path`.
  Say that the room check could not run, and stop. Do not fall back to
  reading Skein's database or its data folder.

### 3. Present the plan and confirm

Removal is destructive. Show one table — worktree, branch, ahead,
evidence (`PR #n MERGED` / `cherry: all -` / `0 ahead`), dirty count,
room state — and the exclusions with their reason. Then ask once, with
AskUserQuestion, before touching anything. Do not ask per worktree.

A worktree is **excluded** (kept, and named in the report) when any of:

- dirty files > 0
- a commit ahead of `origin/main` with neither a MERGED PR nor a `-`
  from `git cherry`
- `find_rooms_for_path` returned a room with `safe_to_remove: false`
  for it (an open room)
- `git worktree list` marks it `locked`

### 4. Remove

Just before removing each approved worktree, repeat its dirty check
and its `find_rooms_for_path` call — the confirmation was a wait, and a
room may have opened on it since. If either result has changed, skip
that worktree and say so.

```bash
for wt in <each approved path>; do git worktree remove "$wt"; done
git worktree prune
for b in <each approved branch>; do git branch -D "$b"; done
```

`-D`, not `-d`: `-d` refuses squash-merged branches by design, and the
evidence gathered in step 1 is what justifies the force. Never
`--force` on `git worktree remove` — a worktree that refuses removal
was excluded in step 3 for a reason.

If the `<repo>-wt` folder is then empty, `rmdir` it; new rooms recreate
it on demand.

### 5. Report

Counts removed, the exclusions with reasons, and the one-line state
after: `git worktree list` and `git branch --list` should both show
only `main` when the sweep is complete. Note that archived rooms whose
folder is now gone still appear in Reopen recent and will report the
missing path if reopened.

## Safety

- **Read-only until the confirmation** — steps 1 and 2 change nothing.
- **Dirty means excluded**, never force-removed. Unsaved `files`
  harness buffers live only in Skein's memory, not on disk, so a clean
  `git status` in the worktree is the whole check.
- **Remote is out of scope.** This skill never deletes a remote
  branch; the repo's merge flow already deletes the head branch on
  merge, and anything left on the remote deserves a look, not a sweep.
- **`main` is never touched**, and the sweep runs from the main
  checkout, never from inside a worktree.
- **Nothing private in the report.** Paths in the confirmation table
  are for the user's terminal; do not paste them into issues or PRs.

## Example

```
worktree                       branch                     ahead  evidence         dirty  room
C:/git/skein-wt/skein-212      feat/212-review-surface    6      PR #222 MERGED   0      archived
C:/git/skein-wt/skein-248      skein/skein-248            1      cherry: all -    0      archived
C:/git/skein-wt/skein-215      skein/skein-215            0      0 ahead          0      archived

Excluded:
  C:/git/skein-wt/skein-270    skein/skein-270            2      cherry: 1 +      0      open   (room open, commit not on main)

Remove 3 worktrees and 3 branches? [Yes / No]
```
