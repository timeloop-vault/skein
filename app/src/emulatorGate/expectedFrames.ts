// Pinned differences of the frames gate (#807), keyed `fixture/cut`, each
// category with its mechanism. Same rules as expected.ts: an unexpected
// difference fails, and so does a pin that no longer differs. Every pin left
// is inherent to the engine or the cut: none is an encoder shortcoming (#807
// fixed those).

import type { Category } from "./compare.ts";

export type Pins = Record<string, Partial<Record<Category, string>>>;

/** Raw xterm.js is the side with the flag: a CUP-positioned rewrite keeps the isWrapped left by an earlier autowrap, and the recording does that. The engine has the row unwrapped, and every frame viewer does too, since the encoder erases a positioned row with EL at column 1 (#807). Seen on the last screen row, where the TUI redraws its input line. */
const STALE_RAW_WRAP =
	"raw xterm.js keeps a stale isWrapped on a row the application rewrote by CUP; the engine and the frame viewers (which EL at column 1) have it unwrapped";

/** A shrink leaves a wide char starting in the last column of a row, which a frame cannot draw (the client would wrap it and leave a ghost); the encoder writes one blank there, so only the text differs. xterm.js's own reflow keeps the character. */
const WIDE_LAST_COL =
	"wide emoji starts in the last column after the 120->80 reflow: the frame writes one blank there (a wrapped write would leave a ghost on the next row), xterm.js's own reflow keeps the character";

/** The alt-screen resize without reflow (T-1's ALT_RESIZE_WRAP): xterm.js reflows the main-style wrap flags, the engine does not; the frames follow the engine. */
const ALT_RESIZE_WRAP =
	"alt-screen resize without reflow: raw xterm.js keeps isWrapped on the continuation row, the engine (and so the frames) does not";

const SYNC_OPEN =
	"cut inside an open ?2026 update: raw xterm.js has applied the open frame, the frames show the last complete one (headless tracks the mode, does not hold the writes)";

const NOTHING_SENT =
	"cut inside a synchronized update sends nothing: a fresh viewer's full is empty, so B-full is a blank terminal";

const SKIPPED_LAGS =
	"the skipped viewer is framed only at cuts and this cut sends nothing: it still shows the after-hyperlinks state, the stream viewer already got the frame after event 227";

/** Terminal A (the raw recording) against B-stream (the daemon's frames, applied in order). */
export const A_VS_STREAM: Pins = {
	"claude-session/resize-100x30": { screenWrap: ALT_RESIZE_WRAP },
	"claude-session/resize-80x24": { screenText: WIDE_LAST_COL, screenWrap: STALE_RAW_WRAP },
	"claude-session/resize-120x30": { screenWrap: ALT_RESIZE_WRAP },
	"claude-session/end-of-stream": { screenWrap: STALE_RAW_WRAP },
	"scrollback-session/in-sync-frame": {
		cursor: SYNC_OPEN,
		screenText: SYNC_OPEN,
		screenStyles: SYNC_OPEN,
		modes: SYNC_OPEN,
	},
};

/** B-stream against B-full (a fresh viewer's attach snapshot). */
export const STREAM_VS_FULL: Pins = {
	"scrollback-session/in-sync-frame": {
		cursor: NOTHING_SENT,
		screenText: NOTHING_SENT,
		screenStyles: NOTHING_SENT,
		screenWrap: NOTHING_SENT,
		historyText: NOTHING_SENT,
		historyStyles: NOTHING_SENT,
		historyWrap: NOTHING_SENT,
		hyperlinks: NOTHING_SENT,
	},
};

/** B-stream against B-skipped (a viewer framed only at the cuts). */
export const STREAM_VS_SKIPPED: Pins = {
	"scrollback-session/in-sync-frame": {
		screenText: SKIPPED_LAGS,
		screenStyles: SKIPPED_LAGS,
		screenWrap: SKIPPED_LAGS,
		historyText: SKIPPED_LAGS,
		historyStyles: SKIPPED_LAGS,
		historyWrap: SKIPPED_LAGS,
		hyperlinks: SKIPPED_LAGS,
	},
};
