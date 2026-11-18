/// <reference types="node" />
// Reads NAME.engine.json (the Rust engine's pictures, README "The engine
// pictures") into the picture form of picture.ts.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { FIXTURE_DIR, type FixtureName } from "./fixtures.ts";
import { type Modes, type Picture, type Row, type StyleKey, styleKey } from "./picture.ts";

type Colour = "default" | { p: number } | { rgb: string };
interface RawStyle {
	fg: Colour;
	bg: Colour;
	bold: boolean;
	italic: boolean;
	dim: boolean;
	underline: boolean;
	inverse: boolean;
	invisible: boolean;
	strikethrough: boolean;
}
type RawRun = [number, number, number, string] | [number, number, number, string, string];
interface RawRow {
	wrapped: boolean;
	runs: RawRun[];
}
interface RawCut {
	name: string;
	afterEvent: number;
	rows: number;
	cols: number;
	altScreen: boolean;
	cursor: { row: number; col: number; visible: boolean };
	modes: {
		app_cursor_keys: boolean;
		app_keypad: boolean;
		bracketed_paste: boolean;
		focus_reporting: boolean;
		mouse_tracking: Modes["mouseTracking"];
		sync_pending: boolean;
	};
	history: RawRow[];
	screen: RawRow[];
}
interface RawFile {
	version: number;
	fixture: string;
	styles: RawStyle[];
	cuts: RawCut[];
}

function colour(c: Colour): string {
	if (c === "default") return "def";
	return "p" in c ? `p${c.p}` : `rgb:${c.rgb}`;
}

export function loadEnginePictures(name: FixtureName): Map<number, Picture> {
	const raw = JSON.parse(
		readFileSync(resolve(FIXTURE_DIR, `${name}.engine.json`), "utf8"),
	) as RawFile;
	const styles: StyleKey[] = raw.styles.map((s) => styleKey(colour(s.fg), colour(s.bg), s));
	const rows = (rs: RawRow[]): Row[] =>
		rs.map((r) => ({
			wrapped: r.wrapped,
			runs: r.runs.map((u) => ({
				col: u[0],
				end: u[1],
				style: styles[u[2]] ?? `?style${u[2]}`,
				text: u[3],
				link: u[4] ?? "",
			})),
		}));
	const out = new Map<number, Picture>();
	for (const c of raw.cuts) {
		out.set(c.afterEvent, {
			rows: c.rows,
			cols: c.cols,
			altScreen: c.altScreen,
			cursor: c.cursor,
			modes: {
				appCursorKeys: c.modes.app_cursor_keys,
				appKeypad: c.modes.app_keypad,
				bracketedPaste: c.modes.bracketed_paste,
				focusReporting: c.modes.focus_reporting,
				mouseTracking: c.modes.mouse_tracking,
				syncPending: c.modes.sync_pending,
			},
			history: rows(c.history),
			screen: rows(c.screen),
		});
	}
	return out;
}
