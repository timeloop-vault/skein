// What a live PTY's terminal feeds back: the #238 `harnessInput` seam
// registration, the keystroke/data forwarding to `pty_write`, and the
// resize observer. Split out of useTerminalSpawn.ts (#459); the order the
// effect calls these in is part of its contract.

import { invoke } from "@tauri-apps/api/core";
import type { FitAddon } from "@xterm/addon-fit";
import type { Terminal } from "@xterm/xterm";
import { harnessActivity } from "./harnessActivity.ts";
import { harnessInput } from "./harnessInput.ts";
import type { ScreenCell } from "./promptScreen.ts";
import { fitTerminal } from "./terminalFit.ts";
import type { OutputGate } from "./terminalOutputGate.ts";
import { isHostHidden } from "./terminalOutputVisibility.ts";
import type { HarnessKind } from "./types.ts";

/** The slice of xterm's `Terminal` the screen read touches. */
export type ScreenTerm = Pick<Terminal, "rows" | "cols"> & {
	buffer: {
		active: Pick<
			Terminal["buffer"]["active"],
			"baseY" | "cursorX" | "cursorY" | "getNullCell" | "getLine"
		>;
	};
};

/** #413: the visible screen only (baseY.., not viewportY, so a user
 *  scrolled into history doesn't matter); null when any line is missing. */
export function readVisibleScreen(
	term: ScreenTerm,
): { rows: ScreenCell[][]; cursorX: number; cursorY: number } | null {
	const buf = term.buffer.active;
	const reuse = buf.getNullCell();
	const rows: ScreenCell[][] = [];
	for (let y = 0; y < term.rows; y++) {
		const line = buf.getLine(buf.baseY + y);
		if (!line) return null;
		const cells: ScreenCell[] = [];
		for (let x = 0; x < term.cols; x++) {
			const cell = line.getCell(x, reuse);
			if (!cell) return null;
			cells.push({ ch: cell.getChars(), dim: cell.isDim() !== 0 });
		}
		rows.push(cells);
	}
	return { rows, cursorX: buf.cursorX, cursorY: buf.cursorY };
}

/** #238: publish this harness to the `harnessInput` seam now that its
 *  PTY is live. `paste`/`bracketedPaste` read xterm state directly;
 *  `submit` is a separate `pty_write` of a bare "\r" after the paste,
 *  per the seam's contract. Returns the unregister function. */
export function registerInputTarget(
	term: Terminal,
	harnessId: string,
	harnessKind: HarnessKind,
	id: string,
	gate: OutputGate,
): () => void {
	// One pending re-read per gate: once the flushed output has parsed, a null
	// screen must not leave held mail waiting for some unrelated trigger.
	// checkScreen → noteDraftEvent(screenEmpty) → subscribeDraftCleared →
	// useMailDelivery's runSerialized (harnessInputRegistry.ts:225).
	let recheckPending = false;
	const recheckWhenSettled = () => {
		gate.flush();
		if (recheckPending) return;
		recheckPending = true;
		gate.whenDrained(() => {
			recheckPending = false;
			harnessInput.checkScreen(harnessId);
		});
	};
	return harnessInput.register(harnessId, {
		// #592: `waiting` comes from the transcript, not PTY quiet, so output may
		// still sit in the gate. Until it settles, report "can't tell" (null /
		// false both refuse) and flush so a retry reads fresh state.
		paste: (text) => term.paste(text),
		bracketedPaste: () => {
			if (gate.isSettled()) return term.modes.bracketedPasteMode;
			gate.flush();
			return false;
		},
		submit: () => {
			void invoke("pty_write", { id, data: "\r" });
		},
		kind: harnessKind,
		screen: () => {
			if (gate.isSettled()) return readVisibleScreen(term);
			recheckWhenSettled();
			return null;
		},
	});
}

/** Forward terminal input to the child, and feed the activity model.
 *  Returns the `onData` disposable (the caller stops forwarding on
 *  exit); the `onKey` hook lives for the terminal's lifetime. */
