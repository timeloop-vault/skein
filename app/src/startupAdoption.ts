// #539 — adopting a second `startup` session id, decided by disk.
//
// Skein spawns `claude --session-id A` and tails `A.jsonl`. Observed
// (#539): Claude fired a second SessionStart ~9 s later with source
// `startup` and a DIFFERENT id B, and every conversation row went to
// `B.jsonl`; `A.jsonl` never existed. Yet `followedSession` drops every
// `startup` on purpose: anthropics/claude-code#78455 reports a PHANTOM
// startup with a different id that never materialises. Neither id can be
// trusted from the ping alone, so the transcripts on disk decide.
//
// Rule: adopt the candidate iff ITS transcript exists AND the bound one
// does not.
//   - bound exists            -> drop. The bound session is real, so the
//     candidate is the #78455 phantom or a nested child (`claude -p`),
//     whichever order they arrived in.
//   - bound missing, candidate exists -> adopt (the #539 case).
//   - neither exists          -> wait and probe again later: the real one
//     may simply not have written its first row yet.
//
// Accepted limit: an id change after the bound transcript already exists
// is not followed by this path (clear/resume/fork are `followedSession`'s).
//
// Accepted limit: anything that starts a `claude` inheriting this harness's
// SKEIN_HARNESS_ID and writes a transcript BEFORE the user's first prompt
// (e.g. a user SessionStart hook running `claude -p`) reads exactly like
// #539 (bound missing, candidate exists) and would be adopted. The rule
// cannot tell these apart, and the case is narrow.
//
// Pure and IO-free, like `shellClaim.ts`: the caller does the probes.

type Payload = { sessionId?: string | null; source?: string | null };

/// The candidate id worth probing, or null. `current` undefined means no
/// session is bound yet, which first-writer-wins capture owns.
export function startupCandidate(current: string | undefined, payload: Payload): string | null {
	if (payload.source !== "startup") return null;
	const id = payload.sessionId;
	if (typeof id !== "string" || id === "") return null;
	if (current === undefined || id === current) return null;
	return id;
}

export type AdoptionDecision = "adopt" | "drop" | "wait";

export function adoptionDecision(boundExists: boolean, candidateExists: boolean): AdoptionDecision {
	if (boundExists) return "drop";
	if (candidateExists) return "adopt";
	return "wait";
}

export type PendingAdoption = { roomId: string; bound: string; candidates: string[] };

const entries = new Map<string, PendingAdoption>();

export const startupAdoption = {
	/// Record a candidate for a harness. A different `bound` replaces the
	/// whole entry (the earlier candidates were against a stale binding).
	note(harnessId: string, roomId: string, bound: string, candidate: string): void {
		const cur = entries.get(harnessId);
		if (!cur || cur.bound !== bound) {
			entries.set(harnessId, { roomId, bound, candidates: [candidate] });
			return;
		}
		if (!cur.candidates.includes(candidate)) cur.candidates.push(candidate);
	},

	pending(harnessId: string): PendingAdoption | null {
		const cur = entries.get(harnessId);
		return cur ? { ...cur, candidates: [...cur.candidates] } : null;
	},

	/// Remove one candidate; the entry goes when it empties.
	drop(harnessId: string, candidate: string): void {
		const cur = entries.get(harnessId);
		if (!cur) return;
		cur.candidates = cur.candidates.filter((c) => c !== candidate);
		if (cur.candidates.length === 0) entries.delete(harnessId);
	},

	forget(harnessId: string): void {
		entries.delete(harnessId);
	},

	harnessIds(): string[] {
		return [...entries.keys()];
	},
};
