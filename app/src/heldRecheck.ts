// Re-reads the screen of a harness whose mail is held (#733). The old
// webview flow called `harnessInput.checkScreen` before every automatic mail
// check, which is what released an `unknown` draft latch (#413); the daemon
// now does the checking and never sees a screen, so while mail is held the
// client re-reads it itself: once when the hold starts, then every
// `HELD_RECHECK_MS` until the hold ends or the harness unregisters. A
// released latch reaches the daemon as Held -> Clean (`clientFacts.ts`),
// which is what makes it deliver. Pure timing logic; the wiring is
// `useDaemonDelivery.ts`.

export const HELD_RECHECK_MS = 2_000;

export interface HeldRecheckDeps {
	/// `harnessInput.checkScreen`.
	check: (harnessId: string) => void;
	/// `harnessInput.isRegistered`.
	isRegistered: (harnessId: string) => boolean;
	setInterval?: (fn: () => void, ms: number) => unknown;
	clearInterval?: (handle: unknown) => void;
}

export function createHeldRecheck(deps: HeldRecheckDeps) {
	const start = deps.setInterval ?? ((fn, ms) => setInterval(fn, ms));
	const stop = deps.clearInterval ?? ((h) => clearInterval(h as ReturnType<typeof setInterval>));
	const timers = new Map<string, unknown>();
	const end = (id: string): void => {
		if (!timers.has(id)) return;
		stop(timers.get(id));
		timers.delete(id);
	};
	return {
		/// The harness's held flag changed (true = the daemon holds its mail).
		setHeld(id: string, held: boolean): void {
			if (!held) {
				end(id);
				return;
			}
			if (timers.has(id)) return;
			deps.check(id);
			timers.set(
				id,
				start(() => {
					if (!deps.isRegistered(id)) {
						end(id);
						return;
					}
					deps.check(id);
				}, HELD_RECHECK_MS),
			);
		},
		dispose(): void {
			for (const id of [...timers.keys()]) end(id);
		},
		/// Harness ids being re-read, for tests.
		active(): string[] {
			return [...timers.keys()];
		},
	};
}
