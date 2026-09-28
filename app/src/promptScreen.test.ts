import { describe, expect, it } from "vitest";
import {
	type ComposerReading,
	type ScreenCell,
	type ScreenSnapshot,
	readComposer,
} from "./promptScreen";
import type { HarnessKind } from "./types";

/// Test snapshots keep mutable rows so cases can poke individual cells.
interface MutableSnap extends ScreenSnapshot {
	rows: ScreenCell[][];
}

const COLS = 120;
const ROWS = 40;
const RULE = "─".repeat(COLS);

/// Build a snapshot from `{ rowIndex: text }`. `dimFrom` marks every
/// non-blank cell of that row from the given column on as dim (Claude's
/// placeholder).
function screen(
	lines: Record<number, string>,
	cursorX: number,
	cursorY: number,
	dimFrom: Record<number, number> = {},
): MutableSnap {
	const rows: ScreenCell[][] = [];
	for (let y = 0; y < ROWS; y++) {
		const chars = [...(lines[y] ?? "")];
		const from = dimFrom[y] ?? Number.POSITIVE_INFINITY;
		const cells: ScreenCell[] = [];
		for (let x = 0; x < COLS; x++) {
			const ch = chars[x] ?? " ";
			cells.push({ ch, dim: x >= from && ch !== " " });
		}
		rows.push(cells);
	}
	return { rows, cursorX, cursorY };
}

const PLACE = 'Try "fix typecheck errors"';
const CLAUDE_TAIL = {
	38: "  ⚠ Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker",
	39: "  -- INSERT -- ⏵⏵ auto mode on (shift+tab to cycle)",
};

/// Claude composer between its two rules; the input row is 36.
const claude = (input: string, x: number, dimFrom?: number): MutableSnap =>
	screen(
		{ 35: RULE, 36: input, 37: RULE, ...CLAUDE_TAIL },
		x,
		36,
		dimFrom === undefined ? {} : { 36: dimFrom },
	);

const MODEL = "  Build · Qwen3.8-27B · 5090 (~115-150 tok/s) RTX 5090 (local)";
const EDGE = `╹${"▀".repeat(60)}`;
const pad = (n: number, s: string): string => " ".repeat(n) + s;

/// opencode composer: bar at column `bar`, first text row at `firstText`
/// (padding row above it, padding + model row + bottom edge below the text).
function opencode(
	bar: number,
	texts: string[],
	cursorX: number,
	cursorY: number,
	firstText = 20,
): MutableSnap {
	const lines: Record<number, string> = {};
	lines[firstText - 1] = pad(bar, "┃");
	texts.forEach((t, i) => {
		lines[firstText + i] = pad(bar, `┃${t === "" ? "" : `  ${t}`}`);
	});
	const after = firstText + texts.length;
	lines[after] = pad(bar, "┃");
	lines[after + 1] = pad(bar, `┃${MODEL}`);
	lines[after + 2] = pad(bar, EDGE);
	return screen(lines, cursorX, cursorY);
}

const PH = 'Ask anything… "Fix broken tests"';

describe("readComposer: captured Claude Code 2.1.284 states", () => {
	const cases: [string, ScreenSnapshot, ComposerReading][] = [
		["A empty placeholder", claude(`❯ ${PLACE}`, 2, 2), "empty"],
		["B typed", claude("❯ hello world", 13), "text"],
		["C erased back to placeholder", claude(`❯ ${PLACE}`, 2, 2), "empty"],
		[
			"D multi-line",
			screen({ 34: RULE, 35: "❯ line one", 36: "  line two", 37: RULE, ...CLAUDE_TAIL }, 10, 36),
			"unknown",
		],
		["D2 cleared", claude(`❯ ${PLACE}`, 2, 2), "empty"],
		[
			"E after a turn: bare glyph row",
			screen(
				{
					6: "❯ reply with just the word ok",
					8: "● ok",
					35: RULE,
					36: "❯ ",
					37: RULE,
					...CLAUDE_TAIL,
				},
				2,
				36,
			),
			"empty",
		],
	];
	it.each(cases)("%s", (_n, snap, want) => {
		expect(readComposer("claude", snap)).toBe(want);
	});
});

describe("readComposer: captured opencode 1.18.32 states", () => {
	const cases: [string, ScreenSnapshot, ComposerReading][] = [
		["A empty start screen", opencode(23, [PH], 26, 20), "empty"],
		["B typed", opencode(23, ["hello world"], 37, 20), "text"],
		["C erased", opencode(23, [PH], 26, 20), "empty"],
		[
			"D multi-line (cursor on last row)",
			opencode(23, ["line one", "line two"], 34, 20, 19),
			"text",
		],
		["D2 cleared", opencode(23, [PH], 26, 20), "empty"],
		["E after a turn", opencode(2, [""], 5, 33, 33), "empty"],
		["F same as E", opencode(2, [""], 5, 33, 33), "empty"],
	];
	it.each(cases)("%s", (_n, snap, want) => {
		expect(readComposer("opencode", snap)).toBe(want);
	});
});

