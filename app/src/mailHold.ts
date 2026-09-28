// Per-harness "mail held by the draft guard" marker (#413), the same tiny
// store + useSyncExternalStore shape as `mailStore.ts`. `useMailDelivery.ts`
// writes it (the decision itself is `mailHeld` in mailNudge.ts); the tab
// reads it and can ask for a "deliver now" via `release`, which
// `useMailDelivery` subscribes to with `onRelease`. `get` is a plain
// selector so a non-React view (#414) can read it too.

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
const releaseListeners = new Set<(harnessId: string) => void>();

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
		if (!holds.delete(harnessId)) return;
		emit(harnessId);
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

	/// Ask for held mail to be delivered now, bypassing only the draft guard.
	release(harnessId: string): void {
		for (const cb of [...releaseListeners]) cb(harnessId);
	},

	onRelease(cb: (harnessId: string) => void): () => void {
		releaseListeners.add(cb);
		return () => {
			releaseListeners.delete(cb);
		};
	},
};

export function useMailHold(harnessId: string): MailHold {
	return useSyncExternalStore(
		(cb) => mailHold.subscribe(harnessId, cb),
		() => mailHold.get(harnessId),
	);
}
