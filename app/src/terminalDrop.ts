// terminalDrop — the pure drag-overlay controller behind a terminal's file drop (#41, #733 B).
//
// On drag-enter the overlay shows the local answer at once (terminal exists
// and is registered); a local refusal is final. Otherwise the daemon's
// `insert` answer is asked ONCE per drag and replaces the local OK, so a
// refusal reason (a permission dialog, say) shows as before. An answer that
// lands after the drag ended (leave, drop, dispose) is dropped.

import type { GateResult } from "./harnessInputGate.ts";

export interface DropGateDeps {
	/** The synchronous local check; null = no local refusal. */
	local: () => GateResult | null;
	/** The daemon's `insert` answer. */
	ask: () => Promise<GateResult>;
	/** Render the overlay (null = hide it). */
	set: (gate: GateResult | null) => void;
}

export interface DropGate {
	/** The pointer is over the pane; the first call per drag starts the check. */
	hover(): void;
	/** The drag ended or left the pane (leave, drop, outside hit-test, unmount). */
	end(): void;
}

export function createDropGate({ local, ask, set }: DropGateDeps): DropGate {
	let seq = 0;
	let active = false;

	return {
		hover() {
			if (active) return;
			active = true;
			const mine = ++seq;
			const refusal = local();
			if (refusal) {
				set(refusal);
				return;
			}
			set({ ok: true });
			void ask().then((answer) => {
				if (!active || mine !== seq) return;
				set(answer);
			});
		},
		end() {
			const wasActive = active;
			active = false;
			seq++;
			if (wasActive) set(null);
		},
	};
}
