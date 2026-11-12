// Process-wide handle on the rooms autosave flush (#594). useRoomsHydrate
// registers the live scheduler's flush here; the exit paths (window close,
// Cmd+Q, updater install) call it without needing the store threaded to
// them — useAppWindowEffects mounts before the store exists.

let flusher: (() => Promise<void>) | null = null;

export function registerRoomsFlusher(fn: (() => Promise<void>) | null): void {
	flusher = fn;
}

let drainer: (() => Promise<void>) | null = null;

export function registerRoomsDrainer(fn: (() => Promise<void>) | null): void {
	drainer = fn;
}

/** Wait until the client's pending room mutations have reached the daemon
 *  (#731), bounded. Unlike `flushRoomsBounded` it does not ask the daemon to
 *  write to disk. A no-op before the sync exists. */
export async function drainRoomsBounded(timeoutMs = 3000): Promise<void> {
	if (!drainer) return;
	let timer: ReturnType<typeof setTimeout> | undefined;
	try {
		await Promise.race([
			drainer(),
			new Promise<void>((resolve) => {
				timer = setTimeout(resolve, timeoutMs);
			}),
		]);
	} catch (err) {
		console.error("[skein] rooms drain failed:", err);
	} finally {
		if (timer !== undefined) clearTimeout(timer);
	}
}

/** Flush any pending rooms save, bounded so an exit path can never hang. */
export async function flushRoomsBounded(timeoutMs = 1500): Promise<void> {
	if (!flusher) return;
	let timer: ReturnType<typeof setTimeout> | undefined;
	try {
		await Promise.race([
			flusher(),
			new Promise<void>((resolve) => {
				timer = setTimeout(resolve, timeoutMs);
			}),
		]);
	} catch (err) {
		console.error("[skein] rooms flush failed:", err);
	} finally {
		if (timer !== undefined) clearTimeout(timer);
	}
}
