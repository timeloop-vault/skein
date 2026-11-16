// The two facts only the webview has, reported to the daemon (#733):
// whether a harness's composer holds a draft (what `checkDraft` would
// refuse an automatic send for, with its refusal text) and the user-input
// counter. The daemon delivers prompts and mail and needs both to do it
// safely. Pure: what to report, and whether it differs from the last report
// for that harness, and the rising `report` number that lets the daemon
// drop a report that arrives out of order. The wiring is `useDaemonDelivery.ts`.

import type { ComposerDraft } from "./composerDraft.ts";
import { checkDraft } from "./composerDraft.ts";

/// The `draft` argument of `harness_client_facts`. `reason` only when held.
export type DraftFact = { held: false } | { held: true; reason: string };

export interface ClientFacts {
	draft: DraftFact;
	userInputSeq: number;
}

export function clientFacts(draft: ComposerDraft, userInputSeq: number): ClientFacts {
	const refusal = checkDraft(draft);
	return {
		draft: refusal === null ? { held: false } : { held: true, reason: refusal.reason },
		userInputSeq,
	};
}

export function sameFacts(a: ClientFacts, b: ClientFacts): boolean {
	return (
		a.userInputSeq === b.userInputSeq &&
		a.draft.held === b.draft.held &&
		(a.draft.held ? a.draft.reason : "") === (b.draft.held ? b.draft.reason : "")
	);
}

/// Remembers the last facts sent per harness and says whether a new reading
/// is worth sending. `force` (a terminal registering) sends even when
/// identical, since the daemon may have dropped what it knew with the old PTY.
export function createFactsDiffer() {
	const lastSent = new Map<string, ClientFacts>();
	return {
		/// The facts to send now, or null when identical to the last send.
		next(harnessId: string, facts: ClientFacts, force = false): ClientFacts | null {
			const prev = lastSent.get(harnessId);
			if (!force && prev !== undefined && sameFacts(prev, facts)) return null;
			lastSent.set(harnessId, facts);
			return facts;
		},
		forget(harnessId: string): void {
			lastSent.delete(harnessId);
		},
	};
}

/// The `report` argument of `harness_client_facts`: a number that rises with
/// every call, so the daemon can drop a report that overtook a newer one.
/// Seeded from the clock, so a reloaded page always sends larger numbers than
/// the page it replaced (the daemon keeps the last per harness).
export function createReportCounter(seed: number = Date.now()): () => number {
	let n = seed;
	return () => {
		n += 1;
		return n;
	};
}
