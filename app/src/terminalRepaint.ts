// Re-attach repaint (#731). A view attached to a process that was already
// running starts blank: the daemon replays nothing. Shrinking the PTY by one
// row and back sends SIGWINCH twice, which makes a full-screen tool redraw.
// A plain shell stays blank until its next output; that is accepted.

import { invoke } from "@tauri-apps/api/core";

export const REPAINT_DELAY_MS = 80;

export interface RepaintDeps {
	/** The terminal's current size (read when restoring, so a real resize in
	 *  between wins). */
	size: () => { rows: number; cols: number };
	resize: (rows: number, cols: number) => void;
	delayMs?: number;
	setTimer?: (fn: () => void, ms: number) => unknown;
	clearTimer?: (handle: unknown) => void;
}

/** Nudges the PTY one row smaller, then back to the real size after a delay
 *  (so the two resizes are not coalesced). Returns a cancel function; after
 *  it runs while the restore is still pending, the restoring resize is sent at
 *  once instead of later, so a cancel never leaves the PTY one row short. */
export function forceRepaint(d: RepaintDeps): () => void {
	const setTimer = d.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
	const clearTimer = d.clearTimer ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
	const { rows, cols } = d.size();
	if (rows <= 1) return () => {};
	d.resize(rows - 1, cols);
	let pending = true;
	const restore = () => {
		pending = false;
		const now = d.size();
		d.resize(now.rows, now.cols);
	};
	const handle = setTimer(restore, d.delayMs ?? REPAINT_DELAY_MS);
	return () => {
		if (!pending) return;
		clearTimer(handle);
		restore(); // never leave the PTY one row short
	};
}

/** `forceRepaint` against a live xterm and its PTY id. */
export function repaintPty(term: { rows: number; cols: number }, id: string): () => void {
	return forceRepaint({
		size: () => ({ rows: term.rows, cols: term.cols }),
		resize: (rows, cols) => void invoke("pty_resize", { id, rows, cols }),
	});
}
