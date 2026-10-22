// #639: keystroke-to-echo timing. Pure per-PTY tracker; the caller supplies
// timestamps (performance.now()) and records the spans via perfTelemetry.
//
// The first unanswered keystroke wins, so a burst measures its first key's
// echo. Any output after that clears it (an approximation: the first output
// chunk is taken to be the echo).
//
// Each input is classified (key: typing, Backspace, Tab, Alt+key; enter;
// paste: unbracketed input over 8 chars; ctrl: arrows, other escape
// sequences, Ctrl-letters, lone Esc; mouse) and tagged with the
// harness phase at input time, and the echo is recorded three ways: the
// aggregate `pty.echo`, `pty.echo.<kind>` and `pty.echo@<phase>`. Mouse is
// its own bucket because hover/click reports often produce no repaint, so
// their "echo" is whatever unrelated output comes next; separating them
// tells a slow child repaint apart from inputs that never answer.

import { recordSpan } from "./perfTelemetry.ts";

export type EchoKind = "key" | "enter" | "ctrl" | "mouse" | "paste";
export type EchoPhase = "waiting" | "running" | "other";

/** Long enough to still capture the multi-second (up to ~30 s) stalls #639 saw in the field, short enough that an unanswered key can't masquerade as a minutes-long echo. */
export const STALE_MS = 60_000;

type Pending = { t: number; kind: EchoKind; phase: EchoPhase };

const pending = new Map<string, Pending>();

/** True for xterm's auto-replies to child queries (CPR, DA, OSC, DCS), which are not keystrokes. */
export function isTerminalReply(data: string): boolean {
	if (!data.startsWith("\x1b")) return false;
	const rest = data.slice(1);
	return /^\[\d+;\d+R$/.test(rest) || /^\[[?>]?[\d;]*c$/.test(rest) || /^[\]P]/.test(rest);
}

// SGR reports (one or more) or X10/normal `ESC [ M` + 3 bytes.
// biome-ignore lint/suspicious/noControlCharactersInRegex: matching control characters is the point
const MOUSE_RE = /^(?:\x1b\[<\d+;\d+;\d+[Mm])+$/;
// biome-ignore lint/suspicious/noControlCharactersInRegex: matching control characters is the point
const X10_MOUSE_RE = /^\x1b\[M[\s\S]{3}$/;
// biome-ignore lint/suspicious/noControlCharactersInRegex: matching control characters is the point
const PRINTABLE_RE = /^[^\x00-\x1f\x7f]+$/;
// Backspace (DEL or BS) and Tab.
const KEY_EDIT_RE = /^[\x7f\b\t]$/;
// Alt+key: ESC then exactly one printable char.
// biome-ignore lint/suspicious/noControlCharactersInRegex: matching control characters is the point
const ALT_KEY_RE = /^\x1b[^\x00-\x1f\x7f]$/;
/** Input longer than this (string length) is a paste, not typing. */
const PASTE_MIN_LEN = 8;

/** What kind of input xterm's `onData` string is. */
export function classifyInput(data: string): EchoKind {
	if (MOUSE_RE.test(data) || X10_MOUSE_RE.test(data)) return "mouse";
	if (data === "\r" || data === "\r\n" || data === "\n") return "enter";
	// Bracketed paste and focus reports never get here: terminalSpawnInput.ts
	// filters them out first, so a long string is an unbracketed paste.
	if (data.length > PASTE_MIN_LEN) return "paste";
	if (PRINTABLE_RE.test(data) || KEY_EDIT_RE.test(data) || ALT_KEY_RE.test(data)) return "key";
	return "ctrl";
}

/** Bucket a harness phase for echo telemetry. */
export function echoPhase(phase: string | undefined | null): EchoPhase {
	return phase === "waiting" || phase === "running" ? phase : "other";
}

/** Note a keystroke at `t`; ignored if a fresh one is already pending for this PTY. */
export function noteInput(ptyId: string, t: number, kind: EchoKind, phase: EchoPhase): void {
	const p = pending.get(ptyId);
	if (p === undefined || t - p.t > STALE_MS) pending.set(ptyId, { t, kind, phase });
}

/** Output arrived at `t`: the pending keystroke's latency, kind and phase (cleared), or null. */
export function noteOutput(
	ptyId: string,
	t: number,
): { ms: number; kind: EchoKind; phase: EchoPhase } | null {
	const p = pending.get(ptyId);
	if (p === undefined) return null;
	pending.delete(ptyId);
	return t - p.t > STALE_MS ? null : { ms: t - p.t, kind: p.kind, phase: p.phase };
}

/** Output arrived on `ptyId` (null before spawn resolves: skipped); records `pty.echo` plus its kind and phase splits. */
export function recordEcho(ptyId: string | null, t: number): void {
	const r = ptyId ? noteOutput(ptyId, t) : null;
	if (r === null) return;
	recordSpan("pty.echo", r.ms, "wall");
	recordSpan(`pty.echo.${r.kind}`, r.ms, "wall");
	recordSpan(`pty.echo@${r.phase}`, r.ms, "wall");
}

/** Drop state for a PTY that exited or was torn down. */
export function forget(ptyId: string): void {
	pending.delete(ptyId);
}
