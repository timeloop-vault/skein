import type { HarnessKind } from "./types";

/// One terminal cell. `ch` is the cell's characters: " " for a written blank,
/// "" for the trailing half of a wide character (some terminals also report an
/// untouched cell as "", which is why blankness below tolerates it).
export interface ScreenCell {
	ch: string;
	dim: boolean;
}

/// A snapshot of the visible terminal screen (not scrollback). Row 0 is the
/// top of the viewport; every row has one cell per column.
export interface ScreenSnapshot {
	rows: readonly (readonly ScreenCell[])[];
	cursorX: number;
	cursorY: number;
}

/// "empty" is the only answer a caller may act on: the composer is recognised
/// AND holds nothing. "text" is informational (recognised, holds a draft).
/// Anything not positively recognised is "unknown". A false "empty" lets an
/// automatic mail nudge paste over a user's half-typed draft, which is the
/// dangerous direction, so every rule below is a conjunction and any doubt
/// falls through to "unknown".
export type ComposerReading = "empty" | "text" | "unknown";

type Row = readonly ScreenCell[];

const isBlank = (c: ScreenCell): boolean => c.ch === " " || c.ch === "";

/// True if the row holds a wide character or a multi-codepoint cell. We have
/// never captured those on a composer row, so we do not guess what they mean.
/// A "" cell that follows a non-blank cell is the trailing half of a wide
/// char; a "" after a blank (or at the start) is just an untouched cell.
function hasWide(row: Row): boolean {
	for (let i = 0; i < row.length; i++) {
		const c = row[i];
		if (c === undefined) return true;
		if ([...c.ch].length > 1) return true;
		if (c.ch === "") {
			const prev = row[i - 1];
			if (prev !== undefined && !isBlank(prev)) return true;
		}
	}
	return false;
}

const rowText = (cells: Row): string => cells.map((c) => c.ch).join("");

/// A full-width horizontal rule: every non-blank cell is `─` and they cover
/// most of the width, so a stray `─` in some other row never qualifies.
function isRule(row: Row | undefined): boolean {
	if (row === undefined || row.length < 10) return false;
	let n = 0;
	for (const c of row) {
		if (isBlank(c)) continue;
		if (c.ch !== "─") return false;
		n++;
	}
	return n >= row.length * 0.8;
}

/// Claude Code: the live input row is `❯ ` at column 0 sandwiched between two
/// full-width rules. Anchoring on the cursor row (not on "the last `❯` row")
/// keeps history rows, which also start with `❯`, and dialogs such as the
/// trust prompt (` ❯ No, exit`, glyph at column 1) out of the running. A
/// multi-line draft puts the cursor on a glyph-less continuation row or leaves
/// a non-rule row below the cursor, so it can never satisfy this.
function readClaude(snap: ScreenSnapshot): ComposerReading {
	const row = snap.rows[snap.cursorY];
	if (row === undefined || row.length < 3) return "unknown";
	if (row[0]?.ch !== "❯" || row[1]?.ch !== " ") return "unknown";
	if (!isRule(snap.rows[snap.cursorY - 1]) || !isRule(snap.rows[snap.cursorY + 1])) {
		return "unknown";
	}
	if (hasWide(row)) return "unknown";

	const tail = row.slice(2);
	const filled = tail.filter((c) => !isBlank(c));
	if (filled.some((c) => !c.dim)) return "text";
	// Only from here on is everything after the glyph blank or dim.
	if (snap.cursorX !== 2) return "unknown";
	if (filled.length === 0) return "empty";
	// Dim text is Claude's placeholder. Typed text is never dim, so this is
	// only ever the placeholder; the prefix check guards against some other
	// dim hint we have not seen.
	return rowText(tail).trim().startsWith('Try "') ? "empty" : "unknown";
}

// opencode's placeholder is coloured grey, not dim, so attributes cannot tell
// it from a draft; the text has to match its exact shape instead.
const OPENCODE_PLACEHOLDER = /^Ask anything… "[^"]*"$/;

/// opencode: a left-bar `┃` box: padding row, text row(s) starting at bar+3,
/// padding row, then the model row and the `╹▀▀▀` bottom edge. Empty means the
/// cursor sits at bar+3 on a text row whose rows directly above and below are
/// pure padding (a bar and nothing else), with the model row two below and the
/// bottom edge three below at the same column. A multi-line draft has a text
/// row adjacent to the cursor row, so the padding test fails; the model-row
/// and edge checks make us refuse any layout we have not captured.
function readOpencode(snap: ScreenSnapshot): ComposerReading {
	const { rows, cursorX, cursorY } = snap;
	const row = rows[cursorY];
	if (row === undefined) return "unknown";
	const bar = row.findIndex((c) => !isBlank(c));
	if (bar < 0 || row[bar]?.ch !== "┃") return "unknown";
	if (hasWide(row)) return "unknown";

	const tail = row.slice(bar + 1);
	const first = tail.findIndex((c) => !isBlank(c));
	const text = rowText(tail).trim();
	const placeholder = first === 2 && OPENCODE_PLACEHOLDER.test(text);
	if (first >= 0 && !placeholder) return "text";

	if (cursorX !== bar + 3) return "unknown";
	const isPadding = (r: Row | undefined): boolean =>
		r !== undefined && r[bar]?.ch === "┃" && r.every((c, i) => i === bar || isBlank(c));
	if (!isPadding(rows[cursorY - 1]) || !isPadding(rows[cursorY + 1])) {
		return "unknown";
	}
	const model = rows[cursorY + 2];
	if (model?.[bar]?.ch !== "┃" || !model.slice(bar + 1).some((c) => !isBlank(c))) {
		return "unknown";
	}
	if (rows[cursorY + 3]?.[bar]?.ch !== "╹") return "unknown";
	return "empty";
}

/// Read a screen snapshot and say whether the harness's composer is provably
/// empty. Only Claude Code and opencode have a recognised layout.
export function readComposer(kind: HarnessKind, snap: ScreenSnapshot): ComposerReading {
	if (
		!Number.isInteger(snap.cursorX) ||
		!Number.isInteger(snap.cursorY) ||
		snap.cursorX < 0 ||
		snap.cursorY < 0 ||
		snap.cursorY >= snap.rows.length
	) {
		return "unknown";
	}
	if (kind === "claude") return readClaude(snap);
	if (kind === "opencode") return readOpencode(snap);
	return "unknown";
}
