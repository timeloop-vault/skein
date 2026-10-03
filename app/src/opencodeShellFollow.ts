// #517 — following an `opencode` typed into a pane's POST-EXIT SHELL.
// After opencode exits and Enter drops the pane into a shell, a hand-typed
// `opencode --session <id> --port <p>` is a new process Skein never
// spawned. The Rust `pty_scan_opencode` command reads that PTY's own
// descendants; this module decides what the scan means and re-binds the
// session / re-points the SSE adapter, in memory only. The pure pieces
// (`isOpencodeShellSpawn`, `decideShellRebind`, `createScanScheduler`) are
// unit-tested; `startOpencodeShellFollow` is the thin wiring.

import { invoke } from "@tauri-apps/api/core";
import { attachOpencodeEvents } from "./harnessEventsOpencode.ts";
import { runsKindProgram } from "./shellClaim.ts";

/** Mirror of the Rust `OpencodeScan`. */
export interface OpencodeScan {
	pid: number;
	sessionId: string | null;
	port: number | null;
	/** That very pid listens on `port`. */
	portConfirmed: boolean;
	/** Started with `-c`/`--continue`. */
	continueLast: boolean;
}

/** Shell mode for opencode = the kind's own program is not what runs. */
export function isOpencodeShellSpawn(kind: string, program: string | undefined): boolean {
	return kind === "opencode" && program !== undefined && !runsKindProgram(program, "opencode");
}

export interface ShellRebind {
	sessionId?: string;
	port?: number;
}

/** What to change given a scan; null when nothing. A port is only trusted
 *  once confirmed (a bogus `--port` must not repoint at a stranger). */
export function decideShellRebind(
	scan: OpencodeScan,
	current: { sessionId: string | undefined; port: number | undefined },
): ShellRebind | null {
	const out: ShellRebind = {};
	if (scan.sessionId !== null && scan.sessionId !== current.sessionId)
		out.sessionId = scan.sessionId;
	if (scan.portConfirmed && scan.port !== null && scan.port !== current.port) out.port = scan.port;
	return out.sessionId === undefined && out.port === undefined ? null : out;
}

export const DEBOUNCE_MS = 400;
export const MIN_GAP_MS = 2000;
/** Gap once the current pid is settled: a running TUI never goes quiet. */
export const SETTLED_GAP_MS = 15_000;
export const RECHECK_MS = 2000;

export interface ScanResult {
	again: boolean;
	/** Keep re-checking even after a follow-up (a pid seen but not yet proven
	 *  started; bounded, since it proves or vanishes). */
	retry?: boolean;
	settledPid: number | null;
}

const NO_SCAN: ScanResult = { again: false, settledPid: null };

/** A bogus `--session` is visible to a scan for ~1.5 s, then the process
 *  exits without binding its port: its argv proves nothing until it has
 *  lived this long, or its port is confirmed. */
export const STARTUP_PROOF_MS = 3000;

/** Tracks when each pid was first seen; `proven` = port confirmed, or seen
 *  at least STARTUP_PROOF_MS ago and still here. */
export function createStartupProof(now: () => number) {
	const firstSeen = new Map<number, number>();
	return {
		proven(scan: OpencodeScan): boolean {
			const t = now();
			const first = firstSeen.get(scan.pid) ?? t;
			firstSeen.set(scan.pid, first);
			return scan.portConfirmed || t - first >= STARTUP_PROOF_MS;
		},
		clear: () => firstSeen.clear(),
	};
}

export interface SchedulerDeps {
	/** Runs one scan. `again`: a follow-up re-check is wanted. `settledPid`:
	 *  the pid whose handling is complete (null = nothing settled). */
	scan: () => Promise<ScanResult>;
	now: () => number;
	setTimer: (fn: () => void, ms: number) => unknown;
	clearTimer: (t: unknown) => void;
}

/** Coalesces output ticks: trailing debounce, one scan in flight, a minimum
 *  gap between scan starts, and ONE follow-up re-check per tick-started
 *  chain (a bogus `--session` exits within seconds; the TUI binds its port
 *  after starting). A quiet pane schedules nothing. */
export function createScanScheduler(d: SchedulerDeps): { tick: () => void; dispose: () => void } {
	let timer: unknown = null;
	let inFlight = false;
	let dirty = false;
	let disposed = false;
	let settledPid: number | null = null;
	let lastStart = Number.NEGATIVE_INFINITY;

	const schedule = (delay: number, followUp: boolean) => {
		if (timer !== null) d.clearTimer(timer);
		const wait = Math.max(
			delay,
			lastStart + (settledPid === null ? MIN_GAP_MS : SETTLED_GAP_MS) - d.now(),
		);
		timer = d.setTimer(() => {
			timer = null;
			void run(followUp);
		}, wait);
	};

	const run = async (followUp: boolean) => {
		if (disposed) return;
		if (inFlight) {
			dirty = true;
			return;
		}
		inFlight = true;
		dirty = false;
		lastStart = d.now();
		let again = false;
		let retry = false;
		try {
			const res = await d.scan();
			again = res.again;
			retry = res.retry === true;
			settledPid = res.settledPid;
		} catch {
			settledPid = null;
		}
		inFlight = false;
		if (disposed) return;
		if (dirty) schedule(DEBOUNCE_MS, false);
		else if (retry || (again && !followUp)) schedule(RECHECK_MS, true);
	};

	return {
		tick() {
			if (disposed) return;
			if (inFlight) dirty = true;
			else schedule(DEBOUNCE_MS, false);
		},
		dispose() {
			disposed = true;
			if (timer !== null) d.clearTimer(timer);
			timer = null;
		},
	};
}

