// The body of the "Worktree sweep" actions nudge (#358).
//
// Split out of nudgeRegistry.ts because this body runs to a page — a
// procedure, not a one-liner like the #238 review nudges — and inlining
// it there would swamp the registry list. It is deliberately generic:
// any repo, any default branch, Claude Code and opencode alike — no
// client-specific tool invocations, no Skein install paths, nothing
// that only makes sense on this machine. Its one Skein-specific step is
// the room check, which goes through the `find_rooms_for_path` agent
// verb (#354; see docs/agent-api.md, "Finding rooms by path") rather
// than assuming anything about how Skein stores rooms.

export const WORKTREE_SWEEP_BODY = `Sweep this repository's git worktrees and local branches: remove the ones whose work has already landed on the default branch, and nothing else. Follow these steps in order. Steps 1–3 only read; nothing is removed until I explicitly confirm in step 4.

1. Inventory
- Run from the repository's main worktree (the first entry of \`git worktree list\`). If you are inside a linked worktree, run git with \`-C <main worktree path>\`.
- Refresh: \`git fetch --prune\` on the default remote (usually \`origin\`).
- Find the default branch: \`git symbolic-ref --short refs/remotes/origin/HEAD\` (adjust the remote name if it is not \`origin\`; if that ref is not set, ask me rather than guess). Call it \`<base>\` below, e.g. \`origin/main\`.
- List worktrees with \`git worktree list --porcelain\` and local branches with \`git branch --format='%(refname:short)'\`. Never consider the main worktree or the default branch itself.

2. Check Skein's rooms
- A worktree can be the folder of a Skein room. For every candidate worktree path, call the Skein tool \`find_rooms_for_path\` with \`{ "path": "<worktree path>" }\`. Your client may show it with a prefix.
- If that tool is not available, or a call fails, STOP. Report that the room check could not run and remove nothing. Do not work around it by guessing.
- If any response has \`unreadable_rooms\` greater than 0, STOP. Report that Skein could not read some of its rooms, so a folder's absence from the result proves nothing, and remove nothing.
- Decide on \`safe_to_remove\` and nothing else. If any returned room has \`safe_to_remove: false\`, exclude that worktree: it belongs to an open room. \`match\` (\`cwd\`, \`inside_room\`, \`contains_room\`) only explains why a room was returned. Never use it to override \`safe_to_remove\`.
- A worktree whose only rooms are archived (\`safe_to_remove: true\`), or that has no rooms, passes this check. Note any archived rooms in the plan: after removal, reopening them will report the folder as missing.

3. Collect evidence that the work landed
Squash and rebase merges leave branch commits that are not ancestors of \`<base>\`, so \`git branch --merged\` and \`git branch -d\` are the wrong test. For each branch, gather:
- Commits ahead: \`git rev-list --count <base>..<branch>\`. Zero ahead passes.
- A merged pull or merge request for the branch, via the forge's CLI if one is installed and authenticated (for example \`gh pr list --state merged --head <branch>\` on GitHub, or \`glab mr list --merged --source-branch <branch>\` on GitLab). A merged PR/MR passes.
- Patch equivalence: \`git cherry <base> <branch>\`. If every line starts with \`-\`, every commit's change is already on \`<base>\`, and the branch passes.
A branch with commits ahead that has neither a merged PR/MR nor an all-\`-\` cherry result has NOT landed. Exclude it.
For each worktree, also check \`git -C <path> status --porcelain\`. Any output means dirty: exclude it and never force it.
Exclude as well any worktree that \`git worktree list\` marks \`locked\`, and any worktree whose branch is excluded.

4. Present the plan and wait for my confirmation
Show one table of what you propose to remove: worktree path, branch, commits ahead, landed evidence (\`PR/MR #n merged\`, \`cherry: all -\`, or \`0 ahead\`), dirty count, and room state (none / archived). Then list every exclusion with its reason (dirty, not landed, open room, locked).
Ask me once for the whole plan, then stop and wait for my answer. Do NOT remove anything until I reply with an explicit yes. Anything other than a clear yes means remove nothing. If I approve only part of the plan, remove only that part.

5. Remove (only after my yes)
- Just before removing each approved worktree, repeat its dirty check and its \`find_rooms_for_path\` check. If either result has changed, skip that worktree and say so.
- \`git worktree remove <path>\` for each approved worktree. Never pass \`--force\`: a worktree that refuses was not safe to remove.
- \`git worktree prune\`.
- \`git branch -D <branch>\` for each approved branch. \`-D\` is needed because \`-d\` refuses squash-merged branches; the evidence from step 3 is what justifies it.
- Never delete remote branches, never touch the default branch or the main worktree, and never delete any other files or folders.

6. Report
Say what was removed, what was kept and why, and show the final \`git worktree list\` and \`git branch\` output.`;
