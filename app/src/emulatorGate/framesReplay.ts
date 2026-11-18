// The daemon's frames through @xterm/headless (#807), three ways, one picture
// per cut each: B-stream (a viewer attached from the start), B-full (a fresh
// viewer at every cut) and B-skipped (a viewer framed only at the cuts).

import type { FramesFile } from "./framesFixture.ts";
import { type Picture, pictureOf } from "./picture.ts";
import { makeTerm, write } from "./terminals.ts";

export type ByCut = Map<string, Picture>;

/** Stream items in order, a picture after all items with afterEvent <= the cut's. */
export async function streamPictures(f: FramesFile): Promise<ByCut> {
	const out: ByCut = new Map();
	const { term } = makeTerm(f.cols, f.rows);
	let i = 0;
	for (const cut of f.cuts) {
		while (i < f.stream.length && (f.stream[i]?.afterEvent ?? 0) <= cut.afterEvent) {
			const item = f.stream[i++];
			if (!item) break;
			if (item.kind === "resize") term.resize(item.cols, item.rows);
			else await write(term, item.bytes);
		}
		out.set(cut.name, pictureOf(term));
	}
	return out;
}

/** A fresh terminal of the cut's size per cut, written with `full`. */
export async function fullPictures(f: FramesFile): Promise<ByCut> {
	const out: ByCut = new Map();
	for (const cut of f.cuts) {
		const { term } = makeTerm(cut.cols, cut.rows);
		await write(term, cut.full);
		out.set(cut.name, pictureOf(term));
		term.dispose();
	}
	return out;
}

/** One terminal; at each cut resize if needed, write `skipped`, take the picture. */
export async function skippedPictures(f: FramesFile): Promise<ByCut> {
	const out: ByCut = new Map();
	const { term } = makeTerm(f.cols, f.rows);
	for (const cut of f.cuts) {
		if (term.cols !== cut.cols || term.rows !== cut.rows) term.resize(cut.cols, cut.rows);
		await write(term, cut.skipped);
		out.set(cut.name, pictureOf(term));
	}
	return out;
}
