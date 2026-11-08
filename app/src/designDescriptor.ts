// designDescriptor — the element-descriptor half of the beacon parser
// (#433): validates and caps the untrusted element shapes the injected
// script posts. Split out of designPreview.ts; same rules apply, every
// value is UNTRUSTED and only plain, length-capped data comes out.

import type { ElementDescriptor, ElementRect, ElementSource } from "./elementAnchor";

export const MAX_BEACON_TEXT = 500;
/** Same cap as Rust `check_rel_path`. */
const MAX_SOURCE_PATH = 500;
const NUL = String.fromCharCode(0);
const MAX_ATTRS = 16;

export const clipTo = (s: string, n: number): string => (s.length > n ? s.slice(0, n) : s);
export const cap = (s: string): string => clipTo(s, MAX_BEACON_TEXT);
export const isObj = (v: unknown): v is Record<string, unknown> =>
	typeof v === "object" && v !== null;
export const num = (v: unknown): number | null =>
	typeof v === "number" && Number.isFinite(v) ? v : null;

/** A worktree-relative path from a URL the preview served
 *  (`/preview/<token>/<path>`, possibly mangled by Babel into
 *  `/http:/127.0.0.1:1234/preview/tok/proto/shell.jsx`). Undefined for
 *  anything else, and for paths that could escape the worktree. */
export const stripSourcePrefix = (fileName: string): string | undefined => {
	let name = fileName;
	const q = name.search(/[?#]/);
	if (q >= 0) name = name.slice(0, q);
	const at = name.indexOf("/preview/");
	if (at < 0) return undefined;
	const segs = name.slice(at + "/preview/".length).split("/");
	if (segs.length < 2 || segs[0] === "") return undefined; // token + path
	const out: string[] = [];
	for (const raw of segs.slice(1)) {
		let seg: string;
		try {
			seg = decodeURIComponent(raw);
		} catch {
			return undefined;
		}
		if (
			seg === "" ||
			seg === ".." ||
			seg === "." ||
			seg.includes("\\") ||
			seg.includes("/") ||
			seg.includes(":") ||
			seg.includes(NUL)
		) {
			return undefined;
		}
		out.push(seg);
	}
	const path = out.join("/");
	return [...path].length > MAX_SOURCE_PATH ? undefined : path;
};

const parseRect = (v: unknown): ElementRect | null => {
	if (!isObj(v)) return null;
	const x = num(v.x);
	const y = num(v.y);
	const w = num(v.w);
	const h = num(v.h);
	return x === null || y === null || w === null || h === null ? null : { x, y, w, h };
};

const parseSource = (v: unknown): ElementSource | undefined => {
	if (!isObj(v) || typeof v.fileName !== "string") return undefined;
	const file = stripSourcePrefix(v.fileName);
	const line = num(v.lineNumber);
	if (file === undefined || line === null) return undefined;
	const column = num(v.columnNumber);
	return column === null ? { file, line } : { file, line, column };
};

const MAX_FRAME_DEPTH = 8;
const parseFrame = (v: unknown): string[] | undefined => {
	if (!Array.isArray(v)) return undefined;
	const out = v
		.filter((s): s is string => typeof s === "string" && s !== "")
		.slice(0, MAX_FRAME_DEPTH)
		.map((s) => clipTo(s, 1000));
	return out.length > 0 ? out : undefined;
};

export const parseDescriptor = (v: unknown): ElementDescriptor | null => {
	if (!isObj(v)) return null;
	if (typeof v.selector !== "string" || typeof v.tag !== "string" || typeof v.text !== "string") {
		return null;
	}
	const rect = parseRect(v.rect);
	if (!rect) return null;
	const attrs: Record<string, string> = {};
	if (isObj(v.attrs)) {
		for (const [k, val] of Object.entries(v.attrs)) {
			if (Object.keys(attrs).length >= MAX_ATTRS) break;
			if (typeof val === "string") attrs[clipTo(k, 64)] = clipTo(val, 300);
		}
	}
	const out: ElementDescriptor = {
		selector: clipTo(v.selector, 1000),
		tag: clipTo(v.tag, 64),
		text: cap(v.text),
		attrs,
		rect,
	};
	if (typeof v.odId === "string" && v.odId !== "") out.odId = clipTo(v.odId, 200);
	const source = parseSource(v.rawSource);
	if (source) out.source = source;
	const frame = parseFrame(v.frame);
	if (frame) out.frame = frame;
	return out;
};
