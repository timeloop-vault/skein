// The screen-reading feed (#623, #732). A TUI that never goes byte-quiet
// (Hermes redraws a clock and a spinner constantly) can't reach L2a idle or
// an L2b tail match, so the daemon's phase machine moves it between running
// and waiting from what its rendered screen says. Only the webview has the
// xterm buffer, so once a second this reads it for every non-authoritative
// harness that is running or waiting and forwards a CHANGED reading to the
// daemon, which owns the debounce and every transition.

import { type PhaseInput, sendPhaseInput } from "./phaseInput.ts";
import { phaseMirror } from "./phaseMirror.ts";
import type { ScreenPhase } from "./screenPhase.ts";

export const SCREEN_FEED_INTERVAL_MS = 1_000;

/// Injected rather than imported: the input registry owns the terminals.
/// The registry sets this once at module load; unset reads as "unknown".
let reader: ((id: string) => ScreenPhase) | null = null;

export function setScreenPhaseReader(fn: ((id: string) => ScreenPhase) | null): void {
	reader = fn;
}

type ScreenReading = Extract<PhaseInput, { kind: "screenReading" }>["reading"];

/// Last reading forwarded to the daemon, per harness, with the spawn it was
/// read for: a respawn keeps the id but the daemon's record starts fresh.
const forwarded = new Map<string, { reading: ScreenReading; spawnedAt: number }>();

function forward(id: string, reading: ScreenReading, spawnedAt = 0): void {
	if (reading === null) {
		if (!forwarded.delete(id)) return;
	} else {
		const prev = forwarded.get(id);
		if (prev && prev.reading === reading && prev.spawnedAt === spawnedAt) return;
		forwarded.set(id, { reading, spawnedAt });
	}
	sendPhaseInput(id, { kind: "screenReading", reading });
}

/// One pass: read and forward changes, and drop the reading of a harness the
/// daemon has forgotten.
export function tickScreenReadings(): void {
	if (reader === null) return;
	const live = new Set(phaseMirror.ids());
	for (const id of [...forwarded.keys()]) if (!live.has(id)) forward(id, null);
	for (const id of live) {
		const a = phaseMirror.get(id);
		if (!a || a.authoritative) continue;
		if (a.phase !== "running" && a.phase !== "waiting") continue;
		let reading: ScreenPhase;
		try {
			reading = reader(id);
		} catch {
			reading = "unknown";
		}
		forward(id, reading, a.spawnedAt);
	}
}

let timer: ReturnType<typeof setInterval> | null = null;

/// Idempotent, so StrictMode's double mount is harmless.
export function startScreenReadingFeed(): () => void {
	if (timer === null) timer = setInterval(tickScreenReadings, SCREEN_FEED_INTERVAL_MS);
	return () => {
		if (timer !== null) clearInterval(timer);
		timer = null;
	};
}

/// Test-only: forget what was forwarded.
export function resetScreenReadings(): void {
	forwarded.clear();
}
