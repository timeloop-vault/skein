// Diff-based sync of the rooms state with the daemon's room store (#729).
// Replaces the coalesced wholesale save (#594): the daemon now decides what is
// written promptly and what is debounced, so the client sends every change.
//
// Pure: no React, no Tauri. Calls and timers are injected.

import { applyRoomChanges } from "./roomChanges";
import {
	diffRooms,
	type RoomChange,
	type RoomMutation,
	type RoomsApplyOutcome,
	type RoomsChanged,
} from "./roomMutations";
import { rebaseRemote } from "./roomsRebase";
import type { Room } from "./types";

export const SYNC_RETRY_MS = 2000;

export interface RoomsSync {
	/** The rooms the daemon is known to hold (after a successful load). */
	setBaseline(rooms: Room[]): void;
	/** Report the latest local rooms; sends the diff unless one call is in flight. */
	update(rooms: Room[]): void;
	/**
	 * Start buffering foreign events (call before the load). Clears the queue
	 * and any baseline, so calling it again is the retry of a failed load.
	 */
	beginHydrate(): void;
	/**
	 * `rows` as the daemon returned them, `local` what the hydrate installs.
	 * Sets the baseline to rows + queued changes, ends buffering and returns
	 * `local` with the queued changes merged in (same reference when none).
	 */
	finishHydrate(rows: readonly Room[], local: readonly Room[]): Room[];
	/**
	 * A daemon change event. Null for our own echo, no changes or no baseline.
	 * Otherwise moves the baseline once, now, and returns a PURE rebase to use
	 * as a setState updater (same reference back when nothing changes).
	 */
	remote(payload: RoomsChanged): ((local: readonly Room[]) => Room[]) | null;
	/** Drain the chain, send the latest diff, then flush the daemon. Never rejects. */
	flush(): Promise<void>;
	dispose(): void;
}

export interface RoomsSyncOptions {
	clientId: string;
	apply: (mutations: RoomMutation[]) => Promise<RoomsApplyOutcome>;
	flushDaemon: () => Promise<unknown>;
	/** False sends nothing (not loaded / load failed). */
	canSync?: () => boolean;
	onError?: (what: string, err: unknown) => void;
	retryMs?: number;
	setTimer?: (fn: () => void, ms: number) => unknown;
	clearTimer?: (handle: unknown) => void;
}

export function createRoomsSync(opts: RoomsSyncOptions): RoomsSync {
	const retryMs = opts.retryMs ?? SYNC_RETRY_MS;
	const setTimer = opts.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
	const clearTimer = opts.clearTimer ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
	const onError =
		opts.onError ??
		((what, err) => {
			console.error(`[skein] ${what} failed:`, err instanceof Error ? err.message : String(err));
		});

	let baseline: Room[] | null = null;
	let latest: Room[] | null = null;
	let running = false;
	let chain: Promise<void> = Promise.resolve();
	let retryTimer: unknown = null;
	let disposed = false;
	// Remote changes received while a call is in flight; re-applied on top of
	// the snapshot that call sent.
	let flightRemote: RoomChange[] = [];
	// Foreign changes received between beginHydrate and finishHydrate.
	let hydrating = false;
	let queued: RoomChange[] = [];

	const cancelRetry = () => {
		if (retryTimer !== null) clearTimer(retryTimer);
		retryTimer = null;
	};

	const run = async (): Promise<void> => {
		try {
			for (;;) {
				if (disposed || baseline === null || latest === null) return;
				if (opts.canSync && !opts.canSync()) return;
				const snap = latest;
				const mutations = diffRooms(baseline, snap);
				if (mutations.length === 0) return;
				flightRemote = [];
				let outcome: RoomsApplyOutcome;
				try {
					outcome = await opts.apply(mutations);
				} catch (err) {
					onError("rooms_apply", err);
					// A newer snapshot may have arrived mid-flight; its update() was
					// swallowed by this call, so the retry is what sends it.
					if (!disposed) {
						retryTimer = setTimer(() => {
							retryTimer = null;
							void pump();
						}, retryMs);
					}
					return;
				}
				for (const s of outcome.skipped ?? []) {
					console.warn(
						`[skein] rooms_apply skipped mutation ${s.index} (${mutations[s.index]?.type ?? "?"}): ${s.reason}`,
					);
				}
				baseline = applyRoomChanges(snap, flightRemote);
			}
		} finally {
			running = false;
		}
	};

	const pump = (): Promise<void> => {
		if (running) return chain;
		cancelRetry();
		running = true;
		chain = run();
		return chain;
	};

	return {
		setBaseline(rooms) {
			baseline = rooms;
		},
		beginHydrate() {
			hydrating = true;
			queued = [];
			baseline = null;
		},
		finishHydrate(rows, local) {
			const changes = queued;
			queued = [];
			hydrating = false;
			baseline = applyRoomChanges(rows, changes);
			return rebaseRemote(rows, local, changes).local;
		},
		update(rooms) {
			if (disposed) return;
			latest = rooms;
			void pump();
		},
		remote(payload) {
			if (payload.origin === opts.clientId || payload.changes.length === 0) return null;
			if (hydrating) {
				queued.push(...payload.changes);
				return null;
			}
			if (baseline === null) return null;
			const oldBaseline = baseline;
			const changes = payload.changes;
			baseline = applyRoomChanges(oldBaseline, changes);
			if (running) flightRemote.push(...changes);
			return (local) => rebaseRemote(oldBaseline, local, changes).local;
		},
		async flush() {
			while (running) await chain;
			await pump();
			try {
				await opts.flushDaemon();
			} catch (err) {
				onError("rooms_flush", err);
			}
		},
		dispose() {
			disposed = true;
			cancelRetry();
		},
	};
}
