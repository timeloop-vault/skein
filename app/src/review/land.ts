// The land actions' data layer (#214, epic #52 D9).
//
// Mirrors `app/src-tauri/src/land.rs`. Every type here is the serde
// shape of a DTO over there; when one changes, both change. (#69 tracks
// generating these instead of hand-maintaining them.)
//
// Two terminal actions, and both are outward-facing:
//   merge  — the room's branch into its base, locally.
//   pr     — push the branch, then open a pull request for it.
//
// Neither fires without an explicit confirmation, and the Skein-local
// review never travels with either: the PR body comes from the branch's
// own commits, and there is no code path from a review comment to it.

import { invoke } from "@tauri-apps/api/core";

/** Where a merge into the base would actually run. */
export interface MergeTarget {
	/** "worktree" — base is checked out there and the merge runs there.
	 *  "unchecked" — base is checked out nowhere; only a fast-forward is
	 *  possible. "none" — there is no local base branch at all. */
	kind: "worktree" | "unchecked" | "none";
	path?: string;
	/** Uncommitted files in that worktree. Any at all blocks the merge. */
	dirty: number;
}

export interface LandPreflight {
	isRepo: boolean;
	branch?: string;
	/** The review's base, as picked or guessed. */
	base?: string;
	/** The local branch a merge would actually hit. Differs from `base`
	 *  when that is a remote-tracking ref (`origin/main`) — the ordinary
	 *  case, since the pane guesses `origin/HEAD` first. Shown wherever
	 *  the merge is described, so the substitution is never silent. */
	mergeBase?: string;
	baseResolved: boolean;
	baseIsLocal: boolean;
	/** Commits that would land. */
	ahead: number;
	behind: number;
	/** A sample of the room's uncommitted paths — a warning, not a
	 *  blocker: a merge takes commits, so these do not travel. */
	dirty: string[];
	dirtyTotal: number;
	mergeTarget: MergeTarget;
	canFastForward: boolean;
	remote?: string;
	upstream?: string;
	/** The base as a branch on the remote — what `gh --base` wants. */
	prBase?: string;
	ghVersion?: string;
	/** Commit summaries, oldest first. */
	commits: string[];
	suggestedTitle: string;
	suggestedBody: string;
	/** Empty means merge-to-base can run. Rendered verbatim. */
	mergeBlockers: string[];
	/** Empty means push-and-open-PR can run. Rendered verbatim. */
	prBlockers: string[];
}

export interface MergeOutcome {
	kind: "uptodate" | "fastforward" | "merged";
	base: string;
	branch: string;
	baseSha?: string;
	worktree?: string;
	detail: string;
}

/** Push and PR are one action to the user and two to git, so this can
 *  say the first half worked and the second did not. `prError` set with
 *  no `url` means the branch *is* on the remote. */
export interface LandPrResult {
	remote: string;
	branch: string;
	pushDetail: string;
	url?: string;
	/** `false` when a pull request already existed for this branch. */
	created: boolean;
	bodyVerified: boolean;
	prError?: string;
}

export const landPreflight = (roomId: string, cwd: string): Promise<LandPreflight> =>
	invoke<LandPreflight>("land_preflight", { roomId, cwd });

export const landMerge = (roomId: string, cwd: string): Promise<MergeOutcome> =>
	invoke<MergeOutcome>("land_merge", { roomId, cwd });

export const landOpenPr = (
	roomId: string,
	cwd: string,
	title: string,
	body: string,
): Promise<LandPrResult> => invoke<LandPrResult>("land_open_pr", { roomId, cwd, title, body });

// ── derived helpers ───────────────────────────────────────────────

/// One line saying what a merge would do, for the confirmation.
///
/// The distinction matters to the user: a fast-forward leaves no merge
/// commit and is what most rooms produce, while a diverged base means a
/// merge commit lands on a branch someone else may be reading.
export function mergeSummary(p: LandPreflight): string {
	const base = p.mergeBase ?? p.base ?? "the base";
	const branch = p.branch ?? "this branch";
	const commits = `${p.ahead} commit${p.ahead === 1 ? "" : "s"}`;
	const where =
		p.mergeTarget.kind === "worktree"
			? ` in ${p.mergeTarget.path}`
			: " by moving the branch ref (it is checked out nowhere)";
	const how = p.canFastForward ? "fast-forward" : "merge";
	return `${how} ${commits} from ${branch} into ${base}${where}`;
}

/// The warning a merge carries but does not refuse over: work that is
/// on disk and not in a commit simply does not travel.
export function dirtyWarning(p: LandPreflight): string | undefined {
	if (p.dirtyTotal === 0) return undefined;
	const shown = p.dirty.slice(0, 5).join(", ");
	const more = p.dirtyTotal > 5 ? `, +${p.dirtyTotal - 5} more` : "";
	return `${p.dirtyTotal} uncommitted file${
		p.dirtyTotal === 1 ? "" : "s"
	} in this room will not be included (${shown}${more})`;
}

/// How the result line reads after a merge.
export function mergeResultLine(o: MergeOutcome): string {
	switch (o.kind) {
		case "uptodate":
			return `${o.base} already had everything on ${o.branch} — nothing moved`;
		case "fastforward":
			return `fast-forwarded ${o.base} to ${o.branch}`;
		default:
			return `merged ${o.branch} into ${o.base}`;
	}
}