describe("readComposer: must never read empty", () => {
	const notEmpty = (kind: HarnessKind, snap: ScreenSnapshot): void => {
		expect(readComposer(kind, snap)).not.toBe("empty");
	};

	it("claude: cursor moved to x=2 at the start of a non-dim draft", () => {
		expect(readComposer("claude", claude("❯ hello", 2))).toBe("text");
	});
	it("claude: user typed the placeholder shape (non-dim)", () => {
		expect(readComposer("claude", claude('❯ Try "x"', 9))).toBe("text");
		expect(readComposer("claude", claude('❯ Try "x"', 2))).toBe("text");
	});
	it("claude: dim text that is not the placeholder", () => {
		notEmpty("claude", claude("❯ some hint", 2, 2));
	});
	it("claude: cursor not at x=2 on an empty-looking row", () => {
		notEmpty("claude", claude("❯ ", 5));
		notEmpty("claude", claude(`❯ ${PLACE}`, 10, 2));
	});
	it("claude: cursor on a history row that is not between rules", () => {
		notEmpty(
			"claude",
			screen({ 6: "❯ reply with just the word ok", 35: RULE, 36: "❯ ", 37: RULE }, 2, 6),
		);
	});
	it("claude: rule rows missing", () => {
		notEmpty("claude", screen({ 36: "❯ " }, 2, 36));
		notEmpty("claude", screen({ 35: RULE, 36: "❯ " }, 2, 36));
		notEmpty("claude", screen({ 36: "❯ ", 37: RULE }, 2, 36));
	});
	it("claude: a short stray rule does not count", () => {
		notEmpty("claude", screen({ 35: "───", 36: "❯ ", 37: "───" }, 2, 36));
	});
	it("claude: cursor on a rule row", () => {
		notEmpty("claude", screen({ 35: RULE, 36: "❯ ", 37: RULE }, 2, 35));
		notEmpty("claude", screen({ 35: RULE, 36: "❯ ", 37: RULE }, 2, 37));
	});
	it("claude: trust dialog row", () => {
		notEmpty("claude", screen({ 13: RULE, 14: " ❯ No, exit", 15: RULE }, 2, 14));
	});
	it("claude: wide char on the row", () => {
		for (const dim of [false, true]) {
			const snap = claude("❯ ", 2);
			const row = [...(snap.rows[36] ?? [])];
			row[2] = { ch: "日", dim };
			row[3] = { ch: "", dim };
			snap.rows[36] = row;
			notEmpty("claude", snap);
		}
	});

	it("opencode: draft with the cursor moved to the text start", () => {
		expect(readComposer("opencode", opencode(23, ["hello world"], 26, 20))).toBe("text");
	});
	it("opencode: typed text that is not the placeholder", () => {
		notEmpty("opencode", opencode(2, ["Ask anything"], 5, 20));
		notEmpty("opencode", opencode(2, ["Ask anything… then more"], 5, 20));
		notEmpty("opencode", opencode(2, ["hi"], 5, 20));
	});
	it("opencode: cursor not at bar+3 on a blank row", () => {
		notEmpty("opencode", opencode(2, [""], 6, 20));
		notEmpty("opencode", opencode(2, [""], 3, 20));
	});
	it("opencode: multi-line draft, cursor on a blank row next to text", () => {
		notEmpty("opencode", opencode(2, ["", "text"], 5, 20));
		notEmpty("opencode", opencode(2, ["text", ""], 5, 21));
	});
	it("opencode: no model row or bottom edge", () => {
		const blank = (n: number, snap: MutableSnap): void => {
			snap.rows[n] = (snap.rows[n] ?? []).map(() => ({ ch: " ", dim: false }));
		};
		const noModel = opencode(2, [""], 5, 20);
		blank(22, noModel);
		notEmpty("opencode", noModel);
		const noEdge = opencode(2, [""], 5, 20);
		blank(23, noEdge);
		notEmpty("opencode", noEdge);
	});
	it("opencode: bar mismatch on a padding row", () => {
		const snap = opencode(2, [""], 5, 20);
		snap.rows[21] = screen({ 0: pad(4, "┃") }, 0, 0).rows[0] ?? [];
		notEmpty("opencode", snap);
	});
	it("opencode: wide char on the row", () => {
		const snap = opencode(2, [""], 5, 20);
		const row = [...(snap.rows[20] ?? [])];
		row[5] = { ch: "日", dim: false };
		row[6] = { ch: "", dim: false };
		snap.rows[20] = row;
		notEmpty("opencode", snap);
	});
	it("opencode: cursor on a row with no bar", () => {
		notEmpty("opencode", opencode(2, [""], 5, 5));
	});

	it("empty snapshot and out-of-range cursor", () => {
		for (const kind of ["claude", "opencode"] as const) {
			expect(readComposer(kind, { rows: [], cursorX: 0, cursorY: 0 })).toBe("unknown");
			const ok = kind === "claude" ? claude("❯ ", 2) : opencode(2, [""], 5, 20);
			expect(readComposer(kind, { ...ok, cursorY: 99 })).toBe("unknown");
			expect(readComposer(kind, { ...ok, cursorY: -1 })).toBe("unknown");
			expect(readComposer(kind, { ...ok, cursorX: -1 })).toBe("unknown");
		}
	});
	it("kinds other than claude and opencode", () => {
		for (const kind of ["copilot", "byoh", "files"] as const) {
			expect(readComposer(kind, claude("❯ ", 2))).toBe("unknown");
			expect(readComposer(kind, opencode(2, [""], 5, 20))).toBe("unknown");
		}
	});
	it("a layout of the wrong CLI is unknown", () => {
		expect(readComposer("opencode", claude("❯ ", 2))).toBe("unknown");
		expect(readComposer("claude", opencode(2, [""], 5, 20))).toBe("unknown");
	});
});
