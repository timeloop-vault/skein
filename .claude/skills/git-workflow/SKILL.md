---
name: git-workflow
description: Create branches and commits following Skein's conventions. Use when creating a branch for a feature, fix, chore, refactor, perf, ci or docs change, and when committing. Handles branch naming (feat/198-slug), commit subjects (type(#issue): imperative, lowercase, ~80 chars), when a body is warranted and what belongs in it, the required Claude trailers, explicit staging, and what the pre-commit gate actually runs.
---

# Git Workflow

Create branches and commits following Skein conventions.

## When to Use This Skill

- User wants a branch for their work
- User wants to commit changes
- User says "create a branch", "commit this", "commit my changes"
- You have uncommitted changes and need to organize them

## Agent Instructions

1. **Check state first:**
   ```bash
   git status --short
   git log --oneline -5     # match the prevailing subject style
   ```

2. **Find the issue number.** Skein is issue-driven — nearly every
   commit names one. If the user hasn't given one, look for it in the
   conversation, or ask. Only genuinely issue-less work (a typo fix, a
   docs tidy) uses a bare `docs:` / `fix:` prefix.

3. **Infer the type** from the changes:
   - `docs/`, `*.md` → `docs`
   - New capability → `feat`
   - Bug fix → `fix`
   - Structure-only, no behavior change → `refactor`
   - Measurable speed/memory win → `perf`
   - `.github/`, `.githooks/` → `ci`
   - Deps, tooling, release config → `chore`

4. **Propose branch + subject with your reasoning**, then execute.
   Ask only when the type or issue is genuinely ambiguous — don't
   prompt for everything.

## Branch Creation

Current convention is `<type>/<issue>-<slug>`:

| Work Type | Format                    | Example                     |
| --------- | ------------------------- | --------------------------- |
| Feature   | `feat/<n>-<slug>`         | `feat/199-cost-state`        |
| Fix       | `fix/<n>-<slug>`          | `fix/198-plan-card-todowrite`|
| Perf      | `perf/<n>-<slug>`         | `perf/165-livecontext-backfill` |
| Refactor  | `refactor/<n>-<slug>`     | `refactor/150-keybindings-table` |
| Docs      | `docs/<n>-<slug>`         | `docs/173-claude-md-rewrite` |

Older branches omit the number (`fix/copy-hardening`). Include it —
it makes the stack readable when several are open at once.

```bash
git checkout -b <branch-name> main
```

Branch from an up-to-date `main` (`git checkout main && git pull
--ff-only` first). Everything lands via PR — see the `pr-workflow`
skill. Never commit directly to `main`.

## Staging

**Stage explicitly. Never `git add -A` without reading `git status
--short` first.** Skein-specific things that sweep in accidentally:

- **`app/package-lock.json`** — `npm install` rewrites unrelated
  metadata (`"peer": true` markers move between npm versions). Unless
  you deliberately changed a dependency, `git checkout --
  app/package-lock.json` before committing.
- **Scratch and probe files** — anything you created in the repo to
  test behavior. Put temporary files in the scratchpad directory, not
  the worktree.
- **`target/`, `app/node_modules/`, `app/dist/`** — gitignored, but
  verify rather than assume.

## Commit Messages

### Format

```
<type>(#<issue>): <subject>

<body>

Co-Authored-By: Claude <model> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_<id>
```

Multiple issues: `fix(#200,#201,#202): make the pre-commit gate pass on
Windows`. A scope instead of an issue is rare but valid when the area
matters more than the ticket — `fix(notifications): …` with the issues
at the end as `(#62, #64)`.

### Subject

- Imperative, lowercase, no trailing period
- Target ≤80 characters; the repo median is ~67. Two of the last 40
  exceed 80, so it's a target, not a gate — but if you're over, tighten
  the subject rather than spilling into the body.
- Says **WHAT** changed, not how

### Body

**Skein commits normally have a body** — 3–12 non-blank lines is
typical. This differs from repos where a bare subject is the norm; here
the body is where the *why* lives, and reviewers rely on it.

Write the body to answer what the diff cannot:

- The root cause, when the fix doesn't reveal it
- Why this approach and not the obvious alternative
- A non-obvious consequence, or a deliberate omission
- Evidence, when the claim is empirical ("151 passed; previously 0 run")

Do **not** re-list the changes. A bulleted inventory of files touched is
the diff rewritten as prose — cut it. If a change feels like it needs a
change-log body, that's a signal to split the commit.

Keep it proportional: a one-line fix rarely needs 20 lines of prose.

### Trailers

Required when Claude authors the commit. Use the exact model name and
session URL given in the current session's attribution instructions —
these change between sessions and models, so don't copy them from an
older commit.

### Nothing private

Never put PII, secrets, machine-specific absolute paths, or private
infrastructure details in a commit message or in committed content.

### Examples

Subject-only, where the change speaks for itself:

```
fix(#189): let the + harness picker be dismissed without picking
```

Body earning its place — root cause the diff doesn't show:

```
fix(#154): normalize path separators in matchGitFile (Windows diff card)

git reports forward slashes; the harness patch payload carries the OS
separator, so every Windows patch missed its file and the card fell
back to the worktree diff.
```

Body carrying evidence:

```
fix(#201): give test binaries the Common-Controls v6 manifest

rfd imports TaskDialogIndirect, which only the side-by-side v6 comctl32
exports — System32's is still v5.82. tauri_build embeds the manifest
into the app binary only, so the test harness bound to v5 and died at
load. 157 tests now run on Windows; previously zero.
```

### Anti-pattern: narrating the diff

❌ Restates what the diff already shows:
```
feat(#199): adopt cost-state row

Add the cost-state row handling.

- Add COST_STATE to action_kind
- Add extract_cost_state to harness_actions_claude.rs
- Add sessionTotals to feedItems.tsx
- Update LiveContext.tsx to render it
```

✅ Carries the why:
```
feat(#199): adopt Claude's cost-state row for authoritative session cost

turn_cost carries usage but no money, so a Claude room's cost has read
$0 since the card shipped. #91 parked this as a hand-maintained price
table; cost-state means upstream reports the number instead, correct
across model switches for free.
```

## The Pre-commit Gate

`.githooks/pre-commit` runs, in order: cargo fmt (both manifests),
clippy `-D warnings` (both), `cargo test` (both), tsc, biome. Activate
with `git config core.hooksPath .githooks`.

Two traps worth knowing before you debug a failure:

- **`cargo test --workspace` does NOT reach the tauri crate** (#168).
  It's excluded from the workspace; the hook runs it as a second
  explicit step. ~157 of the tests live there.
- **Biome must run from `app/`** (`cd app && npx biome check .`), never
  from the repo root with a path argument.

If the hook fails on something you didn't touch, check whether `main`
itself is red before working around it — that has happened (#200), and
the fix is a separate commit, not `--no-verify`. Never use `--no-verify`
to get past a gate; it skips all five checks, not the one that failed.

## Safe Git Commands

Avoid anything that opens an editor — no `-i` rebases, no bare
`git commit --amend`.

```bash
git commit --amend -m "new message"     # always -m
git log main..HEAD --oneline            # commits unique to this branch
git reset --soft main && git commit -m "…"   # squash locally
```

Force-push only on non-`main` branches, only with `--force-with-lease`,
and see `pr-workflow` for when it's appropriate after a PR is open.
