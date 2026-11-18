// Reads a headless terminal into plain comparable data.

import type { IBuffer, IBufferCell, Terminal } from "@xterm/headless";

export interface CellInfo {
	/** chars, width, colour modes and values, attribute flags. */
	key: string;
	chars: string;
	link: string;
}

export interface LineInfo {
	cells: CellInfo[];
	text: string;
	wrapped: boolean;
}

export interface TermState {
	cols: number;
	rows: number;
	active: "normal" | "alternate";
	cursorX: number;
	cursorY: number;
	screen: LineInfo[];
	/** Normal buffer rows above its screen (also while the alternate is active). */
	history: LineInfo[];
	/** Normal buffer's own screen; only differs in meaning while alt is active. */
	normalScreen: LineInfo[];
	modes: Record<string, string>;
}

function colour(isDefault: boolean, rgb: boolean, v: number): string {
	if (isDefault) return "def";
	if (rgb) return `rgb:${v.toString(16).padStart(6, "0")}`;
	return `pal:${v}`;
}

function flags(c: IBufferCell): string {
	let f = "";
	if (c.isBold()) f += "b";
	if (c.isItalic()) f += "i";
	if (c.isDim()) f += "d";
	if (c.isUnderline()) f += "u";
	if (c.isBlink()) f += "k";
	if (c.isInverse()) f += "r";
	if (c.isInvisible()) f += "x";
	if (c.isStrikethrough()) f += "s";
	if (c.isOverline()) f += "o";
	return f;
}

// xterm's public cell API has no hyperlink accessor; the OSC 8 id lives in
// the line's extended attributes and the target in the OSC link service.
interface CoreLike {
	_core?: {
		_oscLinkService?: { getLinkData(id: number): { uri?: string } | undefined };
		buffers?: { normal?: unknown; alt?: unknown; active?: unknown };
	};
}
interface InternalLine {
	_extendedAttrs?: Array<{ urlId?: number } | undefined>;
}
interface InternalBuffer {
	lines: { get(i: number): InternalLine | undefined };
}

function linkOf(term: Terminal, buf: IBuffer, y: number, x: number): string {
	const core = (term as unknown as CoreLike)._core;
	const ib = (buf.type === "normal" ? core?.buffers?.normal : core?.buffers?.alt) as
		| InternalBuffer
		| undefined;
	const id = ib?.lines.get(y)?._extendedAttrs?.[x]?.urlId;
	if (!id) return "";
	return core?._oscLinkService?.getLinkData(id)?.uri ?? `#${id}`;
}

function readLine(term: Terminal, buf: IBuffer, y: number, cols: number): LineInfo {
	const line = buf.getLine(y);
	const cells: CellInfo[] = [];
	if (!line) return { cells, text: "", wrapped: false };
	const scratch = buf.getNullCell();
	for (let x = 0; x < cols; x++) {
		const c = line.getCell(x, scratch);
		if (!c) break;
		const chars = c.getChars();
		const fgS = colour(c.isFgDefault(), c.isFgRGB(), c.getFgColor());
		const bgS = colour(c.isBgDefault(), c.isBgRGB(), c.getBgColor());
		const link = linkOf(term, buf, y, x);
		cells.push({
			key: `${JSON.stringify(chars)} w${c.getWidth()} fg=${fgS} bg=${bgS} [${flags(c)}]`,
			chars,
			link,
		});
	}
	return { cells, text: line.translateToString(true), wrapped: line.isWrapped };
}

function readRange(term: Terminal, buf: IBuffer, from: number, to: number, cols: number) {
	const out: LineInfo[] = [];
	for (let y = from; y < to; y++) out.push(readLine(term, buf, y, cols));
	return out;
}

export function readState(term: Terminal): TermState {
	const { cols, rows } = term;
	const active = term.buffer.active;
	const normal = term.buffer.normal;
	const m = term.modes;
	return {
		cols,
		rows,
		active: active.type,
		cursorX: active.cursorX,
		cursorY: active.cursorY,
		screen: readRange(term, active, active.baseY, active.baseY + rows, cols),
		history: readRange(term, normal, 0, normal.baseY, cols),
		normalScreen: readRange(term, normal, normal.baseY, normal.baseY + rows, cols),
		modes: {
			applicationCursorKeys: String(m.applicationCursorKeysMode),
			applicationKeypad: String(m.applicationKeypadMode),
			bracketedPaste: String(m.bracketedPasteMode),
			insert: String(m.insertMode),
			mouseTracking: m.mouseTrackingMode,
			origin: String(m.originMode),
			reverseWraparound: String(m.reverseWraparoundMode),
			sendFocus: String(m.sendFocusMode),
			synchronizedOutput: String(m.synchronizedOutputMode),
			wraparound: String(m.wraparoundMode),
		},
	};
}
