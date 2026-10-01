---
name: pr-workflow
description: Push branches and open pull requests following Skein's conventions, merge them once the reviewer has signed off, and keep a stack healthy after merges. Use when opening a PR, pushing for review, merging a PR, or when the user says "create PR", "open pull request", "push and create PR", "merge the PR", "merge it". Covers the gh CLI body traps (--body-file, never --body @-, and verifying the body landed), the PR description shape, stacked PRs, the rebase every stacked PR needs after a squash-merge, and the sign-off-gated self-merge (review_status approved and not stale for the PR head, checks green, then gh pr merge --squash).
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
comments. The exceptions are the stack rebase above (mechanical, no
content change) and a reviewer explicitly asking for a history rewrite.

## 7. Review

The repo owner reviews before merge. When review comments arrive, triage
each one, apply the accepted fixes, and reply on every thread. Leave
threads for the reviewer to resolve. Merging is gated on the reviewer's
sign-off (§8); you cannot grant it yourself.

## 8. Merge

You merge your own PR, but only when both gates hold. If either fails,
stop and say which.

1. **Sign-off.** Call the Skein MCP tool `review_status`
   (`docs/agent-api.md`). Merge only if `approved` is true, `stale` is
   false, **and** `approved_sha` equals the PR's head commit — what was
   signed off must be exactly what was pushed. If you committed or
   pushed after the sign-off it reads stale: ask the reviewer to look
   again. Never merge on an old approval.
2. **Green.** If the PR has checks, every one must pass — none pending,
   none failing. CI here is `workflow_dispatch`-only, so most PRs have
   none; `gh pr checks` then prints "no checks reported" and exits 1,
   which passes the gate — the pre-commit gate the commit already passed
   is sufficient. Read the output, not just the exit code: exit 8
   (pending) or any failing check does not pass. The PR must also be
   mergeable, and `mergeStateStatus` must be `CLEAN` — anything else
   (`BEHIND`, `BLOCKED`, `UNSTABLE`, `DIRTY`, …) stops the merge.

```bash
n=<pr-number>
gh pr view $n --json headRefOid -q .headRefOid   # must equal review_status.approved_sha
gh pr checks $n                                  # all pass, or "no checks reported"
gh pr view $n --json mergeable,mergeStateStatus  # MERGEABLE / CLEAN

gh pr merge $n --squash
gh pr view $n --json state,mergeCommit           # MERGED + a merge commit
git push origin --delete <branch>                # remote only; "remote ref does not exist" is harmless
```

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
