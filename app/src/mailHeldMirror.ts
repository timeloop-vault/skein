// Mirrors the daemon's held-mail set into `mailHold` (#733). The daemon owns
// the decision; this only applies `mail_held_snapshot` (an array of harness
// ids) and `skein://mail-held` events (`{ harnessId, held }`) in an order that
// cannot lose one: events that arrive before the snapshot answer are buffered
// and replayed on top of it, since an event is newer than a snapshot request
// made after the listener was attached.

export interface MailHeldEvent {
	harnessId: string;
	held: boolean;
}

export function createHeldMirror(apply: (harnessId: string, held: boolean) => void) {
	let seeded = false;
	let buffered: MailHeldEvent[] = [];
	const held = new Set<string>();
	const set = (id: string, value: boolean): void => {
		if (value === held.has(id)) return;
		if (value) held.add(id);
		else held.delete(id);
		apply(id, value);
	};
	return {
		event(e: MailHeldEvent): void {
			if (!seeded) {
				buffered.push(e);
				return;
			}
			set(e.harnessId, e.held);
		},
		snapshot(ids: readonly string[]): void {
			const incoming = new Set(ids);
			for (const id of [...held]) if (!incoming.has(id)) set(id, false);
			for (const id of incoming) set(id, true);
			seeded = true;
			const replay = buffered;
			buffered = [];
			for (const e of replay) set(e.harnessId, e.held);
		},
		/// A harness that no longer exists.
		forget(id: string): void {
			held.delete(id);
		},
		/// Ids currently held, for tests.
		heldIds(): string[] {
			return [...held];
		},
	};
}
