// Nudge bodies + selection (#238).
//
// Fixed, one-line prompts pasted into the room's active harness and
// submitted — no settings UI, no persistence, the three bodies are
// constants shipped in code. Which one applies is a pure function of
// the review's own state (sign-off + unresolved-thread count), so it is
// testable without a harness, a PTY, or React.

import type { SignoffState } from "./signoff.ts";

export const NUDGE_LAND = "The review is signed off. Land this branch per this repo's conventions.";

export const NUDGE_LAPSED =
	"I approved an earlier commit; review has lapsed since you committed. Check review_status.";

export const NUDGE_ADDRESS_COMMENTS =
	"Read the open review comments in Skein (list_comments) and address them.";

export interface Nudge {
	label: string;
	body: string;
}

/// Which nudge (if any) applies right now. `undefined` means there is
/// nothing to nudge about — the caller renders a disabled button rather
/// than omitting it, so "no nudge available" reads as a state rather
/// than as a missing feature.
export function selectNudge(signoff: SignoffState, unresolvedCount: number): Nudge | undefined {
	if (signoff === "approved") {
		return { label: "Nudge: land", body: NUDGE_LAND };
	}
	if (signoff === "stale") {
		return { label: "Nudge: lapsed", body: NUDGE_LAPSED };
	}
	if (unresolvedCount > 0) {
		return { label: "Nudge: address comments", body: NUDGE_ADDRESS_COMMENTS };
	}
	return undefined;
}
