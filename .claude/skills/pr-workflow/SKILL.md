---
name: pr-workflow
description: Push branches and open pull requests following Skein's conventions, merge them once the reviewer has signed off, and keep a stack healthy after merges. Use when opening a PR, pushing for review, merging a PR, or when the user says "create PR", "open pull request", "push and create PR", "merge the PR", "merge it". Covers the gh CLI body traps (--body-file, never --body @-, and verifying the body landed), the PR description shape, stacked PRs, the rebase every stacked PR needs after a squash-merge, and the sign-off-gated self-merge (review_status approved and not stale for the PR head, the head containing current origin/main, checks green, then gh pr merge --squash).
---

# PR Workflow

Open and maintain pull requests following Skein conventions.

## Prerequisites

- Changes committed (see the `git-workflow` skill)
- On a branch that isn't `main`
- The pre-commit gate passed — it runs on every commit, so a clean
  commit means fmt/clippy/tests/tsc/biome all passed

## 1. Verify State

```bash
git status --short               # clean
git log main..HEAD --oneline     # exactly the commits you intend
```

If you rebased (see §5), the hook did **not** run — re-run the gate
manually before pushing:

```bash
bash .githooks/pre-commit
```

## 2. Push

```bash
git push -u origin $(git branch --show-current)
```

## 3. Create the PR

### Title

The commit subject: `<type>(#<issue>): <subject>`. For a multi-commit
PR, the subject of the dominant change, or a summary in the same form.

### ⚠️ The `gh` body trap — read this before every PR

**`gh` does not support curl's `@-` stdin syntax.** `--body @-` sets the
body to the literal two-character string `@-`, silently discards your
heredoc, exits 0, and prints a valid URL. Nothing looks wrong. This has
already shipped 8 empty issues and PRs in this repo.

```bash
# ✅ correct
gh pr create --title "…" --body-file - <<'EOF'
## Problem
…
EOF

# ❌ silently produces a PR whose entire body is "@-"
gh pr create --title "…" --body @- <<'EOF'
```

The same applies to `gh issue create`, `gh pr edit`, `gh issue edit`.

**Always verify the body landed:**

```bash
gh pr view <n> --json body -q '.body' | wc -c
```

A length of `3` means you hit the trap (`@-` plus a newline). Repair
with `gh pr edit <n> --body-file -`.

Related: use **real newlines**, never literal `\n` escape sequences —
those render as visible backslashes on GitHub. A quoted heredoc
(`<<'EOF'`) handles this and also stops the shell expanding backticks
and `$` inside the body.

### Description shape

Skein PR bodies are freeform but converge on this. Use the headings
that earn their place; drop the rest.

```markdown
Closes #<n>.

## Problem
[Why this exists — the symptom, and the evidence for it. Numbers,
error text, or a measurement beat adjectives.]

## Fix
[What changed and the reasoning behind the approach. Call out anything
a reviewer would otherwise have to reverse-engineer.]

## Not fixed here
[Deliberate omissions and follow-ups, with issue links. Optional but
valued — it stops a reviewer flagging a known gap.]

## Verification
[How you know it works: test counts, before/after table, or the
in-app behavior you checked. Skein is a desktop app — "tests pass" is
not the same as "the app does the thing".]
```

The **What** is the title's job — don't repeat it as a heading.

`Closes #n` must be in the **body**, not just the title, for GitHub to
auto-close the issue on merge. Verify the body landed (above) or the
link is lost.

When Claude authors the PR, end with the footer from the current
session's attribution instructions (the "Generated with Claude Code"
line plus the session link).

**Nothing private:** no PII, secrets, machine-specific absolute paths,
or private infrastructure in titles, bodies, or comments.

## 4. Stacked PRs

When work depends on an unmerged branch, stack it rather than bundling
unrelated changes:

```bash
git checkout -b feat/199-cost-state fix/198-plan-card-todowrite
gh pr create --base fix/198-plan-card-todowrite --title "…" --body-file - <<'EOF'
```

Say so in the body — "Stacked on #204; base flips to `main` when it
merges" — so a reviewer knows to review the parent first. Merge bottom
of the stack first, one PR at a time (see §8).

## 5. After a merge: rebase the rest of the stack

**This repo squash-merges.** A squash replaces your commit with a new
one carrying the same content under a different SHA. Git then no longer
recognizes that your remaining branches' base is already on `main`, so
the next PR in the stack starts re-proposing the already-merged files.

Symptom: the child PR's file list grows to include files from the
merged PR.

```bash
gh pr view <child> --json files -q '.files[].path'
```

Fix — drop the merged commit explicitly rather than trusting rebase's
duplicate detection:

```bash
git fetch --prune origin
git rebase --onto origin/main <old-merged-sha> <child-branch>
# then, for anything stacked on the child:
git rebase --onto <child-branch> <old-child-sha> <grandchild-branch>

bash .githooks/pre-commit          # rebases bypass the hook
git push --force-with-lease origin <branch>
```

GitHub auto-retargets a child PR's base to `main` when its parent merges
and the branch is deleted, so the base usually needs no manual change —
but the rebase is still required. (This is why §8 deletes the parent's
remote branch right after the merge.)