export function shellFollowHint(
	rebound: boolean,
	hasSession: boolean,
	continueLast = false,
): string {
	if (rebound) {
		return "opencode started without --port — Skein follows its session but can't see its activity. Restart the harness for live status.";
	}
	if (hasSession) {
		return "opencode started without --port — Skein can't see its activity. Restart the harness for live status.";
	}
	return continueLast
		? "opencode -c started without --port — Skein can't tell which session it continued. Restart the harness for live status."
		: "opencode started without --session or --port — Skein can't follow it. Restart the harness to resume.";
}

/** How long a proven pid's declared port may stay unconfirmed before the
 *  re-checks stop (opencode that never binds must not poll forever). */
export const PORT_WAIT_MS = 30_000;

/** Per-pid clock for "proven, port declared, not yet listening". */
export function createPortWait(now: () => number) {
	const since = new Map<number, number>();
	return {
		/** True while this pid is still inside its wait window. */
		waiting(pid: number): boolean {
			const t = now();
			const first = since.get(pid) ?? t;
			since.set(pid, first);
			return t - first < PORT_WAIT_MS;
		},
		clear: () => since.clear(),
	};
}

export interface ShellFollowDeps {
	ptyIdRef: { current: string | null };
	getSessionId: () => string | undefined;
	getPort: () => number | undefined;
	onSessionFollowed: ((sessionId: string) => void) | undefined;
	/** Detach the current adapter and attach one on `port`. */
	repoint: (port: number) => void;
	hint: (text: string) => void;
}

/** Starts following; call `onOutput` for every PTY data chunk. */
export function startOpencodeShellFollow(deps: ShellFollowDeps): {
	onOutput: () => void;
	dispose: () => void;
} {
	let disposed = false;
	let lastPid: number | null = null;
	let lastArgvSession: string | null = null;
	const hinted = new Set<number>();
	const proof = createStartupProof(() => Date.now());
	const portWait = createPortWait(() => Date.now());
	const scheduler = createScanScheduler({
		now: () => Date.now(),
		setTimer: (fn, ms) => setTimeout(fn, ms),
		clearTimer: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
		scan: async () => {
			const id = deps.ptyIdRef.current;
			if (id === null) return NO_SCAN;
			const scan = await invoke<OpencodeScan | null>("pty_scan_opencode", { id });
			if (disposed || deps.ptyIdRef.current !== id) return NO_SCAN;
			if (scan === null) {
				proof.clear();
				portWait.clear();
				return { again: true, settledPid: null };
			}
			if (!proof.proven(scan)) return { again: true, retry: true, settledPid: null };
			// The argv id is only news when this process or its id is new;
			// otherwise opencode's own /new (followed over SSE) would be
			// rolled back to the stale argv id on every tick.
			const stale = scan.pid === lastPid && scan.sessionId === lastArgvSession;
			lastPid = scan.pid;
			lastArgvSession = scan.sessionId;
			const live = deps.getSessionId();
			const change = decideShellRebind(scan, {
				sessionId: stale ? (scan.sessionId ?? live) : live,
				port: deps.getPort(),
			});
			if (change?.sessionId !== undefined) deps.onSessionFollowed?.(change.sessionId);
			if (change?.port !== undefined) deps.repoint(change.port);
			if (scan.port === null && !hinted.has(scan.pid)) {
				hinted.add(scan.pid);
				deps.hint(
					shellFollowHint(
						change?.sessionId !== undefined,
						scan.sessionId !== null,
						scan.continueLast,
					),
				);
			}
			// Proven but its port not yet listening: keep re-checking (the TUI
			// may not repaint), until PORT_WAIT_MS.
			const pending = scan.port !== null && !scan.portConfirmed;
			if (pending && portWait.waiting(scan.pid)) {
				return { again: true, retry: true, settledPid: null };
			}
			return { again: false, settledPid: pending ? null : scan.pid };
		},
	});
	return {
		onOutput: () => scheduler.tick(),
		dispose() {
			disposed = true;
			scheduler.dispose();
		},
	};
}

export interface OpencodeAdapter {
	detach: () => void;
	port: number;
}

export interface PaneFollowParams {
	harnessId: string;
	roomId: string;
	cwd: string;
	ptyIdRef: { current: string | null };
	sessionIdRef: { current: string | undefined };
	adapter: { current: OpencodeAdapter | null };
	onSessionCaptured: ((sessionId: string) => void) | undefined;
	onSessionFollowed: ((sessionId: string) => void) | undefined;
	setHint: (hint: string | null) => void;
	hintTimerRef: { current: ReturnType<typeof setTimeout> | null };
}

const HINT_MS = 8000;

/** `startOpencodeShellFollow` wired to a pane: repoint swaps the SSE
 *  adapter held in `adapter`, the hint uses the pane's transient hint. */
export function followOpencodeShell(
	kind: string,
	program: string | undefined,
	p: PaneFollowParams,
) {
	if (!isOpencodeShellSpawn(kind, program)) return null;
	return startOpencodeShellFollow({
		ptyIdRef: p.ptyIdRef,
		getSessionId: () => p.sessionIdRef.current,
		getPort: () => p.adapter.current?.port,
		onSessionFollowed: p.onSessionFollowed,
		repoint: (port) => {
			p.adapter.current?.detach();
			const detach = attachOpencodeEvents(
				p.harnessId,
				p.roomId,
				p.cwd,
				port,
				p.sessionIdRef.current,
				p.onSessionCaptured,
				() => p.sessionIdRef.current,
				p.onSessionFollowed,
			);
			p.adapter.current = { detach, port };
		},
		hint: (text) => {
			if (p.hintTimerRef.current) clearTimeout(p.hintTimerRef.current);
			p.setHint(text);
			p.hintTimerRef.current = setTimeout(() => {
				p.setHint(null);
				p.hintTimerRef.current = null;
			}, HINT_MS);
		},
	});
}
