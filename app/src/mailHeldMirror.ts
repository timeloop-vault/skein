// Mirrors the daemon's held-mail set into `mailHold` (#733). The daemon owns
// the decision; this only applies `mail_held_snapshot` (`{ seq, harnessIds }`)
// and `skein://mail-held` events (`{ harnessId, held, seq }`) in an order that
// cannot lose one: events that arrive before the snapshot answer are buffered
// and the ones newer than the snapshot are replayed on top of it. The daemon
// updates the held set before it emits, and the snapshot reads its seq before
// the set, so an event at or below the snapshot seq is already in the set.
// Later events are absolute facts and safe to re-apply. An unstamped event
// (no seq, or 0) is always treated as newer.

interface MailHeldEvent {
	harnessId: string;
	held: boolean;
	seq?: number;
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
		snapshot(ids: readonly string[], seq = 0): void {
			const incoming = new Set(ids);
			for (const id of [...held]) if (!incoming.has(id)) set(id, false);
			for (const id of incoming) set(id, true);
			seeded = true;
			const replay = buffered;
			buffered = [];
			for (const e of replay) {
				if (e.seq !== undefined && e.seq > 0 && e.seq <= seq) continue;
				set(e.harnessId, e.held);
			}
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
