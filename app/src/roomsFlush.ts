// Process-wide handle on the rooms autosave flush (#594). useRoomsHydrate
// registers the live scheduler's flush here; the exit paths (window close,
// Cmd+Q, updater install) call it without needing the store threaded to
// them — useAppWindowEffects mounts before the store exists.

let flusher: (() => Promise<void>) | null = null;

export function registerRoomsFlusher(fn: (() => Promise<void>) | null): void {
	flusher = fn;
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