export function attachPtyInput(term: Terminal, harnessId: string, id: string): { dispose(): void } {
	const dataDisposable = term.onData((data) => {
		// Focus-in / focus-out escapes are sent by xterm
		// when the child enabled DECSET 1004 (Claude Code,
		// opencode both do). The child typically reacts
		// with a full screen redraw — those bytes come
		// back through channel.onmessage and would
		// otherwise count as "activity" and reset the
		// idle timer. Mute the activity window for this
		// harness so the induced redraw doesn't lie about
		// what the child is doing. Covers every focus
		// path: harness switch, alt+tab back to Skein,
		// modal-close focus return, click into pane. Epic
		// #50.
		if (data === "\x1b[I" || data === "\x1b[O") {
			harnessActivity.muteInducedOutput(harnessId);
		}
		// #383: a bracketed paste (middle-click, a right-click/
		// menu paste) never reaches `onKey` below, so it's the
		// only paste path this store would otherwise miss
		// entirely. The seam's own `term.paste()` lands here
		// too — harmless, since the composer read is `unknown`
		// either way and its own `seamSubmit` clears it.
		if (data.startsWith("\x1b[200~")) {
			harnessInput.noteDraftEvent(harnessId, { type: "userPaste" });
		}
		void invoke("pty_write", { id, data });
	});
	// Separate hook for "did the user actually press a
	// key in this terminal?" — used by L5a notification
	// gating to tell real work cycles apart from startup
	// banner cycles, and (#86) to tell whether a keystroke
	// answered a permission dialog. onKey is the right
	// primitive for both: onData fires for *anything* the
	// terminal sends to the child, including auto-responses
	// to queries like `\x1b[6n` (cursor position) and
	// `\x1b[5n` (device status). Treating those as input was
	// the bug that lit up every room on Skein restart, and
	// would just as wrongly clear a permission dialog nobody
	// answered. `key` is the exact bytes this keystroke sends
	// to the child — the same string `onData` would carry for
	// it — so `recordInput` can classify it without a second
	// copy of the escape-sequence logic.
	// Caveat: onKey doesn't fire for paste — if the user
	// pastes without ever typing, their first task-idle
	// transition won't bump, and pasting an answer into a
	// permission dialog won't clear it either. Corner-case
	// false negative we'll address with a paste listener if
	// it matters in practice.
	term.onKey(({ key }) => {
		harnessActivity.recordInput(harnessId, key);
		// #380: a human is typing — `sendPrompt`'s gap/retry
		// checks need to tell that apart from its own
		// machine-written "\r".
		harnessInput.noteUserInput(harnessId);
		// #383: fold the same keystroke into the composer-draft
		// inference.
		harnessInput.noteDraftEvent(harnessId, { type: "key", key });
	});
	return dataDisposable;
}

/** Observe the host and keep xterm + the PTY sized to it. */
export function observeResize(
	term: Terminal,
	fit: FitAddon,
	host: HTMLDivElement,
	ptyIdRef: { current: string | null },
	gate: OutputGate,
): ResizeObserver {
	// Track the dims we last sent so we can skip the
	// pty_resize round-trip when nothing actually
	// changed. Most ResizeObserver fires on a hidden→
	// visible flip end up with the same rows/cols xterm
	// already had — and many TUIs (Claude Code, opencode)
	// react to SIGWINCH by repainting their entire screen,
	// which then comes back to us as PTY output and
	// counts as "activity" in the harnessActivity store.
	// The visible symptom before this guard: switching
	// to an idle background harness made its tab dot go
	// green for 8s before settling back to idle. Epic #50.
	let lastSentRows = term.rows;
	let lastSentCols = term.cols;
	const resizeObserver = new ResizeObserver(() => {
		// Phase 3 guard: when the room goes display:none,
		// the host shrinks to 0×0 and the observer fires.
		// Fitting to that size would tell xterm + the child
		// that the terminal is 1×1, permanently squishing
		// whatever's already in the scrollback. Skip while
		// hidden — the next tick (visible again) refits.
		if (isHostHidden(host)) return;
		// #592: xterm's resize() is synchronous and ignores queued writes,
		// so fit only once everything buffered while hidden has parsed at
		// the old size.
		gate.whenDrained(() => {
			// A disposed gate never calls back, so teardown needs no check.
			if (isHostHidden(host)) return;
			try {
				fitTerminal(term, fit);
			} catch {
				// fit can throw during teardown when the host
				// element has been detached; ignore.
				return;
			}
			const cur = ptyIdRef.current;
			if (!cur) return;
			if (term.rows === lastSentRows && term.cols === lastSentCols) return;
			lastSentRows = term.rows;
			lastSentCols = term.cols;
			void invoke("pty_resize", { id: cur, rows: term.rows, cols: term.cols });
		});
	});
	resizeObserver.observe(host);
	return resizeObserver;
}
