// Per-harness unread-mail marker (#329), mirroring the tiny-store +
// useSyncExternalStore shape `subagents.ts` and `harnessActivity.ts`
// already use — a plain module-level Map with a matching listener set,
// no React state.
//
// This module owns only the DISPLAY side: "how much unread mail does
// this harness have right now, and who is it from" — enough to render
// the tab marker. It never fetches or decides anything itself;
// `useMailDelivery.ts` is what calls `mail_unread`, decides whether to
// nudge, and writes the result in here via `mailStore.set`.

import { useSyncExternalStore } from "react";

export interface UnreadMail {
	count: number;
	/// Distinct sender room names, oldest-unread-first — same order the
	/// backend's `unread_mail` returns, straight from `mail_unread`.
	fromRoomNames: readonly string[];
}

const EMPTY: UnreadMail = { count: 0, fromRoomNames: [] };

const unread = new Map<string, UnreadMail>();
const listeners = new Map<string, Set<() => void>>();

const emit = (harnessId: string): void => {
	for (const cb of listeners.get(harnessId) ?? []) cb();
};

export const mailStore = {
	/// Record the latest known unread state for `harnessId`. No-op (and
	/// no emit) when nothing actually changed, so a redundant re-fetch
	/// doesn't re-render every subscriber.
	set(harnessId: string, count: number, fromRoomNames: readonly string[]): void {
		const cur = unread.get(harnessId);
		if (
			cur &&
			cur.count === count &&
			cur.fromRoomNames.length === fromRoomNames.length &&
			cur.fromRoomNames.every((n, i) => n === fromRoomNames[i])
		) {
			return;
		}
		if (count === 0 && fromRoomNames.length === 0) {
			if (!cur) return;
			unread.delete(harnessId);
		} else {
			unread.set(harnessId, { count, fromRoomNames: [...fromRoomNames] });
		}
		emit(harnessId);
	},

	/// Drop a harness entirely. Wired into the same teardown path as
	/// `harnessActivity.forget`/`subagents.forget` — otherwise this map
	/// grows for the life of the app.
	forget(harnessId: string): void {
		if (!unread.delete(harnessId)) return;
		emit(harnessId);
	},

	get(harnessId: string): UnreadMail {
		return unread.get(harnessId) ?? EMPTY;
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

/// React hook: this harness's current unread-mail marker, kept live.
export function useUnreadMail(harnessId: string): UnreadMail {
	return useSyncExternalStore(
		(cb) => mailStore.subscribe(harnessId, cb),
		() => mailStore.get(harnessId),
	);
}
