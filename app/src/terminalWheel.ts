// terminalWheel — how a trackpad scroll reaches a TUI that turned mouse
// tracking on (Claude Code's fullscreen renderer, opencode). #569.
//
// Left to xterm.js 6.0.0 it stops tracking the finger:
// `CoreMouseService.consumeWheelEvent` scales every event under 50 px by
// 0.3 (its "likely a trackpad" guess), and `bindMouse` then sends ONE
// wheel report per DOM event, however many lines the event was worth. A
// slow swipe crawls and a fast one is capped at the event rate. Native
// macOS terminals (iTerm2, Ghostty) send one report per cell height of
// travel, and that is the stream Claude Code tunes for here: Skein
// strips TERM_PROGRAM and xterm.js answers no XTVERSION, so Claude Code
// picks its native-terminal wheel profile (1 line per report,
// accelerating on rapid reports — the ramp a Settings switch turns off,
// see `claude_settings_args` in harness_config.rs).
//
// So Skein counts the lines itself and writes that many SGR reports.
// Every other case (no tracking, X10, a non-SGR encoding, a modifier
// held) still goes to xterm unchanged. macOS only — see terminalSetup.

import type { Terminal } from "@xterm/xterm";

/** `WheelEvent.deltaMode` values; the DOM constants don't exist under
 *  vitest's node environment. Anything else is DOM_DELTA_PIXEL. */
const DOM_DELTA_LINE = 1;
const DOM_DELTA_PAGE = 2;

/** The part of a line scrolled but not yet reported. */
export interface WheelCarry {
	lines: number;
}

/** Whole lines one wheel event is worth — one per cell height of pixel
 *  travel, positive = down. The remainder carries to the next event and
 *  is dropped when the direction reverses, so a turn-around answers at
 *  once instead of first paying back the old direction's fraction. */
export function wheelLines(
	carry: WheelCarry,
	deltaY: number,
	deltaMode: number,
	cellHeight: number,
	rows: number,
): number {
	const lines =
		deltaMode === DOM_DELTA_LINE
			? deltaY
			: deltaMode === DOM_DELTA_PAGE
				? deltaY * rows
				: deltaY / cellHeight;
	if (lines === 0 || !Number.isFinite(lines)) return 0;
	if (Math.sign(lines) !== Math.sign(carry.lines)) carry.lines = 0;
	carry.lines += lines;
	const whole = Math.trunc(carry.lines);
	carry.lines -= whole;
	return whole;
}

/** `|lines|` SGR (DECSET 1006) wheel reports at a 1-based cell;
 *  negative `lines` scrolls up. */
export function sgrWheelReports(lines: number, col: number, row: number): string {
	const button = lines < 0 ? 64 : 65;
	return `\x1b[<${button};${col};${row}M`.repeat(Math.abs(lines));
}

/** Whether SGR encoding is active after a DECSET (`set`) or DECRST,
 *  mirroring xterm 6.0.0's InputHandler: 1006 selects SGR, 1016 selects
 *  SGR-pixels, and resetting either falls back to the default
 *  encoding. */
export function sgrAfterDecMode(
	sgr: boolean,
	params: readonly (number | number[])[],
	set: boolean,
): boolean {
	let on = sgr;
	for (const p of params) {
		if (p === 1006) on = set;
		else if (p === 1016) on = false;
	}
	return on;
}

function clamp(n: number, min: number, max: number): number {
	return Math.min(max, Math.max(min, n));
}

/** Takes over wheel reporting for `term` as described above. Call once,
 *  at creation, before any child output is written. */
export function attachWheelReports(term: Terminal): void {
	// xterm keeps the encoding private, so follow the same sequences it
	// does. Returning false lets xterm's own handler apply each one too.
	let sgr = false;
	term.parser.registerCsiHandler({ prefix: "?", final: "h" }, (params) => {
		sgr = sgrAfterDecMode(sgr, params, true);
		return false;
	});
	term.parser.registerCsiHandler({ prefix: "?", final: "l" }, (params) => {
		sgr = sgrAfterDecMode(sgr, params, false);
		return false;
	});
	term.parser.registerEscHandler({ final: "c" }, () => {
		sgr = false; // RIS resets the encoding along with everything else
		return false;
	});

	const carry: WheelCarry = { lines: 0 };
	term.attachCustomWheelEventHandler((ev) => {
		const tracking = term.modes.mouseTrackingMode;
		if (!sgr || tracking === "none" || tracking === "x10") return true;
		if (ev.altKey || ev.ctrlKey || ev.metaKey || ev.shiftKey) return true;
		const rect = term.element?.querySelector(".xterm-screen")?.getBoundingClientRect();
		if (!rect || rect.width === 0 || rect.height === 0) return true;
		const cellHeight = rect.height / term.rows;
		const lines = wheelLines(carry, ev.deltaY, ev.deltaMode, cellHeight, term.rows);
		if (lines !== 0) {
			const cellWidth = rect.width / term.cols;
			const col = clamp(Math.floor((ev.clientX - rect.left) / cellWidth) + 1, 1, term.cols);
			const row = clamp(Math.floor((ev.clientY - rect.top) / cellHeight) + 1, 1, term.rows);
			// One write, so the PTY sees the burst together, the way a native
			// terminal sends it. `true` matches what xterm passes for its own
			// mouse reports.
			term.input(sgrWheelReports(lines, col, row), true);
		}
		// xterm sends nothing itself, and still preventDefaults the event.
		return false;
	});
}
