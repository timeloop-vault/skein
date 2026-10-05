// Pure helpers for the pop-out broadcast (#493): a leading+trailing
// throttle, a "did anything visible change" test, and the entry-point
// routing decision.

import type { CcSnapshotPayload } from "./popoutProtocol.ts";

export const SNAPSHOT_THROTTLE_MS = 250;
/** `now` alone is re-sent at least this often so relative times keep moving. */
export const NOW_REFRESH_MS = 20_000;

export interface Throttled<T> {
	push(value: T): void;
	/** Send the pending value (if any) right now. */
	flush(): void;
	cancel(): void;
}

/** At most one `send` per `ms`: the first push goes out at once, later ones
 *  within the window collapse to the newest, sent when the window ends. */
export function createThrottle<T>(send: (value: T) => void, ms: number): Throttled<T> {
	let last = Number.NEGATIVE_INFINITY;
	let pending: { value: T } | null = null;
	let timer: ReturnType<typeof setTimeout> | null = null;

	const fire = () => {
		timer = null;
		if (!pending) return;
		const { value } = pending;
		pending = null;
		last = Date.now();
		send(value);
	};
	return {
		push(value) {
			pending = { value };
			if (timer !== null) return;
			const wait = last + ms - Date.now();
			if (wait <= 0) fire();
			else timer = setTimeout(fire, wait);
		},
		flush() {
			if (timer !== null) clearTimeout(timer);
			fire();
		},
		cancel() {
			if (timer !== null) clearTimeout(timer);
			timer = null;
			pending = null;
		},
	};
}

/** Whether `next` is worth sending after `prev` (the last one sent). */
export function snapshotChanged(
	prev: { json: string; now: number } | null,
	next: CcSnapshotPayload,
): { send: boolean; json: string } {
	const json = JSON.stringify({ s: next.sections, a: next.activeRoomId });
	if (!prev) return { send: true, json };
	return { send: json !== prev.json || next.now - prev.now >= NOW_REFRESH_MS, json };
}

/** What the strip button / Mod+0 / palette do: raise the pop-out when there
 *  is one, otherwise toggle the in-app view as before. */
export function toggleAction(poppedOut: boolean): "raise" | "toggle" {
	return poppedOut ? "raise" : "toggle";
}
