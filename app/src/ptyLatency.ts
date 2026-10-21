// #639: keystroke-to-echo timing. Pure per-PTY tracker; the caller supplies
// timestamps (performance.now()) and records the spans via perfTelemetry.
//
// The first unanswered keystroke wins, so a burst measures its first key's
// echo. Any output after that clears it (an approximation: the first output
// chunk is taken to be the echo).

import { recordSpan } from "./perfTelemetry.ts";

/** Long enough to still capture the multi-second (up to ~30 s) stalls #639 saw in the field, short enough that an unanswered key can't masquerade as a minutes-long echo. */
export const STALE_MS = 60_000;

const pending = new Map<string, number>();

/** True for xterm's auto-replies to child queries (CPR, DA, OSC, DCS), which are not keystrokes. */
export function isTerminalReply(data: string): boolean {
	if (!data.startsWith("\x1b")) return false;
	const rest = data.slice(1);
	return /^\[\d+;\d+R$/.test(rest) || /^\[[?>]?[\d;]*c$/.test(rest) || /^[\]P]/.test(rest);
}

/** Note a keystroke at `t`; ignored if a fresh one is already pending for this PTY. */
export function noteInput(ptyId: string, t: number): void {
	const t0 = pending.get(ptyId);
	if (t0 === undefined || t - t0 > STALE_MS) pending.set(ptyId, t);
}

/** Output arrived at `t`: ms since the pending keystroke (cleared), or null. */
export function noteOutput(ptyId: string, t: number): number | null {
	const t0 = pending.get(ptyId);
	if (t0 === undefined) return null;
	pending.delete(ptyId);
	return t - t0 > STALE_MS ? null : t - t0;
}

/** Output arrived on `ptyId` (null before spawn resolves: skipped); records `pty.echo`. */
export function recordEcho(ptyId: string | null, t: number): void {
	const ms = ptyId ? noteOutput(ptyId, t) : null;
	if (ms !== null) recordSpan("pty.echo", ms, "wall");
}

/** Drop state for a PTY that exited or was torn down. */
export function forget(ptyId: string): void {
	pending.delete(ptyId);
}
