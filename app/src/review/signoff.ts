// The reviewer's sign-off (#214, epic #52 D9 as corrected).
//
// Mirrors `app/src-tauri/src/review_surface/signoff.rs`. Every type here
// is the serde shape of a DTO over there; when one changes, both change.
// (#69 tracks generating these instead of hand-maintaining them.)
//
// This is the one fact about a review that lives nowhere but Skein.
// #213 lets the agent read the reviewer's comments; the sign-off is
// what lets it ask whether the reviewer is *done*, so it can land the
// branch the way this repository lands branches. Skein does not merge,
// push, or open pull requests — the agent does, per its own skills and
// the repo's conventions, and Skein would only guess those wrong.
//
// A sign-off approves a *commit*, not a room: it records the HEAD it
// was granted against, so an agent that commits afterwards makes its
// own approval stale rather than silently extending it.

import { invoke } from "@tauri-apps/api/core";

export interface SignoffStatus {
	/** Approved, and HEAD is still what was approved. The only field a
	 *  "may I land?" decision should read. */
	approved: boolean;
	/** Approved once, but HEAD has moved past it. Never true at the
	 *  same time as `approved`. */
	stale: boolean;
	/** The sha that was approved — present for a stale sign-off too,
	 *  which is the case it matters for. */
	approvedSha?: string;
	headSha?: string;
	approvedMs?: number;
	note?: string;
	baseRef?: string;
	/** Commits between the approved sha and HEAD. Absent when the two
	 *  are not comparable — a rebase or an amend, where a number would
	 *  be a fabrication. */
	commitsSince?: number;
	/** Open threads. Never a blocker — the reviewer decides. */
	unresolvedCount: number;
	/** Open threads the agent has not claimed to have handled. */
	unaddressedCount: number;
	/** False for a room with no repository: nothing to sign off on. */
	canSignOff: boolean;
}

export const fetchSignoff = (roomId: string, cwd: string): Promise<SignoffStatus> =>
	invoke<SignoffStatus>("review_signoff_status", { roomId, cwd });

export const setSignoff = (
	roomId: string,
	cwd: string,
	approved: boolean,
	note?: string,
): Promise<SignoffStatus> =>
	invoke<SignoffStatus>("review_set_signoff", { roomId, cwd, approved, note: note ?? null });

// ── derived helpers ───────────────────────────────────────────────

/// The short sha, for a control that has one line to say it in.
export const short = (sha: string | undefined): string => sha?.slice(0, 8) ?? "";

/// What the control reads as. Three states, because "approved" and
/// "approved, but not of this code" are different answers and
/// collapsing them is exactly the lie the head_sha column exists to
/// prevent.
export type SignoffState = "none" | "approved" | "stale";

export const signoffState = (s: SignoffStatus | undefined): SignoffState => {
	if (!s) return "none";
	if (s.approved) return "approved";
	return s.stale ? "stale" : "none";
};

/// The sentence the confirmation asks. Names what is being approved and
/// what is still open, so the reviewer is never approving blind.
export function approveConfirmation(s: SignoffStatus): string {
	const what = s.headSha ? `commit ${short(s.headSha)}` : "this branch";
	const open =
		s.unresolvedCount > 0
			? `\n\n${s.unresolvedCount} comment thread${
					s.unresolvedCount === 1 ? " is" : "s are"
				} still open. Approving does not close ${s.unresolvedCount === 1 ? "it" : "them"}.`
			: "";
	return `Sign off on ${what}?\n\nThe agent can read this and will treat it as clearance to land the branch. If anything is committed afterwards, the sign-off lapses until you renew it.${open}`;
}

/// What the pane says under a stale sign-off. The reviewer needs to
/// know their approval stopped applying and why.
export function staleExplanation(s: SignoffStatus): string {
	const since =
		s.commitsSince === undefined
			? "the branch has been rewritten since"
			: `${s.commitsSince} commit${s.commitsSince === 1 ? "" : "s"} landed since`;
	return `approved ${short(s.approvedSha)}, but ${since} — review the new work and sign off again`;
}
