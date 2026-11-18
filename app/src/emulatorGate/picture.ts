// The "picture" form (#793, slice T-1): rows of runs with start and end
// column, style, text and link, the same shape the Rust engine dumps in
// NAME.engine.json. Terminal A (xterm.js) is converted into it here, and the
// engine's file is read into it, so the two compare picture with picture.

import type { IBuffer, IBufferCell, Terminal } from "@xterm/headless";

/** Canonical style text: `fg=<c> bg=<c> flags`; colours `def`, `p<n>`, `rgb:<hex>`. */
export type StyleKey = string;

export interface Run {
	col: number;
	/** Exclusive. */
	end: number;
	style: StyleKey;
	text: string;
	link: string;
}

export interface Row {
	wrapped: boolean;
	runs: Run[];
}

export interface Modes {
	appCursorKeys: boolean;
	appKeypad: boolean;
	bracketedPaste: boolean;
	focusReporting: boolean;
	mouseTracking: "off" | "click" | "drag" | "motion";
	syncPending: boolean;
}

export interface Picture {
	rows: number;
	cols: number;
	altScreen: boolean;
	cursor: { row: number; col: number; visible: boolean };
	modes: Modes;
	/** Empty while the alternate screen is active (the engine cannot read it). */
	history: Row[];
	screen: Row[];
}

export const DEFAULT_STYLE: StyleKey = "fg=def bg=def -";

// Style keys list the seven flags both sides know, in this order.
const FLAG_ORDER = ["bold", "italic", "dim", "underline", "inverse", "invisible", "strikethrough"];
export function styleKey(fg: string, bg: string, flags: object): StyleKey {
	const f = FLAG_ORDER.filter((n) => (flags as Record<string, unknown>)[n] === true).join(",");
	return `fg=${fg} bg=${bg} ${f || "-"}`;
}

// ---- xterm.js -> picture ------------------------------------------------

interface Core {
	_oscLinkService?: { getLinkData(id: number): { uri?: string } | undefined };
	coreService?: { isCursorHidden?: boolean };
}
interface InternalCell {
	hasExtendedAttrs?(): number;
	extended?: { urlId?: number };
}

const core = (t: Terminal) => (t as unknown as { _core: Core })._core;

// The public cell has no hyperlink accessor. Read the OSC 8 id from the
// internal cell, and only when the cell really carries extended attributes: a
// line keeps a stale entry in its attribute table after the cell is erased.
function linkOf(t: Terminal, c: IBufferCell): string {
	// (headless hands out its internal CellData as the public cell.)
	const cell = c as unknown as InternalCell;
	const id = cell.hasExtendedAttrs?.() ? cell.extended?.urlId : 0;
	return id ? (core(t)._oscLinkService?.getLinkData(id)?.uri ?? `#${id}`) : "";
}

function readRow(t: Terminal, buf: IBuffer, y: number, cols: number): Row {
	const line = buf.getLine(y);
	const row: Row = { wrapped: line?.isWrapped ?? false, runs: [] };
	if (!line) return row;
	const scratch = buf.getNullCell();
	let cur: Run | undefined;
	for (let x = 0; x < cols; x++) {
		const c = line.getCell(x, scratch);
		if (!c) break;
		const col = (isDef: boolean, rgb: boolean, v: number) =>
			isDef ? "def" : rgb ? `rgb:${v.toString(16).padStart(6, "0")}` : `p${v}`;
		const style = styleKey(
			col(c.isFgDefault(), c.isFgRGB(), c.getFgColor()),
			col(c.isBgDefault(), c.isBgRGB(), c.getBgColor()),
			{
				bold: !!c.isBold(),
				italic: !!c.isItalic(),
				dim: !!c.isDim(),
				underline: !!c.isUnderline(),
				inverse: !!c.isInverse(),
				invisible: !!c.isInvisible(),
				strikethrough: !!c.isStrikethrough(),
			},
		);
		const link = linkOf(t, c);
		// A wide character's spacer cell (width 0) adds no text.
		const text = c.getWidth() === 0 ? "" : c.getChars() || " ";
		if (cur && cur.style === style && cur.link === link) {
			cur.text += text;
			cur.end = x + 1;
		} else {
			cur = { col: x, end: x + 1, style, text, link };
			row.runs.push(cur);
		}
	}
	// Trailing blank default cells are trimmed (also the tail of a longer run).
	const last = row.runs[row.runs.length - 1];
	if (last && last.style === DEFAULT_STYLE && last.link === "") {
		const kept = last.text.replace(/ +$/, "");
		if (kept === "") row.runs.pop();
		else {
			last.end -= last.text.length - kept.length;
			last.text = kept;
		}
	}
	return row;
}

const MOUSE: Record<string, Modes["mouseTracking"]> = {
	none: "off",
	x10: "click",
	vt200: "click",
	drag: "drag",
	any: "motion",
};

export function pictureOf(t: Terminal): Picture {
	const { cols, rows } = t;
	const active = t.buffer.active;
	const normal = t.buffer.normal;
	const alt = active.type === "alternate";
	const m = t.modes;
	const range = (b: IBuffer, from: number, to: number) => {
		const out: Row[] = [];
		for (let y = from; y < to; y++) out.push(readRow(t, b, y, cols));
		return out;
	};
	return {
		rows,
		cols,
		altScreen: alt,
		cursor: {
			row: active.cursorY,
			col: active.cursorX,
			visible: !core(t).coreService?.isCursorHidden,
		},
		modes: {
			appCursorKeys: m.applicationCursorKeysMode,
			appKeypad: m.applicationKeypadMode,
			bracketedPaste: m.bracketedPasteMode,
			focusReporting: m.sendFocusMode,
			mouseTracking: MOUSE[m.mouseTrackingMode] ?? "off",
			syncPending: m.synchronizedOutputMode,
		},
		history: alt ? [] : range(normal, 0, normal.baseY),
		screen: range(active, active.baseY, active.baseY + rows),
	};
}
