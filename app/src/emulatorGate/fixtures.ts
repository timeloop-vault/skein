/// <reference types="node" />
// Loads the recorded byte streams, and cut lists (#793). Fixtures stay where the Rust tests read them.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const FIXTURE_DIR = resolve(here, "../../../crates/skein-daemon/src/pty/engine/fixtures");
export const ENGINE_MOD = resolve(here, "../../../crates/skein-daemon/src/pty/engine/mod.rs");

export type GateEvent =
	| { ms: number; kind: "data"; b64: string }
	| { ms: number; kind: "resize"; cols: number; rows: number };

export interface Recording {
	version: number;
	cols: number;
	rows: number;
	events: GateEvent[];
}

export interface Cut {
	name: string;
	afterEvent: number;
}

export const FIXTURES = ["claude-session", "scrollback-session"] as const;
export type FixtureName = (typeof FIXTURES)[number];

function readJson<T>(file: string): T {
	return JSON.parse(readFileSync(resolve(FIXTURE_DIR, file), "utf8")) as T;
}

export function loadFixture(name: FixtureName): {
	recording: Recording;
	cuts: Cut[];
} {
	return {
		recording: readJson<Recording>(`${name}.json`),
		cuts: readJson<Cut[]>(`${name}.cuts.json`),
	};
}

/** `HISTORY_ROWS` parsed out of the Rust source, so the two cannot drift. */
export function rustHistoryRows(): number {
	const src = readFileSync(ENGINE_MOD, "utf8");
	const m = /pub const HISTORY_ROWS:\s*usize\s*=\s*([0-9_]+)\s*;/.exec(src);
	if (!m?.[1]) throw new Error("HISTORY_ROWS not found in engine/mod.rs");
	return Number(m[1].replaceAll("_", ""));
}
