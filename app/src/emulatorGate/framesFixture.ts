/// <reference types="node" />
// Loader for NAME.frames.json (#807): the redraw encoder's frames the daemon
// would send, pinned next to the recordings. Format: crates/skein-daemon/src/
// pty/engine/fixtures/README.md, "The frames".

import { Buffer } from "node:buffer";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { FIXTURE_DIR, type FixtureName } from "./fixtures.ts";

export type StreamItem =
	| { afterEvent: number; kind: "frame"; bytes: Buffer }
	| { afterEvent: number; kind: "resize"; cols: number; rows: number };

export interface FramesCut {
	name: string;
	afterEvent: number;
	cols: number;
	rows: number;
	/** The attach snapshot (empty inside a synchronized update). */
	full: Buffer;
	/** The frame of a viewer framed only at cuts, in cut order. */
	skipped: Buffer;
}

export interface FramesFile {
	cols: number;
	rows: number;
	stream: StreamItem[];
	cuts: FramesCut[];
}

interface RawItem {
	afterEvent: number;
	kind: "frame" | "resize";
	b64?: string;
	cols?: number;
	rows?: number;
}
interface RawCut {
	name: string;
	afterEvent: number;
	cols: number;
	rows: number;
	full: string;
	skipped: string;
}
interface Raw {
	version: number;
	fixture: string;
	cols: number;
	rows: number;
	stream: RawItem[];
	cuts: RawCut[];
}

export function loadFrames(name: FixtureName): FramesFile {
	const raw = JSON.parse(readFileSync(resolve(FIXTURE_DIR, `${name}.frames.json`), "utf8")) as Raw;
	if (raw.version !== 1 || raw.fixture !== name) throw new Error(`unexpected frames file ${name}`);
	return {
		cols: raw.cols,
		rows: raw.rows,
		stream: raw.stream.map((i): StreamItem => {
			if (i.kind === "frame") {
				return {
					afterEvent: i.afterEvent,
					kind: "frame",
					bytes: Buffer.from(i.b64 ?? "", "base64"),
				};
			}
			return { afterEvent: i.afterEvent, kind: "resize", cols: i.cols ?? 0, rows: i.rows ?? 0 };
		}),
		cuts: raw.cuts.map((c) => ({
			name: c.name,
			afterEvent: c.afterEvent,
			cols: c.cols,
			rows: c.rows,
			full: Buffer.from(c.full, "base64"),
			skipped: Buffer.from(c.skipped, "base64"),
		})),
	};
}
