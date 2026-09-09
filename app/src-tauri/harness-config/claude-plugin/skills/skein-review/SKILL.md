---
description: Read and address the reviewer's review comments on your work inside Skein, and check whether the reviewer has signed the work off. Use when the user mentions review comments or Skein's review pane, asks you to address review feedback, or asks you to land, merge, push or open a PR for a branch — the sign-off is the gate to check first.
---

# Addressing a Skein review

You are running inside a **Skein room**: a task with its own git
worktree, reviewed by the human who directed you. They review in
Skein's review pane rather than on GitHub, and their comments reach you
through the `review` MCP server this plugin registers — the tools are
named `list_comments`, `get_comment`, `get_diff`, `reply`,
`mark_addressed` and `review_status`.

Nothing tells you when a comment appears. Check when the user asks you
to, and before you treat a branch as finished.

## The loop

1. **`list_comments`** — defaults to unresolved, which is what still
   needs doing. Each thread carries its file, its **current** line
   range, whether its anchor has gone outdated, whether an `addressed`
   claim is already standing on it, and the whole conversation.
2. **`get_comment`** for anything whose intent is not obvious from the
   list. It returns three things worth keeping apart:
   - `anchor_lines` — the code the comment was *written against*. For an
     outdated thread this is the only honest account of what was meant.
   - `current_context` — that region of the file as it stands now.
   - `diff_context` — the hunk it lands in, as a unified diff.
3. **Make the change.** Ordinary work in the worktree.
4. **`reply`** — say what you did, or ask if the comment is ambiguous.
   Your reply appears in the reviewer's pane immediately.
5. **`mark_addressed`** — pass the `commit_sha` when a commit made the
   change. This is a *claim*, not a closure: it renders as a badge
   beside the reviewer's resolve control and does not close anything.

Use `get_diff` to read the change under review: `branch` (the default)
is everything this branch does against its base, committed and
uncommitted together; `pending` is the not-yet-reviewed set; `commit`
needs a `commit_sha`. A whole-branch diff is capped and reports
`truncated: true` rather than trailing off — ask per file when it does.

## Two things you cannot do, by design

**You cannot resolve a thread.** The reviewer closes a comment after
reading your reply. If you could close your own comments the review
would have no gate. Asking for it by another name will not work — the
refusal is by name, not by omission.

**You cannot sign off your own work.** `review_status` reads the
sign-off; nothing grants one.

## Before landing a branch

Call **`review_status`** and read `approved`. It is true **only when
the sign-off names the commit `HEAD` points at now** — a sign-off
approves a commit, not a room, so committing after being approved
invalidates your own clearance and `stale` becomes true. `guidance`
says the same thing in words.

If `approved` is false, do not merge, push or open a pull request. Say
what is outstanding and let the reviewer decide.

If it is true, **Skein does not land the branch — you do.** Which merge
strategy, which forge, and what a PR body looks like are things this
repository already states: read `.claude/skills/` and `CLAUDE.md` and
follow them. Skein owns only the one fact that lives nowhere else,
which is whether the human said yes.
