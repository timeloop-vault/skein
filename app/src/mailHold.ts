// Per-harness "mail held by the draft guard" marker (#413), the same tiny
// store + useSyncExternalStore shape as `mailStore.ts`. A MIRROR since #733:
// the daemon decides what is held and `useMailDisplay.ts` writes it from
// `mail_held_snapshot` / `skein://mail-held`; the tab reads it. `get` is a
// plain selector so a non-React view (#414) can read it too.

import { useSyncExternalStore } from "react";

export interface MailHold {
	held: boolean;
	/// Why the last "deliver now" was refused; cleared by the next nudge
	/// actually sent, or by any later check that finds nothing held.
	releaseRefusal?: string;
}

const NOT_HELD: MailHold = { held: false };

const holds = new Map<string, MailHold>();
const listeners = new Map<string, Set<() => void>>();

const heldListeners = new Set<(harnessId: string, held: boolean) => void>();

const emitHeld = (harnessId: string, held: boolean): void => {
	for (const cb of [...heldListeners]) cb(harnessId, held);
};

const emit = (harnessId: string): void => {
	for (const cb of listeners.get(harnessId) ?? []) cb();
};

export const mailHold = {
	/// Record whether mail is held. Not held drops the entry (and any
	/// releaseRefusal). No emit when nothing changed.
	set(harnessId: string, held: boolean): void {
		const cur = holds.get(harnessId);
		if (!held) {
			if (!cur) return;
			holds.delete(harnessId);
		} else {
			if (cur?.held) return;
			holds.set(harnessId, { held: true });
		}
		emit(harnessId);
		if ((cur?.held ?? false) !== held) emitHeld(harnessId, held);
	},

	/// `cb(id, held)` whenever a harness's held flag turns true or false
	/// (not for a releaseRefusal-only change). Returns an unsubscribe.
	subscribeHeld(cb: (harnessId: string, held: boolean) => void): () => void {
		heldListeners.add(cb);
		return () => {
			heldListeners.delete(cb);
		};
	},

	/// Record why a deliver-now attempt was refused (kept on the entry,
	/// held or not, until the next check that finds nothing held).
	setReleaseRefusal(harnessId: string, reason: string): void {
		const cur = holds.get(harnessId);
		if (cur?.releaseRefusal === reason) return;
		holds.set(harnessId, { held: cur?.held ?? false, releaseRefusal: reason });
		emit(harnessId);
	},

	forget(harnessId: string): void {
		const cur = holds.get(harnessId);
		if (!holds.delete(harnessId)) return;
		emit(harnessId);
		if (cur?.held) emitHeld(harnessId, false);
	},

	get(harnessId: string): MailHold {
		return holds.get(harnessId) ?? NOT_HELD;
	},

	subscribe(harnessId: string, cb: () => void): () => void {
		let set = listeners.get(harnessId);
		if (!set) {
			set = new Set();
			listeners.set(harnessId, set);
		}
		set.add(cb);
		return () => {
			const s = listeners.get(harnessId);
			if (!s) return;
			s.delete(cb);
			if (s.size === 0) listeners.delete(harnessId);
		};
	},
};

export function useMailHold(harnessId: string): MailHold {
	return useSyncExternalStore(
		(cb) => mailHold.subscribe(harnessId, cb),
		() => mailHold.get(harnessId),
	);
}
