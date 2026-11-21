// terminalPathText — the pure half of Cmd/Ctrl-click on a file path in a
// terminal: which stretches of the rows around the hovered one might name
// a file, and where in the buffer each lies. Whether a candidate is a real
// file is the backend's call (`resolve_terminal_paths`), so this side errs
// towards proposing: a stray `and/or` costs one failed stat, never a
// false link.
//
// Two kinds of line break split a path. xterm's own soft wrap is exact
// (`isWrapped`), so `logicalLine` joins those rows outright. A CLI that
// lays out its own text (Claude Code and opencode, via Ink) breaks a word
// longer than its column with a real newline, often re-indenting the
// rest, and nothing in the buffer says that happened. So a token that
// ends one row and the token that starts the next are tried both joined
// and apart, and the longest that exists wins: a wrong join names a file
// that isn't there and loses on its own. A join is only tried where the
// row shape allows a hard break at all (`mayContinue`).

/** 1-based, like xterm's `IBufferCellPosition`. */
export interface CellPos {
	x: number;
	y: number;
}

/** A logical line: its text, the cell each UTF-16 unit came from, and
 *  the 1-based buffer rows it spans. */
export interface Row {
	text: string;
	cells: CellPos[];
	top: number;
	bottom: number;
}

/** One way to read a link: `text` is the whole of it, `:line:col`
 *  included; `path` is what to resolve. `start`/`end` are inclusive. */
export interface PathVariant {
	path: string;
	text: string;
	start: CellPos;
	end: CellPos;
}

/** One link's readings, longest first; the link is the first that exists. */
export type PathLink = PathVariant[];

/** Bounds the per-hover stat batch: variants across every link. */
export const MAX_PATHS = 64;
/** Rows a hard-broken path may span on either side of the hovered one. */
export const MAX_JOINED_ROWS = 4;

