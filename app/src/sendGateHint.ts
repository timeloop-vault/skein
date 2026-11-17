// sendGateHint — the pure controller behind the render-time send hint (#733 B).
//
// A disabled-button tooltip needs a synchronous answer per prompt body, the
// daemon's answer is async. The only body-dependent rule is "a multi-line
// body needs paste mode", so the controller asks twice per refresh (a
// single-line and a multi-line probe) and `get` picks by newline. Answers
// from a superseded refresh are dropped. A refusal that clears by time
// ("settling", "busy") re-polls every 2 s, any other refusal every 5 s (it can
// clear with no phase change); an ok answer does not poll, a phase change
// refreshes it through the hook, and the daemon decides at click time anyway.

import type { GateResult } from "./harnessInputGate.ts";

export const SINGLE_PROBE = "x";
export const MULTI_PROBE = "x\ny";
/// Re-poll interval while the refusal clears by time ("settling", "busy").
export const GATE_POLL_MS = 2000;
/// Slow re-poll while any other refusal stands: the wire no longer carries the
/// facts behind them, so they can clear with no phase-mirror update.
export const GATE_SLOW_POLL_MS = 5000;

const TIMED_CODES = new Set(["settling", "busy"]);

export type GateAsk = (body: string) => Promise<GateResult>;

export interface SendGateHint {
	refresh(): void;
	get(body: string): GateResult;
	subscribe(cb: () => void): () => void;
	dispose(): void;
}

const isTimed = (r: GateResult): boolean =>
	!r.ok && r.code !== undefined && TIMED_CODES.has(r.code);

export function createSendGateHint(
	ask: GateAsk,
	pollMs = GATE_POLL_MS,
	slowPollMs = GATE_SLOW_POLL_MS,
): SendGateHint {
	let single: GateResult = { ok: true };
	let multi: GateResult = { ok: true };
	let seq = 0;
	let disposed = false;
	let timer: ReturnType<typeof setTimeout> | null = null;
	const listeners = new Set<() => void>();

	const clearTimer = () => {
		if (timer !== null) clearTimeout(timer);
		timer = null;
	};

	const refresh = () => {
		if (disposed) return;
		const mine = ++seq;
		clearTimer();
		void Promise.all([ask(SINGLE_PROBE), ask(MULTI_PROBE)]).then(([s, m]) => {
			if (disposed || mine !== seq) return;
			single = s;
			multi = m;
			for (const cb of [...listeners]) cb();
			if (isTimed(s) || isTimed(m)) timer = setTimeout(refresh, pollMs);
			else if (!s.ok || !m.ok) timer = setTimeout(refresh, slowPollMs);
		});
	};

	return {
		refresh,
		get: (body) => (body.includes("\n") ? multi : single),
		subscribe(cb) {
			listeners.add(cb);
			return () => listeners.delete(cb);
		},
		dispose() {
			disposed = true;
			clearTimer();
			listeners.clear();
		},
	};
}
