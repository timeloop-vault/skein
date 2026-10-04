// designShow — pure pieces of the agent's show_element (#512): turning the
// iframe's answers into the verb's result, and the pending-request table
// that gives every iframe round trip a deadline.

import type { ShowElementResult } from "./designControl.ts";
import type { ElementDescriptor } from "./elementAnchor.ts";

/** How long the iframe has to answer before the agent gets `not_ready`. */
export const FRAME_TIMEOUT_MS = 3000;

/** The result for a CSS-selector request, from the frame's match count and
 *  the descriptor of the match (only sent for a unique one). */
export function selectorResult(
	count: number,
	element: ElementDescriptor | null,
	invalid = false,
): ShowElementResult {
	if (invalid)
		return { tier: "not_found", highlighted: false, element: null, invalidSelector: true };
	if (count === 1 && element) return { tier: "selector", highlighted: true, element, count };
	if (count > 1) return { tier: "ambiguous", highlighted: false, element: null, count };
	return { tier: "not_found", highlighted: false, element: null, count: 0 };
}

/** Settle a placement's claim against the highlight round trip: the verb
 *  never says `highlighted` unless the frame confirmed exactly one match.
 *  `shown` is null when the round trip rejected or timed out. */
export function confirmHighlight(
	result: ShowElementResult,
	shown: { count: number } | null,
): ShowElementResult {
	if (!result.highlighted) return result;
	if (shown && shown.count === 1) return result;
	return { ...result, highlighted: false };
}

/** Pending iframe round trips, keyed by requestId, each with a deadline. */
export class PendingRequests<T> {
	private seq = 0;
	private readonly waiting = new Map<
		string,
		{ resolve: (v: T) => void; reject: (e: Error) => void; timer: ReturnType<typeof setTimeout> }
	>();

	constructor(
		private readonly prefix: string,
		private readonly timeoutMs: number = FRAME_TIMEOUT_MS,
	) {}

	owns(requestId: string): boolean {
		return requestId.startsWith(this.prefix);
	}

	/** Start a request; `send` posts it to the frame with the id given. */
	begin(send: (requestId: string) => void): Promise<T> {
		this.seq += 1;
		const id = `${this.prefix}${this.seq}`;
		return new Promise<T>((resolve, reject) => {
			const timer = setTimeout(() => {
				this.waiting.delete(id);
				reject(new Error("not_ready: the design pane's frame did not answer"));
			}, this.timeoutMs);
			this.waiting.set(id, { resolve, reject, timer });
			try {
				send(id);
			} catch (e) {
				clearTimeout(timer);
				this.waiting.delete(id);
				reject(new Error(`not_ready: ${String(e)}`));
			}
		});
	}

	/** Answer a request; false when it is unknown (late or foreign). */
	settle(requestId: string, value: T): boolean {
		const w = this.waiting.get(requestId);
		if (!w) return false;
		clearTimeout(w.timer);
		this.waiting.delete(requestId);
		w.resolve(value);
		return true;
	}

	/** Reject everything outstanding (unmount, reload). */
	rejectAll(message: string): void {
		for (const [id, w] of this.waiting) {
			clearTimeout(w.timer);
			this.waiting.delete(id);
			w.reject(new Error(`not_ready: ${message}`));
		}
	}
}
