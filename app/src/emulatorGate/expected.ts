// Expected differences between xterm.js (A) and the Rust engine (B) at the
// cuts of the two fixtures (#793 gate, slice T-1). Keyed `fixture/cut`; each
// category maps to its mechanism. An entry that no longer differs fails, and
// so does a difference with no entry. A mechanism the gate could not explain
// starts with `UNEXPLAINED:` and is counted by pictures.test.ts. (xterm.js's
// underline on link cells is not pinned here: linkUnderline.ts excuses it
// cell by cell.)

import type { Category } from "./compare.ts";

/** alacritty keeps the wrap flag on the last cell of the row being continued (alacritty.rs row_dump reads it there); xterm.js keeps isWrapped on the continuation row. */
const ALT_RESIZE_WRAP =
	"alt-screen resize without reflow: alacritty's WRAPLINE sits on the previous row's last cell, which a shrink cuts and a grow leaves at the old last column; xterm.js keeps isWrapped on the continuation row";
const STALE_WRAP =
	"overwriting the previous row (a repaint) clears alacritty's WRAPLINE on its last cell; xterm.js leaves isWrapped true on the row below (probed: write past the margin, rewrite the row above, flag stays)";

/** A synchronized update is held back by the engine until ?2026l; xterm.js shows the frame as it arrives (sync_pending and the mode agree). */
const SYNC_HELD = "engine holds the open ?2026 frame back; xterm.js applies it at once";

export const EXPECTED: Record<string, Partial<Record<Category, string>>> = {
	"claude-session/resize-100x30": { screenWrap: ALT_RESIZE_WRAP },
	"claude-session/resize-80x24": { screenWrap: ALT_RESIZE_WRAP },
	"claude-session/resize-120x30": { screenWrap: ALT_RESIZE_WRAP },
	"claude-session/end-of-stream": { screenWrap: STALE_WRAP },
	"scrollback-session/in-sync-frame": {
		cursor: SYNC_HELD,
		screenText: SYNC_HELD,
		screenStyles: SYNC_HELD,
	},
};
