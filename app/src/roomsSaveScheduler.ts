// Coalesced autosave for the rooms blob (#594). Every `rooms` change used
// to ship the whole array through `db_save_rooms`; badge/phase churn made
// that a stream of full wipe+re-insert transactions.
//
// Rust reads the PERSISTED rooms on many paths (agent token auth, fs scope,
// open requests, list_rooms/get_room, create/close room and harness verbs
// that read the db and then round-trip to the webview). Those only care
// WHICH rooms/harnesses exist and their scope, so a change to the
// `structuralKey` saves immediately — that covers add/remove, archive/
// reopen/retire, cwd repoint, harness add/remove/reorder and every agent
// round-trip. Anything else (badges, phases, session ids, titles…) is
// debounced and flushed on close / quit / update / page hide.
//
// Pure: no React, no Tauri. Timers are injectable for tests.

import type { Room } from "./types.ts";

export const SAVE_DELAY_MS = 500;
export const SAVE_MAX_WAIT_MS = 2000;

/** Everything a Rust-side read decides existence or scope on. */
export function structuralKey(rooms: readonly Room[]): string {
	return JSON.stringify(
		rooms.map((r) => [
			r.id,
			r.name,
			r.archived ?? null,
			r.retired ?? null,
			r.cwd ?? null,
			r.repoRoot ?? null,
			r.branch ?? null,
			r.createdBy ? [r.createdBy.roomId, r.createdBy.harnessId] : null,
			r.closedBy ? [r.closedBy.roomId, r.closedBy.harnessId ?? null, r.closedBy.at] : null,
			r.harnesses.map((h) => [
				h.id,
				h.kind,
				h.name,
				h.agent ?? null,
				h.cwd ?? null,
				h.remote ?? null,
				h.sessionId ?? null,
				h.createdBy ? [h.createdBy.roomId, h.createdBy.harnessId ?? null] : null,
			]),
		]),
	);
}

export interface RoomsSaveScheduler {
	/** Report the latest rooms; saves now if structural, else debounces. */
	update(rooms: Room[]): void;
	/** Save the latest rooms now if anything is pending. Never rejects. */
	flush(): Promise<void>;
	pending(): boolean;
	/** Cancel the timer; nothing more is saved. */
	dispose(): void;
}

export interface RoomsSaveSchedulerOptions {
	save: (rooms: Room[]) => Promise<unknown>;
	/** Checked when the debounce timer fires; false drops the pending save. */
	canSave?: () => boolean;
	delayMs?: number;
	maxWaitMs?: number;
	setTimer?: (fn: () => void, ms: number) => unknown;
	clearTimer?: (handle: unknown) => void;
}

export function createRoomsSaveScheduler(opts: RoomsSaveSchedulerOptions): RoomsSaveScheduler {
	const delayMs = opts.delayMs ?? SAVE_DELAY_MS;
	const maxWaitMs = opts.maxWaitMs ?? SAVE_MAX_WAIT_MS;
	const setTimer = opts.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
	const clearTimer = opts.clearTimer ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));

	let latest: Room[] | null = null;
	let lastKey: string | null = null;
	let timer: unknown = null;
	let dirtySince = 0;
	let isPending = false;
	let disposed = false;
	let inflight: Promise<void> = Promise.resolve();

	const cancel = () => {
		if (timer !== null) clearTimer(timer);
		timer = null;
	};

	// Saves are chained so invokes are issued in order (Rust mints its seq
	// ticket inside the async command). A failed save is logged and re-armed.
	const saveNow = (rooms: Room[]): Promise<void> => {
		cancel();
		isPending = false;
		const p = inflight
			.then(() => opts.save(rooms))
			.then(
				() => undefined,
				(err: unknown) => {
					const msg = err instanceof Error ? err.message : String(err);
					console.error("[skein] db_save_rooms failed:", msg);
					if (disposed || rooms !== latest) return;
					isPending = true;
					dirtySince = Date.now();
					arm(maxWaitMs);
				},
			);
		inflight = p;
		return p;
	};

	const arm = (wait: number) => {
		cancel();
		timer = setTimer(() => {
			timer = null;
			if (!latest) return;
			if (opts.canSave && !opts.canSave()) {
				isPending = false;
				return;
			}
			void saveNow(latest);
		}, wait);
	};

	return {
		update(rooms) {
			if (disposed) return;
			latest = rooms;
			const key = structuralKey(rooms);
			if (key !== lastKey) {
				lastKey = key;
				void saveNow(rooms);
				return;
			}
			const now = Date.now();
			if (!isPending) {
				isPending = true;
				dirtySince = now;
			}
			arm(Math.max(0, Math.min(delayMs, dirtySince + maxWaitMs - now)));
		},
		flush() {
			if (!isPending || !latest) return inflight;
			return saveNow(latest);
		},
		pending: () => isPending,
		dispose() {
			disposed = true;
			cancel();
			isPending = false;
		},
	};
}