Repeat after each merge in the stack. The rebased child's head changed,
so it needs a fresh sign-off before it can merge (§8).

## 6. Force-pushing

Only on non-`main` branches, only `--force-with-lease`, never plain
`--force`.

**Never force-push after review has started** — it can orphan review
comments. The exceptions are the stack rebase above and the up-to-date
rebase before a merge (§8 gate 2) — both mechanical, no content change —
and a reviewer explicitly asking for a history rewrite.

## 7. Review

The repo owner reviews before merge. When review comments arrive, triage
each one, apply the accepted fixes, and reply on every thread. Leave
threads for the reviewer to resolve. Merging is gated on the reviewer's
sign-off (§8); you cannot grant it yourself.

## 8. Merge

You merge your own PR, but only when all three gates hold. If any
fails, stop and say which.

1. **Sign-off.** Call the Skein MCP tool `review_status`
   (`docs/agent-api.md`). Merge only if `approved` is true, `stale` is
   false, **and** `approved_sha` equals the PR's head commit — what was
   signed off must be exactly what was pushed. If you committed or
   pushed after the sign-off it reads stale: ask the reviewer to look
   again. Never merge on an old approval.
2. **Up to date.** The PR head must contain the current `origin/main`:
   after `git fetch origin`, `git merge-base --is-ancestor origin/main
   <head>` must exit 0. That ancestry check is the authority, not
   GitHub. Without branch protection `mergeStateStatus` can read `CLEAN`
   for a branch that is behind, and a squash-merge then lands a combined
   tree that no gate and no reviewer ever saw — #476 and #488 both
   merged that way, harmless only because their changes were
   file-disjoint. If the head is behind, bring it up to date before
   anything else:

   ```bash
   git rebase origin/main
   bash .githooks/pre-commit          # rebases bypass the hook
   git push --force-with-lease origin <branch>
   ```

   The head changed, so the sign-off now reads stale by design: ask the
   reviewer for a fresh one on the new head, then start again at gate 1.
   Never merge on the sign-off of the pre-rebase head. If the rebase
   conflicted, say so when you ask — the resolution is new content the
   reviewer has not read.
3. **Green.** If the PR has checks, every one must pass — none pending,
   none failing. CI here is `workflow_dispatch`-only, so most PRs have
   none; `gh pr checks` then prints "no checks reported" and exits 1,
   which passes the gate — the pre-commit gate the commit already passed
   is sufficient. Read the output, not just the exit code: exit 8
   (pending) or any failing check does not pass. The PR must also be
   mergeable, and `mergeStateStatus` must be `CLEAN` — anything else
   (`BEHIND`, `BLOCKED`, `UNSTABLE`, `DIRTY`, …) stops the merge. `CLEAN`
   is necessary, not sufficient: it does not replace gate 2.

**Re-check right before merging.** `main` can move while you wait for
the sign-off, so run gates 1 and 2 again immediately before
`gh pr merge`, in the same sequence as the merge itself. If `origin/main`
moved past the head, go back to gate 2 — rebase, gate, push, fresh
sign-off — and loop until both hold at once.

```bash
n=<pr-number>
git fetch origin main "pull/$n/head"            # pull/<n>/head guarantees the head object is local
head=$(gh pr view $n --json headRefOid -q .headRefOid)  # must equal review_status.approved_sha
git merge-base --is-ancestor origin/main "$head" || echo "NOT UP TO DATE: stop"  # any non-zero exit stops (gate 2)
gh pr checks $n                                  # all pass, or "no checks reported"
gh pr view $n --json mergeable,mergeStateStatus  # MERGEABLE / CLEAN

gh pr merge $n --squash --match-head-commit "$head"
gh pr view $n --json state,mergeCommit           # MERGED + a merge commit
git push origin --delete <branch>                # remote only; "remote ref does not exist" is harmless
```

`--match-head-commit` makes GitHub refuse the merge if the head moved
after you checked it, so what merges is the head that was signed off.
Every check above is a stop, not a log line: a sha that differs from
`approved_sha`, or any non-zero exit from `merge-base --is-ancestor`
(1 = behind, 128 = the head object is missing), means you do not run
`gh pr merge`. Nothing GitHub-side guards `main` itself: if it moves in
the seconds between the ancestry check and the merge, the squash still
lands on the newer `main`. Keep that window to the length of this block
and do not pause inside it.

⚠️ **Do not pass `--delete-branch`.** It deletes the local branch too,
and that branch is checked out in the room's worktree; the worktree-sweep
skill removes it later. Delete only the remote, after the merge is
verified — GitHub's auto-delete may already have done it, which is fine.
Exit 0 from `gh pr merge` is not proof: read the state back.

**Stacks:** merge bottom-up only. After each merge and remote-branch
delete (which retargets the child to `main`), do the §5 rebase on the
next PR and push. Its head changed, so it needs its own fresh sign-off
on the new head before it can merge.

**Report:** in a Skein room working for a director, `landed` means
merged — give the merge commit sha — not "PR opened".
