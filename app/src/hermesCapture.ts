// #624: Hermes mints its own session id at launch (`YYYYMMDD_HHMMSS_<hex>`,
// local time) and writes the row to its state.db only at the first prompt,
// possibly many minutes later. So, unlike opencodeCapture.ts, there is NO
// deadline: poll until the Rust claim rule assigns this harness a row, or
// until stopped.
//
// The rule runs in Rust over every pending harness in the same cwd, so two
// Hermes harnesses spawned together each get their own row instead of both
// taking the first. This module is the registry that feeds it: `pending`
// (live, unbound harnesses) and `claimed` (ids already bound).

import { invoke as tauriInvoke } from "@tauri-apps/api/core";

interface PendingEntry {
	cwd: string;
	spawnedAtMs: number;
}

const pending = new Map<string, PendingEntry>();
const claimed = new Set<string>();
const loops = new Map<string, () => void>();

/** Record an id already bound to some harness (e.g. a resumed one). */
export const markClaimed = (sessionId: string): void => {
	claimed.add(sessionId);
};

export type ClaimInvoke = (
	cmd: "hermes_claim_session",
	args: {
		cwd: string;
		harnessId: string;
		pending: { harnessId: string; spawnedAtMs: number }[];
		claimed: string[];
		localMinusUtcMinutes: number;
	},
) => Promise<string | null>;

export interface HermesCaptureParams {
	harnessId: string;
	cwd: string;
	spawnedAtMs: number;
	onCapture: (sessionId: string) => void;
	invoke?: ClaimInvoke;
	setTimer?: (fn: () => void, ms: number) => unknown;
	clearTimer?: (handle: unknown) => void;
}

/** 1 s for the first 2 minutes, then 3 s. */
export const hermesPollDelay = (elapsedMs: number): number => (elapsedMs < 120_000 ? 1_000 : 3_000);

/** Start polling for this harness's session; returns an idempotent stop. */
export const startHermesCapture = (p: HermesCaptureParams): (() => void) => {
	const { harnessId, cwd, spawnedAtMs, onCapture } = p;
	const call = p.invoke ?? (tauriInvoke as ClaimInvoke);
	const setTimer = p.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
	const clearTimer = p.clearTimer ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
	// A re-attach for the same harness (respawn) replaces the old loop.
	loops.get(harnessId)?.();

	let stopped = false;
	let timer: unknown = null;
	const startedAt = Date.now();
	pending.set(harnessId, { cwd, spawnedAtMs });

	const stop = (): void => {
		if (stopped) return;
		stopped = true;
		if (timer !== null) clearTimer(timer);
		timer = null;
		pending.delete(harnessId);
		if (loops.get(harnessId) === stop) loops.delete(harnessId);
	};
	loops.set(harnessId, stop);

	const schedule = (): void => {
		if (stopped) return;
		timer = setTimer(() => void tick(), hermesPollDelay(Date.now() - startedAt));
	};

	const tick = async (): Promise<void> => {
		timer = null;
		if (stopped) return;
		try {
			const id = await call("hermes_claim_session", {
				cwd,
				harnessId,
				pending: [...pending]
					.filter(([, e]) => e.cwd === cwd)
					.map(([hid, e]) => ({ harnessId: hid, spawnedAtMs: e.spawnedAtMs })),
				claimed: [...claimed],
				localMinusUtcMinutes: -new Date(spawnedAtMs).getTimezoneOffset(),
			});
			// Re-check after the await: stopped, or another capture took it.
			if (stopped) return;
			if (id !== null && id !== undefined && !claimed.has(id)) {
				claimed.add(id);
				stop();
				console.info(`[skein] hermes capture: ${id} (${cwd})`);
				onCapture(id);
				return;
			}
		} catch {
			// Transient (locked db) — try again on the next tick.
		}
		schedule();
	};

	console.info(`[skein] hermes capture started for ${harnessId} in ${cwd}`);
	schedule();
	return stop;
};

/** Is this argv[0] the hermes binary (`hermes`, `hermes.exe`,
 *  `C:\...\hermes.cmd`)? A post-exit shell respawns the pane with a
 *  shell instead, and that must not start capture. */
export function isHermesProgram(program: string | undefined): boolean {
	if (!program) return false;
	const base = program.split(/[\\/]/).pop() ?? "";
	return base.replace(/\.[^.]*$/, "").toLowerCase() === "hermes";
}
