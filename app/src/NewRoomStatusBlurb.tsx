import type { RepoStatus } from "./NewRoomDialogTypes.ts";

// Split out of `useNewRoomForm.tsx` (#461): the one-line folder status
// under the Folder field.
export const folderStatusBlurb = (repoStatus: RepoStatus, resolvedFromWorktree: boolean) => {
	switch (repoStatus.kind) {
		case "empty":
			return null;
		case "checking":
			return <span style={{ color: "var(--fg-3)" }}>checking…</span>;
		case "valid":
			// The worktree's *name* is deliberately not repeated here: the
			// blurb slot is ~82 characters wide and a real branch name eats
			// most of it, and the user just browsed there anyway. The only
			// thing they need told is that the field moved.
			return (
				<span style={{ color: "var(--ok)" }}>
					✓ git repo{repoStatus.head ? ` (HEAD: ${repoStatus.head})` : ""}
					{resolvedFromWorktree ? " · resolved from a worktree" : ""}
				</span>
			);
		case "not-a-repo":
			return (
				<span style={{ color: "var(--fg-3)" }}>
					not a git repo — harnesses run in this folder as-is.
				</span>
			);
		case "missing":
			return <span style={{ color: "var(--err)" }}>folder not found — pick another.</span>;
	}
};
