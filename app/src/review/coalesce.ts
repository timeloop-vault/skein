// Single-flight-with-trailing-rerun for the review pane's refresh fetches
// (#171 slice c). The watcher ticks ~5/s during a build (the Rust side
// debounces 200 ms per change, not per burst of changes), and each tick
// used to start its own `review_scope` / `review_file` invoke — a full
// worktree diff — with nothing to stop them piling up in flight together.
//
// A coalescer wraps one async `run`: while a call is in flight, further
// `request()`s don't start a new run, they just mark the run "dirty".
// When the in-flight run settles — success OR error — a dirty coalescer
// runs exactly once more. N requests during one flight collapse to at
// most one trailing run, never N.
//
// `dispose()` is the other half of the contract: a hook effect creates a
// fresh coalescer per "real" key (room/cwd/scope/path/commitSha) and
// disposes the old one on cleanup. A result that arrives after dispose —
// because its key changed out from under it — is dropped rather than
// applied, the same job the hooks' own `cancelled` flags already did.

export type CoalescedResult<T> = { ok: true; value: T } | { ok: false; error: unknown };

export interface Coalescer {
	/** Ask for a run. Starts immediately if idle, otherwise marks dirty
	 *  for exactly one trailing rerun once the in-flight run settles. */
	request: () => void;
	/** Stop starting new runs and stop delivering settled ones. */
	dispose: () => void;
}

export function createCoalescer<T>(
	run: () => Promise<T>,
	onSettle: (result: CoalescedResult<T>) => void,
): Coalescer {
	let inFlight = false;
	let dirty = false;
	let disposed = false;

	const start = () => {
		inFlight = true;
		run()
			// Two-argument `then`, not `.then().catch()`: a throwing success
			// handler must not be reported a second time as a failed run.
			.then(
				(value) => {
					if (disposed) return;
					onSettle({ ok: true, value });
				},
				(error: unknown) => {
					if (disposed) return;
					onSettle({ ok: false, error });
				},
			)
			.finally(() => {
				inFlight = false;
				if (disposed) return;
				if (dirty) {
					dirty = false;
					start();
				}
			});
	};

	return {
		request: () => {
			if (disposed) return;
			if (inFlight) {
				dirty = true;
				return;
			}
			start();
		},
		dispose: () => {
			disposed = true;
		},
	};
}

/// True when two fetch responses are the same value, so a caller can skip
/// the `setState` and the re-render it would trigger. `JSON.stringify`
/// comparison is enough for these DTOs — plain data, no cycles, no
/// functions — and cheap next to the fetch it is guarding.
export function sameJson(a: unknown, b: unknown): boolean {
	if (a === b) return true;
	return JSON.stringify(a) === JSON.stringify(b);
}
