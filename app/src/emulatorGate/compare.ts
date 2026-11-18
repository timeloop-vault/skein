// Compares two pictures by category (#793 gate, second version). Side A is
// xterm.js, side B the Rust engine. A difference is reported compactly:
// category, first differing row and column, an excerpt of both sides and how
// many rows differ.

import { excuseLinkUnderline } from "./linkUnderline.ts";
import { DEFAULT_STYLE, type Picture, type Row } from "./picture.ts";

export const CATEGORIES = [
	"activeBuffer",
	"cursor",
	"screenText",
	"screenStyles",
	"screenWrap",
	"historyText",
	"historyStyles",
	"historyWrap",
	"hyperlinks",
	"modes",
] as const;
export type Category = (typeof CATEGORIES)[number];

export interface Diff {
	category: Category;
	/** "screen" or "history" for row categories, "" otherwise. */
	where: string;
	row: number;
	col: number;
	rowsDiffering: number;
	/** xterm.js and the engine. */
	a: string;
	b: string;
}

export function esc(s: string): string {
	let out = "";
	for (const ch of s) {
		const n = ch.charCodeAt(0);
		out += n < 0x20 || n === 0x7f ? `\\x${n.toString(16).padStart(2, "0")}` : ch;
	}
	return out;
}

/** Terminal cell width of one code point (enough for the fixtures; both sides get the same one). */
function width(cp: number): number {
	if (cp === 0x200d || (cp >= 0xfe00 && cp <= 0xfe0f) || (cp >= 0x300 && cp <= 0x36f)) return 0;
	const wide =
		(cp >= 0x1100 && cp <= 0x115f) ||
		(cp >= 0x2e80 && cp <= 0xa4cf) ||
		(cp >= 0xac00 && cp <= 0xd7a3) ||
		(cp >= 0xf900 && cp <= 0xfaff) ||
		(cp >= 0xfe30 && cp <= 0xfe6f) ||
		(cp >= 0xff00 && cp <= 0xff60) ||
		(cp >= 0xffe0 && cp <= 0xffe6) ||
		(cp >= 0x1f300 && cp <= 0x1f64f) ||
		(cp >= 0x1f900 && cp <= 0x1f9ff) ||
		(cp >= 0x20000 && cp <= 0x3fffd);
	return wide ? 2 : 1;
}

/** Every non-blank character of a row with the column it starts at: `col:char`. */
function glyphs(r: Row): string[] {
	const out: string[] = [];
	for (const run of r.runs) {
		let col = run.col;
		for (const ch of run.text) {
			const cp = ch.codePointAt(0) ?? 0;
			if (ch !== " ") out.push(`${col}:${ch}`);
			col += width(cp);
		}
		// A run whose text does not span its columns is itself misaligned.
		if (col !== run.end) out.push(`${run.col}:span${col - run.col}/${run.end - run.col}`);
	}
	return out;
}

/** Style runs with adjacent equal styles merged, trailing default blanks dropped. */
function styles(r: Row): string[] {
	const out: string[] = [];
	let last: { col: number; end: number; s: string } | undefined;
	for (const run of r.runs) {
		if (last && last.s === run.style && last.end === run.col) last.end = run.end;
		else {
			last = { col: run.col, end: run.end, s: run.style };
			out.push("");
		}
		out[out.length - 1] = `${last.col}-${last.end}:${last.s}`;
	}
	// A trailing default-styled run is blank by construction of the trimming.
	return out.filter((e, i) => !(i === out.length - 1 && e.endsWith(`:${DEFAULT_STYLE}`)));
}

function links(r: Row): string[] {
	const out: string[] = [];
	let last: { col: number; end: number; l: string } | undefined;
	for (const run of r.runs) {
		if (!run.link) {
			last = undefined;
			continue;
		}
		if (last && last.l === run.link && last.end === run.col) {
			last.end = run.end;
			out[out.length - 1] = `${last.col}-${last.end}:${last.l}`;
		} else {
			last = { col: run.col, end: run.end, l: run.link };
			out.push(`${last.col}-${last.end}:${last.l}`);
		}
	}
	return out;
}

const text = (r: Row) =>
	r.runs
		.map((u) => u.text)
		.join("")
		.replace(/ +$/, "");

type RowView = (r: Row) => string[];
const colOf = (e: string | undefined) => (e === undefined ? 0 : Number.parseInt(e, 10) || 0);