// Paths with spaces are out of reach without quoting, so a token ends at
// whitespace or at a character no CLI prints inside an unquoted path.
const TOKEN = /[^\s"'`<>|]+/g;
const LEADING = /^[([{@]+/;
const TRAILING = /[)\]}.,;:!?]+$/;
// `Read(` in `Read(/tmp/shot.png)`, how Claude Code heads a tool call (#823).
// No separator in the name, so `app/(group)/page.tsx` is not a call.
const CALL_PREFIX = /^[A-Za-z_][\w.-]*\(/;
const LINE_SUFFIX = /:\d+(?::\d+)?$/;
const WINDOWS_ABSOLUTE = /^[A-Za-z]:[\\/]/;

function looksLikePath(path: string): boolean {
	if (path.includes("://") || path.includes("*")) return false;
	if (path.startsWith("//")) return false;
	if (path.startsWith("~")) return /^~[\\/]./.test(path);
	if (WINDOWS_ABSOLUTE.test(path)) return true;
	// Absolute or relative, it needs a separator and something that isn't one.
	return /[\\/]/.test(path) && /[^\\/.]/.test(path);
}

interface Piece {
	row: number;
	start: number;
	end: number;
}

function tokens(rows: Row[], row: number): Piece[] {
	const text = rows[row]?.text ?? "";
	return [...text.matchAll(TOKEN)].map((m) => ({
		row,
		start: m.index,
		end: m.index + m[0].length,
	}));
}

/** `pieces` joined with nothing between them, read as a path: surrounding
 *  brackets, a leading `@` (Claude Code's file mentions) and trailing
 *  punctuation are dropped, a `:line(:col)` suffix stays in the link but
 *  not in `path`. A `Name(` call prefix gives a second, shorter reading
 *  without it (#823). Empty when nothing is path-shaped — URLs are left
 *  to the URL matcher. */
function variants(rows: Row[], pieces: Piece[]): PathVariant[] {
	let raw = "";
	const cells: CellPos[] = [];
	for (const p of pieces) {
		const row = rows[p.row];
		if (!row) return [];
		raw += row.text.slice(p.start, p.end);
		cells.push(...row.cells.slice(p.start, p.end));
	}
	const lead = LEADING.exec(raw)?.[0].length ?? 0;
	const trail = TRAILING.exec(raw)?.[0].length ?? 0;
	const call = CALL_PREFIX.exec(raw.slice(lead))?.[0].length ?? 0;
	const out: PathVariant[] = [];
	for (const from of call > 0 ? [lead, lead + call] : [lead]) {
		const text = raw.slice(from, raw.length - trail);
		const path = text.replace(LINE_SUFFIX, "");
		const start = cells[from];
		const end = cells[raw.length - trail - 1];
		if (looksLikePath(path) && start && end) out.push({ path, text, start, end });
	}
	return out;
}

/** The column of `row`'s last non-space cell; 0 when blank. */
function endColumn(row: Row | undefined): number {
	const last = row ? /\S\s*$/.exec(row.text) : null;
	return (last && row?.cells[last.index]?.x) || 0;
}

/** A hard break fills its row to the column edge, and the row after it
 *  wraps at that same edge, so a continuation never reaches further
 *  right than the row it continues. That rules out joining a listing's
 *  `src/` to the `target/` below it without knowing the CLI's layout. */
function mayContinue(rows: Row[], upper: number): boolean {
	return endColumn(rows[upper]) >= endColumn(rows[upper + 1]);
}

/** Every link that could touch `rows[focus]`. A token alone on its row
 *  may continue on both sides, so the chain grows through rows holding a
 *  single token and stops at the first that holds more (or none). */
export function pathLinks(rows: Row[], focus: number): PathLink[] {
	const here = tokens(rows, focus);
	const out: PathLink[] = [];
	let budget = MAX_PATHS;
	here.forEach((tok, i) => {
		const before: Piece[] = [];
		for (let r = focus - 1; i === 0 && r >= 0 && focus - r <= MAX_JOINED_ROWS; r--) {
			if (!mayContinue(rows, r)) break;
			const t = tokens(rows, r);
			const last = t[t.length - 1];
			if (!last) break;
			before.unshift(last);
			if (t.length > 1) break;
		}
		const after: Piece[] = [];
		for (
			let r = focus + 1;
			i === here.length - 1 && r < rows.length && r - focus <= MAX_JOINED_ROWS;
			r++
		) {
			if (!mayContinue(rows, r - 1)) break;
			const t = tokens(rows, r);
			const first = t[0];
			if (!first) break;
			after.push(first);
			if (t.length > 1) break;
		}
		const pieces = [...before, tok, ...after];
		const k = before.length;
		const link: PathLink = [];
		for (let len = pieces.length; len >= 1; len--) {
			for (let s = Math.max(0, k - len + 1); s <= k && s + len <= pieces.length; s++) {
				link.push(...variants(rows, pieces.slice(s, s + len)));
			}
		}
		if (link.length > 0 && link.length <= budget) {
			out.push(link);
			budget -= link.length;
		}
	});
	return out;
}

/** The slice of xterm's buffer API read here; `IBuffer` satisfies it. */
export interface CellLike {
	getChars(): string;
	getWidth(): number;
}
export interface LineLike {
	readonly isWrapped: boolean;
	readonly length: number;
	getCell(x: number): CellLike | undefined;
}
export interface BufferLike {
	getLine(y: number): LineLike | undefined;
}

/** Joins rows on either side of a row this far at most. */
const MAX_WRAPPED_ROWS = 32;

/** The soft-wrapped logical line containing 1-based row `y`. Empty cells
 *  read as spaces; the second half of a wide character is skipped. */
export function logicalLine(buf: BufferLike, y: number): Row {
	const row = y - 1;
	let top = row;
	while (top > 0 && row - top < MAX_WRAPPED_ROWS && buf.getLine(top)?.isWrapped) top--;
	let bottom = row;
	while (bottom - row < MAX_WRAPPED_ROWS && buf.getLine(bottom + 1)?.isWrapped) bottom++;

	let text = "";
	const cells: CellPos[] = [];
	for (let r = top; r <= bottom; r++) {
		const line = buf.getLine(r);
		if (!line) break;
		for (let x = 0; x < line.length; x++) {
			const cell = line.getCell(x);
			if (!cell || cell.getWidth() === 0) continue;
			const chars = cell.getChars() || " ";
			text += chars;
			for (let k = 0; k < chars.length; k++) cells.push({ x: x + 1, y: r + 1 });
		}
	}
	return { text, cells, top: top + 1, bottom: bottom + 1 };
}

/** The logical line holding 1-based row `y`, with up to
 *  `MAX_JOINED_ROWS` logical lines on either side; `focus` indexes it. */
export function rowsAround(buf: BufferLike, y: number): { rows: Row[]; focus: number } {
	const here = logicalLine(buf, y);
	const rows = [here];
	let focus = 0;
	for (let top = here.top; focus < MAX_JOINED_ROWS && top > 1; focus++) {
		const above = logicalLine(buf, top - 1);
		rows.unshift(above);
		top = above.top;
	}
	// `getLine` is 0-based, so `getLine(bottom)` is the row after `bottom`.
	for (let bottom = here.bottom, n = 0; n < MAX_JOINED_ROWS && buf.getLine(bottom); n++) {
		const below = logicalLine(buf, bottom + 1);
		rows.push(below);
		bottom = below.bottom;
	}
	return { rows, focus };
}
