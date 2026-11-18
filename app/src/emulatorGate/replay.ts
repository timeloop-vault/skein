/// <reference types="node" />
// The recording through an xterm.js headless terminal (the client's own
// engine), read into the plain model at each wanted event.

import { Buffer } from "node:buffer";
import type { Recording } from "./fixtures.ts";
import { readState, type TermState } from "./model.ts";
import { type Picture, pictureOf } from "./picture.ts";
import { makeTerm, write } from "./terminals.ts";

/** One pass over the recording, capturing the state after each wanted event. */
export async function baselinesAt(
	rec: Recording,
	afterEvents: number[],
): Promise<Map<number, TermState>> {
	const want = new Set(afterEvents);
	const last = Math.max(...afterEvents);
	const out = new Map<number, TermState>();
	const t = makeTerm(rec.cols, rec.rows);
	for (let i = 0; i <= last; i++) {
		const ev = rec.events[i];
		if (!ev) throw new Error(`no event ${i}`);
		if (ev.kind === "data") await write(t.term, Buffer.from(ev.b64, "base64"));
		else t.term.resize(ev.cols, ev.rows);
		if (want.has(i)) out.set(i, readState(t.term));
	}
	return out;
}

/** The same pass, reading each wanted state in the picture form. */
export async function picturesAt(
	rec: Recording,
	afterEvents: number[],
): Promise<Map<number, Picture>> {
	const want = new Set(afterEvents);
	const last = Math.max(...afterEvents);
	const out = new Map<number, Picture>();
	const t = makeTerm(rec.cols, rec.rows);
	for (let i = 0; i <= last; i++) {
		const ev = rec.events[i];
		if (!ev) throw new Error(`no event ${i}`);
		if (ev.kind === "data") await write(t.term, Buffer.from(ev.b64, "base64"));
		else t.term.resize(ev.cols, ev.rows);
		if (want.has(i)) out.set(i, pictureOf(t.term));
	}
	return out;
}