function rowsDiff(
	category: Category,
	where: string,
	a: Row[],
	b: Row[],
	view: RowView,
	show: (r: Row | undefined, col: number) => string,
): Diff | undefined {
	let first = -1;
	let col = 0;
	let count = 0;
	const n = Math.max(a.length, b.length);
	for (let i = 0; i < n; i++) {
		const va = a[i] ? view(a[i] as Row) : ["<no row>"];
		const vb = b[i] ? view(b[i] as Row) : ["<no row>"];
		if (va.join("\n") === vb.join("\n")) continue;
		count++;
		if (first < 0) {
			first = i;
			let k = 0;
			while (k < va.length && k < vb.length && va[k] === vb[k]) k++;
			col = colOf(va[k] ?? vb[k]);
		}
	}
	if (first < 0) return undefined;
	return {
		category,
		where,
		row: first,
		col,
		rowsDiffering: count,
		a: show(a[first], col),
		b: show(b[first], col),
	};
}

const showText = (r: Row | undefined, col: number) => {
	if (!r) return "<no row>";
	const t = [...text(r)];
	const from = Math.max(0, col - 8);
	return esc(`${from > 0 ? "…" : ""}${t.slice(from, from + 32).join("")}`);
};
const showView = (view: RowView) => (r: Row | undefined) =>
	r ? view(r).slice(0, 4).join(" ") : "<no row>";
const showWrap = (r: Row | undefined) => (r ? `wrapped=${r.wrapped}` : "<no row>");
const wrapView: RowView = (r) => [String(r.wrapped)];

export interface CompareOptions {
	/** Apply the link-underline excuse (xterm.js vs the Rust engine). Default true. */
	excuse?: boolean;
	/** Skip the history categories. Default: when B is on the alternate screen. */
	skipHistory?: boolean;
}

export function compare(
	a: Picture,
	b: Picture,
	opts: CompareOptions = {},
): { diffs: Diff[]; skippedHistory: boolean; excused: number } {
	const diffs: Diff[] = [];
	const skipHist = opts.skipHistory ?? b.altScreen;
	const none = (rows: Row[]) => ({ rows, excused: 0 });
	const exScreen = opts.excuse === false ? none(a.screen) : excuseLinkUnderline(a.screen, b.screen);
	const exHist =
		skipHist || opts.excuse === false ? none(a.history) : excuseLinkUnderline(a.history, b.history);
	const plain = (category: Category, av: string, bv: string) => {
		if (av !== bv) {
			diffs.push({ category, where: "", row: 0, col: 0, rowsDiffering: 0, a: av, b: bv });
		}
	};
	plain("activeBuffer", a.altScreen ? "alternate" : "normal", b.altScreen ? "alternate" : "normal");
	const cur = (p: Picture) => `row ${p.cursor.row} col ${p.cursor.col} visible ${p.cursor.visible}`;
	plain("cursor", cur(a), cur(b));
	const skippedHistory = skipHist;
	const parts: Array<
		[Category, string, Row[], Row[], RowView, (r: Row | undefined, c: number) => string]
	> = [
		["screenText", "screen", a.screen, b.screen, glyphs, showText],
		["screenStyles", "screen", exScreen.rows, b.screen, styles, showView(styles)],
		["screenWrap", "screen", a.screen, b.screen, wrapView, showWrap],
	];
	if (!skippedHistory) {
		parts.push(
			["historyText", "history", a.history, b.history, glyphs, showText],
			["historyStyles", "history", exHist.rows, b.history, styles, showView(styles)],
			["historyWrap", "history", a.history, b.history, wrapView, showWrap],
		);
	}
	for (const [cat, where, ra, rb, view, show] of parts) {
		const d = rowsDiff(cat, where, ra, rb, view, show);
		if (d) diffs.push(d);
	}
	// Hyperlinks: screen first, then history; one entry, the first place that differs.
	const linkParts: Array<[string, Row[], Row[]]> = [["screen", a.screen, b.screen]];
	if (!skippedHistory) linkParts.push(["history", a.history, b.history]);
	let total = 0;
	let firstLink: Diff | undefined;
	for (const [where, ra, rb] of linkParts) {
		const d = rowsDiff("hyperlinks", where, ra, rb, links, showView(links));
		if (!d) continue;
		total += d.rowsDiffering;
		firstLink ??= d;
	}
	if (firstLink) diffs.push({ ...firstLink, rowsDiffering: total });
	const ma = Object.entries(a.modes).sort();
	const mb = Object.entries(b.modes).sort();
	const md = ma.filter(([, v], i) => v !== mb[i]?.[1]);
	if (md.length) {
		diffs.push({
			category: "modes",
			where: "",
			row: 0,
			col: 0,
			rowsDiffering: md.length,
			a: md.map(([k, v]) => `${k}=${v}`).join(" "),
			b: md.map(([k]) => `${k}=${b.modes[k as keyof Picture["modes"]]}`).join(" "),
		});
	}
	return { diffs, skippedHistory, excused: exScreen.excused + exHist.excused };
}

export function formatDiff(d: Diff): string {
	const at = d.where ? ` ${d.where} row ${d.row} col ${d.col} (${d.rowsDiffering} rows)` : "";
	return `${d.category}${at}: xterm [${d.a}] vs engine [${d.b}]`;
}
