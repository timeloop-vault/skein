// "Show in design pane" (#434): the review pane asks a design harness to
// focus an element thread. The design body may be unmounted or hidden when
// the request is made, so the latest request per harness is kept until the
// body takes it; a mounted body hears it through `subscribeDesignFocus`.

import type { Harness } from "./types.ts";

export type DesignFocus = { roomId: string; harnessId: string; entry: string; threadId: string };

const pending = new Map<string, DesignFocus>();
const subs = new Map<string, Set<(f: DesignFocus) => void>>();

export function publishDesignFocus(f: DesignFocus): void {
	const listeners = [...(subs.get(f.harnessId) ?? [])];
	// A mounted body that heard it has handled it; only an absent one
	// needs the request kept for when it mounts.
	if (listeners.length === 0) pending.set(f.harnessId, f);
	else pending.delete(f.harnessId);
	for (const cb of listeners) cb(f);
}

export function subscribeDesignFocus(harnessId: string, cb: (f: DesignFocus) => void): () => void {
	let set = subs.get(harnessId);
	if (!set) {
		set = new Set();
		subs.set(harnessId, set);
	}
	set.add(cb);
	return () => {
		set.delete(cb);
		if (set.size === 0 && subs.get(harnessId) === set) subs.delete(harnessId);
	};
}

/** The pending request for a body that mounted or became visible after
 *  it was published. Clears it. */
export function takeDesignFocus(harnessId: string): DesignFocus | undefined {
	const f = pending.get(harnessId);
	pending.delete(harnessId);
	return f;
}

/** Which design harness shows `entry`: the first already on it, else the
 *  first design harness (which then has to be pointed at `entry`). */
export function pickDesignHarness(
	room: { harnesses: Pick<Harness, "id" | "kind" | "designEntry">[] },
	entry: string,
): { harnessId: string; setEntry: boolean } | null {
	const designs = room.harnesses.filter((h) => h.kind === "design");
	const exact = designs.find((h) => h.designEntry === entry);
	if (exact) return { harnessId: exact.id, setEntry: false };
	const first = designs[0];
	return first ? { harnessId: first.id, setEntry: true } : null;
}
