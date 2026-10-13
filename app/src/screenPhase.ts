import type { ScreenCell, ScreenSnapshot } from "./promptScreen";
import type { HarnessKind } from "./types";

/// What the rendered screen says about the harness's turn. "waiting" = it is
/// the user's turn (end of turn, or an approval box is open); "running" = a
/// turn is in flight. "unknown" means the screen was not positively
/// recognised and the caller falls back to the byte-stream heuristics.
export type ScreenPhase = "waiting" | "running" | "unknown";

const rowText = (cells: readonly ScreenCell[]): string => cells.map((c) => c.ch).join("");

// How many rows from the bottom the status line is searched in. It is the
// last row on every captured frame; a little slack covers a taller footer.
const STATUS_SEARCH_ROWS = 4;

// The context-usage field of Hermes's status line, e.g. `13.9k/200k`.
const USAGE = /\d+(?:\.\d+)?[kKM]?\/\d+(?:\.\d+)?[kKM]\b/;

/// Hermes's status line is `─ <state> │ <model> │ <used>/<window> │ … ─ <title>`.
/// Identified by all of: a leading `─`, at least two ` │ ` separators and the
/// usage field. A shell that merely prints "ready │" fails the leading rule.
/// Returns the text after the leading `─ `, or undefined.
function findStatusLine(snap: ScreenSnapshot): string | undefined {
	for (let y = snap.rows.length - 1; y >= Math.max(0, snap.rows.length - STATUS_SEARCH_ROWS); y--) {
		const row = snap.rows[y];
		if (row === undefined) continue;
		const text = rowText(row).trim();
		if (!text.startsWith("─ ")) continue;
		if (text.split(" │ ").length < 3 || !USAGE.test(text)) continue;
		return text.slice(2);
	}
	return undefined;
}

/// The approval box: a `║`-edged row carrying the title and another carrying
/// the footer hint. Both are required so output that quotes one of the phrases
/// is not mistaken for a dialog.
function hasApprovalBox(snap: ScreenSnapshot): boolean {
	let title = false;
	let footer = false;
	for (const row of snap.rows) {
		const text = rowText(row).trim();
		if (!text.startsWith("║")) continue;
		if (text.includes("approval required")) title = true;
		if (text.includes("Esc/Ctrl+C deny")) footer = true;
	}
	return title && footer;
}

/// Hermes Agent `--tui` (v0.21.7, English locale). It redraws a clock and a
/// spinner constantly, so byte-quiet never happens; the phase comes from the
/// status line instead. A draft in the composer does not matter: `ready`
/// means it is still the user's turn. The rotating face and verb of the busy
/// status are never matched.
function readHermes(snap: ScreenSnapshot): ScreenPhase {
	if (hasApprovalBox(snap)) return "waiting";
	const status = findStatusLine(snap);
	if (status === undefined) return "unknown";
	return status.startsWith("ready │ ") ? "waiting" : "running";
}

/// Read a screen snapshot and say which phase it shows (#623). Only Hermes has
/// a recognised layout, pinned to captured v0.21.7 screens in English; every
/// conjunctive rule falls through to "unknown" on any doubt.
export function readScreenPhase(kind: HarnessKind, snap: ScreenSnapshot): ScreenPhase {
	switch (kind) {
		case "hermes":
			return readHermes(snap);
		default:
			return "unknown";
	}
}
